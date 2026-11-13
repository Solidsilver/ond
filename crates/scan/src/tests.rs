use super::*;
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;

/// Build a tiny valid FLAC file with STREAMINFO, optional VORBIS_COMMENT, and optional PICTURE.
fn make_flac(tags: &[(&str, &str)], picture: Option<&[u8]>, duration_secs: u64) -> Vec<u8> {
    let sample_rate: u32 = 44_100;
    let channels_minus_1: u8 = 1; // 2 channels
    let bps_minus_1: u8 = 15; // 16-bit
    let total_samples: u64 = sample_rate as u64 * duration_secs;

    // 34-byte STREAMINFO payload
    let mut streaminfo = [0u8; 34];
    streaminfo[0..2].copy_from_slice(&4096u16.to_be_bytes());
    streaminfo[2..4].copy_from_slice(&4096u16.to_be_bytes());
    let sr_ch_bps: u32 = ((sample_rate & 0xFFFFF) << 12) | ((u32::from(channels_minus_1) & 0x7) << 9) | ((u32::from(bps_minus_1) & 0x1F) << 4);
    let combined = (u64::from(sr_ch_bps) << 32) | (total_samples & 0x0F_FFFF_FFFF);
    streaminfo[10..18].copy_from_slice(&combined.to_be_bytes());

    let mut out = Vec::new();
    out.extend_from_slice(b"fLaC");

    let has_pic = picture.is_some();
    // STREAMINFO header (last=false since we always write VORBIS_COMMENT)
    write_flac_block(&mut out, 0, false, &streaminfo);

    // VORBIS_COMMENT payload (little-endian lengths)
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
    write_flac_block(&mut out, 4, !has_pic, &vc);

    if let Some(pic_bytes) = picture {
        let mut pic = Vec::new();
        pic.extend_from_slice(&3u32.to_be_bytes()); // Picture type 3: Front Cover
        let mime = b"image/png";
        pic.extend_from_slice(&(mime.len() as u32).to_be_bytes());
        pic.extend_from_slice(mime);
        pic.extend_from_slice(&0u32.to_be_bytes()); // description len
        pic.extend_from_slice(&1u32.to_be_bytes()); // width
        pic.extend_from_slice(&1u32.to_be_bytes()); // height
        pic.extend_from_slice(&24u32.to_be_bytes()); // depth
        pic.extend_from_slice(&0u32.to_be_bytes()); // indexed colors
        pic.extend_from_slice(&(pic_bytes.len() as u32).to_be_bytes());
        pic.extend_from_slice(pic_bytes);
        write_flac_block(&mut out, 6, true, &pic);
    }

    // Minimal valid FLAC frame (4096 samples, 44.1kHz, 2ch, 16-bit, constant zero).
    let mut frame = vec![0xFF, 0xF8, 0xC9, 0x18, 0x00];
    frame.push(crc8(&frame));
    frame.extend_from_slice(&[0x00, 0x00, 0x00]); // left constant zero
    frame.extend_from_slice(&[0x00, 0x00, 0x00]); // right constant zero
    let crc = crc16(&frame);
    frame.extend_from_slice(&crc.to_be_bytes());
    out.extend_from_slice(&frame);

    out
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
        }
    }
    crc
}

fn write_flac_block(out: &mut Vec<u8>, block_type: u8, is_last: bool, payload: &[u8]) {
    let header_byte = if is_last { 0x80 | block_type } else { block_type };
    let len = payload.len() as u32;
    out.push(header_byte);
    out.push(((len >> 16) & 0xff) as u8);
    out.push(((len >> 8) & 0xff) as u8);
    out.push((len & 0xff) as u8);
    out.extend_from_slice(payload);
}

/// Build a tiny valid PCM WAV file.
fn make_wav(duration_secs: u32) -> Vec<u8> {
    let sample_rate: u32 = 44_100;
    let channels: u16 = 2;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample / 8);
    let block_align = channels * (bits_per_sample / 8);
    let data_len = sample_rate * u32::from(block_align) * duration_secs;

    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.resize(44 + data_len as usize, 0);
    out
}

