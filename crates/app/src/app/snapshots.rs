//! Review snapshots: renders the real views, offscreen through wgpu, to
//! `target/snapshots/<scene>-<theme>.png`. Not a regression test (nothing is
//! compared); it's for looking at the UI without running the app:
//!
//! ```sh
//! cargo test -p ond-player --release snapshots -- --ignored --nocapture
//! ```
//!
//! The data is a fixed, realistic library with generated cover art, a queue
//! with several sources and a track part-way through. No server, no audio
//! device, no keychain; settings and caches live in a temp dir.

use super::*;
use crate::queue::Item;
use audio::{DeviceCaps, DeviceInfo, DeviceKind, RateRange, SignalPath, StreamFormat};
use egui::{Color32, ColorImage, Pos2, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use library::Contributor;
use std::path::{Path, PathBuf};

struct NullOutput;

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

const FULL: Vec2 = vec2(1440.0, 900.0);

/// (title, album artist, year, genre, label)
const ALBUMS: &[(&str, &str, i32, &str, &str)] = &[
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

const VANGUARD: &[(&str, &str, u32)] = &[
    ("Gloria's Step (Take 2)", "Scott LaFaro", 370),
    ("My Man's Gone Now", "George Gershwin", 383),
    ("Solar", "Miles Davis", 535),
    ("Alice in Wonderland (Take 2)", "Sammy Fain", 517),
    ("All of You (Take 2)", "Cole Porter", 500),
    ("Jade Visions (Take 2)", "Scott LaFaro", 233),
];

const BLUE: &[(&str, u32)] = &[
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

fn album_id(i: usize) -> String {
    format!("al-{i:02}")
}

fn album(i: usize) -> Album {
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

fn person(role: &str, sub: Option<&str>, name: &str) -> Contributor {
    Contributor { role: role.into(), sub_role: sub.map(Into::into), name: name.into(), artist_id: None }
}

#[allow(clippy::too_many_arguments)]
fn track(id: String, album: usize, title: &str, artist: &str, n: u32, disc: u32, secs: u32, composer: Option<&str>, bits: u32, rate: u32) -> Track {
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
        id,
        album_id: album_id(album),
        title: title.into(),
        artist: Some(artist.into()),
        artist_ref: None,
        track: Some(n),
        disc: Some(disc),
        duration_secs: Some(secs),
        suffix: Some("flac".into()),
        content_type: None,
        size: None,
        bit_rate_kbps: None,
        bit_depth: Some(bits),
        sample_rate: Some(rate),
        channels: Some(2),
        track_gain: None,
        album_gain: None,
        track_peak: None,
        album_peak: None,
        composer: composer.map(Into::into),
        contributors,
        cover_art: Some(album_id(album)),
        artists: vec![library::ArtistLink { id: None, name: artist.into() }],
    }
}

fn vanguard() -> Vec<Track> {
    VANGUARD
        .iter()
        .enumerate()
        .map(|(n, (t, c, s))| track(format!("vg-{n}"), 0, t, "Bill Evans Trio", n as u32 + 1, 1, *s, Some(c), 24, 96_000))
        .collect()
}

/// Two discs: the album, then the bonus disc.
fn kind_of_blue() -> Vec<Track> {
    BLUE.iter()
        .enumerate()
        .map(|(n, (t, s))| {
            let (disc, no) = if n < 5 { (1, n + 1) } else { (2, n - 4) };
            track(format!("kb-{n}"), 1, t, "Miles Davis", no as u32, disc, *s, Some("Miles Davis"), 16, 44_100)
        })
        .collect()
}

fn filler(album: usize, count: usize) -> Vec<Track> {
    let (_, artist, ..) = ALBUMS[album];
    (0..count)
        .map(|n| track(format!("f{album}-{n}"), album, &format!("{} Part {}", ALBUMS[album].0.split(' ').next().unwrap_or("Song"), n + 1), artist, n as u32 + 1, 1, 200 + 17 * n as u32, None, 16, 44_100))
        .collect()
}

/// A cover: a two-tone diagonal gradient with a disc, different per album.
fn cover(i: usize) -> ColorImage {
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


fn devices() -> audio::Event {
    let caps = |name: &str, bits, max, post: Option<&str>| DeviceCaps {
        name: name.into(),
        configs: vec![RateRange { channels: 2, min: 44_100, max }],
        max_bits: Some(bits),
        post_processed: post.map(Into::into),
        ..Default::default()
    };
    let dev = |uid: &str, name: &str, kind, caps, is_default| DeviceInfo { uid: uid.into(), name: name.into(), kind, caps, is_default };
    audio::Event::Devices {
        devices: vec![
            dev("spk", "MacBook Pro Speakers", DeviceKind::BuiltInSpeakers, caps("MacBook Pro Speakers", 24, 96_000, Some("macOS speaker processing")), true),
            dev("hp", "External Headphones", DeviceKind::BuiltInHeadphones, caps("External Headphones", 24, 96_000, None), false),
            dev("umc", "UMC22", DeviceKind::Usb, caps("UMC22", 16, 48_000, None), false),
        ],
        current: Some("umc".into()),
    }
}

fn signal() -> audio::Event {
    audio::Event::SignalPath(SignalPath {
        output: "CoreAudio · UMC22".into(),
        source: StreamFormat { sample_rate: 96_000, channels: 2, bits_per_sample: Some(24) },
        device_rate: 48_000,
        device_bits: Some(16),
        resampled: true,
        dithered: true,
        bit_perfect: audio::BitPerfectStatus::Unavailable { reason: "device max 16/48, file is 24/96".into() },
        channels: Default::default(),
        gain: Default::default(),
        transcoded: None,
    })
}

/// A signal path for the readout scenes: `source` bits/rate into `device`
/// bits/rate, with the given verdict and an optional transcode.
fn signal_with(source: (u32, u32), device: (u32, u32), verdict: audio::BitPerfectStatus, transcoded: Option<&str>) -> audio::Event {
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
fn populated(ctx: &egui::Context, dir: &Path) -> OndApp {
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
        crate::favorites::Starred { songs: loved_songs, albums: vec![album_id(0), album_id(1), album_id(5)], artists: vec![("ar-be".into(), "Bill Evans".into()), ("ar-md".into(), "Miles Davis".into())] },
        vec![(vg[1].id.clone(), 5), (vg[4].id.clone(), 4)],
    );
    app.data.credited.insert("Scott LaFaro".into(), vec![album_id(0), album_id(21), album_id(22)]);
    app.data.credited.insert("Bill Evans".into(), vec![album_id(0), album_id(1), album_id(21), album_id(22), album_id(23)]);

    // Kind of Blue measured and clean; the Vanguard (a 24/96 release of a
    // 1961 tape) flagged as upsampled, so both album-page variants show.
    analysed(&mut app, 1, None);
    let up = analysis::Verdict {
        kind: analysis::VerdictKind::Upsampled { from_hz: 44_100, cutoff_hz: 21_400 },
        confidence: analysis::Confidence::High,
    };
    analysed(&mut app, 0, Some(up));
    app.analysis.loaded = true;

    fill_queue(&mut app);
    app.output.handle(&devices());
    app.output.handle(&signal());
    app
}

/// Stored results for every track of `album`, with plausible spread.
fn analysed(app: &mut OndApp, album: usize, verdict: Option<analysis::Verdict>) {
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

fn fill_queue(app: &mut OndApp) {
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
        q.upcoming.push(Item { uid: 10_000 + k as u64, track: t, source: playlist.clone() });
    }
    app.engine.status.preview(PlayState::Playing, 141.0, 383.0, 0.72);
}

struct Shooter {
    out: PathBuf,
    theme: &'static str,
}

impl Shooter {
    fn shoot(&self, h: &mut Harness<'_, Option<OndApp>>, scene: &str) {
        h.run_steps(4);
        self.shoot_now(h, scene);
    }

    /// Renders the frame as it is, without letting anything settle first
    /// (for the middle of a fade).
    fn shoot_now(&self, h: &mut Harness<'_, Option<OndApp>>, scene: &str) {
        match h.render() {
            Ok(img) => {
                let path = self.out.join(format!("{scene}-{}.png", self.theme));
                img.save(&path).unwrap_or_else(|e| panic!("saving {}: {e}", path.display()));
                println!("  {}", path.display());
            }
            Err(e) => panic!("rendering {scene}: {e}"),
        }
    }
}

fn app_mut<'h>(h: &'h mut Harness<'_, Option<OndApp>>) -> &'h mut OndApp {
    h.state_mut().as_mut().expect("app is built")
}

fn reset(h: &mut Harness<'_, Option<OndApp>>, size: Vec2, route: Route) {
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
    app.ui.toasts = super::toasts::Toasts::default();
    app.session.unreachable = false;
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
fn adapt(h: &mut Harness<'_, Option<OndApp>>) {
    app_mut(h).prefs.edit(|s| s.adapt_colors = true);
}

/// Turns on the experimental visuals `on` sets (the rest stay off).
fn experiment(h: &mut Harness<'_, Option<OndApp>>, on: impl FnOnce(&mut crate::visuals::Experimental)) {
    let mut x = crate::visuals::Experimental::default();
    on(&mut x);
    app_mut(h).prefs.edit(|s| s.experimental = x);
}

/// Waits for the playing cover's look and the album page's (worked out in
/// the background), then lets the colors finish easing in.
fn settle_look(h: &mut Harness<'_, Option<OndApp>>) {
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
fn fixture_waveform(app: &mut OndApp) {
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
                t if t < 0.55 => if ((t * 9.0) as u32).is_multiple_of(2) { 0.55 } else { 0.85 },
                t if t < 0.62 => 0.3,
                t if t < 0.9 => if ((t * 11.0) as u32).is_multiple_of(2) { 0.65 } else { 0.95 },
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
fn feed_tap(app: &OndApp, frames: usize) {
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
            let kick = 0.6 * (-beat * 18.0).exp() * (std::f32::consts::TAU * 50.0 * t).sin();
            let noise = ((seed % 2000) as f32 / 1000.0 - 1.0) * (0.002 + 0.02 * (-((t * 4.0).fract()) * 30.0).exp());
            tones.iter().map(|(hz, a)| a * (std::f32::consts::TAU * hz * root * t).sin()).sum::<f32>() * 0.8 + kick + noise
        })
        .collect();
    tap.write(&mono, &[], 1, RATE);
}

/// Steps `n` frames with fresh audio in the tap before each.
fn steps_with_audio(h: &mut Harness<'_, Option<OndApp>>, n: usize) {
    for _ in 0..n {
        feed_tap(app_mut(h), 4096);
        h.step();
    }
}

/// Plays Kind of Blue instead (another cover than the fixture's).
fn play_blue(app: &mut OndApp) {
    app.player.queue.play_now(kind_of_blue(), 2, Source::Album { id: album_id(1), name: ALBUMS[1].0.into() });
}

/// Hovers the first widget labelled `label` inside `area`, if any.
fn hover_label(h: &mut Harness<'_, Option<OndApp>>, label: &str, area: egui::Rect) -> bool {
    let at: Option<Pos2> = h.query_all_by_label_contains(label).map(|n| n.rect()).find(|r| area.contains(r.center())).map(|r| r.center());
    match at {
        Some(p) => {
            h.hover_at(p);
            true
        }
        None => false,
    }
}

fn click_label(h: &mut Harness<'_, Option<OndApp>>, label: &str, area: egui::Rect) -> bool {
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
const VERSES: [&str; 26] = [
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

/// Stand-ins for macOS's traffic lights, which the offscreen renderer can't
/// draw: three circles where `chrome` puts the real ones, to judge spacing.
fn traffic_lights(ctx: &egui::Context, bar_h: Option<f32>) {
    let Some(h) = bar_h else { return };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("snapshot-traffic-lights")));
    for (i, c) in [egui::Color32::from_rgb(0xFF, 0x5F, 0x57), egui::Color32::from_rgb(0xFE, 0xBC, 0x2E), egui::Color32::from_rgb(0x28, 0xC8, 0x40)].into_iter().enumerate() {
        painter.circle_filled(pos2(18.0 + 7.0 + 20.0 * i as f32, h / 2.0), 6.0, c);
    }
}

/// The current track's lyrics, as the server would give them: one line every
/// eight seconds from 0:20, so 2:21 into the fixture lands mid-song.
fn fixture_lyrics(app: &mut OndApp, synced: bool) {
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
const SCRAPED: &str = "3 ContributorsMy Man's Gone Now Lyrics[Verse 1]\n\
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
fn scraped_lyrics(app: &mut OndApp) {
    let Some(id) = app.player.queue.current.as_ref().map(|c| c.track.id.clone()) else { return };
    app.lyrics.insert(id, crate::lyrics::Fetch::Done(crate::lyrics::from_text(SCRAPED)));
}

/// The playing song made as long-winded as the one in the user's screenshot:
/// a composer credit with a comma in it, and every ReplayGain value. Returns
/// the song as it was, to put back.
fn long_info(app: &mut OndApp) -> Option<Item> {
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
fn info_fixture(app: &mut OndApp) {
    let Some(id) = app.player.queue.current.as_ref().map(|c| c.track.id.clone()) else { return };
    app.song_stats.insert(id, Some(SongStats { plays: Some(14), last_played: Some("2026-10-21T21:40:00Z".into()) }));
}

fn scenes(h: &mut Harness<'_, Option<OndApp>>, s: &Shooter) {
    // The Player: artwork, transport and the queue; lyrics in a drawer that
    // widens the window.
    let player = vec2(380.0, 880.0);
    reset(h, player, Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    fixture_lyrics(app_mut(h), true);
    h.run_steps(4);
    s.shoot(h, "player");

    reset(h, vec2(player.x + crate::window::DRAWER_W, player.y), Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    app_mut(h).edit_windows(|w| w.lyrics_open = true);
    fixture_lyrics(app_mut(h), true);
    h.run_steps(20);
    s.shoot(h, "player-lyrics");

    reset(h, vec2(player.x + crate::window::DRAWER_W, player.y), Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    app_mut(h).edit_windows(|w| w.lyrics_open = true);
    fixture_lyrics(app_mut(h), false);
    h.run_steps(4);
    s.shoot(h, "player-lyrics-plain");

    reset(h, player, Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    app_mut(h).ui.np_details = true;
    h.hover_at(pos2(190.0, 150.0));
    h.run_steps(4);
    s.shoot(h, "player-details-art-hover");

    // The full window's sidebar on its Lyrics tab.
    reset(h, FULL, Route::Recent);
    app_mut(h).ui.queue_ui.tab = QueueTab::Lyrics;
    fixture_lyrics(app_mut(h), true);
    h.run_steps(20);
    s.shoot(h, "sidebar-lyrics");

    reset(h, FULL, Route::Recent);
    app_mut(h).ui.queue_ui.tab = QueueTab::Lyrics;
    fixture_lyrics(app_mut(h), false);
    s.shoot(h, "sidebar-lyrics-plain");

    reset(h, FULL, Route::Recent);
    app_mut(h).ui.queue_ui.tab = QueueTab::Lyrics;
    if let Some(id) = app_mut(h).player.queue.current.as_ref().map(|c| c.track.id.clone()) {
        app_mut(h).lyrics.insert(id, crate::lyrics::Fetch::Done(None));
    }
    s.shoot(h, "sidebar-lyrics-none");

    // Unsynced lyrics straight from a scraper, after cleaning: stanzas under
    // section labels, no debris.
    reset(h, FULL, Route::Recent);
    app_mut(h).ui.queue_ui.tab = QueueTab::Lyrics;
    scraped_lyrics(app_mut(h));
    h.run_steps(2);
    s.shoot(h, "sidebar-lyrics-scraped");

    reset(h, vec2(player.x + crate::window::DRAWER_W, player.y), Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    app_mut(h).edit_windows(|w| w.lyrics_open = true);
    scraped_lyrics(app_mut(h));
    h.run_steps(4);
    s.shoot(h, "player-lyrics-scraped");

    reset(h, vec2(900.0, 700.0), Route::Recent);
    app_mut(h).ui.queue_ui.tab = QueueTab::Lyrics;
    app_mut(h).ui.queue_sheet = true;
    fixture_lyrics(app_mut(h), true);
    h.run_steps(20);
    s.shoot(h, "narrow-lyrics-sheet");

    reset(h, FULL, Route::Recent);
    // The LCD's artwork (left end of the display, centred on the window).
    h.hover_at(pos2(472.0, 26.0));
    s.shoot(h, "lcd-art-hover");

    // The LCD at a mid width (the right controls start to crowd the centre)
    // and with the pointer on its seek line.
    reset(h, vec2(1100.0, 760.0), Route::Recent);
    s.shoot(h, "lcd-1100");
    reset(h, FULL, Route::Recent);
    h.hover_at(pos2(720.0, 41.0));
    h.run_steps(2);
    s.shoot(h, "lcd-seek-hover");

    reset(h, FULL, Route::Recent);
    s.shoot(h, "recent");

    // Linux: our own window buttons in the toolbar (all three, as KDE has them).
    crate::csd::preview(&h.ctx, true);
    crate::csd::set_window(&h.ctx, true, true);
    reset(h, FULL, Route::Recent);
    s.shoot(h, "linux-titlebar");
    reset(h, vec2(380.0, 880.0), Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    h.run_steps(4);
    s.shoot(h, "linux-player-titlebar");
    crate::csd::preview(&h.ctx, false);
    crate::csd::set_window(&h.ctx, false, false);

    reset(h, FULL, Route::Albums);
    app_mut(h).ui.sort = AlbumSort::Year;
    s.shoot(h, "albums-by-year");

    reset(h, FULL, Route::Artist { name: "Bill Evans".into(), id: None });
    s.shoot(h, "artist");

    reset(h, FULL, Route::Album(album_id(0)));
    s.shoot(h, "album");

    reset(h, FULL, Route::Album(album_id(0)));
    let main = egui::Rect::from_min_max(pos2(216.0, 52.0), pos2(1120.0, 876.0));
    if !hover_label(h, "Solar", main) {
        h.hover_at(pos2(560.0, 520.0));
    }
    s.shoot(h, "album-track-hover");

    reset(h, FULL, Route::Album(album_id(1)));
    s.shoot(h, "album-two-discs");

    // The Info tab: the playing song's credits, album, file, analysis, plays.
    reset(h, FULL, Route::Album(album_id(0)));
    info_fixture(app_mut(h));
    app_mut(h).ui.queue_ui.tab = QueueTab::Info;
    h.run_steps(2);
    s.shoot(h, "info");

    // Narrow: the Info tab in the queue sheet.
    reset(h, vec2(900.0, 700.0), Route::Recent);
    info_fixture(app_mut(h));
    app_mut(h).ui.queue_ui.tab = QueueTab::Info;
    app_mut(h).ui.queue_sheet = true;
    h.run_steps(2);
    s.shoot(h, "narrow-info-sheet");

    // A song with little known about it, and nothing playing at all.
    reset(h, FULL, Route::Recent);
    let saved = app_mut(h).player.queue.current.clone();
    if let Some(cur) = app_mut(h).player.queue.current.as_mut() {
        let t = &mut cur.track;
        t.id = "sparse".into();
        t.contributors.clear();
        t.composer = None;
        t.bit_depth = None;
        t.track_gain = None;
        t.album_gain = None;
        t.track_peak = None;
        t.size = None;
    }
    app_mut(h).ui.queue_ui.tab = QueueTab::Info;
    h.run_steps(2);
    s.shoot(h, "info-sparse");
    app_mut(h).player.queue.current = None;
    h.run_steps(2);
    s.shoot(h, "info-nothing");
    app_mut(h).player.queue.current = saved;

    // Long credits and a long ReplayGain line wrap inside the column, at the
    // default width and at the narrowest; then a short song in the same app
    // shows nothing left over from the long one.
    reset(h, FULL, Route::Album(album_id(0)));
    let saved = long_info(app_mut(h));
    info_fixture(app_mut(h));
    app_mut(h).ui.queue_ui.tab = QueueTab::Info;
    h.run_steps(3);
    s.shoot(h, "info-long");
    { let mut c = app_mut(h).columns(); c.right = super::layout::RIGHT_MIN; app_mut(h).set_columns(c); }
    h.run_steps(3);
    s.shoot(h, "info-long-min");
    { let mut c = app_mut(h).columns(); c.right = super::layout::RIGHT_DEFAULT; app_mut(h).set_columns(c); }
    app_mut(h).player.queue.current = saved;
    info_fixture(app_mut(h));
    h.run_steps(3);
    s.shoot(h, "info-after-long");

    // Column edges: the sidebar's under the pointer, then both dragged wide
    // and narrow.
    reset(h, FULL, Route::Recent);
    h.hover_at(pos2(8.0 + super::layout::LEFT_DEFAULT, 400.0));
    h.run_steps(2);
    s.shoot(h, "sidebar-edge-hover");
    reset(h, FULL, Route::Recent);
    app_mut(h).set_columns(super::layout::Columns { left: super::layout::LEFT_MAX, right: super::layout::RIGHT_MAX });
    h.run_steps(3);
    s.shoot(h, "columns-wide");
    reset(h, FULL, Route::Recent);
    app_mut(h).set_columns(super::layout::Columns { left: super::layout::LEFT_MIN, right: super::layout::RIGHT_MIN });
    h.run_steps(3);
    s.shoot(h, "columns-narrow");

    // Album details unfolded: credits and the per-track measurements, and an
    // album not yet measured.
    // (A taller window, so the unfolded section is in view.)
    let tall = vec2(FULL.x, 1300.0);
    reset(h, tall, Route::Album(album_id(0)));
    app_mut(h).ui.album_details_open = true;
    h.run_steps(3);
    s.shoot(h, "album-details");
    reset(h, tall, Route::Album(album_id(2)));
    app_mut(h).ui.album_details_open = true;
    h.run_steps(3);
    s.shoot(h, "album-details-unanalysed");

    // The Player's queue area has the same Info tab.
    reset(h, vec2(380.0, 880.0), Route::Recent);
    info_fixture(app_mut(h));
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    app_mut(h).ui.queue_ui.tab = QueueTab::Info;
    h.run_steps(4);
    s.shoot(h, "player-info");

    reset(h, FULL, Route::Album(album_id(0)));
    let header = egui::Rect::from_min_max(pos2(216.0, 52.0), pos2(1120.0, 300.0));
    if !hover_label(h, "likely upsampled", header) {
        eprintln!("  (verdict chip not found by label)");
    }
    h.run_steps(30);
    s.shoot(h, "album-verdict-hover");


    reset(h, FULL, Route::Recent);
    let queue = egui::Rect::from_min_max(pos2(1120.0, 52.0), pos2(1440.0, 876.0));
    if !hover_label(h, "Blue in Green", queue) {
        h.hover_at(pos2(1280.0, 160.0));
    }
    s.shoot(h, "queue-hover");

    reset(h, FULL, Route::Album(album_id(0)));
    app_mut(h).actions.push(Action::OpenPalette);
    h.run_steps(2);
    {
        let app = app_mut(h);
        if let (Some(p), Some(index)) = (app.ui.palette.as_mut(), app.search.index.as_mut()) {
            p.query = "bob".into();
            p.refresh(index);
        }
    }
    s.shoot(h, "palette");

    reset(h, FULL, Route::Recent);
    let toolbar = egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1440.0, 52.0));
    if !click_label(h, "UMC22", toolbar) {
        eprintln!("  (output chip not found by label; picker scene shows the closed chip)");
    }
    s.shoot(h, "output-picker");

    reset(h, FULL, Route::Recent);
    app_mut(h).engine.status.preview(PlayState::Buffering, 141.0, 383.0, 0.38);
    s.shoot(h, "buffering");

    // Library actions: playlists, favorites, menus, sheets.
    reset(h, FULL, Route::Playlist { id: "pl-0".into(), name: "Late night, low volume".into() });
    s.shoot(h, "playlist");

    reset(h, FULL, Route::Album(album_id(0)));
    // Right-click a track, then open "Add to playlist".
    if let Some(row) = h.query_all_by_label_contains("Solar").find(|n| main.contains(n.rect().center())) {
        row.click_secondary();
    }
    h.run_steps(3);
    let sub: Option<Pos2> = h.query_all_by_label_contains("Add to playlist").map(|n| n.rect().center()).next();
    match sub {
        Some(p) => {
            h.hover_at(p);
            h.run_steps(6);
        }
        None => eprintln!("  (track menu didn't open; add-to-playlist shows the page)"),
    }
    s.shoot(h, "add-to-playlist");

    // The same menu's Rating submenu: stars drawn as icons.
    reset(h, FULL, Route::Album(album_id(0)));
    if let Some(row) = h.query_all_by_label_contains("Solar").find(|n| main.contains(n.rect().center())) {
        row.click_secondary();
    }
    h.run_steps(3);
    let rating: Option<Pos2> = h.query_all_by_label("Rating").map(|n| n.rect().center()).next();
    match rating {
        Some(p) => {
            h.hover_at(p);
            h.run_steps(6);
        }
        None => eprintln!("  (track menu didn't open; rating-menu shows the page)"),
    }
    s.shoot(h, "rating-menu");

    reset(h, FULL, Route::Favorites);
    s.shoot(h, "favorites");

    reset(h, FULL, Route::Favorites);
    app_mut(h).ui.fav_tab = favorites_page::FavTab::Albums;
    s.shoot(h, "favorites-albums");

    reset(h, FULL, Route::Favorites);
    let loved = {
        let app = app_mut(h);
        app.ui.fav_tab = favorites_page::FavTab::Artists;
        let loved = std::mem::take(&mut app.favorites);
        app.favorites.load(crate::favorites::Starred::default(), Vec::new());
        loved
    };
    s.shoot(h, "favorites-empty");
    app_mut(h).favorites = loved;

    reset(h, FULL, Route::Album(album_id(0)));
    app_mut(h).ui.overlay = Some(Overlay::Shortcuts);
    s.shoot(h, "shortcuts");

    reset(h, vec2(900.0, 640.0), Route::Album(album_id(0)));
    app_mut(h).ui.overlay = Some(Overlay::Shortcuts);
    s.shoot(h, "narrow-shortcuts");

    reset(h, FULL, Route::Album(album_id(0)));
    app_mut(h).ui.overlay = Some(Overlay::new_playlist(vec![vanguard().remove(2)]));
    s.shoot(h, "new-playlist");

    reset(h, FULL, Route::Playlist { id: "pl-0".into(), name: "Late night, low volume".into() });
    app_mut(h).ui.overlay = Some(Overlay::Delete { id: "pl-0".into(), name: "Late night, low volume".into() });
    s.shoot(h, "delete-playlist");

    // The sidebar footer: server status (click for details), shortcuts, settings.
    reset(h, FULL, Route::Recent);
    let sidebar = egui::Rect::from_min_max(pos2(0.0, 600.0), pos2(216.0, 900.0));
    if !click_label(h, "Connected to", sidebar) {
        eprintln!("  (server status not found by label)");
    }
    h.run_steps(3);
    s.shoot(h, "server-popover");

    reset(h, FULL, Route::Recent);
    if !hover_label(h, "Keyboard shortcuts", sidebar) {
        eprintln!("  (help button not found by label)");
    }
    h.run_steps(30);
    s.shoot(h, "footer-help-hover");

    reset(h, FULL, Route::Recent);
    app_mut(h).session.online = false;
    s.shoot(h, "footer-connecting");

    // The diagnostics bar, on, while playing and syncing; and at a narrow width.
    reset(h, FULL, Route::Recent);
    {
        let app = app_mut(h);
        app.prefs.edit(|s| s.diagnostics_bar = true);
        app.sync.tracks = true;
        app.sync.progress = Some(crate::sync::Progress { done: 412, total: 1866, unit: crate::sync::Unit::Tracks });
        app.diagnostics.note_frame(Duration::from_micros(1800));
    }
    h.run_steps(2);
    s.shoot(h, "diagnostics-bar");
    let bar = egui::Rect::from_min_max(pos2(0.0, 870.0), pos2(1440.0, 900.0));
    if !hover_label(h, "Engine:", bar) {
        eprintln!("  (diagnostics segment not found by label)");
    }
    h.run_steps(30);
    s.shoot(h, "diagnostics-bar-hover");

    reset(h, vec2(900.0, 700.0), Route::Recent);
    app_mut(h).prefs.edit(|s| s.diagnostics_bar = true);
    h.run_steps(2);
    s.shoot(h, "narrow-diagnostics-bar");

    reset(h, FULL, Route::Recent);
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::General));
    s.shoot(h, "settings-general-diagnostics");

    // Another accent color: everything accented takes it, in either theme.
    reset(h, FULL, Route::Album(album_id(0)));
    app_mut(h).prefs.edit(|s| s.accent = theme::Accent::Teal);
    s.shoot(h, "accent-teal-album");
    // The user's own (a bright yellow, darkened or lightened to read), and the picker.
    reset(h, FULL, Route::Recent);
    app_mut(h).prefs.edit(|s| s.accent = theme::Accent::Custom([0xF5, 0xC5, 0x18]));
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::General));
    h.run_steps(2);
    h.get_by_label("Custom color…").click();
    s.shoot(h, "settings-accent-picker");
    egui::Popup::close_all(&h.ctx);

    for section in settings_page::Section::ALL {
        reset(h, FULL, Route::Recent);
        app_mut(h).actions.push(Action::OpenSettings(section));
        s.shoot(h, &format!("settings-{}", section.label().to_lowercase()));
    }

    // Playback with ReplayGain and a transcode on, so every control shows.
    reset(h, FULL, Route::Recent);
    {
        let app = app_mut(h);
        let mut p = app.output.prefs().clone();
        p.replay_gain = crate::playback::RgMode::Auto;
        p.quality = crate::playback::QualityMode::Auto;
        app.actions.push(Action::SetPlaybackPrefs(p));
        app.actions.push(Action::OpenSettings(settings_page::Section::Playback));
    }
    s.shoot(h, "settings-playback-on");

    // A library run waiting on playback (the preview status is mid-download).
    reset(h, FULL, Route::Recent);
    {
        let app = app_mut(h);
        let jobs: Vec<_> = (0..40).map(|i| crate::analyze::Job { track_id: format!("j{i}"), url: String::new(), suffix: None, stamp: "s".into(), secs: 240.0, local: None }).collect();
        app.analysis.runner.enqueue(jobs);
        app.actions.push(Action::OpenSettings(settings_page::Section::Library));
    }
    s.shoot(h, "settings-analysing");

    reset(h, vec2(900.0, 640.0), Route::Recent);
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::Output));
    s.shoot(h, "narrow-settings");

    // Places and their empty states.
    reset(h, FULL, Route::Songs);
    s.shoot(h, "songs");

    reset(h, FULL, Route::Songs);
    let all = std::mem::take(&mut app_mut(h).data.tracks.all);
    s.shoot(h, "songs-empty");
    app_mut(h).data.tracks.all = all;

    reset(h, FULL, Route::Offline);
    s.shoot(h, "offline-empty");

    reset(h, FULL, Route::Artists);
    s.shoot(h, "artists");

    reset(h, FULL, Route::Playlist { id: "pl-4".into(), name: "Car".into() });
    app_mut(h).data.playlist_tracks.insert("pl-4".into(), Arc::default());
    s.shoot(h, "playlist-empty");

    reset(h, vec2(900.0, 700.0), Route::Recent);
    s.shoot(h, "narrow-recent");

    reset(h, vec2(900.0, 700.0), Route::Recent);
    app_mut(h).actions.push(Action::ToggleQueue);
    s.shoot(h, "narrow-queue-sheet");

    reset(h, vec2(760.0, 640.0), Route::Albums);
    s.shoot(h, "narrow-rail");

    reset(h, vec2(900.0, 700.0), Route::Album(album_id(7)));
    s.shoot(h, "narrow-long-album");


    reset(h, vec2(280.0, 280.0), Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Mini);
    s.shoot(h, "mini");
    h.hover_at(pos2(140.0, 140.0));
    h.run_steps(6);
    s.shoot(h, "mini-hover");

    reset(h, FULL, Route::Recent);
    {
        let app = app_mut(h);
        app.player.queue = crate::queue::Queue::default();
        app.engine.status.preview(PlayState::Stopped, 0.0, 0.0, 0.0);
    }
    s.shoot(h, "empty-queue");

    experimental_scenes(h, s);


    // No status bar: the signal path lives in the now-playing display.
    let lcd = egui::Rect::from_min_max(pos2(400.0, 0.0), pos2(1000.0, 52.0));
    for (scene, ev) in [
        ("lcd-limit", signal()),
        ("lcd-verified", signal_with((16, 44_100), (16, 44_100), audio::BitPerfectStatus::Verified, None)),
        ("lcd-normal", signal_with((24, 96_000), (24, 96_000), audio::BitPerfectStatus::Off, None)),
        ("lcd-requested", signal_with((16, 44_100), (16, 44_100), audio::BitPerfectStatus::Requested { reason: "shared mode".into() }, None)),
        ("lcd-transcoded", signal_with((16, 44_100), (16, 44_100), audio::BitPerfectStatus::Off, Some("AAC 256"))),
    ] {
        reset(h, FULL, Route::Recent);
        app_mut(h).output.handle(&ev);
        s.shoot(h, scene);
    }
    reset(h, FULL, Route::Recent);
    if !hover_label(h, "Signal path", lcd) {
        eprintln!("  (readout not found by label)");
    }
    h.run_steps(30);
    s.shoot(h, "lcd-readout-hover");

    reset(h, FULL, Route::Recent);
    {
        let app = app_mut(h);
        app.sync.tracks = true;
        app.sync.progress = Some(crate::sync::Progress { done: 412, total: 1866, unit: crate::sync::Unit::Tracks });
    }
    s.shoot(h, "sidebar-syncing");
    let side = egui::Rect::from_min_max(pos2(0.0, 700.0), pos2(216.0, 900.0));
    if !hover_label(h, "Syncing tracks", side) {
        eprintln!("  (sync status not found by label)");
    }
    h.run_steps(30);
    s.shoot(h, "sidebar-syncing-hover");

    reset(h, FULL, Route::Recent);
    app_mut(h).sync.albums = true;
    s.shoot(h, "sidebar-syncing-albums");

    reset(h, FULL, Route::Recent);
    app_mut(h).session.online = false;
    app_mut(h).session.unreachable = true;
    s.shoot(h, "offline-pill");

    reset(h, vec2(760.0, 640.0), Route::Albums);
    {
        let app = app_mut(h);
        app.sync.tracks = true;
        app.sync.progress = Some(crate::sync::Progress { done: 1200, total: 1866, unit: crate::sync::Unit::Tracks });
    }
    s.shoot(h, "narrow-rail-syncing");

    reset(h, FULL, Route::Album(album_id(0)));
    app_mut(h).show_error("Couldn't load Sunday at the Village Vanguard: the server didn't answer.");
    h.run_steps(1);
    app_mut(h).show_notice("Skipped 1 song that isn't downloaded");
    h.run_steps(1);
    app_mut(h).show_notice("Added 6 songs to Late night, low volume");
    s.shoot(h, "toasts");

    reset(h, vec2(380.0, 880.0), Route::Recent);
    {
        let app = app_mut(h);
        app.edit_windows(|w| w.mode = WindowMode::Player);
        app.output.handle(&signal_with((16, 44_100), (16, 44_100), audio::BitPerfectStatus::Verified, None));
    }
    s.shoot(h, "player-verified");

    reset(h, FULL, Route::Recent);
    app_mut(h).login = Some(LoginForm { server: "http://localhost:4533".into(), ..Default::default() });
    s.shoot(h, "login");

    reset(h, FULL, Route::Recent);
    app_mut(h).login = Some(LoginForm {
        server: "https://music.example.com".into(),
        username: "demo".into(),
        error: Some(crate::bridge::connect_error(&subsonic::Error::Api { code: 40, message: "Wrong username or password".into() })),
        ..Default::default()
    });
    s.shoot(h, "login-error");

    reset(h, FULL, Route::Recent);
    app_mut(h).login = Some(LoginForm { server: "https://music.example.com".into(), username: "demo".into(), busy: true, ..Default::default() });
    s.shoot(h, "login-busy");
}

/// The experimental visuals, each on its own.
fn experimental_scenes(h: &mut Harness<'_, Option<OndApp>>, s: &Shooter) {
    let player = vec2(380.0, 880.0);
    // Colors from the album art: the Player, the now-playing display and the
    // accent (links, the selected tab), for a warm cover and a cool one.
    for (blue, name) in [(false, "x-colors"), (true, "x-colors-alt")] {
        reset(h, player, Route::Recent);
        app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
        if blue {
            play_blue(app_mut(h));
        }
        adapt(h);
        settle_look(h);
        s.shoot(h, &format!("{name}-player"));
        reset(h, FULL, Route::Album(album_id(if blue { 1 } else { 0 })));
        if blue {
            play_blue(app_mut(h));
        }
        adapt(h);
        settle_look(h);
        s.shoot(h, &format!("{name}-full"));
    }

    // An album page in its own album's colors while another album plays:
    // the now-playing display, the transport and the queue keep the playing one's.
    reset(h, FULL, Route::Album(album_id(20)));
    adapt(h);
    settle_look(h);
    s.shoot(h, "colors-album-page");

    // Blurred artwork: behind the Player (alone, and with the art's colors),
    // under Mini's controls, and halfway through the fade to the next cover.
    reset(h, player, Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    experiment(h, |x| x.backdrop = true);
    settle_look(h);
    s.shoot(h, "x-backdrop-player");
    reset(h, player, Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    play_blue(app_mut(h));
    experiment(h, |x| x.backdrop = true);
    adapt(h);
    settle_look(h);
    s.shoot(h, "x-backdrop-colors-player");
    // The next cover arrives: two steps (half a second) into the fade.
    {
        let app = app_mut(h);
        app.player.queue.play_now(vanguard(), 3, Source::Album { id: album_id(0), name: ALBUMS[0].0.into() });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while app.art.look(&album_id(0)).is_none() {
            assert!(std::time::Instant::now() < deadline, "the next cover's look never arrived");
            std::thread::sleep(Duration::from_millis(10));
            app.art.poll();
        }
    }
    h.run_steps(2);
    s.shoot_now(h, "x-backdrop-crossfade");
    reset(h, vec2(280.0, 280.0), Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Mini);
    experiment(h, |x| x.backdrop = true);
    settle_look(h);
    h.hover_at(pos2(140.0, 140.0));
    h.run_steps(6);
    s.shoot(h, "x-backdrop-mini-hover");

    // The waveform seek bar: in the Player and the now-playing display, with
    // the pointer on it, and before its peaks are ready (the plain line).
    reset(h, player, Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    experiment(h, |x| x.waveform = true);
    s.shoot(h, "x-waveform-loading-player");
    fixture_waveform(app_mut(h));
    s.shoot(h, "x-waveform-player");
    reset(h, FULL, Route::Recent);
    experiment(h, |x| x.waveform = true);
    fixture_waveform(app_mut(h));
    s.shoot(h, "x-waveform-full");
    h.hover_at(pos2(820.0, 41.0));
    h.run_steps(2);
    s.shoot(h, "x-waveform-lcd-hover");

    // Everything that's still on the page at once, in the Player.
    reset(h, player, Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    experiment(h, |x| {
        x.backdrop = true;
        x.waveform = true;
    });
    adapt(h);
    fixture_waveform(app_mut(h));
    settle_look(h);
    s.shoot(h, "x-together-player");

    // The live spectrum: along the foot of the Player's artwork, and faint
    // behind the now-playing display's text.
    reset(h, player, Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    experiment(h, |x| x.spectrum = true);
    steps_with_audio(h, 4);
    s.shoot_now(h, "x-spectrum-player");
    reset(h, FULL, Route::Recent);
    experiment(h, |x| x.spectrum = true);
    steps_with_audio(h, 4);
    s.shoot_now(h, "x-spectrum-full");
    reset(h, FULL, Route::Recent);
    experiment(h, |x| {
        x.spectrum = true;
        x.waveform = true;
    });
    adapt(h);
    fixture_waveform(app_mut(h));
    settle_look(h);
    steps_with_audio(h, 4);
    s.shoot_now(h, "x-spectrum-colors-full");

    // The visualizer, on the GPU: two seconds in (the title still showing),
    // then later with the trails built up; in the album's colors and in
    // another album's; and the painter version used without wgpu.
    let frames = |h: &mut Harness<'_, Option<OndApp>>, n: usize| {
        for _ in 0..n {
            // At 60 frames a second, as the app draws it.
            let t = h.ctx.input(|i| i.time) + 1.0 / 60.0;
            h.input_mut().time = Some(t);
            feed_tap(app_mut(h), 4096);
            h.step();
            // Each render is one GPU frame: the feedback builds up across them.
            let _ = h.render();
        }
    };
    let wide = vec2(1280.0, 800.0);
    reset(h, wide, Route::Recent);
    experiment(h, |x| x.visualizer = true);
    settle_look(h);
    app_mut(h).toggle_visualizer();
    frames(h, 90);
    s.shoot_now(h, "x-visualizer-title");
    frames(h, 240);
    s.shoot_now(h, "x-visualizer");
    reset(h, wide, Route::Recent);
    play_blue(app_mut(h));
    experiment(h, |x| x.visualizer = true);
    settle_look(h);
    app_mut(h).toggle_visualizer();
    frames(h, 300);
    s.shoot_now(h, "x-visualizer-alt");
    app_mut(h).close_visualizer();
    reset(h, wide, Route::Recent);
    experiment(h, |x| x.visualizer = true);
    settle_look(h);
    let gpu = app_mut(h).gpu.take();
    app_mut(h).toggle_visualizer();
    frames(h, 300);
    s.shoot_now(h, "x-visualizer-painter");
    app_mut(h).close_visualizer();
    app_mut(h).gpu = gpu;
    // The spectrogram in the Info tab, full window and Player, after a few
    // seconds of the fixture chord.
    for (size, player, name) in [(FULL, false, "x-spectrogram-full"), (player, true, "x-spectrogram-player")] {
        reset(h, size, Route::Recent);
        if player {
            app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
        }
        app_mut(h).ui.queue_ui.tab = QueueTab::Info;
        app_mut(h).ui.spectrogram.clear();
        experiment(h, |x| x.spectrogram = true);
        frames(h, 150);
        s.shoot_now(h, name);
    }
    // What a GPU frame costs with the visualizer against the ordinary window,
    // rendered offscreen and read back (the read-back is in both).
    let render_ms = |h: &mut Harness<'_, Option<OndApp>>| {
        frames(h, 5);
        let t = std::time::Instant::now();
        frames(h, 30);
        t.elapsed().as_secs_f64() * 1000.0 / 30.0
    };
    reset(h, wide, Route::Recent);
    let plain = render_ms(h);
    experiment(h, |x| x.visualizer = true);
    app_mut(h).toggle_visualizer();
    let vis = render_ms(h);
    app_mut(h).close_visualizer();
    println!("  render + read-back per frame at {}x{} px: window {plain:.2} ms, visualizer {vis:.2} ms", wide.x * 2.0, wide.y * 2.0);
    // The shortcut list shows the visualizer's key only while it's on.
    reset(h, FULL, Route::Recent);
    experiment(h, |x| x.visualizer = true);
    app_mut(h).ui.overlay = Some(Overlay::Shortcuts);
    s.shoot(h, "x-visualizer-shortcuts");
}

#[test]
#[ignore = "renders PNGs for review; run with --ignored"]
fn snapshots() {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let out = out.canonicalize().unwrap();
    for (theme, name) in [(egui::Theme::Light, "light"), (egui::Theme::Dark, "dark")] {
        let dir = tempfile::tempdir().unwrap();
        // Our own render state (the harness's default one, made here), so the
        // app knows the format it renders to, for the visualizer's pipelines.
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), eframe::egui_wgpu::RendererOptions::PREDICTABLE);
        let format = rs.target_format;
        let mut h = Harness::builder()
            .with_size(FULL)
            .with_pixels_per_point(2.0)
            .with_theme(theme)
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_os(egui::os::OperatingSystem::Mac)
            .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs))
            .build_ui_state(
                |ui, app: &mut Option<OndApp>| {
                    if let Some(app) = app {
                        if app.login.is_some() {
                            app.login_ui(ui);
                        } else {
                            app.main_ui(ui);
                        }
                        traffic_lights(ui.ctx(), app.title_bar_h());
                    }
                },
                None,
            );
        let ctx = h.ctx.clone();
        let mut app = populated(&ctx, dir.path());
        app.gpu = Some(format);
        // `build` installs the theme with the saved (system) preference.
        theme::set_pref(&ctx, if theme == egui::Theme::Dark { ThemePref::Dark } else { ThemePref::Light });
        *h.state_mut() = Some(app);
        println!("{name}:");
        scenes(&mut h, &Shooter { out: out.clone(), theme: name });
        // Drop the app (and its engine/runtime) before the next theme.
        *h.state_mut() = None;
    }
}

/// A song with long credits and a long ReplayGain line in the Info tab, then a
/// short one in the same app: at both column widths everything stays inside
/// the queue column (nothing widened it, nothing scrolled it sideways).
#[test]
fn info_stays_inside_its_column_after_a_long_song() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::builder()
        .with_size(FULL)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut Option<OndApp>| {
                if let Some(app) = app {
                    app.main_ui(ui);
                }
            },
            None,
        );
    let ctx = h.ctx.clone();
    *h.state_mut() = Some(populated(&ctx, dir.path()));
    let check = |h: &mut Harness<'_, Option<OndApp>>, what: &str| {
        let right = app_mut(h).columns().right;
        let column = egui::Rect::from_x_y_ranges(FULL.x - right..=FULL.x, 0.0..=FULL.y);
        for label in ["Up next", "Info", "Elisha A. Hoffman", "Anthony Johnson Showalter"] {
            for n in h.query_all_by_label_contains(label) {
                let r = n.rect();
                if r.center().x < FULL.x - right - 40.0 {
                    continue; // the same word elsewhere in the window
                }
                assert!(column.contains(r.min) && r.max.x <= column.max.x + 0.5, "{what}: “{label}” at {r:?} outside the column {column:?}");
            }
        }
    };
    for right in [super::layout::RIGHT_DEFAULT, super::layout::RIGHT_MIN] {
        reset(&mut h, FULL, Route::Album(album_id(0)));
        { let mut c = app_mut(&mut h).columns(); c.right = right; app_mut(&mut h).set_columns(c); }
        let saved = long_info(app_mut(&mut h));
        app_mut(&mut h).ui.queue_ui.tab = QueueTab::Info;
        h.run_steps(4);
        check(&mut h, "long song");
        app_mut(&mut h).player.queue.current = saved;
        h.run_steps(4);
        check(&mut h, "short song after the long one");
    }
    *h.state_mut() = None;
}

/// The column edges answer the pointer: a resize cursor over them, dragging
/// changes the width (within its range), and the widths are saved.
#[test]
fn column_edges_drag_and_persist() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::builder()
        .with_size(FULL)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut Option<OndApp>| {
                if let Some(app) = app {
                    app.main_ui(ui);
                }
            },
            None,
        );
    let ctx = h.ctx.clone();
    *h.state_mut() = Some(populated(&ctx, dir.path()));
    reset(&mut h, FULL, Route::Recent);
    // (The harness's own panel has an 8 pt margin around the app.)
    let edge = pos2(8.0 + super::layout::LEFT_DEFAULT, 400.0);
    h.hover_at(edge);
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeHorizontal, "resize cursor over the sidebar's edge");
    h.drag_at(edge);
    h.run_steps(1);
    h.hover_at(edge + vec2(60.0, 0.0));
    h.run_steps(1);
    h.drop_at(edge + vec2(60.0, 0.0));
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).columns().left, super::layout::LEFT_DEFAULT + 60.0);
    // The queue's edge, dragged far out, stops at its limit.
    let q = pos2(FULL.x - 8.0 - super::layout::RIGHT_DEFAULT, 400.0);
    h.drag_at(q);
    h.run_steps(1);
    h.hover_at(q - vec2(600.0, 0.0));
    h.run_steps(1);
    h.drop_at(q - vec2(600.0, 0.0));
    h.run_steps(2);
    let c = app_mut(&mut h).columns();
    assert_eq!(c.right, c.right_limit(FULL.x));
    // Saved when let go, and restored from there.
    app_mut(&mut h).prefs.flush();
    let saved = Settings::read(&dir.path().join("settings.json")).expect("settings written");
    let restored = super::layout::Columns::restore(saved.sidebar_width, saved.queue_width);
    assert_eq!(restored, c);
    *h.state_mut() = None;
}

