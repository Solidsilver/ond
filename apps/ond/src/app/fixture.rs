//! The fixture library the UI tests and snapshots share: real-looking albums
//! with generated covers, a queue with several sources, a track part-way
//! through, devices and a signal path, and the harness helpers that reset
//! the window between scenes. No server, no audio device, no keychain;
//! settings and caches live in a temp dir.

use super::*;
use crate::queue::Item;
use audio::{DeviceCaps, DeviceInfo, DeviceKind, RateRange, SignalPath, StreamFormat};
use egui::{Color32, ColorImage, Pos2, Vec2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use library::Contributor;
use std::path::{Path, PathBuf};

pub(super) struct NullOutput;

impl audio::OutputBackend for NullOutput {
    fn name(&self) -> &str {
        "null"
    }
    fn device_caps(&mut self) -> Option<audio::DeviceCaps> {
        None
    }
    fn open(&mut self, _: audio::StreamFormat, _: audio::RingInput, _: Arc<audio::Status>) -> Result<(), audio::OutputError> {
        Ok(())
    }
    fn drained(&self) -> bool {
        true
    }
    fn close(&mut self) {}
}

pub(super) const FULL: Vec2 = vec2(1440.0, 900.0);

/// Queue end and start labels must not change with the time of a render.
pub(super) fn queue_time() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(15, 0, 0).unwrap()
}

/// (title, album artist, year, genre, label)
pub(super) const ALBUMS: &[(&str, &str, i32, &str, &str)] = &[
    ("Sunday at the Village Vanguard", "Bill Evans Trio", 1961, "Jazz", "Riverside"),
    ("Kind of Blue (Legacy Edition)", "Miles Davis", 1959, "Jazz", "Columbia"),
    ("Fingerlings", "Andrew Bird", 2002, "Indie Folk", "Grimsey"),
    ("The Mysterious Production of Eggs", "Andrew Bird", 2005, "Indie Folk", "Righteous Babe"),
    ("Selected Ambient Works 85–92", "Aphex Twin", 1992, "Electronic", "Apollo"),
    ("Vespertine", "Björk", 2001, "Art Pop", "One Little Indian"),
    ("async", "坂本龍一", 2017, "Ambient", "Commmons"),
    ("Les Misérables (Original Broadway Cast Recording) [Deluxe 25th Anniversary Edition]", "Original Broadway Cast", 1987, "Musical", "Geffen"),
    ("The Great Divide", "Luke Combs & Billy Strings", 2024, "Bluegrass", "Sony Nashville"),
    ("Live at the Legion", "Billy Strings & Bryan Sutton", 2023, "Bluegrass", "Rounder"),
    ("Listen to the Radio", "Billy Strings, Molly Tuttle, Sierra Hull, Bryan Sutton & Del McCoury", 2022, "Bluegrass", "Rounder"),
    ("The Hillbilly Thomists", "The Hillbilly Thomists", 2017, "Bluegrass", "Dominicana Records"),
    ("Marigold", "The Hillbilly Thomists", 2023, "Bluegrass", "Dominicana Records"),
    ("Moon Safari", "Air", 1998, "Electronic", "Virgin"),
    ("Remain in Light", "Talking Heads", 1980, "New Wave", "Sire"),
    ("In Rainbows", "Radiohead", 2007, "Alternative", "XL"),
    ("OK Computer", "Radiohead", 1997, "Alternative", "Parlophone"),
    ("Blue Lines", "Massive Attack", 1991, "Trip Hop", "Wild Bunch"),
    ("Rumours", "Fleetwood Mac", 1977, "Rock", "Warner Bros."),
    ("Back to Black", "Amy Winehouse", 2006, "Soul", "Island"),
    ("Giant Steps", "John Coltrane", 1960, "Jazz", "Atlantic"),
    ("Portrait in Jazz", "Bill Evans Trio", 1960, "Jazz", "Riverside"),
    ("Waltz for Debby", "Bill Evans Trio", 1962, "Jazz", "Riverside"),
    ("Everybody Digs Bill Evans", "Bill Evans", 1958, "Jazz", "Riverside"),
    ("Somethin' Else", "Cannonball Adderley", 1958, "Jazz", "Blue Note"),
    ("Discovery", "Daft Punk", 2001, "Electronic", "Virgin"),
    ("Random Access Memories", "Daft Punk", 2013, "Electronic", "Columbia"),
    ("“Heroes”", "David Bowie", 1977, "Rock", "RCA"),
    ("Blonde on Blonde", "Bob Dylan", 1966, "Folk Rock", "Columbia"),
    ("Highway 61 Revisited", "Bob Dylan", 1965, "Folk Rock", "Columbia"),
    ("Exodus", "Bob Marley & The Wailers", 1977, "Reggae", "Island"),
    ("Night Moves", "Bob Seger & the Silver Bullet Band", 1976, "Rock", "Capitol"),
    ("Timbuktu (Original Motion Picture Soundtrack)", "Amine Bouhafa", 2014, "Soundtrack", "Lakeshore"),
    ("Back to the Moon", "Johannes Motschmann, David Panzl, Boris Bolles", 2019, "Modern Classical", "Neue Meister"),
    ("Goldberg Variations, BWV 988 (1981 Recording)", "Glenn Gould", 1982, "Classical", "CBS Masterworks"),
    ("Pet Sounds", "The Beach Boys", 1966, "Pop", "Capitol"),
    ("Hounds of Love", "Kate Bush", 1985, "Art Pop", "EMI"),
    ("Songs of Leonard Cohen", "Leonard Cohen", 1967, "Folk", "Columbia"),
    ("A Love Supreme", "John Coltrane", 1965, "Jazz", "Impulse!"),
    ("Mezzanine", "Massive Attack", 1998, "Trip Hop", "Virgin"),
];

