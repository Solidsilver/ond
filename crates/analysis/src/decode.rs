//! Decodes a whole file with symphonia and runs it through an [`Analyzer`].

use crate::{Analyzer, TrackAnalysis};
use crate::peaks::{PeakMeter, Peaks};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use std::io::{Read, Seek, SeekFrom};

#[derive(Debug, Clone, PartialEq)]
pub enum AnalyzeError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for AnalyzeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Failed(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for AnalyzeError {}

/// Decodes `source` to the end and analyses it. `cancel` is polled between
/// packets; reads that block (a download) should also give up when it's set.
pub fn analyze(source: Box<dyn MediaSource>, suffix: Option<&str>, cancel: &dyn Fn() -> bool) -> Result<TrackAnalysis, AnalyzeError> {
    let mut analyzer: Option<Analyzer> = None;
    decode(source, suffix, cancel, &mut |rate, channels, bits, samples| {
        let a = analyzer.get_or_insert_with(|| Analyzer::new(rate, channels, bits));
        // A mid-stream format change is rare enough to skip rather than handle.
        if channels == a.channels && rate == a.rate {
            a.push(samples);
        }
    })?;
    analyzer.map(Analyzer::finish).ok_or_else(|| AnalyzeError::Failed("no audio decoded".into()))
}

/// The peaks of `source` for a waveform (see [`Peaks`]); `cancel` as for [`analyze`].
pub fn peaks(source: Box<dyn MediaSource>, suffix: Option<&str>, cancel: &dyn Fn() -> bool) -> Result<Peaks, AnalyzeError> {
    let mut meter: Option<PeakMeter> = None;
    decode(source, suffix, cancel, &mut |rate, channels, _, samples| {
        let m = meter.get_or_insert_with(|| PeakMeter::new(rate, channels));
        if channels == m.channels && rate == m.rate {
            m.push(samples);
        }
    })?;
    meter.map(PeakMeter::finish).ok_or_else(|| AnalyzeError::Failed("no audio decoded".into()))
}

/// [`peaks`] for any seekable reader.
pub fn peaks_reader<R: Read + Seek + Send + Sync + 'static>(reader: R, len: Option<u64>, suffix: Option<&str>, cancel: &dyn Fn() -> bool) -> Result<Peaks, AnalyzeError> {
    peaks(Box::new(Source { reader, len }), suffix, cancel)
}

/// Takes decoded blocks: rate, channels, (lossless) bit depth, interleaved samples.
type Sink<'a> = dyn FnMut(u32, usize, Option<u32>, &[f32]) + 'a;

/// Decodes `source` to the end, handing each block of interleaved samples to
/// `sink` with the rate, channel count and (lossless) bit depth.
fn decode(source: Box<dyn MediaSource>, suffix: Option<&str>, cancel: &dyn Fn() -> bool, sink: &mut Sink<'_>) -> Result<(), AnalyzeError> {
    let fail = |what: &str, e: &dyn std::fmt::Display| AnalyzeError::Failed(format!("{what}: {e}"));
    let mss = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = suffix {
        hint.with_extension(ext);
    }
    let mut reader = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| if cancel() { AnalyzeError::Cancelled } else { fail("probe", &e) })?;
    let (mut track_id, mut bits, mut decoder) = open_track(reader.as_ref())?;

    let mut scratch: Vec<f32> = Vec::new();
    loop {
        if cancel() {
            return Err(AnalyzeError::Cancelled);
        }
        let packet = match reader.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(_) if cancel() => return Err(AnalyzeError::Cancelled),
            // The track list changed (a chained Ogg stream, say): start again
            // with whatever is the audio track now.
            Err(SymError::ResetRequired) => {
                (track_id, bits, decoder) = open_track(reader.as_ref())?;
                continue;
            }
            Err(e) => return Err(fail("read", &e)),
        };
        if packet.track_id != track_id {
            continue;
        }
        let mut retried = false;
        loop {
            let again = match decoder.decode(&packet) {
                Ok(buf) => {
                    let spec = buf.spec();
                    let channels = spec.channels().count().max(1);
                    scratch.resize(buf.samples_interleaved(), 0.0);
                    buf.copy_to_slice_interleaved(&mut scratch);
                    sink(spec.rate(), channels, bits, &scratch);
                    false
                }
                Err(SymError::DecodeError(_)) => false,
                // The stream's format changed: a fresh decoder, then this
                // packet again (once).
                Err(SymError::ResetRequired) if !retried => true,
                Err(e) => return Err(fail("decode", &e)),
            };
            if !again {
                break;
            }
            retried = true;
            (track_id, bits, decoder) = open_track(reader.as_ref())?;
            if packet.track_id != track_id {
                break;
            }
        }
    }
    Ok(())
}

/// A track to decode: its id, its (lossless) bit depth and a decoder for it.
type Opened = (u32, Option<u32>, Box<dyn AudioDecoder>);

/// The default audio track, opened.
fn open_track(reader: &dyn FormatReader) -> Result<Opened, AnalyzeError> {
    let track = reader.default_track(TrackType::Audio).ok_or_else(|| AnalyzeError::Failed("no audio track".into()))?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| AnalyzeError::Failed("no audio codec parameters".into()))?;
    let mut opts = AudioDecoderOptions::default();
    opts.gapless = true;
    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &opts)
        .map_err(|e| AnalyzeError::Failed(format!("decoder: {e}")))?;
    // Lossy decoders don't report a bit depth; lossless ones do.
    Ok((track.id, params.bits_per_sample, decoder))
}

/// [`analyze`] for any seekable reader (a file, a stream cache's reader);
/// `len` helps symphonia seek within formats that need the file size.
pub fn analyze_reader<R: Read + Seek + Send + Sync + 'static>(
    reader: R,
    len: Option<u64>,
    suffix: Option<&str>,
    cancel: &dyn Fn() -> bool,
) -> Result<TrackAnalysis, AnalyzeError> {
    analyze(Box::new(Source { reader, len }), suffix, cancel)
}

struct Source<R> {
    reader: R,
    len: Option<u64>,
}

impl<R: Read> Read for Source<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf)
    }
}

impl<R: Seek> Seek for Source<R> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.reader.seek(to)
    }
}

impl<R: Read + Seek + Send + Sync> MediaSource for Source<R> {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.len
    }
}