fn write_file(dir: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn reads_flac_and_wav_tags_sort_musicbrainz_replaygain_and_skips_corrupt_files() {
    let tmp = TempDir::new().unwrap();
    let flac = make_flac(
        &[
            ("TITLE", "Waltz for Debby"),
            ("TITLESORT", "Waltz for Debby"),
            ("ARTIST", "Bill Evans Trio"),
            ("ARTISTSORT", "Evans, Bill Trio"),
            ("ALBUMARTIST", "Bill Evans"),
            ("ALBUMARTISTSORT", "Evans, Bill"),
            ("ALBUM", "Waltz for Debby"),
            ("ALBUMSORT", "Waltz for Debby"),
            ("TRACKNUMBER", "1"),
            ("DISCNUMBER", "2"),
            ("DATE", "1961-06-25"),
            ("GENRE", "Jazz"),
            ("LABEL", "Riverside"),
            ("COMPOSER", "Bill Evans"),
            ("MUSICBRAINZ_TRACKID", "mb-rec-1"),
            ("MUSICBRAINZ_ALBUMID", "mb-alb-1"),
            ("MUSICBRAINZ_RELEASEGROUPID", "mb-rg-1"),
            ("MUSICBRAINZ_ARTISTID", "mb-art-1"),
            ("MUSICBRAINZ_ALBUMARTISTID", "mb-aa-1"),
            ("REPLAYGAIN_TRACK_GAIN", "-6.25 dB"),
            ("REPLAYGAIN_ALBUM_GAIN", "-5.50 dB"),
            ("REPLAYGAIN_TRACK_PEAK", "0.950000"),
            ("REPLAYGAIN_ALBUM_PEAK", "0.980000"),
        ],
        None,
        180,
    );
    write_file(tmp.path(), "Bill Evans/Waltz/01.flac", &flac);
    write_file(tmp.path(), "Loose/Field Recording.wav", &make_wav(10));
    write_file(tmp.path(), "Broken/corrupt.flac", b"not actually a flac");
    write_file(tmp.path(), "Broken/not-audio.mp3", b"plain text");
    write_file(tmp.path(), "Bill Evans/Waltz/notes.txt", b"liner notes");

    let res = scan(&[tmp.path().to_path_buf()]);
    assert_eq!(res.tracks.len(), 2);
    assert_eq!(res.skipped, 2, "only corrupt audio files should count as skipped");

    let t = res.tracks.iter().find(|t| t.suffix == "flac").unwrap();
    assert_eq!(t.title, "Waltz for Debby");
    assert_eq!(t.artist.as_deref(), Some("Bill Evans Trio"));
    assert_eq!(t.album_artist.as_deref(), Some("Bill Evans"));
    assert_eq!(t.sort_artist.as_deref(), Some("Evans, Bill Trio"));
    assert_eq!(t.sort_album_artist.as_deref(), Some("Evans, Bill"));
    assert_eq!(t.sort_album.as_deref(), Some("Waltz for Debby"));
    assert_eq!(t.track, Some(1));
    assert_eq!(t.disc, Some(2));
    assert_eq!(t.year, Some(1961));
    assert_eq!(t.duration_secs, Some(180));
    assert_eq!(t.sample_rate, Some(44_100));
    assert_eq!(t.bit_depth, Some(16));
    assert_eq!(t.channels, Some(2));
    assert_eq!(t.mb_recording_id.as_deref(), Some("mb-rec-1"));
    assert_eq!(t.mb_album_id.as_deref(), Some("mb-alb-1"));
    assert_eq!(t.mb_release_group_id.as_deref(), Some("mb-rg-1"));
    assert_eq!(t.track_gain, Some(-6.25));
    assert_eq!(t.album_gain, Some(-5.50));
    assert_eq!(t.track_peak, Some(0.95));
    assert_eq!(t.album_peak, Some(0.98));
    assert_eq!(t.labels, vec!["Riverside"]);
    assert_eq!(t.genres, vec!["Jazz"]);

    let wav = res.tracks.iter().find(|t| t.suffix == "wav").unwrap();
    assert_eq!(wav.title, "Field Recording");
    assert_eq!(wav.album, "Loose");
    assert_eq!(wav.duration_secs, Some(10));
    assert_eq!(wav.sample_rate, Some(44_100));
}

#[test]
fn groups_multi_disc_albums_and_detects_folder_compilations() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "Artist/Double/d2t1.flac",
        &make_flac(&[("ALBUM", "Double"), ("ALBUMARTIST", "Alice"), ("ARTIST", "Alice"), ("DISCNUMBER", "2"), ("TRACKNUMBER", "1")], None, 100),
    );
    write_file(
        tmp.path(),
        "Artist/Double/d1t1.flac",
        &make_flac(&[("ALBUM", "Double"), ("ALBUMARTIST", "Alice"), ("ARTIST", "Alice"), ("DISCNUMBER", "1"), ("TRACKNUMBER", "1")], None, 90),
    );

    // One folder with one album title and mixed track artists -> compilation.
    write_file(tmp.path(), "Comp/01.flac", &make_flac(&[("ALBUM", "Summer Hits"), ("ARTIST", "Band One"), ("TRACKNUMBER", "1")], None, 120));
    write_file(tmp.path(), "Comp/02.flac", &make_flac(&[("ALBUM", "Summer Hits"), ("ARTIST", "Band Two"), ("TRACKNUMBER", "2")], None, 130));

    let res = scan(&[tmp.path().to_path_buf()]);
    assert_eq!(res.albums.len(), 2);

    let double = res.albums.iter().find(|a| a.name == "Double").unwrap();
    assert_eq!(double.song_count, 2);
    assert!(!double.compilation);
    let double_tracks: Vec<&ScannedTrack> = res.tracks.iter().filter(|t| t.album_id == double.id).collect();
    assert_eq!(double_tracks.len(), 2);
    assert_eq!(double_tracks[0].disc, Some(1));
    assert_eq!(double_tracks[1].disc, Some(2));

    let comp = res.albums.iter().find(|a| a.name == "Summer Hits").unwrap();
    assert!(comp.compilation);
    assert_eq!(comp.artist.as_deref(), Some("Various Artists"));
    assert_eq!(comp.song_count, 2);
}