pub(super) const VANGUARD: &[(&str, &str, u32)] = &[
    ("Gloria's Step (Take 2)", "Scott LaFaro", 370),
    ("My Man's Gone Now", "George Gershwin", 383),
    ("Solar", "Miles Davis", 535),
    ("Alice in Wonderland (Take 2)", "Sammy Fain", 517),
    ("All of You (Take 2)", "Cole Porter", 500),
    ("Jade Visions (Take 2)", "Scott LaFaro", 233),
];

pub(super) const BLUE: &[(&str, u32)] = &[
    ("So What", 562),
    ("Freddie Freeloader", 586),
    ("Blue in Green", 337),
    ("All Blues", 693),
    ("Flamenco Sketches", 566),
    ("So What (Live at the Kurhaus, Scheveningen, Netherlands, April 9, 1960)", 750),
    ("On Green Dolphin Street", 588),
    ("Fran-Dance", 352),
    ("Stella by Starlight", 287),
    ("Love for Sale", 708),
];

pub(super) fn album_id(i: usize) -> String {
    format!("al-{i:02}")
}

pub(super) fn album(i: usize) -> Album {
    let (name, artist, year, genre, label) = ALBUMS[i];
    Album {
        id: album_id(i),
        name: name.into(),
        sort: None,
        artist: Some(artist.into()),
        artist_ref: None,
        year: Some(year),
        genre: Some(genre.into()),
        song_count: 10,
        duration_secs: 2700 + 61 * i as u32,
        cover_art: Some(album_id(i)),
        created: Some(format!("2026-10-{:02}", 22 - (i % 20))),
        labels: vec![label.into()],
        genres: vec![genre.into()],
        artists: vec![library::ArtistLink { id: None, name: artist.into() }],
    }
}