#[cfg(test)]
mod perf;

/// A live visual asks for frames, and has the engine copy what plays, only
/// while it's on screen and something plays; otherwise the app's own pace
/// (nothing sooner than its clock) and the tap off.
#[test]
fn live_visuals_ask_for_frames_only_while_playing() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::builder()
        .with_size(FULL)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut Option<OndApp>| {
                if let Some(app) = app {
                    app.main_ui(ui);
                }
            },
            None,
        );
    let ctx = h.ctx.clone();
    *h.state_mut() = Some(populated(&ctx, dir.path()));
    let soon = |h: &mut Harness<'_, Option<OndApp>>| h.output().viewport_output.values().any(|v| v.repaint_delay <= Duration::from_millis(50));
    reset(&mut h, FULL, Route::Recent);
    h.run_steps(3);
    assert!(!soon(&mut h), "every switch off: no extra frames");
    assert!(!app_mut(&mut h).engine.status.tap.active());
    experiment(&mut h, |x| x.spectrum = true);
    steps_with_audio(&mut h, 3);
    assert!(soon(&mut h), "the spectrum on screen while playing wants frames");
    assert!(app_mut(&mut h).engine.status.tap.active(), "and the tap is on");
    app_mut(&mut h).engine.status.preview(PlayState::Paused, 141.0, 383.0, 0.72);
    h.run_steps(3);
    assert!(!soon(&mut h), "paused: no extra frames");
    assert!(!app_mut(&mut h).engine.status.tap.active(), "and the tap is off");
    // Playing again, but in a view without it (Mini shows no spectrum).
    app_mut(&mut h).engine.status.preview(PlayState::Playing, 141.0, 383.0, 0.72);
    app_mut(&mut h).edit_windows(|w| w.mode = WindowMode::Mini);
    h.set_size(vec2(280.0, 280.0));
    h.run_steps(3);
    assert!(!soon(&mut h), "not on screen: no extra frames");
    assert!(!app_mut(&mut h).engine.status.tap.active());
    *h.state_mut() = None;
}

