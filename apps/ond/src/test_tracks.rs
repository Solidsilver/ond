//! What the tests build their tracks from, across the crate. (The UI's
//! fixture library, with its albums and queue, is `app::fixture`; it builds
//! its tracks here too.) `Track` has no `Default`: a track without an id,
//! album and title means nothing, and production code shouldn't be able to
//! make one by accident. Tests start from [`track`] and set what they're
//! about with struct update syntax, so adding a field to `Track` touches
//! this file alone.

use library::Track;

/// A track with an id, an album and a title, and nothing else known.
pub fn track(id: &str, album: &str, title: &str) -> Track {
    Track {
        id: id.into(),
        album_id: album.into(),
        title: title.into(),
        artist: None,
        artist_ref: None,
        track: None,
        disc: None,
        duration_secs: None,
        suffix: None,
        content_type: None,
        size: None,
        bit_rate_kbps: None,
        bit_depth: None,
        sample_rate: None,
        channels: None,
        track_gain: None,
        album_gain: None,
        track_peak: None,
        album_peak: None,
        composer: None,
        contributors: Vec::new(),
        cover_art: None,
        artists: Vec::new(),
    }
}

/// One stable account across the UI and playback fixtures.
pub fn account() -> accounts::AccountId {
    "00000000-0000-4000-8000-000000000001".parse().unwrap()
}

pub fn key(id: &str) -> crate::source::TrackKey {
    crate::source::TrackKey::new(account(), id)
}

/// A 64 px PNG filled with one colour, for a cover a fake source serves or the
/// cache already holds.
pub fn png(rgba: [u8; 4]) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(64, 64, image::Rgba(rgba));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// A tiny but whole FLAC file (a second of silence, mono 16-bit) with
/// Vorbis comments `tags` and, if given, an embedded picture: what a test's
/// folder of music is made of.
pub fn flac(tags: &[(&str, &str)], picture: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::from(b"fLaC".as_slice());

    let sr = 44_100u32;
    let channels_minus_1 = 1u32;
    let bps_minus_1 = 15u32;
    let total_samples: u64 = 44_100;
    let mut streaminfo = [0u8; 34];
    streaminfo[0..2].copy_from_slice(&4096u16.to_be_bytes());
    streaminfo[2..4].copy_from_slice(&4096u16.to_be_bytes());
    let sr_ch_bps = ((sr & 0xFFFFF) << 12) | ((channels_minus_1 & 0x7) << 9) | ((bps_minus_1 & 0x1F) << 4);
    let combined = (u64::from(sr_ch_bps) << 32) | (total_samples & 0x0F_FFFF_FFFF);
    streaminfo[10..18].copy_from_slice(&combined.to_be_bytes());

    let si_header_last = tags.is_empty() && picture.is_none();
    out.push(if si_header_last { 0x80 } else { 0x00 });
    out.extend_from_slice(&(streaminfo.len() as u32).to_be_bytes()[1..4]);
    out.extend_from_slice(&streaminfo);

    if !tags.is_empty() {
        let mut vc = Vec::new();
        let vendor = b"ond-test";
        vc.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        vc.extend_from_slice(vendor);
        vc.extend_from_slice(&(tags.len() as u32).to_le_bytes());
        for (k, v) in tags {
            let entry = format!("{k}={v}");
            vc.extend_from_slice(&(entry.len() as u32).to_le_bytes());
            vc.extend_from_slice(entry.as_bytes());
        }
        let last_flag = if picture.is_none() { 0x80 } else { 0x00 };
        out.push(last_flag | 4);
        out.extend_from_slice(&(vc.len() as u32).to_be_bytes()[1..4]);
        out.extend_from_slice(&vc);
    }

    if let Some(pic) = picture {
        let mut block = Vec::new();
        block.extend_from_slice(&3u32.to_be_bytes());
        let mime = b"image/png";
        block.extend_from_slice(&(mime.len() as u32).to_be_bytes());
        block.extend_from_slice(mime);
        block.extend_from_slice(&0u32.to_be_bytes());
        block.extend_from_slice(&1u32.to_be_bytes());
        block.extend_from_slice(&1u32.to_be_bytes());
        block.extend_from_slice(&24u32.to_be_bytes());
        block.extend_from_slice(&0u32.to_be_bytes());
        block.extend_from_slice(&(pic.len() as u32).to_be_bytes());
        block.extend_from_slice(pic);
        out.push(0x80 | 6);
        out.extend_from_slice(&(block.len() as u32).to_be_bytes()[1..4]);
        out.extend_from_slice(&block);
    }

    // Append one constant-zero FLAC frame so symphonia probes the stream.
    let mut frame = vec![0xFF, 0xF8, 0xC9, 0x18, 0x00];
    let mut crc8 = 0u8;
    for &b in &frame {
        crc8 ^= b;
        for _ in 0..8 {
            crc8 = if crc8 & 0x80 != 0 { (crc8 << 1) ^ 0x07 } else { crc8 << 1 };
        }
    }
    frame.push(crc8);
    frame.extend_from_slice(&[0x00; 6]);
    let mut crc16 = 0u16;
    for &b in &frame {
        crc16 ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc16 = if crc16 & 0x8000 != 0 { (crc16 << 1) ^ 0x8005 } else { crc16 << 1 };
        }
    }
    frame.extend_from_slice(&crc16.to_be_bytes());
    out.extend_from_slice(&frame);

    out
}