pub(super) fn person(role: &str, sub: Option<&str>, name: &str) -> Contributor {
    Contributor { role: role.into(), sub_role: sub.map(Into::into), name: name.into(), artist_id: None }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn track(id: String, album: usize, title: &str, artist: &str, n: u32, disc: u32, secs: u32, composer: Option<&str>, bits: u32, rate: u32) -> Track {
    let mut contributors = Vec::new();
    if album == 0 {
        contributors = vec![
            person("performer", Some("piano"), "Bill Evans"),
            person("performer", Some("double bass"), "Scott LaFaro"),
            person("performer", Some("drums"), "Paul Motian"),
            person("producer", None, "Orrin Keepnews"),
            person("engineer", None, "Dave Jones"),
        ];
    }
    if let Some(c) = composer {
        contributors.push(person("composer", None, c));
    }
    Track {
        artist: Some(artist.into()),
        track: Some(n),
        disc: Some(disc),
        duration_secs: Some(secs),
        suffix: Some("flac".into()),
        bit_depth: Some(bits),
        sample_rate: Some(rate),
        channels: Some(2),
        composer: composer.map(Into::into),
        contributors,
        cover_art: Some(album_id(album)),
        artists: vec![library::ArtistLink { id: None, name: artist.into() }],
        ..crate::test_tracks::track(&id, &album_id(album), title)
    }
}

pub(super) fn vanguard() -> Vec<Track> {
    VANGUARD.iter().enumerate().map(|(n, (t, c, s))| track(format!("vg-{n}"), 0, t, "Bill Evans Trio", n as u32 + 1, 1, *s, Some(c), 24, 96_000)).collect()
}

/// Two discs: the album, then the bonus disc.
pub(super) fn kind_of_blue() -> Vec<Track> {
    BLUE.iter()
        .enumerate()
        .map(|(n, (t, s))| {
            let (disc, no) = if n < 5 { (1, n + 1) } else { (2, n - 4) };
            track(format!("kb-{n}"), 1, t, "Miles Davis", no as u32, disc, *s, Some("Miles Davis"), 16, 44_100)
        })
        .collect()
}

pub(super) fn filler(album: usize, count: usize) -> Vec<Track> {
    let (_, artist, ..) = ALBUMS[album];
    (0..count)
        .map(|n| {
            track(
                format!("f{album}-{n}"),
                album,
                &format!("{} Part {}", ALBUMS[album].0.split(' ').next().unwrap_or("Song"), n + 1),
                artist,
                n as u32 + 1,
                1,
                200 + 17 * n as u32,
                None,
                16,
                44_100,
            )
        })
        .collect()
}

/// A cover: a two-tone diagonal gradient with a disc, different per album.
pub(super) fn cover(i: usize) -> ColorImage {
    const N: usize = 256;
    let hue = |h: f32, s: f32, v: f32| -> Color32 {
        let c = egui::ecolor::Hsva::new(h.fract(), s, v, 1.0);
        Color32::from(c)
    };
    let a = hue(i as f32 * 0.137, 0.55, 0.55);
    let b = hue(i as f32 * 0.137 + 0.12, 0.45, 0.22);
    let disc = hue(i as f32 * 0.137 + 0.5, 0.35, 0.9);
    let (cx, cy, r) = (N as f32 * (0.3 + 0.4 * ((i * 7 % 10) as f32 / 10.0)), N as f32 * 0.42, N as f32 * (0.14 + (i % 4) as f32 * 0.04));
    let mut pixels = Vec::with_capacity(N * N);
    for y in 0..N {
        for x in 0..N {
            let t = (x + y) as f32 / (2 * N) as f32;
            let mut px = a.lerp_to_gamma(b, t);
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            if d < r {
                px = disc;
            } else if d < r + 1.5 {
                px = px.lerp_to_gamma(disc, (r + 1.5 - d) / 1.5);
            }
            pixels.push(px);
        }
    }
    ColorImage::new([N, N], pixels)
}

/// A near-white cover (a pale, nearly blank sleeve with a faint emboss and
/// a small grey title): the hardest case for anything drawn over artwork.
pub(super) fn white_cover() -> ColorImage {
    const N: usize = 256;
    let mut pixels = Vec::with_capacity(N * N);
    for y in 0..N {
        for x in 0..N {
            let t = (x + y) as f32 / (2 * N) as f32;
            let mut px = Color32::from_rgb(250, 249, 246).lerp_to_gamma(Color32::from_rgb(236, 236, 231), t);
            let d = ((x as f32 - 128.0).powi(2) + (y as f32 - 120.0).powi(2)).sqrt();
            if (60.0..63.0).contains(&d) {
                px = px.lerp_to_gamma(Color32::from_rgb(222, 222, 218), 0.6);
            }
            if (200..206).contains(&y) && (88..168).contains(&x) {
                px = Color32::from_rgb(196, 196, 192);
            }
            pixels.push(px);
        }
    }
    ColorImage::new([N, N], pixels)
}

/// The cover id of [`white_cover`].
pub(super) const WHITE: &str = "white-cover";

/// Plays a few songs whose cover is [`white_cover`].
pub(super) fn play_white(app: &mut OndApp) {
    let mut songs = filler(13, 4);
    for t in &mut songs {
        t.cover_art = Some(WHITE.into());
    }
    app.player.queue.play_now(songs, 1, Source::Album { id: album_id(13), name: ALBUMS[13].0.into() });
}

pub(super) fn devices() -> audio::Event {
    let caps = |name: &str, bits, max, post: Option<&str>| DeviceCaps {
        name: name.into(),
        configs: vec![RateRange { channels: 2, min: 44_100, max }],
        max_bits: Some(bits),
        post_processed: post.map(Into::into),
        integer: true,
        ..Default::default()
    };
    let dev = |uid: &str, name: &str, kind, caps, is_default| DeviceInfo { uid: uid.into(), name: name.into(), kind, caps, is_default };
    audio::Event::Devices {
        devices: vec![
            dev("spk", "MacBook Pro Speakers", DeviceKind::BuiltInSpeakers, caps("MacBook Pro Speakers", 24, 96_000, Some("macOS speaker processing")), true),
            // A Mac's headphone jack is float-only.
            dev(
                "hp",
                "External Headphones",
                DeviceKind::BuiltInHeadphones,
                DeviceCaps { integer: false, ..caps("External Headphones", 24, 96_000, None) },
                false,
            ),
            dev("tv", "LG TV", DeviceKind::Hdmi, caps("LG TV", 24, 192_000, None), false),
            dev("umc", "UMC22", DeviceKind::Usb, caps("UMC22", 16, 48_000, None), false),
            dev("bh", "BlackHole 2ch", DeviceKind::Virtual, caps("BlackHole 2ch", 24, 768_000, None), false),
            dev("zoom", "ZoomAudioDevice", DeviceKind::Virtual, caps("ZoomAudioDevice", 24, 192_000, None), false),
        ],
        current: Some("umc".into()),
    }
}

pub(super) fn signal() -> audio::Event {
    audio::Event::SignalPath(SignalPath {
        output: "CoreAudio · UMC22".into(),
        source: StreamFormat { sample_rate: 96_000, channels: 2, bits_per_sample: Some(24) },
        device_rate: 48_000,
        device_bits: Some(16),
        resampled: true,
        dithered: true,
        bit_perfect: audio::BitPerfectStatus::Unavailable { reason: "device max 16/48, file is 24/96".into(), device_limit: true },
        channels: Default::default(),
        gain: Default::default(),
        transcoded: None,
    })
}

/// A signal path for the readout scenes: `source` bits/rate into `device`
/// bits/rate, with the given verdict and an optional transcode.
pub(super) fn signal_with(source: (u32, u32), device: (u32, u32), verdict: audio::BitPerfectStatus, transcoded: Option<&str>) -> audio::Event {
    audio::Event::SignalPath(SignalPath {
        output: "CoreAudio · External Headphones".into(),
        source: StreamFormat { sample_rate: source.1, channels: 2, bits_per_sample: Some(source.0) },
        device_rate: device.1,
        device_bits: Some(device.0),
        resampled: source.1 != device.1,
        dithered: source.0 > device.0,
        bit_perfect: verdict,
        channels: Default::default(),
        gain: Default::default(),
        transcoded: transcoded.map(Into::into),
    })
}

/// The app with the fixture library, queue and devices.
pub(super) fn populated(ctx: &egui::Context, dir: &Path) -> OndApp {
    let paths = Paths {
        config_file: dir.join("settings.json"),
        library_db: dir.join("library.redb"),
        stream_cache: dir.join("streams"),
        art_cache: dir.join("art"),
        offline: dir.join("offline"),
    };
    let mut app = OndApp::build(ctx, paths, Box::new(NullOutput)).unwrap();
    // Let start-up messages (cached albums from the empty db, device list from
    // the null output) land first, so they don't overwrite the fixture.
    std::thread::sleep(Duration::from_millis(300));
    while let Ok(m) = app.inbox.try_recv() {
        app.handle(m);
    }
    app.login = None;
    app.session.server_label = "demo · navidrome 0.64.2".into();
    app.sync.label = format!("{} albums", ALBUMS.len());

    let albums: Vec<Album> = (0..ALBUMS.len()).map(album).collect();
    for i in 0..ALBUMS.len() {
        app.art.preload(&album_id(i), cover(i));
    }
    app.art.preload(WHITE, white_cover());
    app.data.newest = albums.iter().map(|a| a.id.clone()).collect();
    app.data.set_albums(albums);
    app.rebuild_search();
    app.data.tracks.set_album(&album_id(0), vanguard());
    app.data.tracks.set_album(&album_id(1), kind_of_blue());
    for i in [2, 20, 23] {
        app.data.tracks.set_album(&album_id(i), filler(i, 9));
    }
    // The flat list the Songs view and search read (the per-album map stays as is).
    let mut all: Vec<Track> = app.data.tracks.by_album.values().flat_map(|s| s.iter()).cloned().collect();
    all.sort_by(|a, b| (&a.album_id, a.disc, a.track).cmp(&(&b.album_id, b.disc, b.track)));
    app.data.tracks.all = Arc::new(all);
    // A connected session: nothing is dimmed as "not downloaded".
    app.session.online = true;
    app.session.settings = Some(Settings { server: "https://music.example.com".into(), username: "demo".into(), ..Default::default() });
    app.session.server_info = "navidrome 0.64.2".into();
    app.data.playlists = ["Late night, low volume", "Headphone test tracks", "Sunday records", "Bluegrass Saturday", "Car"]
        .iter()
        .enumerate()
        .map(|(i, n)| subsonic::Playlist { id: format!("pl-{i}"), name: (*n).into(), song_count: 12, duration: 3000, cover_art: None, owner: None })
        .collect();
    // A playlist mixing albums, for its page and mosaic.
    let mut mix = filler(20, 3);
    mix.extend(kind_of_blue().into_iter().take(2));
    mix.extend(vanguard().into_iter().skip(2).take(3));
    mix.extend(filler(23, 2));
    app.data.playlist_tracks.insert("pl-0".into(), Arc::new(mix));
    // Loved: the playing track and another, an album, an artist; a rating.
    let vg = vanguard();
    let mut loved_songs = vec![vg[1].clone(), vg[4].clone()];
    loved_songs.extend(kind_of_blue().into_iter().take(3));
    app.favorites.load(
        crate::favorites::Starred {
            songs: loved_songs,
            albums: vec![album_id(0), album_id(1), album_id(5)],
            artists: vec![("ar-be".into(), "Bill Evans".into()), ("ar-md".into(), "Miles Davis".into())],
        },
        vec![(vg[1].id.clone(), 5), (vg[4].id.clone(), 4)],
    );
    app.data.credited.insert("Scott LaFaro".into(), vec![album_id(0), album_id(21), album_id(22)]);
    app.data.credited.insert("Bill Evans".into(), vec![album_id(0), album_id(1), album_id(21), album_id(22), album_id(23)]);

    // Kind of Blue measured and clean; the Vanguard (a 24/96 release of a
    // 1961 tape) flagged as upsampled, so both album-page variants show.
    analysed(&mut app, 1, None);
    let up = analysis::Verdict { kind: analysis::VerdictKind::Upsampled { from_hz: 44_100, cutoff_hz: 21_400 }, confidence: analysis::Confidence::High };
    analysed(&mut app, 0, Some(up));
    app.analysis.loaded = true;

    fill_queue(&mut app);
    app.output.handle(&devices());
    app.output.handle(&signal());
    app
}

/// Stored results for every track of `album`, with plausible spread.
pub(super) fn analysed(app: &mut OndApp, album: usize, verdict: Option<analysis::Verdict>) {
    let tracks = app.data.tracks.album(&album_id(album)).cloned().unwrap_or_default();
    for (i, t) in tracks.iter().enumerate() {
        let k = (i % 4) as f64;
        let a = analysis::TrackAnalysis {
            version: analysis::ANALYSIS_VERSION,
            duration_secs: f64::from(t.duration_secs.unwrap_or(0)),
            sample_rate: t.sample_rate.unwrap_or(44_100),
            channels: 2,
            bits: t.bit_depth,
            integrated_lufs: Some(-17.8 + k * 0.9),
            loudness_range_lu: Some(11.0 - k),
            true_peak_dbtp: Some(-2.6 + k * 0.4),
            sample_peak_dbfs: Some(-2.9 + k * 0.4),
            dr: Some(13.4 - k * 0.5),
            effective_bits: None,
            cutoff_hz: verdict.as_ref().map(|_| 21_400),
            verdicts: verdict.clone().into_iter().collect(),
        };
        app.analysis.records.insert(t.id.clone(), analysis::Record { stamp: crate::analyze::stamp_of(t), analysis: a });
        app.analysis.version += 1;
    }
}

pub(super) fn fill_queue(app: &mut OndApp) {
    let q = &mut app.player.queue;
    *q = crate::queue::Queue::default();
    let history = filler(2, 3);
    q.play_now(history, 0, Source::Album { id: album_id(2), name: ALBUMS[2].0.into() });
    q.advance();
    q.advance();
    q.play_now(vanguard(), 1, Source::Album { id: album_id(0), name: ALBUMS[0].0.into() });
    q.play_next(vec![filler(23, 1).remove(0), kind_of_blue().remove(2)]);
    let playlist = Source::Playlist { id: "pl-0".into(), name: "Late night, low volume".into() };
    for (k, t) in filler(20, 9).into_iter().enumerate() {
        q.upcoming.push(Item { uid: 10_000 + k as u64, track: t, source: playlist.clone(), next: false });
    }
    app.engine.status.preview(PlayState::Playing, 141.0, 383.0, 0.72);
}

pub(super) fn app_mut<'h>(h: &'h mut Harness<'_, Option<OndApp>>) -> &'h mut OndApp {
    h.state_mut().as_mut().expect("app is built")
}

pub(super) fn reset(h: &mut Harness<'_, Option<OndApp>>, size: Vec2, route: Route) {
    h.remove_cursor();
    h.set_size(size);
    // Popups (the output picker, menus) live in egui memory: close them so
    // one scene's open popup doesn't leak into the next.
    egui::Popup::close_all(&h.ctx);
    let app = app_mut(h);
    app.edit_windows(|w| w.mode = WindowMode::Full);
    app.edit_windows(|w| w.lyrics_open = false);
    app.ui.palette = None;
    app.ui.overlay = None;
    app.ui.fav_tab = favorites_page::FavTab::default();
    app.prefs.edit(|s| s.show_queue = true);
    app.ui.queue_sheet = false;
    app.analysis.runner.clear();
    app.ui.queue_ui = QueueUi::default();
    app.ui.np_details = false;
    app.ui.lyrics_follow = crate::lyrics::Follow::default();
    app.login = None;
    app.session.online = true;
    app.sync.albums = false;
    app.sync.tracks = false;
    app.sync.progress = None;
    app.ui.toasts = shell::kit::Toasts::default();
    app.session.unreachable = false;
    app.session.connecting = false;
    app.session.sign_in = None;
    app.session.work_offline = false;
    app.update = None;
    app.updates = Default::default();
    app.player.playing_from = None;
    app.diagnostics = super::diagnostics::Diagnostics::default();
    app.prefs.edit(|s| s.diagnostics_bar = false);
    app.diagnostics.set_latency(Some(Duration::from_millis(14)));
    app.output.handle(&signal());
    app.set_columns(super::layout::Columns::default());
    app.ui.album_details_open = false;
    app.prefs.edit(|s| {
        s.experimental = Default::default();
        s.adapt_colors = false;
        s.accent = Default::default();
        s.album_view = Default::default();
        s.artist_view = Default::default();
    });
    app.ui.tint = Default::default();
    app.ui.page_tint = Default::default();
    app.ui.page_tints = None;
    app.ui.nav = Nav::default();
    app.ui.nav.go(route);
    app.on_arrive();
    fill_queue(app);
    h.ctx.data_mut(|d| d.remove::<crate::visuals::colors::Tints>(crate::visuals::colors::id()));
    // Settle layout (measured header heights etc.) before anything is hovered.
    h.run_steps(3);
}

/// Colors adapt to the album art.
pub(super) fn adapt(h: &mut Harness<'_, Option<OndApp>>) {
    app_mut(h).prefs.edit(|s| s.adapt_colors = true);
}

/// Turns the visuals that can be turned on (Appearance's and Experimental's)
/// off, then `on` the ones a scene shows.
pub(super) fn looks(h: &mut Harness<'_, Option<OndApp>>, on: impl FnOnce(&mut crate::settings::Settings)) {
    app_mut(h).prefs.edit(|s| {
        s.backdrop = false;
        s.waveform = false;
        s.waveform_bars = Default::default();
        s.alpha_rail = false;
        s.experimental = Default::default();
        on(s);
    });
}

/// Waits for the playing cover's look and the album page's (worked out in
/// the background), then lets the colors finish easing in.
pub(super) fn settle_look(h: &mut Harness<'_, Option<OndApp>>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        h.step();
        let app = app_mut(h);
        let cover = app.player.queue.current.as_ref().and_then(|c| app.data.cover_for(&c.track).map(str::to_owned));
        let page = app.page_cover().flatten();
        if [cover, page].into_iter().flatten().all(|c| app.art.look(&c).is_some()) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the cover's look never arrived");
        std::thread::sleep(Duration::from_millis(10));
    }
    h.run_steps(6);
}