#[test]
fn the_now_playing_heart_toggles_both_ways() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::builder()
        .with_size(FULL)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut Option<OndApp>| {
                if let Some(app) = app {
                    app.main_ui(ui);
                }
            },
            None,
        );
    let ctx = h.ctx.clone();
    *h.state_mut() = Some(populated(&ctx, dir.path()));
    reset(&mut h, FULL, Route::Recent);
    let id = app_mut(&mut h).player.queue.current.as_ref().unwrap().track.id.clone();
    let loved = |h: &mut Harness<'_, Option<OndApp>>| app_mut(h).favorites.is_on(StarKind::Song, &id);
    assert!(loved(&mut h), "the fixture's playing song starts loved");
    h.get_by_label(&crate::keys::shown("Unlove (⇧⌘L)")).click();
    h.run_steps(2);
    assert!(!loved(&mut h), "unloved from the now-playing display");
    // Clicking straight onto the heart again (the display under it reads as
    // not hovered while the pointer is on the heart) must love it again.
    h.get_by_label(&crate::keys::shown("Love (⇧⌘L)")).click();
    h.run_steps(2);
    assert!(loved(&mut h), "loved again from the now-playing display");
    *h.state_mut() = None;
}

/// Leaving a maximized full window: the compositor answers an un-maximize
/// with the window's old size, so the new size must wait until it has let go,
/// or the old size lands after it and wins (GNOME and KDE both do this). On
/// macOS a zoomed window simply takes the new size.
#[test]
fn leaving_a_maximized_window_resizes_after_it_lets_go() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::builder()
        .with_size(FULL)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut Option<OndApp>| {
                if let Some(app) = app {
                    app.main_ui(ui);
                    app.track_window(ui.ctx());
                }
            },
            None,
        );
    let ctx = h.ctx.clone();
    *h.state_mut() = Some(populated(&ctx, dir.path()));
    reset(&mut h, FULL, Route::Recent);
    let set_max = |h: &mut Harness<'_, Option<OndApp>>, on: bool| {
        h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().maximized = Some(on);
    };
    set_max(&mut h, true);
    h.run_steps(2);
    let commands = |h: &mut Harness<'_, Option<OndApp>>| {
        h.step();
        h.output().viewport_output.values().flat_map(|v| v.commands.clone()).collect::<Vec<_>>()
    };
    let resized = |c: &[egui::ViewportCommand]| c.iter().any(|c| matches!(c, egui::ViewportCommand::InnerSize(_)));

    app_mut(&mut h).set_mode(Some(WindowMode::Mini));
    let first = commands(&mut h);
    if cfg!(target_os = "macos") {
        // Zoom is just a size there: straight to the new one.
        assert!(resized(&first) && !first.contains(&egui::ViewportCommand::Maximized(false)), "{first:?}");
        *h.state_mut() = None;
        return;
    }
    assert!(first.contains(&egui::ViewportCommand::Maximized(false)), "un-maximizes: {first:?}");
    assert!(!resized(&first), "but doesn't resize yet: {first:?}");
    for _ in 0..3 {
        let c = commands(&mut h);
        assert!(!resized(&c), "still maximized, still waiting: {c:?}");
    }
    set_max(&mut h, false);
    let after = commands(&mut h);
    assert!(after.contains(&egui::ViewportCommand::InnerSize(vec2(280.0, 280.0))), "the mini size, once let go: {after:?}");
    *h.state_mut() = None;
}

