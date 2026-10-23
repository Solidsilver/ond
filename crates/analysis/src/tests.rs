use super::*;
use realfft::RealFftPlanner;
use realfft::num_complex::Complex;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Noise with the amplitude spectrum `shape(hz)`, scaled to `rms_dbfs`, plus
/// a 16-bit TPDF dither floor. Built by overlap-adding Hann-windowed blocks of
/// random-phase spectra, so the shape (a brickwall included) carries over.
fn shaped_noise(rate: u32, secs: f64, rms_dbfs: f64, shape: impl Fn(f64) -> f64) -> Vec<f32> {
    const M: usize = 4096;
    let len = (f64::from(rate) * secs) as usize;
    let ifft = RealFftPlanner::<f64>::new().plan_fft_inverse(M);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut out = vec![0.0f64; len + M];
    let mut spec = ifft.make_input_vec();
    let mut block = ifft.make_output_vec();
    let mut pos = 0;
    while pos < len {
        for (k, c) in spec.iter_mut().enumerate() {
            let hz = k as f64 * f64::from(rate) / M as f64;
            let edge = k == 0 || k == M / 2;
            let phase = rng.next() * std::f64::consts::TAU;
            *c = if edge { Complex::new(0.0, 0.0) } else { Complex::from_polar(shape(hz), phase) };
        }
        ifft.process(&mut spec, &mut block).unwrap();
        for (i, s) in block.iter().enumerate() {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / M as f64).cos();
            out[pos + i] += s * w;
        }
        pos += M / 2;
    }
    out.truncate(len);
    let rms = (out.iter().map(|s| s * s).sum::<f64>() / len as f64).sqrt();
    let gain = 10f64.powf(rms_dbfs / 20.0) / rms;
    let lsb = 1.0 / 32768.0;
    out.iter().map(|s| (s * gain + (rng.next() - rng.next()) * lsb) as f32).collect()
}

fn stereo(mono: &[f32]) -> Vec<f32> {
    mono.iter().flat_map(|&s| [s, s]).collect()
}

fn run(rate: u32, bits: Option<u32>, mono: &[f32]) -> TrackAnalysis {
    let mut a = Analyzer::new(rate, 2, bits);
    for chunk in stereo(mono).chunks(4096) {
        a.push(chunk);
    }
    a.finish()
}