/// A made-up waveform for the playing track: a quiet intro, verses and
/// louder choruses, a break, and a fade at the end, with some grain.
pub(super) fn fixture_waveform(app: &mut OndApp) {
    let Some(item) = app.player.queue.current.as_ref() else { return };
    let secs = item.track.duration_secs.unwrap_or(240) as usize;
    let n = secs * analysis::peaks::PEAKS_PER_SEC as usize;
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let values = (0..n)
        .map(|i| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let t = i as f32 / n as f32;
            let section = match t {
                t if t < 0.08 => 0.25 + 2.0 * t,
                t if t < 0.55 => {
                    if ((t * 9.0) as u32).is_multiple_of(2) {
                        0.55
                    } else {
                        0.85
                    }
                }
                t if t < 0.62 => 0.3,
                t if t < 0.9 => {
                    if ((t * 11.0) as u32).is_multiple_of(2) {
                        0.65
                    } else {
                        0.95
                    }
                }
                t => 0.95 * (1.0 - t) * 10.0,
            };
            let grain = 0.7 + 0.3 * (seed % 1000) as f32 / 1000.0;
            (section * grain * 255.0).clamp(0.0, 255.0) as u8
        })
        .collect();
    let id = item.track.id.clone();
    app.waveforms.insert(id, analysis::Peaks { values });
}

