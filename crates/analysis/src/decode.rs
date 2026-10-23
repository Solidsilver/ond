//! Decodes a whole file with symphonia and runs it through an [`Analyzer`].

use crate::{Analyzer, TrackAnalysis};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
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
    let fail = |what: &str, e: &dyn std::fmt::Display| AnalyzeError::Failed(format!("{what}: {e}"));
    let mss = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = suffix {
        hint.with_extension(ext);
    }
    let mut reader = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| if cancel() { AnalyzeError::Cancelled } else { fail("probe", &e) })?;
    let track = reader.default_track(TrackType::Audio).ok_or_else(|| AnalyzeError::Failed("no audio track".into()))?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| AnalyzeError::Failed("no audio codec parameters".into()))?
        .clone();
    // Lossy decoders don't report a bit depth; lossless ones do.
    let bits = params.bits_per_sample;
    let mut opts = AudioDecoderOptions::default();
    opts.gapless = true;
    let mut decoder = symphonia::default::get_codecs().make_audio_decoder(&params, &opts).map_err(|e| fail("decoder", &e))?;

    let mut analyzer: Option<Analyzer> = None;
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
            Err(e) => return Err(fail("read", &e)),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(b) => b,
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(fail("decode", &e)),
        };
        let spec = buf.spec();
        let channels = spec.channels().count().max(1);
        let a = analyzer.get_or_insert_with(|| Analyzer::new(spec.rate(), channels, bits));
        // A mid-stream format change is rare enough to skip rather than handle.
        if channels != a.channels || spec.rate() != a.rate {
            continue;
        }
        scratch.resize(buf.samples_interleaved(), 0.0);
        buf.copy_to_slice_interleaved(&mut scratch);
        a.push(&scratch);
    }
    analyzer.map(Analyzer::finish).ok_or_else(|| AnalyzeError::Failed("no audio decoded".into()))
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