/// The play button over a hovered album cover plays the album. It covers
/// part of the tile, so the pointer on it must not count as leaving the tile.
#[test]
fn the_play_button_on_an_album_cover_plays_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::builder()
        .with_size(FULL)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut Option<OndApp>| {
                if let Some(app) = app {
                    app.main_ui(ui);
                }
            },
            None,
        );
    let ctx = h.ctx.clone();
    *h.state_mut() = Some(populated(&ctx, dir.path()));
    reset(&mut h, FULL, Route::Recent);
    h.run_steps(2);
    let target = album_id(1);
    let label = {
        let a = app_mut(&mut h).data.album(&target).unwrap();
        format!("{} by {}", a.name, a.artist.as_deref().unwrap_or("Unknown artist"))
    };
    let tile = h.get_by_label(&label).rect();
    // Onto the cover, then onto its play button (bottom left of the art).
    h.hover_at(tile.center());
    h.run_steps(2);
    h.get_by_label("Play album").click();
    h.run_steps(3);
    // The album's tracks are queued to play (the test's null output can't
    // actually play them, so look at the queue, not at what's current).
    let q = &app_mut(&mut h).player.queue;
    let queued = q.history.iter().chain(&q.current).chain(&q.upcoming).filter(|i| matches!(&i.source, Source::Album { id, .. } if *id == target)).count();
    assert!(queued > 0, "the album was queued to play");
    *h.state_mut() = None;
}