/// Stands in for the output callback: hands the audio tap `frames` of made-up
/// music, as if they had just played: a chord that moves every two seconds,
/// a soft kick on the beat and a little noise. Deterministic: where it is in
/// the progression follows how much the tap has been given.
pub(super) fn feed_tap(app: &OndApp, frames: usize) {
    const RATE: u32 = 44_100;
    let tap = &app.engine.status.tap;
    let (start, _) = tap.read(&mut []);
    let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ start;
    let tones = [(55.0, 0.35), (110.0, 0.3), (220.0, 0.22), (330.0, 0.16), (440.0, 0.12), (660.0, 0.06), (1320.0, 0.03), (2640.0, 0.015), (5280.0, 0.006)];
    let mono: Vec<f32> = (0..frames)
        .map(|i| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let n = start + i as u64;
            let t = n as f32 / RATE as f32;
            let root = [1.0, 4.0 / 3.0, 3.0 / 2.0, 6.0 / 5.0][(n / (2 * RATE as u64)) as usize % 4];
            let beat = (t * 2.0).fract();
            // A kick drum, its pitch dropping from about 130 Hz to 50.
            let since = beat / 2.0;
            let kick = 0.9 * (-since * 14.0).exp() * (std::f32::consts::TAU * (50.0 * since + 3.2 * (1.0 - (-since * 25.0).exp()))).sin();
            let noise = ((seed % 2000) as f32 / 1000.0 - 1.0) * (0.002 + 0.02 * (-((t * 4.0).fract()) * 30.0).exp());
            tones.iter().map(|(hz, a)| a * (std::f32::consts::TAU * hz * root * t).sin()).sum::<f32>() * 0.8 + kick + noise
        })
        .collect();
    tap.write(&mono, &[], 1, RATE);
}