#[test]
fn moving_entire_folder_root_preserves_track_and_album_ids() {
    let tmp = TempDir::new().unwrap();
    let root_a = tmp.path().join("disk1/Music");
    let root_b = tmp.path().join("disk2/Library");
    write_file(&root_a, "Artist/Album/01.flac", &make_flac(&[("ALBUM", "Record"), ("ARTIST", "Artist"), ("TITLE", "Intro")], None, 60));

    let first = scan(std::slice::from_ref(&root_a));
    std::fs::create_dir_all(root_b.parent().unwrap()).unwrap();
    std::fs::rename(&root_a, &root_b).unwrap();
    let second = scan(std::slice::from_ref(&root_b));

    assert_eq!(first.tracks[0].id, second.tracks[0].id);
    assert_eq!(first.albums[0].id, second.albums[0].id);
}

#[test]
fn incremental_rescan_reuses_unchanged_files_and_reports_moves() {
    let tmp = TempDir::new().unwrap();
    let flac_mbid = make_flac(&[("TITLE", "One"), ("ARTIST", "Art"), ("ALBUM", "Alb"), ("MUSICBRAINZ_RECORDINGID", "mbid-unique-1")], None, 120);
    let flac_tags = make_flac(&[("TITLE", "Two"), ("ARTIST", "Art"), ("ALBUM", "Alb"), ("TRACKNUMBER", "2")], None, 150);
    let flac_stay = make_flac(&[("TITLE", "Three"), ("ARTIST", "Art"), ("ALBUM", "Alb"), ("TRACKNUMBER", "3")], None, 90);

    let p_mbid = write_file(tmp.path(), "Old/one.flac", &flac_mbid);
    let p_tags = write_file(tmp.path(), "Old/two.flac", &flac_tags);
    write_file(tmp.path(), "Keep/three.flac", &flac_stay);

    let cancel = AtomicBool::new(false);
    let mut progress_calls = 0;
    let pass1 = scan_folders(&[tmp.path().to_path_buf()], &ScanIndex::default(), &cancel, |_| progress_calls += 1);
    assert_eq!(pass1.rescanned, 3);
    assert_eq!(pass1.reused, 0);
    assert!(progress_calls >= 3);

    let index_path = tmp.path().join("state/scan.json");
    pass1.index.save(&index_path).unwrap();
    let reloaded = ScanIndex::load(&index_path).unwrap();

    // Rescan with no changes reuses everything.
    let pass2 = scan_folders(&[tmp.path().to_path_buf()], &reloaded, &cancel, |_| {});
    assert_eq!(pass2.reused, 3);
    assert_eq!(pass2.rescanned, 0);
    assert!(pass2.moves.is_empty());

    let old_mbid_id = pass1.tracks.iter().find(|t| t.title == "One").unwrap().id.clone();
    let old_tags_id = pass1.tracks.iter().find(|t| t.title == "Two").unwrap().id.clone();

    // Move one file by path (change title tag too so only MBID matches), move the second by renaming path only.
    let moved_mbid = make_flac(&[("TITLE", "One (Remastered)"), ("ARTIST", "Art"), ("ALBUM", "Alb"), ("MUSICBRAINZ_RECORDINGID", "mbid-unique-1")], None, 120);
    std::fs::remove_file(&p_mbid).unwrap();
    write_file(tmp.path(), "New/renamed_one.flac", &moved_mbid);

    std::fs::create_dir_all(tmp.path().join("New")).unwrap();
    std::fs::rename(&p_tags, tmp.path().join("New/renamed_two.flac")).unwrap();

    let pass3 = scan_folders(&[tmp.path().to_path_buf()], &pass2.index, &cancel, |_| {});
    assert_eq!(pass3.reused, 1);
    assert_eq!(pass3.rescanned, 2);
    let new_mbid_id = pass3.tracks.iter().find(|t| t.rel_path == "New/renamed_one.flac").unwrap().id.clone();
    let new_tags_id = pass3.tracks.iter().find(|t| t.rel_path == "New/renamed_two.flac").unwrap().id.clone();

    assert!(pass3.moves.contains(&TrackMove { from: old_mbid_id, to: new_mbid_id }));
    assert!(pass3.moves.contains(&TrackMove { from: old_tags_id, to: new_tags_id }));
}

