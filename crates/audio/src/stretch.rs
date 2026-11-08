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