/// Steps `n` frames with fresh audio in the tap before each.
pub(super) fn steps_with_audio(h: &mut Harness<'_, Option<OndApp>>, n: usize) {
    for _ in 0..n {
        feed_tap(app_mut(h), 4096);
        h.step();
    }
}

/// Plays Kind of Blue instead (another cover than the fixture's).
pub(super) fn play_blue(app: &mut OndApp) {
    app.player.queue.play_now(kind_of_blue(), 2, Source::Album { id: album_id(1), name: ALBUMS[1].0.into() });
}

/// Hovers the first widget labelled `label` inside `area`, if any.
pub(super) fn hover_label(h: &mut Harness<'_, Option<OndApp>>, label: &str, area: egui::Rect) -> bool {
    let at: Option<Pos2> = h.query_all_by_label_contains(label).map(|n| n.rect()).find(|r| area.contains(r.center())).map(|r| r.center());
    match at {
        Some(p) => {
            h.hover_at(p);
            true
        }
        None => false,
    }
}

pub(super) fn click_label(h: &mut Harness<'_, Option<OndApp>>, label: &str, area: egui::Rect) -> bool {
    let at: Option<Pos2> = h.query_all_by_label_contains(label).map(|n| n.rect()).find(|r| area.contains(r.center())).map(|r| r.center());
    match at {
        Some(p) => {
            h.hover_at(p);
            h.run_steps(1);
            h.drag_at(p);
            h.run_steps(1);
            h.drop_at(p);
            true
        }
        None => false,
    }
}