#[test]
fn embedded_cover_beats_folder_art_and_cover_beats_folder_and_front() {
    let tmp = TempDir::new().unwrap();
    let emb_bytes = b"embedded-png-bytes";
    let dir_cover_bytes = b"cover-jpg-bytes";
    let dir_front_bytes = b"front-png-bytes";

    write_file(tmp.path(), "Both/cover.jpg", dir_cover_bytes);
    write_file(tmp.path(), "Both/front.png", dir_front_bytes);
    write_file(tmp.path(), "Both/01.flac", &make_flac(&[("ALBUM", "Both"), ("ARTIST", "A")], Some(emb_bytes), 30));
    write_file(tmp.path(), "Both/02.flac", &make_flac(&[("ALBUM", "Both"), ("ARTIST", "A")], None, 30));

    write_file(tmp.path(), "FolderOnly/front.png", dir_front_bytes);
    write_file(tmp.path(), "FolderOnly/folder.png", b"folder-png-bytes");
    write_file(tmp.path(), "FolderOnly/01.flac", &make_flac(&[("ALBUM", "FolderOnly"), ("ARTIST", "B")], None, 30));

    let res = scan(&[tmp.path().to_path_buf()]);
    let t1 = res.tracks.iter().find(|t| t.rel_path == "Both/01.flac").unwrap();
    let t2 = res.tracks.iter().find(|t| t.rel_path == "Both/02.flac").unwrap();
    let both_album = res.albums.iter().find(|a| a.name == "Both").unwrap();

    assert_eq!(
        read_cover(&res.index, t1.cover_art.as_deref().unwrap()).unwrap().as_deref(),
        Some(emb_bytes.as_slice()),
        "track with embedded art prefers embedded art"
    );
    assert_eq!(
        read_cover(&res.index, both_album.cover_art.as_deref().unwrap()).unwrap().as_deref(),
        Some(emb_bytes.as_slice()),
        "album with embedded art prefers embedded art over cover.jpg"
    );
    assert_eq!(
        read_cover(&res.index, t2.cover_art.as_deref().unwrap()).unwrap().as_deref(),
        Some(dir_cover_bytes.as_slice()),
        "track without embedded art falls back to cover.jpg"
    );

    let folder_album = res.albums.iter().find(|a| a.name == "FolderOnly").unwrap();
    assert_eq!(
        read_cover(&res.index, folder_album.cover_art.as_deref().unwrap()).unwrap().as_deref(),
        Some(b"folder-png-bytes".as_slice()),
        "folder.png beats front.png when cover.* is absent"
    );
}

#[test]
fn lrc_sidecar_beats_embedded_lyrics_and_parses_offset() {
    let tmp = TempDir::new().unwrap();
    let flac = make_flac(&[("TITLE", "Song"), ("ARTIST", "A"), ("ALBUM", "B"), ("LYRICS", "Embedded fallback\nSecond line")], None, 60);
    let with_lrc = write_file(tmp.path(), "A/with_lrc.flac", &flac);
    write_file(tmp.path(), "A/with_lrc.lrc", b"[offset:250]\n[00:10.50]Line one\n[00:20.00]Line two\n");

    let embedded_only = write_file(tmp.path(), "A/embedded_only.flac", &flac);

    assert_eq!(
        read_lyrics(&with_lrc).unwrap(),
        Some(TrackLyrics::Synced {
            offset_ms: Some(250),
            lines: vec![TimedLine { start_ms: 10_500, text: "Line one".into() }, TimedLine { start_ms: 20_000, text: "Line two".into() }],
        })
    );

    assert_eq!(read_lyrics(&embedded_only).unwrap(), Some(TrackLyrics::Plain("Embedded fallback\nSecond line".into())));
}