/// Linux draws its own window buttons and resize edges over the content
/// (`csd`). They must take only their own spots: the Mini player still shows
/// its controls on hover, the toolbar's Mini button still works, and the
/// close button closes.
#[test]
fn our_window_frame_leaves_the_content_working() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::builder()
        .with_size(FULL)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut Option<OndApp>| {
                if let Some(app) = app {
                    app.main_ui(ui);
                }
            },
            None,
        );
    let ctx = h.ctx.clone();
    *h.state_mut() = Some(populated(&ctx, dir.path()));
    crate::csd::preview(&ctx, true);
    crate::csd::set_window(&ctx, true, true);
    reset(&mut h, FULL, Route::Recent);
    h.run_steps(2);

    // The window's top edge, over the toolbar: dragging resizes, not moves.
    let sent = |h: &Harness<'_, Option<OndApp>>, cmd: &egui::ViewportCommand| h.output().viewport_output.values().any(|v| v.commands.contains(cmd));
    let top = pos2(FULL.x / 2.0, 1.0);
    let mut seen = Vec::new();
    for e in [
        egui::Event::PointerMoved(top),
        egui::Event::PointerButton { pos: top, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
        egui::Event::PointerMoved(top + vec2(0.0, 20.0)),
        egui::Event::PointerButton { pos: top + vec2(0.0, 20.0), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ] {
        h.event(e);
        h.step();
        seen.extend(h.output().viewport_output.values().flat_map(|v| v.commands.clone()));
    }
    assert!(seen.contains(&egui::ViewportCommand::BeginResize(egui::ResizeDirection::North)), "the top edge resizes: {seen:?}");
    assert!(!seen.contains(&egui::ViewportCommand::StartDrag), "and doesn't move the window");
    h.run_steps(2);

    h.get_by_label(&crate::keys::shown("Mini player (⇧⌘M)")).click();
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).window_mode(), WindowMode::Mini, "the toolbar's Mini button switches");

    h.set_size(vec2(280.0, 280.0));
    h.hover_at(pos2(140.0, 140.0));
    h.run_steps(20);
    assert!(h.query_by_label("Next").is_some(), "hovering Mini shows its controls");

    // Back to the full window: the close button closes.
    app_mut(&mut h).edit_windows(|w| w.mode = WindowMode::Full);
    crate::csd::set_window(&ctx, true, true);
    h.set_size(FULL);
    h.run_steps(2);
    h.get_by_label("Close").click();
    h.run_steps(1);
    assert!(sent(&h, &egui::ViewportCommand::Close), "the close button asks the window to close");
    *h.state_mut() = None;
}