/// Made-up words for the lyrics scenes (no real song's).
pub(super) const VERSES: [&str; 26] = [
    "Lamps come on along the water",
    "Somebody's radio two floors down",
    "",
    "I kept the window open for the rain",
    "It never came, the evening did",
    "Coffee cold beside the record sleeve",
    "Needle drifting in the run-out groove",
    "",
    "Hold the note a little longer",
    "Let the room decide when it is done",
    "The bass walks home without us",
    "We follow at a quarter time",
    "",
    "Every chair is facing somewhere else",
    "Every clock is running slow tonight",
    "Turn it down, the neighbours listen too",
    "They'd never say they like it",
    "",
    "Lamps go out along the water",
    "Somebody's radio signs off",
    "I leave the window open anyway",
    "In case the rain remembers",
    "",
    "Hold the note",
    "Let the room decide",
    "When it is done",
];

pub(super) use shell::chrome::stand_in_lights as traffic_lights;

/// The current track's lyrics, as the server would give them: one line every
/// eight seconds from 0:20, so 2:21 into the fixture lands mid-song.
pub(super) fn fixture_lyrics(app: &mut OndApp, synced: bool) {
    let Some(id) = app.player.queue.current.as_ref().map(|c| c.track.id.clone()) else { return };
    let lyrics = if synced {
        crate::lyrics::Lyrics::Synced(
            VERSES.iter().enumerate().map(|(i, v)| crate::lyrics::Line { start_ms: 20_000 + 8_000 * i as i64, text: (*v).into() }).collect(),
        )
    } else {
        crate::lyrics::Lyrics::Plain(VERSES.iter().map(|v| (*v).to_owned()).collect())
    };
    app.lyrics.insert(id, crate::lyrics::Fetch::Done(Some(lyrics)));
}

