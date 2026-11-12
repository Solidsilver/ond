//! Speech stretching on the decode thread, before device-rate conversion.
//!
//! Sonic's pitch-synchronous algorithm is designed for fast speech (including
//! 2–3×). `libsonic` 0.1 is a small Apache-2.0 Rust port with no dependencies or
//! native toolchain, an interleaved float streaming API and C-parity fixtures.
//! `wsola` 0.3 (MIT, Rust, float throughout) also streams, but its full waveform
//! search cost much more in our 48 kHz stereo comparison. Both ports are young;
//! the trait keeps replacement local. Sonic quantizes internally to 16-bit;
//! at exactly 1× the engine never constructs or calls it, preserving music.
//!
//! Sonic buffers up to two pitch periods (2 * floor(rate / 65) source frames)
//! before producing audio, but inserts no leading silence. Output frame zero
//! therefore represents source frame zero, not the number of frames fed when
//! the first output becomes available. Pitch-period overlap makes this a
//! nominal timeline, with local waveform alignment within that 31 ms window.
//! EOF drains the buffered tail; a seek discards it by constructing fresh state.

use libsonic::{SonicError, SonicStream};

/// Interleaved frames in and out, with no startup padding in the output.
/// Implementations may buffer input; `finish` drains it without starting a new
/// time origin. Resetting means replacing the instance, so no old tail leaks.
pub(crate) trait Stretch {
    fn process(&mut self, input: &[f32], output: &mut Vec<f32>);
    fn finish(&mut self, output: &mut Vec<f32>);
}

pub(crate) fn speech(rate: u32, channels: u16, speed: f32) -> Result<Option<Box<dyn Stretch>>, SonicError> {
    if speed == 1.0 {
        return Ok(None);
    }
    let mut stream = SonicStream::new(rate, channels as u32)?;
    stream.set_speed(speed);
    Ok(Some(Box::new(Sonic(stream))))
}

struct Sonic(SonicStream);

impl Stretch for Sonic {
    fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        self.0.write_f32(input);
        output.extend(self.0.read_f32(self.0.available_frames()));
    }

    fn finish(&mut self, output: &mut Vec<f32>) {
        self.0.flush();
        output.extend(self.0.read_f32(self.0.available_frames()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn voiced(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let phase = std::f32::consts::TAU * 200.0 * i as f32 / 48_000.0;
                let sample = phase.sin() * 0.4 + (phase * 2.0).sin() * 0.2;
                [sample, sample * 0.9]
            })
            .collect()
    }

    #[test]
    fn stretching_keeps_the_pitch_at_speech_speeds() {
        let input = voiced(48_000 * 2);
        for speed in [0.5, 1.5, 2.0, 3.0] {
            let mut stretch = speech(48_000, 2, speed).unwrap().unwrap();
            let mut output = Vec::new();
            for piece in input.chunks(2048) {
                stretch.process(piece, &mut output);
            }
            stretch.finish(&mut output);
            let edge = 4800 * 2;
            let middle: Vec<_> = output[edge..output.len() - edge].iter().step_by(2).copied().collect();
            let crossings: Vec<_> = middle.windows(2).enumerate().filter(|(_, pair)| pair[0] < 0.0 && pair[1] >= 0.0).map(|(i, _)| i).collect();
            let frequency = (crossings.len() - 1) as f64 * 48_000.0 / (crossings.last().unwrap() - crossings[0]) as f64;
            assert!((frequency - 200.0).abs() < 1.0, "{speed}×: {frequency} Hz");
        }
    }

    fn cpu_now() -> Option<std::time::Duration> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let mut stamp = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            // A thread CPU clock excludes time spent descheduled by other work.
            if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut stamp) } == 0 {
                return Some(std::time::Duration::new(stamp.tv_sec as u64, stamp.tv_nsec as u32));
            }
        }
        None
    }

    /// Run alone in release so scheduling other tests doesn't distort the cost.
    #[test]
    #[ignore = "manual release performance measurement"]
    fn stretching_ten_seconds_reports_its_cost_and_startup_latency() {
        let input = voiced(48_000 * 10);
        let mut latency = 0;
        let mut stretch = speech(48_000, 2, 2.0).unwrap().unwrap();
        let mut output = Vec::new();
        for frame in input.as_chunks::<2>().0 {
            latency += 1;
            stretch.process(frame, &mut output);
            if !output.is_empty() {
                break;
            }
        }
        let mut costs = Vec::new();
        let mut cpu_costs = Vec::new();
        let mut frames = 0;
        for _ in 0..6 {
            let start = Instant::now();
            let cpu_start = cpu_now();
            let mut stretch = speech(48_000, 2, 2.0).unwrap().unwrap();
            output.clear();
            for piece in input.chunks(2048) {
                stretch.process(piece, &mut output);
            }
            stretch.finish(&mut output);
            frames = std::hint::black_box(output.len()) / 2;
            costs.push(start.elapsed());
            if let Some(elapsed) = cpu_start.zip(cpu_now()).map(|(start, end)| end - start) {
                cpu_costs.push(elapsed);
            }
        }
        costs.remove(0);
        costs.sort();
        if !cpu_costs.is_empty() {
            cpu_costs.remove(0);
            cpu_costs.sort();
            eprintln!("median thread CPU {:?} ({:.3}% of one core during 5 s of output)", cpu_costs[2], cpu_costs[2].as_secs_f64() / 5.0 * 100.0);
        }
        eprintln!(
            "48 kHz stereo, 10 s at 2×: median {:?}; output {frames} frames; first output after {latency} source frames ({:.3} ms)",
            costs[2],
            latency as f64 / 48.0
        );
        assert!((frames as f64 / 48_000.0 - 5.0).abs() < 2.0 / 65.0);
        assert_eq!(latency, 2 * (48_000 / 65));
    }
}