fn sine(rate: u32, secs: f64, hz: f64, amp: f64) -> Vec<f32> {
    (0..(f64::from(rate) * secs) as usize)
        .map(|i| (amp * (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin()) as f32)
        .collect()
}

#[test]
fn very_low_sample_rates_do_not_panic() {
    // At 1–2 kHz the 1 kHz floor sits at or above Nyquist; the spectrum
    // check used to slice backwards and panic here.
    for rate in [1_000, 1_500, 2_000, 4_000, 7_999] {
        let a = run(rate, Some(16), &shaped_noise(rate, 20.0, -12.0, |_| 1.0));
        assert!(a.verdicts.iter().all(|v| !matches!(v.kind, VerdictKind::Upsampled { .. } | VerdictKind::LossySource { .. })), "{rate} Hz");
    }
}

fn kinds(a: &TrackAnalysis) -> Vec<&VerdictKind> {
    a.verdicts.iter().map(|v| &v.kind).collect()
}

#[test]
fn sine_at_minus_23_dbfs_reads_minus_23_lufs() {
    // EBU Tech 3341 case 1: stereo 1 kHz sine at -23 dBFS → -23.0 LUFS.
    let a = run(48_000, Some(24), &sine(48_000, 20.0, 1000.0, 10f64.powf(-23.0 / 20.0)));
    let lufs = a.integrated_lufs.unwrap();
    assert!((lufs + 23.0).abs() < 0.2, "{lufs}");
    let tp = a.true_peak_dbtp.unwrap();
    assert!((tp + 23.0).abs() < 0.3, "{tp}");
    assert!((a.sample_peak_dbfs.unwrap() + 23.0).abs() < 0.1);
    assert!(a.loudness_range_lu.unwrap() < 1.0);
    assert!((a.duration_secs - 20.0).abs() < 0.01);
}

#[test]
fn dr_of_a_full_scale_sine_is_zero() {
    let a = run(44_100, Some(16), &sine(44_100, 30.0, 440.0, 1.0));
    assert!(a.dr.unwrap().abs() < 0.2, "{:?}", a.dr);
    assert_eq!(a.dr_rounded(), Some(0));
}

#[test]
fn dr_uses_second_highest_block_peak_over_loud_rms() {
    // A -20 dB sine with full-scale clicks in two different 3 s blocks:
    // peak (2nd highest) 1.0 over block RMS 0.1 → DR 20.
    let rate = 44_100;
    let mut s = sine(rate, 30.0, 1000.0, 0.1);
    s[rate as usize * 4] = 1.0;
    s[rate as usize * 16] = -1.0;
    let a = run(rate, Some(16), &s);
    assert!((a.dr.unwrap() - 20.0).abs() < 0.2, "{:?}", a.dr);
    // Only one click: the second-highest peak is the sine's 0.1 → DR 0.
    let mut s = sine(rate, 30.0, 1000.0, 0.1);
    s[rate as usize * 4] = 1.0;
    assert!(run(rate, Some(16), &s).dr.unwrap() < 0.5);
}

#[test]
fn silence_has_no_loudness_or_dr() {
    let a = run(44_100, Some(16), &vec![0.0; 44_100 * 5]);
    assert_eq!(a.integrated_lufs, None);
    assert_eq!(a.dr, None);
    assert_eq!(a.sample_peak_dbfs, None);
    assert!(a.verdicts.is_empty());
}

#[test]
fn sixteen_bits_padded_into_24_is_flagged() {
    let noise = shaped_noise(44_100, 10.0, -20.0, |_| 1.0);
    let padded: Vec<f32> = noise.iter().map(|s| (s * 32768.0).round() / 32768.0).collect();
    let a = run(44_100, Some(24), &padded);
    assert_eq!(a.effective_bits, Some(16));
    assert!(kinds(&a).contains(&&VerdictKind::PaddedBits { bits: 24, effective: 16 }));
    assert_eq!(a.verdicts[0].label(), "effectively 16-bit");

    // Real 24-bit content (values on the 24-bit grid, low bits busy) isn't.
    let full: Vec<f32> = noise.iter().map(|s| (s * 8_388_608.0).round() / 8_388_608.0).collect();
    let a = run(44_100, Some(24), &full);
    assert_eq!(a.effective_bits, None);
    assert!(a.verdicts.is_empty(), "{:?}", a.verdicts);
}

#[test]
fn sharp_16k_cutoff_in_lossless_reads_as_lossy_source() {
    let n = shaped_noise(44_100, 20.0, -20.0, |hz| if hz < 16_000.0 { 1.0 } else { 0.0 });
    let a = run(44_100, Some(16), &n);
    let cut = a.cutoff_hz.unwrap();
    assert!((15_800..=16_300).contains(&cut), "{cut}");
    let v = a.verdicts.iter().find(|v| matches!(v.kind, VerdictKind::LossySource { .. })).expect("lossy verdict");
    assert_eq!(v.confidence, Confidence::High);
    assert!(v.evidence().contains("16 kHz") || v.evidence().contains("16."), "{}", v.evidence());
}

#[test]
fn twenty_k_cutoff_is_only_a_possibility() {
    let n = shaped_noise(44_100, 20.0, -20.0, |hz| if hz < 20_000.0 { 1.0 } else { 0.0 });
    let a = run(44_100, Some(16), &n);
    let v = a.verdicts.iter().find(|v| matches!(v.kind, VerdictKind::LossySource { .. })).expect("lossy verdict");
    assert_eq!(v.confidence, Confidence::Medium);
    assert!(v.label().starts_with("possibly"));
}

#[test]
fn content_to_the_anti_alias_filter_is_not_suspicious() {
    // A normal CD: content up to ~21.8 kHz, then the converter's filter.
    let n = shaped_noise(44_100, 20.0, -20.0, |hz| if hz < 21_800.0 { 1.0 } else { 0.0 });
    assert!(run(44_100, Some(16), &n).verdicts.is_empty());
}

#[test]
fn natural_rolloff_is_not_a_cutoff() {
    // Falling 3 dB per kHz above 8 kHz: steep-ish, but gradual and never gone.
    let n = shaped_noise(44_100, 20.0, -20.0, |hz| 10f64.powf(-(hz - 8000.0).max(0.0) / 1000.0 * 3.0 / 20.0));
    let a = run(44_100, Some(16), &n);
    assert!(a.verdicts.is_empty(), "{:?}", a.verdicts);
}

#[test]
fn lossy_files_are_not_accused_of_being_lossy() {
    let n = shaped_noise(44_100, 20.0, -20.0, |hz| if hz < 16_000.0 { 1.0 } else { 0.0 });
    assert!(run(44_100, None, &n).verdicts.is_empty());
}

#[test]
fn hi_res_with_nothing_above_21k_is_upsampled_from_44k() {
    let n = shaped_noise(96_000, 15.0, -20.0, |hz| if hz < 21_000.0 { 1.0 } else { 0.0 });
    let a = run(96_000, Some(24), &n);
    let v = a.verdicts.iter().find(|v| matches!(v.kind, VerdictKind::Upsampled { .. })).expect("upsampled");
    assert!(matches!(v.kind, VerdictKind::Upsampled { from_hz: 44_100, .. }), "{v:?}");
    assert_eq!(v.confidence, Confidence::High);
    assert_eq!(v.label(), "likely upsampled");
    assert!(v.evidence().contains("44.1 kHz"), "{}", v.evidence());
}

#[test]
fn hi_res_with_nothing_above_23k_is_upsampled_from_48k() {
    let n = shaped_noise(96_000, 15.0, -20.0, |hz| if hz < 23_500.0 { 1.0 } else { 0.0 });
    let a = run(96_000, Some(24), &n);
    assert!(a.verdicts.iter().any(|v| matches!(v.kind, VerdictKind::Upsampled { from_hz: 48_000, .. })), "{:?}", a.verdicts);
}

#[test]
fn genuine_hi_res_is_left_alone() {
    // Ultrasonic content 30 dB down, all the way to the converter's filter.
    let n = shaped_noise(96_000, 15.0, -20.0, |hz| if hz < 20_000.0 { 1.0 } else if hz < 46_000.0 { 0.03 } else { 0.0 });
    let a = run(96_000, Some(24), &n);
    assert!(a.verdicts.is_empty(), "{:?}", a.verdicts);
}

#[test]
fn quiet_material_is_not_judged() {
    // Below the loudness gate for the spectrum: too little to go on.
    let n = shaped_noise(44_100, 20.0, -60.0, |hz| if hz < 16_000.0 { 1.0 } else { 0.0 });
    assert!(run(44_100, Some(16), &n).verdicts.is_empty());
}

fn fake(dr: f64, lufs: f64, secs: f64, verdicts: Vec<Verdict>) -> TrackAnalysis {
    TrackAnalysis {
        version: ANALYSIS_VERSION,
        duration_secs: secs,
        sample_rate: 44_100,
        channels: 2,
        bits: Some(16),
        integrated_lufs: Some(lufs),
        loudness_range_lu: None,
        true_peak_dbtp: Some(-1.0),
        sample_peak_dbfs: None,
        dr: Some(dr),
        effective_bits: None,
        cutoff_hz: None,
        verdicts,
    }
}

#[test]
fn album_summary_averages_dr_and_energy() {
    let up = Verdict { kind: VerdictKind::Upsampled { from_hz: 44_100, cutoff_hz: 21_000 }, confidence: Confidence::High };
    let a = fake(8.4, -10.0, 100.0, vec![up.clone()]);
    let b = fake(9.4, -20.0, 100.0, vec![up.clone()]);
    let c = fake(10.0, -10.0, 200.0, vec![]);
    let s = summarize(&[&a, &b, &c], 4);
    assert_eq!(s.dr, Some(9)); // (8.4 + 9.4 + 10) / 3 = 9.27
    // Energy mean: (100·0.1 + 100·0.01 + 200·0.1) / 400 = 0.0775 → -11.1 LUFS.
    assert!((s.lufs.unwrap() + 11.106).abs() < 0.01, "{:?}", s.lufs);
    assert_eq!(s.verdict, Some((up, 2)));
    assert!(!s.complete());

    // A single odd track doesn't label the album.
    let lossy = Verdict { kind: VerdictKind::LossySource { cutoff_hz: 16_000 }, confidence: Confidence::High };
    let d = fake(9.0, -10.0, 100.0, vec![lossy]);
    let s = summarize(&[&c, &c, &d], 3);
    assert_eq!(s.verdict, None);
    assert!(s.complete());
}

#[test]
fn records_round_trip_and_stamps_track_the_file() {
    let r = Record { stamp: stamp(Some(10), Some("flac"), Some(24), Some(96_000)), analysis: fake(9.0, -14.0, 60.0, vec![]) };
    assert_eq!(Record::decode(&r.encode()), Some(r.clone()));
    assert_ne!(r.stamp, stamp(Some(11), Some("flac"), Some(24), Some(96_000)));
    assert!(r.stamp.starts_with(&format!("v{ANALYSIS_VERSION}:")));
}

#[test]
fn decodes_a_24_bit_wav_and_finds_the_padding() {
    use std::io::Write;
    let rate = 48_000u32;
    let s = sine(rate, 5.0, 997.0, 0.5);
    let mut data = Vec::new();
    for &x in &s {
        // 16-bit values written into a 24-bit container.
        let v = ((f64::from(x) * 32768.0).round() as i32) << 8;
        for _ in 0..2 {
            data.extend_from_slice(&v.to_le_bytes()[..3]);
        }
    }
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 6).to_le_bytes());
    wav.extend_from_slice(&6u16.to_le_bytes());
    wav.extend_from_slice(&24u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    let mut f = tempfile::tempfile().unwrap();
    f.write_all(&wav).unwrap();
    use std::io::Seek;
    f.rewind().unwrap();

    let a = analyze(Box::new(f), Some("wav"), &|| false).unwrap();
    assert_eq!(a.bits, Some(24));
    assert_eq!(a.sample_rate, rate);
    assert!((a.duration_secs - 5.0).abs() < 0.01);
    assert!(kinds(&a).contains(&&VerdictKind::PaddedBits { bits: 24, effective: 16 }));
    assert!(analyze(Box::new(std::io::Cursor::new(wav)), Some("wav"), &|| true).is_err());
}