#[test]
fn list_keys_stay_with_an_open_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::builder()
        .with_size(FULL)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_ui_state(
            |ui, app: &mut Option<OndApp>| {
                if let Some(app) = app {
                    app.main_ui(ui);
                }
            },
            None,
        );
    let ctx = h.ctx.clone();
    *h.state_mut() = Some(populated(&ctx, dir.path()));

    // Songs with a selection, Settings open on top.
    reset(&mut h, FULL, Route::Songs);
    let playing = |h: &mut Harness<'_, Option<OndApp>>| app_mut(h).player.queue.current.as_ref().map(|c| c.track.id.clone());
    let before = playing(&mut h);
    app_mut(&mut h).ui.songs.selected = Some(0);
    app_mut(&mut h).ui.overlay = Some(Overlay::Settings);
    h.run_steps(1);
    h.key_press(egui::Key::ArrowDown);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).ui.songs.selected, Some(0), "↓ moved the list under the sheet");
    assert_eq!(playing(&mut h), before, "⏎ played from the list under the sheet");

    // The queue with a selected item: ⌫ must not remove it.
    let app = app_mut(&mut h);
    let uid = app.player.queue.upcoming[0].uid;
    let len = app.player.queue.upcoming.len();
    app.ui.focus = Focus::Queue;
    app.ui.queue_ui.selected = Some(uid);
    h.run_steps(1);
    h.key_press(egui::Key::Backspace);
    h.key_press(egui::Key::Delete);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).player.queue.upcoming.len(), len, "⌫ edited the queue under the sheet");

    // Closed again: the same keys reach the queue.
    app_mut(&mut h).ui.overlay = None;
    h.run_steps(1);
    h.key_press(egui::Key::Backspace);
    h.run_steps(2);
    assert_eq!(app_mut(&mut h).player.queue.upcoming.len(), len - 1, "⌫ works once the sheet is closed");
    *h.state_mut() = None;
}