/// Lyrics as a scraper hands them over: a counter-and-title header glued to
/// the first marker, markers glued to lines, an ad, a trailing "Embed". The
/// words are the fixture's own.
pub(super) const SCRAPED: &str = "3 ContributorsMy Man's Gone Now Lyrics[Verse 1]\n\
Lamps come on along the water\n\
Somebody's radio two floors down\n\
I kept the window open for the rain[Chorus]\n\
Hold the note a little longer\n\
Let the room decide when it is done\n\
The bass walks home without us\n\
\n\
\n\
[Verse 2]\n\
Every chair is facing somewhere else\n\
Every clock is running slow tonightYou might also like[Bridge]\n\
See Bill Evans Trio LiveGet tickets as low as $45\n\
Turn it down, the neighbours listen too\n\
They'd never say they like it\n\
\n\
(Instrumental)\n\
\n\
[Outro]\n\
Lamps go out along the water\n\
In case the rain remembers12Embed";

/// The playing song's lyrics as they arrive from a scraper, cleaned the way
/// the app cleans them.
pub(super) fn scraped_lyrics(app: &mut OndApp) {
    let Some(id) = app.player.queue.current.as_ref().map(|c| c.track.id.clone()) else { return };
    app.lyrics.insert(id, crate::lyrics::Fetch::Done(crate::lyrics::from_text(SCRAPED)));
}

/// The playing song made as long-winded as the one in the user's screenshot:
/// a composer credit with a comma in it, and every ReplayGain value. Returns
/// the song as it was, to put back.
pub(super) fn long_info(app: &mut OndApp) -> Option<Item> {
    let saved = app.player.queue.current.clone();
    if let Some(cur) = app.player.queue.current.as_mut() {
        let t = &mut cur.track;
        t.id = "long-credits".into();
        t.title = "Leaning on the Everlasting Arms (Live at the Old Dominican Chapel)".into();
        t.contributors = vec![
            person("composer", None, "Anthony Johnson Showalter"),
            person("composer", None, "Elisha A. Hoffman, Anthony Johnson Showalter"),
            person("performer", Some("mandolin and harmony vocals"), "Brother Joseph-Anthony Kress, O.P."),
        ];
        t.track_gain = Some(-6.0);
        t.album_gain = Some(-7.0);
        t.track_peak = Some(0.987_654);
        t.size = Some(19_000_000);
        t.bit_rate_kbps = Some(899);
    }
    saved
}

/// Play stats for the playing song, as the server would report them.
pub(super) fn info_fixture(app: &mut OndApp) {
    let Some(id) = app.player.queue.current.as_ref().map(|c| c.track.id.clone()) else { return };
    app.song_stats.insert(id, Some(SongStats { plays: Some(14), last_played: Some("2026-10-21T21:40:00Z".into()) }));
}

/// A managed.json for the scenes and tests: a little from each section.
pub(super) const MANAGED: &str = r#"{
  "server": "https://music.example.com",
  "username": "demo",
  "scrobble": false,
  "accent": "teal",
  "alpha_rail": true,
  "waveform": true,
  "waveform_bars": { "width": 4 },
  "playback": { "replay_gain": "album", "kbps": 192 },
  "device_modes": { "umc": "bit-perfect" },
  "output_device": "umc",
  "offline_limit_gb": 50,
  "experimental": { "visualizer": true },
  "volume": 0.2
}"#;

/// `snap` as it would be after the list of downloads couldn't be read.
pub(super) fn set_aside_snapshot(snap: &offline::Snapshot) -> offline::Snapshot {
    let aside = offline::SetAside { files: vec![PathBuf::from("offline.json.bad-1761400000000")], unknown: 214, unknown_bytes: 6_480_000_000, removing: false };
    offline::Snapshot { set_aside: Some(aside), version: snap.version + 1, ..snap.clone() }
}

/// Scrolls the wheel by `dy` points over `at`, a little at a time.
pub(super) fn wheel(h: &mut Harness<'_, Option<OndApp>>, at: Pos2, dy: f32, steps: usize) {
    h.hover_at(at);
    h.step();
    for _ in 0..steps {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, dy / steps as f32),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Wheel,
        });
        h.run_steps(3);
    }
    h.run_steps(10);
}
