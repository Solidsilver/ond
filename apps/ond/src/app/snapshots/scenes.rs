//! The review scenes: every view in light and dark, the experiments, the
//! visualizers and managed settings, each shot to a PNG.

use super::*;
use crate::app::onboarding::Onboard;
use shell::kit::OnboardingStep;

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

    reset(h, player, Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    app_mut(h).ui.np_details = true;
    app_mut(h).player.playing_from = Some(audio::TrackSource { url: "file:///downloads/t.flac".into(), ..Default::default() });
    h.run_steps(4);
    if !click_label(h, "Signal path", egui::Rect::from_min_size(pos2(0.0, player.y - 40.0), vec2(player.x, 40.0))) {
        panic!("(player's format pill not found)");
    }
    h.run_steps(4);
    s.shoot(h, "player-signal-path");
    egui::Popup::close_all(&h.ctx);

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

    // The alphabet rail, held on a letter partway down: the grid jumps there.
    // (By title: the fixture's albums aren't stored in artist order, as the
    // library's are.)
    reset(h, FULL, Route::Albums);
    app_mut(h).ui.sort = AlbumSort::Title;
    looks(h, |s| s.alpha_rail = true);
    h.run_steps(2);
    let rail = h.get_by_label("Jump to letter").rect();
    let at = pos2(rail.center().x, rail.top() + rail.height() * 0.6);
    h.event(egui::Event::PointerMoved(at));
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.run_steps(4);
    s.shoot_now(h, "albums-alpha-held");
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    looks(h, |_| ());
    h.run_steps(2);
    assert!(h.query_by_label("Jump to letter").is_none(), "switched off: no rail");

    reset(h, FULL, Route::Artist { name: "Bill Evans".into(), id: None });
    s.shoot(h, "artist");
    // Its ⋯: Play next and Add to queue.
    h.get_by_label("More").click();
    h.run_steps(2);
    s.shoot(h, "artist-more");

    reset(h, FULL, Route::Album(album_id(0)));
    s.shoot(h, "album");

    // A very wide window: the track list stops at its width, under the
    // heading, and the Songs table spreads its columns.
    let wide = vec2(2400.0, 900.0);
    reset(h, wide, Route::Album(album_id(0)));
    s.shoot(h, "album-wide");
    reset(h, wide, Route::Songs);
    s.shoot(h, "songs-wide");

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
    {
        let mut c = app_mut(h).prefs_now().columns;
        c.right = super::layout::RIGHT_MIN;
        app_mut(h).set_columns(c);
    }
    h.run_steps(3);
    s.shoot(h, "info-long-min");
    {
        let mut c = app_mut(h).prefs_now().columns;
        c.right = super::layout::RIGHT_DEFAULT;
        app_mut(h).set_columns(c);
    }
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
        panic!("(verdict chip not found by label)");
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
            p.state.query = "bob".into();
            p.refresh(index);
        }
    }
    s.shoot(h, "palette");

    reset(h, FULL, Route::Recent);
    let toolbar = egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1440.0, 52.0));
    let _ = toolbar;
    h.run_steps(2);
    h.get_by(|n| n.label().is_some_and(|l| l.starts_with("Output: UMC22"))).click();
    s.shoot(h, "output-picker");

    // Bit-perfect on a DAC with its own volume: the slider sets it, in green, with the level in dB.
    reset(h, FULL, Route::Recent);
    {
        let app = app_mut(h);
        app.bit_perfect = true;
        app.prefs.edit(|s| {
            s.device_modes.insert("umc".into(), crate::settings::DeviceMode::BitPerfect);
            s.output_device = Some("umc".into());
        });
        app.output.handle(&audio::Event::DeviceVolume(Some(audio::DeviceVolume { db: -21.5, min_db: -63.5, max_db: 0.0, muted: Some(false) })));
        app.prefs.edit(|s| s.device_volume_db = crate::settings::DbReadout::Always);
    }
    h.run_steps(2);
    h.get_by(|n| n.label().is_some_and(|l| l.starts_with("Output: UMC22"))).click();
    s.shoot(h, "output-picker-device-volume");
    app_mut(h).prefs.edit(|s| s.device_volume_db = crate::settings::DbReadout::Adjusting);
    app_mut(h).output.handle(&audio::Event::DeviceVolume(None));
    app_mut(h).bit_perfect = false;
    app_mut(h).prefs.edit(|s| {
        s.device_modes.remove("umc");
        s.output_device = None;
    });

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
        None => panic!("(track menu didn't open; add-to-playlist shows the page)"),
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
        None => panic!("(track menu didn't open; rating-menu shows the page)"),
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

    reset(h, FULL, Route::Recent);
    app_mut(h).ui.overlay = Some(Overlay::SignOut { host: "music.example.com".into(), from_settings: false });
    s.shoot(h, "sign-out");

    // Settings › Queue › Playing something new: Ask.
    reset(h, FULL, Route::Recent);
    app_mut(h).ui.overlay =
        Some(Overlay::KeepAdded { tracks: vanguard(), source: crate::queue::Source::Added, action: QueueAction::Play { start: 0 }, added: 2 });
    s.shoot(h, "keep-added");

    // The sidebar footer: server status (click for details), shortcuts, settings.
    reset(h, FULL, Route::Recent);
    let sidebar = egui::Rect::from_min_max(pos2(0.0, 600.0), pos2(216.0, 900.0));
    if !click_label(h, "music.example.com", sidebar) {
        panic!("(server status not found by label)");
    }
    h.run_steps(3);
    s.shoot(h, "server-popover");

    reset(h, FULL, Route::Recent);
    if !hover_label(h, "Keyboard shortcuts", sidebar) {
        panic!("(help button not found by label)");
    }
    h.run_steps(30);
    s.shoot(h, "footer-help-hover");

    reset(h, FULL, Route::Recent);
    app_mut(h).session.online = false;
    s.shoot(h, "footer-connecting");

    reset(h, FULL, Route::Recent);
    app_mut(h).session.online = false;
    app_mut(h).session.sign_in = Some("Wrong username or password.".into());
    s.shoot(h, "footer-sign-in");

    reset(h, FULL, Route::Recent);
    app_mut(h).session.online = false;
    app_mut(h).session.work_offline = true;
    s.shoot(h, "footer-offline-mode");

    // Where the song plays from (an arrow in the format pill, and its
    // signal path), and the
    // download marks on playlists: one downloaded, one on its way.
    reset(h, FULL, Route::Recent);
    {
        let app = app_mut(h);
        let id = app.player.queue.current.as_ref().map(|c| c.track.id.clone()).unwrap_or_default();
        app.player.playing_from = Some(audio::TrackSource { id, url: "file:///downloads/t.flac".into(), ..Default::default() });
        let pin = |id: &str, done: usize| offline::PinSummary {
            pin: offline::Pin::Playlist(id.into()),
            label: id.into(),
            total: 12,
            done,
            failed: 0,
            bytes_done: 0,
            bytes_total: 0,
            downloading: done < 12,
        };
        let snap = offline::Snapshot { pins: vec![pin("pl-1", 12), pin("pl-2", 5)], ..(*app.downloads.snap).clone() };
        app.downloads.snap = Arc::new(snap);
    }
    s.shoot(h, "origin-downloaded");
    if !click_label(h, "Signal path", egui::Rect::from_min_size(Pos2::ZERO, vec2(FULL.x, 60.0))) {
        panic!("(format pill not found)");
    }
    h.run_steps(4);
    s.shoot(h, "origin-downloaded-path");
    egui::Popup::close_all(&h.ctx);
    app_mut(h).player.playing_from.as_mut().unwrap().url = "https://music.example.com/rest/stream".into();
    app_mut(h).player.playing_from.as_mut().unwrap().transcode = Some(audio::Transcode { format: "mp3".into(), kbps: 320 });
    s.shoot(h, "origin-streamed");

    // The diagnostics bar, on, while playing and syncing; and at a narrow width.
    reset(h, FULL, Route::Recent);
    {
        let app = app_mut(h);
        app.prefs.edit(|s| s.diagnostics_bar = true);
        app.session.sync.tracks = true;
        app.session.sync.progress = Some(crate::sync::Progress { done: 412, total: 1866, unit: crate::sync::Unit::Tracks });
        app.diagnostics.note_frame(Duration::from_micros(1800));
    }
    h.run_steps(2);
    s.shoot(h, "diagnostics-bar");
    let bar = egui::Rect::from_min_max(pos2(0.0, 870.0), pos2(1440.0, 900.0));
    if !hover_label(h, "Engine:", bar) {
        panic!("(diagnostics segment not found by label)");
    }
    h.run_steps(30);
    s.shoot(h, "diagnostics-bar-hover");

    reset(h, vec2(900.0, 700.0), Route::Recent);
    app_mut(h).prefs.edit(|s| s.diagnostics_bar = true);
    h.run_steps(2);
    s.shoot(h, "narrow-diagnostics-bar");

    // Settings › About, where the diagnostics bar's row lives.
    reset(h, FULL, Route::Recent);
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::About));
    s.shoot(h, "settings-about-diagnostics");

    // A newer Ond is out: the notice above the footer, the rail's dot, and
    // Settings › About with the release notes (the GitHub copy's rows with
    // OND_DISTRIBUTION=github).
    reset(h, FULL, Route::Recent);
    let v = crate::updates::Version::parse("0.5.0").unwrap();
    app_mut(h).update = Some(v.clone());
    app_mut(h).updates.latest = Some(v.clone());
    s.shoot(h, "update-available");
    reset(h, vec2(720.0, 600.0), Route::Recent);
    app_mut(h).update = Some(v.clone());
    s.shoot(h, "update-available-rail");
    reset(h, FULL, Route::Recent);
    app_mut(h).update = Some(v.clone());
    app_mut(h).updates.latest = Some(v.clone());
    app_mut(h).updates.notes = Some((
        v.clone(),
        Some(Ok(vec![
            crate::updates::Block::Paragraph("Ond is signed, and Settings has an About page.".into()),
            crate::updates::Block::Heading("Updates".into()),
            crate::updates::Block::Bullet("Settings › About shows the version, checks for updates on request, and shows a new version's release notes.".into()),
            crate::updates::Block::Bullet("The icon rail's gear has a dot when a new version is out.".into()),
        ])),
    ));
    h.run_steps(2);
    if !click_label(h, "Ond 0.5.0 is available", egui::Rect::from_min_size(Pos2::ZERO, vec2(320.0, FULL.y))) {
        panic!("(update notice not found)");
    }
    h.run_steps(4);
    s.shoot(h, "update-available-about");
    reset(h, FULL, Route::Recent);
    app_mut(h).updates.checked = Some(std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600));
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::About));
    s.shoot(h, "about-up-to-date");
    // Installing updates: the switch on; 0.7.0 downloading, ready (in About,
    // narrow too, and in the sidebar's notice), failed, and found where Ond
    // can't replace itself; then the last update not having installed.
    // Checking goes back to notifying after, as later scenes expect.
    {
        use crate::updates::install::{Error, Fallback, State};
        let v = crate::updates::Version::parse("0.7.0").unwrap();
        let installing = |h: &mut _, size, install: Option<State>, about: bool| {
            reset(h, size, Route::Recent);
            let app = app_mut(h);
            app.prefs.edit(|s| s.updates = crate::updates::UpdateCheck::Install);
            app.updates.checked = Some(std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600));
            if let Some(install) = install {
                app.update = Some(v.clone());
                app.updates.latest = Some(v.clone());
                app.updates.notes = Some((v.clone(), Some(Ok(vec![crate::updates::Block::Paragraph("Ond installs its own updates.".into())]))));
                app.updates.install = install;
            }
            if about {
                app.actions.push(Action::OpenSettings(settings_page::Section::About));
            }
        };
        installing(h, FULL, None, true);
        s.shoot(h, "about-install-on");
        installing(h, FULL, Some(State::Downloading { version: v.clone(), received: 12_400_000, total: Some(24_100_000) }), true);
        s.shoot(h, "about-update-downloading");
        installing(h, FULL, Some(State::preview_ready(v.clone())), false);
        s.shoot(h, "update-ready");
        installing(h, FULL, Some(State::preview_ready(v.clone())), true);
        s.shoot(h, "about-update-ready");
        installing(h, vec2(720.0, 600.0), Some(State::preview_ready(v.clone())), true);
        s.shoot(h, "about-update-ready-narrow");
        installing(h, FULL, Some(State::Failed { version: v.clone(), error: Error::Signature }), true);
        s.shoot(h, "about-update-failed");
        installing(h, FULL, Some(State::NotifyOnly { version: v.clone(), reason: Fallback::DiskImage }), true);
        s.shoot(h, "about-update-notify-only");
        installing(h, FULL, None, true);
        app_mut(h).updates.install_error = Some(Error::Worker("couldn't replace /Applications/Ond.app, so the previous copy was put back".into()));
        s.shoot(h, "about-install-error");
        app_mut(h).prefs.edit(|s| s.updates = crate::updates::UpdateCheck::Notify);
    }
    // The channel picked rather than followed, then a beta offered on it
    // with its notes, in a narrow window and a full one (full last, so the
    // Settings window the next scenes open starts where they expect it).
    reset(h, FULL, Route::Recent);
    app_mut(h).prefs.edit(|s| s.update_channel = Some(crate::updates::UpdateChannel::Betas));
    app_mut(h).updates.checked = Some(std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600));
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::About));
    s.shoot(h, "about-channel-betas");
    let beta = crate::updates::Version::parse("99.0.0-beta.1").unwrap();
    for (size, scene) in [(vec2(720.0, 600.0), "about-beta-available-narrow"), (FULL, "about-beta-available")] {
        reset(h, size, Route::Recent);
        let app = app_mut(h);
        app.prefs.edit(|s| s.update_channel = Some(crate::updates::UpdateChannel::Betas));
        app.update = Some(beta.clone());
        app.updates.latest = Some(beta.clone());
        app.updates.notes = Some((
            beta.clone(),
            Some(Ok(vec![
                crate::updates::Block::Paragraph("A first look at what comes next.".into()),
                crate::updates::Block::Heading("Updates".into()),
                crate::updates::Block::Bullet("Settings › About has a Channel: Releases, or Betas as well.".into()),
            ])),
        ));
        app.actions.push(Action::OpenSettings(settings_page::Section::About));
        s.shoot(h, scene);
    }

    // Another accent color: everything accented takes it, in either theme.
    reset(h, FULL, Route::Album(album_id(0)));
    app_mut(h).prefs.edit(|s| s.accent = theme::Accent::Teal);
    s.shoot(h, "accent-teal-album");
    // The user's own (a bright yellow, darkened or lightened to read), and the picker.
    reset(h, FULL, Route::Recent);
    app_mut(h).prefs.edit(|s| s.accent = theme::Accent::Custom([0xF5, 0xC5, 0x18]));
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::Appearance));
    h.run_steps(2);
    h.get_by_label("Custom color…").click();
    s.shoot(h, "settings-accent-picker");
    egui::Popup::close_all(&h.ctx);

    for section in settings_page::Section::ALL {
        reset(h, FULL, Route::Recent);
        app_mut(h).actions.push(Action::OpenSettings(section));
        s.shoot(h, &format!("settings-{}", section.label().to_lowercase()));
    }
    // The one server as a source: unreachable, then its menu open.
    reset(h, FULL, Route::Recent);
    app_mut(h).session.online = false;
    app_mut(h).session.unreachable = true;
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::Sources));
    s.shoot(h, "settings-sources-offline");
    reset(h, FULL, Route::Recent);
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::Sources));
    h.run_steps(2);
    h.get_by_label_contains("music.example.com, Navidrome, ").click();
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "settings-sources-menu");
    egui::Popup::close_all(&h.ctx);

    // A folder of music in use, the server kept behind it: scanning, with
    // files it couldn't read, and with one of its folders unplugged.
    let progress = crate::sync::Progress { done: 1_284, total: 3_960, unit: crate::sync::Unit::Tracks };
    for (scene, setup) in [
        ("settings-sources-local-scanning", (Some(progress), 0, false)),
        ("settings-sources-local-skipped", (None, 3, false)),
        ("settings-sources-local-missing", (None, 0, true)),
    ] {
        reset(h, FULL, Route::Recent);
        let app = app_mut(h);
        use_folder(app);
        let (scan, skipped, missing) = setup;
        app.session.sync.progress = scan;
        app.session.sync.tracks = scan.is_some();
        app.session.skipped = skipped;
        if missing {
            app.session.folder_missing = vec![PathBuf::from("/Volumes/Archive/Lossless")];
        }
        app.actions.push(Action::OpenSettings(settings_page::Section::Sources));
        s.shoot(h, scene);
    }
    // The library from a folder: hearts, ratings and playlists via Ond's store,
    // no downloads, the folder in the sidebar's foot, an album page, its menu,
    // and an Ond playlist.
    reset(h, FULL, Route::Albums);
    use_folder(app_mut(h));
    s.shoot(h, "local-albums");
    reset(h, FULL, Route::Album(album_id(0)));
    use_folder(app_mut(h));
    s.shoot(h, "local-album");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Music").click();
    h.run_steps(2);
    s.shoot(h, "local-footer-menu");
    egui::Popup::close_all(&h.ctx);
    reset(h, FULL, Route::Playlist { id: "pl-0".into(), name: "Late night, low volume".into() });
    use_folder(app_mut(h));
    s.shoot(h, "local-playlist");
    use_server(app_mut(h));

    // A queue set up by hand: no preset matches, so Custom shows.
    reset(h, FULL, Route::Recent);
    {
        let app = app_mut(h);
        let q = crate::queue::QueuePrefs { on_play: crate::queue::OnPlay::KeepBefore, ..Default::default() };
        app.actions.push(Action::Prefs(PrefsAction::Queue(q)));
        app.actions.push(Action::OpenSettings(settings_page::Section::Queue));
    }
    s.shoot(h, "settings-queue-custom");

    // Playback with ReplayGain and a transcode on, so every control shows.
    reset(h, FULL, Route::Recent);
    {
        let app = app_mut(h);
        let mut p = app.output.prefs().clone();
        p.replay_gain = crate::playback::RgMode::Auto;
        p.quality = crate::playback::QualityMode::Auto;
        app.actions.push(Action::Output(OutputAction::Playback(p)));
        app.actions.push(Action::OpenSettings(settings_page::Section::Playback));
    }
    s.shoot(h, "settings-playback-on");

    // A library run waiting on playback (the preview status is mid-download),
    // shot from a stand-in so no worker has to reach that state first.
    reset(h, FULL, Route::Recent);
    {
        let app = app_mut(h);
        app.analysis.runner = crate::analyze::Runner::stand_in(crate::analyze::Progress {
            done: 3,
            total: 40,
            waiting: true,
            running: true,
            remaining_secs: 37.0 * 210.0,
            ..Default::default()
        });
        app.actions.push(Action::OpenSettings(settings_page::Section::Library));
    }
    s.shoot(h, "settings-analysing");

    reset(h, vec2(900.0, 640.0), Route::Recent);
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::Output));
    s.shoot(h, "narrow-settings");

    // A device open in Settings › Output: its mode, whether the menu shows it, its group.
    reset(h, FULL, Route::Recent);
    app_mut(h).ui.output_open.insert("umc".into());
    app_mut(h).prefs.edit(|s| {
        s.device_hidden.insert("zoom".into());
        s.device_groups.insert("tv".into(), "Living room".into());
    });
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::Output));
    s.shoot(h, "settings-output-device");
    app_mut(h).ui.output_open.clear();
    app_mut(h).prefs.edit(|s| {
        s.device_hidden.clear();
        s.device_groups.clear();
    });

    // Places and their empty states.
    reset(h, FULL, Route::Songs);
    s.shoot(h, "songs");

    reset(h, FULL, Route::Songs);
    let all = std::mem::take(&mut Arc::make_mut(&mut app_mut(h).data.tracks).all);
    s.shoot(h, "songs-empty");
    Arc::make_mut(&mut app_mut(h).data.tracks).all = all;

    reset(h, FULL, Route::Offline);
    s.shoot(h, "offline-pins");

    reset(h, FULL, Route::Offline);
    {
        let app = app_mut(h);
        let snap = offline::Snapshot { pins: Vec::new(), used_bytes: 0, ..(*app.downloads.snap).clone() };
        app.downloads.snap = Arc::new(snap);
    }
    s.shoot(h, "offline-empty");

    // The list of downloads couldn't be read: a made-up snapshot, with the
    // store out of the way so it isn't refreshed over.
    reset(h, FULL, Route::Offline);
    let store = app_mut(h).downloads.store.take();
    app_mut(h).downloads.snap = Arc::new(set_aside_snapshot(&app_mut(h).downloads.snap));
    s.shoot(h, "offline-set-aside");
    if !click_label(h, "Remove them", egui::Rect::EVERYTHING) {
        panic!("(Remove them not found by label)");
    }
    s.shoot(h, "offline-set-aside-confirm");
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::Offline));
    s.shoot(h, "settings-offline-set-aside");
    let app = app_mut(h);
    app.downloads.store = store;
    app.downloads.refresh();

    reset(h, FULL, Route::Artists);
    s.shoot(h, "artists");
    // No album covers loaded yet (Albums never opened): each disc is the
    // artist's own colour with their initials.
    app_mut(h).art.unload();
    reset(h, FULL, Route::Artists);
    s.shoot(h, "artists-initials");
    let app = app_mut(h);
    for i in 0..ALBUMS.len() {
        app.art.preload(&album_id(i), cover(i));
    }
    app.art.preload(WHITE, white_cover());
    // Albums the server has no cover for: generated ones beside the real
    // covers, in the grid and on an album's page.
    for i in (1..ALBUMS.len()).step_by(3) {
        app_mut(h).art.preload_missing(&album_id(i));
    }
    reset(h, FULL, Route::Albums);
    s.shoot(h, "albums-missing-art");
    reset(h, FULL, Route::Album(album_id(1)));
    s.shoot(h, "album-missing-art");
    let app = app_mut(h);
    for i in (1..ALBUMS.len()).step_by(3) {
        app.art.preload(&album_id(i), cover(i));
    }
    // Albums as a table, sorted by year the other way round (as a click on
    // its header twice leaves it).
    reset(h, FULL, Route::Albums);
    let app = app_mut(h);
    app.prefs.edit(|s| s.album_view = crate::settings::ViewMode::List);
    (app.ui.sort, app.ui.sort_desc) = (AlbumSort::Year, true);
    h.run_steps(2);
    s.shoot(h, "albums-list");
    let app = app_mut(h);
    (app.ui.sort, app.ui.sort_desc) = (AlbumSort::Artist, false);
    reset(h, FULL, Route::Artists);
    // The server's artists, with photos: most have one, two get the server's
    // stand-in (the same picture for both), so they show their initial.
    let app = app_mut(h);
    let mut names: Vec<&str> = ALBUMS.iter().map(|a| a.1).collect();
    names.sort_unstable();
    names.dedup();
    let artists = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let photo = format!("ar-{i}");
            app.art.preload(&photo, if i % 5 == 3 { white_cover() } else { cover(i + 11) });
            library::Artist { id: format!("ar-{i}"), name: (*name).into(), sort: None, album_count: 2, cover_art: Some(photo) }
        })
        .collect();
    app.data.artists = Arc::new(artists);
    app.rebuild_search();
    s.shoot(h, "artists-photos");
    app_mut(h).prefs.edit(|s| s.artist_view = crate::settings::ViewMode::List);
    h.run_steps(2);
    s.shoot(h, "artists-list");
    // An artist's page: their photo beside the name, its colors on the page.
    let bill = names.iter().position(|n| *n == "Bill Evans").unwrap();
    reset(h, FULL, Route::Artist { name: "Bill Evans".into(), id: Some(format!("ar-{bill}")) });
    adapt(h);
    settle_look(h);
    s.shoot(h, "artist-photo");
    let app = app_mut(h);
    app.data.artists = Arc::default();
    app.rebuild_search();

    reset(h, FULL, Route::Playlist { id: "pl-4".into(), name: "Car".into() });
    app_mut(h).data.playlist_tracks.insert("pl-4".into(), Arc::default());
    s.shoot(h, "playlist-empty");

    reset(h, vec2(900.0, 700.0), Route::Recent);
    s.shoot(h, "narrow-recent");

    reset(h, vec2(900.0, 700.0), Route::Recent);
    app_mut(h).actions.push(Action::Window(WindowAction::ToggleQueue));
    s.shoot(h, "narrow-queue-sheet");

    reset(h, vec2(760.0, 640.0), Route::Albums);
    s.shoot(h, "narrow-rail");

    // Repeat queue with the queue hidden: a badge on its toggle says so,
    // wide and narrow, and its tooltip names it.
    reset(h, FULL, Route::Recent);
    app_mut(h).player.queue.end = crate::queue::EndAction::RepeatQueue;
    app_mut(h).prefs.edit(|s| s.show_queue = false);
    h.run_steps(2);
    s.shoot(h, "queue-repeat-hidden");
    h.get_by_label(&crate::keys::shown("Show queue (⌥⌘U). Then: Repeat queue")).hover();
    h.run_steps(30);
    s.shoot_now(h, "queue-repeat-hover");
    reset(h, vec2(760.0, 640.0), Route::Albums);
    app_mut(h).player.queue.end = crate::queue::EndAction::RepeatQueue;
    s.shoot(h, "narrow-queue-repeat");
    app_mut(h).player.queue.end = crate::queue::EndAction::Stop;

    reset(h, vec2(900.0, 700.0), Route::Album(album_id(7)));
    s.shoot(h, "narrow-long-album");

    reset(h, vec2(280.0, 280.0), Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Mini);
    s.shoot(h, "mini");
    h.hover_at(pos2(140.0, 140.0));
    h.run_steps(6);
    s.shoot(h, "mini-hover");

    // Mini on a near-white cover: the progress line and the corner buttons
    // turn dark to read on it.
    reset(h, vec2(280.0, 280.0), Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Mini);
    play_white(app_mut(h));
    settle_look(h);
    s.shoot(h, "mini-white");
    h.hover_at(pos2(140.0, 140.0));
    h.run_steps(6);
    s.shoot(h, "mini-white-hover");

    reset(h, FULL, Route::Recent);
    {
        let app = app_mut(h);
        app.player.queue = crate::queue::Queue::default();
        app.engine.status.preview(PlayState::Stopped, 0.0, 0.0, 0.0);
    }
    s.shoot(h, "empty-queue");

    appearance_scenes(h, s);

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
        panic!("(readout not found by label)");
    }
    h.run_steps(30);
    s.shoot(h, "lcd-readout-hover");

    reset(h, FULL, Route::Recent);
    {
        let app = app_mut(h);
        app.session.sync.tracks = true;
        app.session.sync.progress = Some(crate::sync::Progress { done: 412, total: 1866, unit: crate::sync::Unit::Tracks });
    }
    s.shoot(h, "sidebar-syncing");
    let side = egui::Rect::from_min_max(pos2(0.0, 700.0), pos2(216.0, 900.0));
    if !hover_label(h, "Syncing ·", side) {
        panic!("(sync status not found by label)");
    }
    h.run_steps(30);
    s.shoot(h, "sidebar-syncing-hover");

    reset(h, FULL, Route::Recent);
    app_mut(h).session.sync.albums = true;
    s.shoot(h, "sidebar-syncing-albums");

    reset(h, FULL, Route::Recent);
    app_mut(h).session.online = false;
    app_mut(h).session.unreachable = true;
    s.shoot(h, "offline-pill");

    reset(h, vec2(760.0, 640.0), Route::Albums);
    {
        let app = app_mut(h);
        app.session.sync.tracks = true;
        app.session.sync.progress = Some(crate::sync::Progress { done: 1200, total: 1866, unit: crate::sync::Unit::Tracks });
    }
    s.shoot(h, "narrow-rail-syncing");

    // Toasts: a notice alone, an error with Retry, a Clear's Undo, then a
    // mixed stack in one column, wide and narrow.
    let failed = |app: &mut OndApp| {
        let error = crate::failure::Failure::Server("Couldn't load Sunday at the Village Vanguard: the server didn't answer.".into());
        app.handle(crate::bridge::LibraryMsg::LoadFailed { key: crate::bridge::ListKey::Album(album_id(0)), error: Some(error) });
    };
    reset(h, FULL, Route::Album(album_id(0)));
    app_mut(h).show_notice("Added 6 songs to Late night, low volume");
    s.shoot(h, "toast-note");
    reset(h, FULL, Route::Album(album_id(0)));
    failed(app_mut(h));
    s.shoot(h, "toast-error-retry");
    reset(h, FULL, Route::Album(album_id(0)));
    app_mut(h).apply(Action::Queue(QueueEdit::Clear));
    s.shoot(h, "toast-undo");
    for (name, size) in [("toasts", FULL), ("toasts-narrow", vec2(560.0, 720.0))] {
        reset(h, size, Route::Album(album_id(0)));
        failed(app_mut(h));
        h.run_steps(1);
        app_mut(h).show_notice("Skipped 1 song that isn't downloaded");
        h.run_steps(1);
        app_mut(h).apply(Action::Queue(QueueEdit::Clear));
        s.shoot(h, name);
    }

    // A device that came back: offered, with a button, until clicked away.
    reset(h, FULL, Route::Recent);
    app_mut(h).handle_engine(audio::Event::DeviceReturned { uid: "hp".into(), name: "Technics EAH-AZ100".into() });
    s.shoot(h, "toast-device-back");

    reset(h, vec2(380.0, 880.0), Route::Recent);
    {
        let app = app_mut(h);
        app.edit_windows(|w| w.mode = WindowMode::Player);
        app.output.handle(&signal_with((16, 44_100), (16, 44_100), audio::BitPerfectStatus::Verified, None));
    }
    s.shoot(h, "player-verified");

    // The first run: the sheet's tiles, then Navidrome's form.
    reset(h, FULL, Route::Recent);
    app_mut(h).login = Some(LoginForm { server: "http://localhost:4533".into(), ..Default::default() });
    app_mut(h).onboarding = Some(Onboard::default());
    s.shoot(h, "login");

    reset(h, FULL, Route::Recent);
    app_mut(h).login = Some(LoginForm { server: "http://localhost:4533".into(), ..Default::default() });
    app_mut(h).onboarding = Some(Onboard { step: OnboardingStep::Form(0), ..Default::default() });
    s.shoot(h, "login-form");

    // A folder of music's form: empty, with two folders, and scanning.
    let two = || vec![PathBuf::from("/Users/demo/Music"), PathBuf::from("/Volumes/Archive/Lossless")];
    for (scene, folders, busy) in [("login-folder-empty", Vec::new(), false), ("login-folder-two", two(), false), ("login-folder-scanning", two(), true)] {
        reset(h, FULL, Route::Recent);
        let app = app_mut(h);
        app.login = Some(LoginForm { server: "http://localhost:4533".into(), ..Default::default() });
        let form = crate::app::sources::FolderForm { folders, busy, ..Default::default() };
        app.onboarding = Some(Onboard { step: OnboardingStep::Form(crate::app::onboarding::FOLDER), folders: form, ..Default::default() });
        if busy {
            app.session.sync.progress = Some(crate::sync::Progress { done: 1_284, total: 3_960, unit: crate::sync::Unit::Tracks });
        }
        s.shoot(h, scene);
    }

    reset(h, FULL, Route::Recent);
    app_mut(h).login = Some(LoginForm {
        server: "https://music.example.com".into(),
        username: "demo".into(),
        error: Some(crate::bridge::connect_error(&subsonic::Error::Api { code: 40, message: "Wrong username or password".into() })),
        ..Default::default()
    });
    app_mut(h).onboarding = Some(Onboard { step: OnboardingStep::Form(0), ..Default::default() });
    s.shoot(h, "login-error");

    reset(h, FULL, Route::Recent);
    app_mut(h).login = Some(LoginForm { server: "https://music.example.com".into(), username: "demo".into(), busy: true, ..Default::default() });
    app_mut(h).onboarding = Some(Onboard { step: OnboardingStep::Form(0), ..Default::default() });
    s.shoot(h, "login-busy");

    // Signing in again over the library: the form alone.
    reset(h, FULL, Route::Recent);
    app_mut(h).login = Some(LoginForm {
        server: "https://music.example.com".into(),
        username: "demo".into(),
        error: Some("Wrong username or password.".into()),
        closable: true,
        ..Default::default()
    });
    s.shoot(h, "login-again");

    // Zoomed. Sizes are in points, so a 1440×900 window at 150% is 960×600
    // of them; the narrowest full window is its minimum, 900×560 points at
    // any zoom (1350×840 on screen at 150%). At 200% that minimum is more
    // than a 1440×900 screen holds, so the window stops at 90% of the
    // screen: 648×405 points.
    for (scene, percent, size, route) in [
        ("zoom-150-recent", 150, FULL / 1.5, Route::Recent),
        ("zoom-150-narrow-albums", 150, vec2(900.0, 560.0), Route::Albums),
        ("zoom-150-narrow-album", 150, vec2(900.0, 560.0), Route::Album(album_id(0))),
        ("zoom-175-small-screen", 175, vec2(1470.0, 956.0) * 0.9 / 1.75, Route::Recent),
        ("zoom-200-small-screen", 200, vec2(1470.0, 956.0) * 0.9 / 2.0, Route::Recent),
    ] {
        reset(h, size, route);
        app_mut(h).apply(Action::Prefs(PrefsAction::Zoom(percent)));
        if scene != "zoom-150-recent" {
            // The toast once is enough.
            app_mut(h).ui.toasts = shell::kit::Toasts::default();
        }
        s.shoot(h, scene);
    }
    reset(h, FULL / 1.5, Route::Recent);
    zoom(h, 150);
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::Appearance));
    h.run_steps(2);
    // The pane remembers where an earlier scene left it scrolled: to the top.
    h.event(egui::Event::PointerMoved(egui::pos2(650.0, 105.0)));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, 5000.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    s.shoot(h, "zoom-150-settings-appearance");
    // The Player at 150%: its minimum, 320×600 points.
    reset(h, vec2(320.0, 600.0), Route::Recent);
    zoom(h, 150);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    s.shoot(h, "zoom-150-player-min");
    reset(h, FULL, Route::Recent);
    reset(h, FULL, Route::Playlists);
    s.shoot(h, "playlists");
}

/// The appearance settings, each on its own: colors from the album art, the
/// blurred artwork behind the Player and Mini, and the waveform seek bar.
fn appearance_scenes(h: &mut Harness<'_, Option<OndApp>>, s: &Shooter) {
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
    looks(h, |s| s.backdrop = true);
    settle_look(h);
    s.shoot(h, "x-backdrop-player");
    reset(h, player, Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    play_blue(app_mut(h));
    looks(h, |s| s.backdrop = true);
    adapt(h);
    settle_look(h);
    s.shoot(h, "x-backdrop-colors-player");
    // The next cover arrives: two steps (half a second) into the fade.
    {
        let app = app_mut(h);
        app.player.queue.play_now(crate::test_tracks::account(), vanguard(), 3, Source::Album { id: album_id(0), name: ALBUMS[0].0.into() });
        let deadline = testkit::Deadline::new();
        while app.art.look(&album_id(0)).is_none() {
            assert!(!deadline.passed(), "the next cover's look never arrived");
            std::thread::sleep(Duration::from_millis(10));
            app.art.poll();
        }
    }
    h.run_steps(2);
    s.shoot_now(h, "x-backdrop-crossfade");
    reset(h, vec2(280.0, 280.0), Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Mini);
    looks(h, |s| s.backdrop = true);
    settle_look(h);
    h.hover_at(pos2(140.0, 140.0));
    h.run_steps(6);
    s.shoot(h, "x-backdrop-mini-hover");

    // A near-white cover behind the Player: the window buttons (and on Linux
    // the window's own) turn dark to read on it; then Mini's corner buttons.
    reset(h, player, Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    play_white(app_mut(h));
    looks(h, |s| s.backdrop = true);
    settle_look(h);
    s.shoot(h, "x-backdrop-white-player");
    crate::csd::preview(&h.ctx, true);
    crate::csd::set_window(&h.ctx, true, true);
    s.shoot(h, "x-backdrop-white-linux-player");
    crate::csd::preview(&h.ctx, false);
    crate::csd::set_window(&h.ctx, false, false);
    reset(h, vec2(280.0, 280.0), Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Mini);
    play_white(app_mut(h));
    looks(h, |s| s.backdrop = true);
    settle_look(h);
    h.hover_at(pos2(140.0, 140.0));
    h.run_steps(6);
    s.shoot(h, "x-backdrop-white-mini-hover");

    // The waveform seek bar: in the Player and the now-playing display, with
    // the pointer on it, and before its peaks are ready (the plain line).
    reset(h, player, Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    looks(h, |s| s.waveform = true);
    s.shoot(h, "x-waveform-loading-player");
    fixture_waveform(app_mut(h));
    s.shoot(h, "x-waveform-player");
    looks(h, |s| {
        s.waveform = true;
        s.waveform_bars = crate::visuals::waveform::Bars { rounded: true, width: 6, gap: 3 };
    });
    s.shoot(h, "x-waveform-rounded-player");
    reset(h, FULL, Route::Recent);
    looks(h, |s| s.waveform = true);
    fixture_waveform(app_mut(h));
    s.shoot(h, "x-waveform-full");
    h.hover_at(pos2(820.0, 41.0));
    h.run_steps(2);
    s.shoot(h, "x-waveform-lcd-hover");
    // Its bars' settings under their preview, scrolled down to them, and in a
    // narrow window with the controls beside or under their names.
    for (size, name) in [(FULL, "x-settings-waveform"), (vec2(560.0, 700.0), "x-settings-waveform-narrow")] {
        reset(h, size, Route::Recent);
        looks(h, |s| {
            s.waveform = true;
            s.waveform_bars.rounded = true;
        });
        app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::Appearance));
        h.run_steps(2);
        wheel(h, pos2(size.x * 0.65, size.y * 0.5), -600.0, 10);
        s.shoot(h, name);
    }

    // Everything that's still on the page at once, in the Player.
    reset(h, player, Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    looks(h, |s| {
        s.backdrop = true;
        s.waveform = true;
    });
    adapt(h);
    fixture_waveform(app_mut(h));
    settle_look(h);
    s.shoot(h, "x-together-player");
    // Leave them off: what follows is the ordinary window's.
    looks(h, |_| ());
}

#[test]
#[ignore = "renders PNGs for review; run with --ignored"]
fn snapshots() {
    shoot_scenes(&[(egui::Theme::Light, "light"), (egui::Theme::Dark, "dark")], None, scenes);
}

/// A server and a music folder live together, in All sources: dots on
/// covers and rows, the filter chips, the switcher and its menu, a merged
/// artist, the mixed-playlist offer, and one source unreachable.
#[test]
#[ignore = "renders PNGs for review; run with --ignored"]
fn sources_snapshots() {
    shoot_scenes(&[(egui::Theme::Light, "light"), (egui::Theme::Dark, "dark")], None, source_scenes);
}

fn source_scenes(h: &mut Harness<'_, Option<OndApp>>, s: &Shooter) {
    reset(h, FULL, Route::Albums);
    two_sources(app_mut(h));
    reset(h, FULL, Route::Albums);
    s.shoot(h, "sources-albums");

    reset(h, FULL, Route::Albums);
    app_mut(h).prefs.edit(|p| p.album_view = crate::settings::ViewMode::List);
    s.shoot(h, "sources-albums-list");

    reset(h, FULL, Route::Songs);
    s.shoot(h, "sources-songs");

    // The folder's chip lit: only its albums, and no dots, since they all
    // come from one source.
    reset(h, FULL, Route::Albums);
    app_mut(h).actions.push(Action::Prefs(PrefsAction::Scope(all_sources::ScopeAction::Chip(shell::kit::SourceChip::Toggle(local_account())))));
    s.shoot(h, "sources-chips-narrowed");
    app_mut(h).actions.push(Action::Prefs(PrefsAction::Scope(all_sources::ScopeAction::Chip(shell::kit::SourceChip::All))));
    h.run_steps(2);

    reset(h, FULL, Route::Albums);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "All sources, 2 sources").click();
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "sources-switcher-menu");
    egui::Popup::close_all(&h.ctx);

    reset(h, FULL, Route::Artist { name: "Bill Evans Trio".into(), id: None });
    s.shoot(h, "sources-artist");

    let album = accounts::ItemKey::new(local_account(), album_id(1)).to_string();
    reset(h, FULL, Route::Album(album));
    s.shoot(h, "sources-album");

    reset(h, FULL, Route::Albums);
    // The chips' scope changes dropped the index; a real one rebuilds in the background.
    app_mut(h).rebuild_search();
    app_mut(h).actions.push(Action::OpenPalette);
    h.run_steps(2);
    {
        let app = app_mut(h);
        if let (Some(p), Some(index)) = (app.ui.palette.as_mut(), app.search.index.as_mut()) {
            p.state.query = "bill".into();
            p.refresh(index);
        }
    }
    s.shoot(h, "sources-search");

    reset(h, FULL, Route::Playlists);
    {
        let app = app_mut(h);
        let (server, local) = (crate::test_tracks::account(), local_account());
        app.multi.pending_playlist = Some(all_sources::PendingMixedPlaylist {
            source: accounts::ItemKey::new(server, "pl-1"),
            name: "Headphone test tracks".into(),
            entries: vec![accounts::ItemKey::new(server, "vg-0"), accounts::ItemKey::new(local, "kb-0")],
        });
        app.ui.overlay = Some(Overlay::MixedPlaylist);
    }
    s.shoot(h, "sources-mixed-playlist");

    // The server stops answering while the folder plays on.
    let unreachable = |h: &mut Harness<'_, Option<OndApp>>| {
        let server = app_mut(h).session.accounts.get_mut(&crate::test_tracks::account()).expect("server source");
        server.online = false;
        server.unreachable = true;
    };
    reset(h, FULL, Route::Albums);
    unreachable(h);
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "All sources, Home server · Offline").click();
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "sources-unreachable");
    egui::Popup::close_all(&h.ctx);

    reset(h, FULL, Route::Albums);
    unreachable(h);
    {
        let folder = app_mut(h).session.accounts.get_mut(&local_account()).expect("folder source");
        folder.skipped = 3;
    }
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::Sources));
    s.shoot(h, "sources-settings");

    // The menu closed: the switcher flags the server it can't reach.
    reset(h, FULL, Route::Albums);
    unreachable(h);
    h.run_steps(2);
    s.shoot(h, "sources-unreachable-flag");
    for source in app_mut(h).session.accounts.values_mut() {
        source.online = true;
        source.unreachable = false;
        source.skipped = 0;
    }

    // The folded rail: the stacked dots alone.
    reset(h, vec2(760.0, 700.0), Route::Albums);
    s.shoot(h, "sources-rail");

    // Three sources, the chips narrowed to two: the switcher names them,
    // and its menu ticks their boxes.
    let office: accounts::AccountId = "00000000-0000-4000-8000-0000000ff1ce".parse().unwrap();
    {
        let app = app_mut(h);
        let mut entry = crate::settings::SourceEntry::subsonic(office, "https://office.example.com", "demo");
        entry.name = Some("Office".into());
        entry.color = accounts::SourceColor::new(4);
        app.prefs.edit(|s| s.sources.push(entry.clone()));
        let session = super::super::session::SourceSession {
            online: true,
            settings: Some(Settings { sources: vec![entry], ..Default::default() }),
            ..Default::default()
        };
        app.session.accounts.insert(office, session);
    }
    reset(h, FULL, Route::Albums);
    for account in [crate::test_tracks::account(), local_account()] {
        app_mut(h).actions.push(Action::Scope(all_sources::ScopeAction::Chip(shell::kit::SourceChip::Toggle(account))));
        h.run_steps(2);
    }
    // The sidebar's usual width has room for the count; a wider one, for
    // the names.
    s.shoot(h, "sources-narrowed-count");
    reset(h, FULL, Route::Albums);
    app_mut(h).prefs.edit(|p| p.sidebar_width = Some(300.0));
    s.shoot(h, "sources-narrowed");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Home server, Music folder, 2 of 3 sources").click();
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    s.shoot(h, "sources-narrowed-menu");
    egui::Popup::close_all(&h.ctx);
    {
        let app = app_mut(h);
        app.actions.push(Action::Scope(all_sources::ScopeAction::All));
        app.prefs.edit(|s| s.sources.retain(|e| e.id != office));
        app.session.accounts.remove(&office);
    }
    h.run_steps(2);

    // The queue holds songs from both: each row's dot follows its artist.
    reset(h, FULL, Route::Songs);
    for (i, item) in app_mut(h).player.queue.upcoming.iter_mut().enumerate() {
        if i % 2 == 1 {
            item.key.account = local_account();
        }
    }
    s.shoot(h, "sources-queue");

    // An Ond playlist holding songs from both carries both dots, on the
    // Playlists page and in the sidebar, and its songs get a Source column.
    {
        let app = app_mut(h);
        let tracks: Vec<library::Track> = app.data.tracks.all.iter().step_by(7).take(8).cloned().collect();
        let playlist = crate::source::Playlist {
            id: "ond:mixed".into(),
            name: "Both at once".into(),
            song_count: tracks.len() as u32,
            duration: tracks.iter().filter_map(|t| t.duration_secs).sum(),
            cover_art: None,
            owner: None,
        };
        app.multi.ond_playlists.push(playlist);
        app.multi.ond_tracks.insert("ond:mixed".into(), std::sync::Arc::new(tracks));
        app.multi.projection = None;
        app.project_sources();
    }
    reset(h, FULL, Route::Playlists);
    s.shoot(h, "sources-playlists");
    reset(h, FULL, Route::Playlist { id: "ond:mixed".into(), name: "Both at once".into() });
    s.shoot(h, "sources-playlist");

    // Mini: the playing song's dot after its artist.
    reset(h, vec2(280.0, 280.0), Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Mini);
    h.hover_at(pos2(140.0, 140.0));
    h.run_steps(6);
    s.shoot(h, "sources-mini-hover");

    // The Player: the same dot after the artist.
    reset(h, vec2(380.0, 880.0), Route::Recent);
    app_mut(h).edit_windows(|w| w.mode = WindowMode::Player);
    s.shoot(h, "sources-player");

    // Last, since nothing undoes it: matching albums combined. Each source
    // holds a copy of one of the other's albums.
    reset(h, FULL, Route::Albums);
    combined_copies(app_mut(h));
    app_mut(h).actions.push(Action::OpenSettings(settings_page::Section::Sources));
    s.shoot(h, "sources-combined-setting");

    // A dot for each source with a copy, after the artist. Newest first
    // brings both combined albums to the top.
    reset(h, FULL, Route::Albums);
    app_mut(h).ui.sort = AlbumSort::Added;
    s.shoot(h, "sources-combined-albums");
    app_mut(h).ui.sort = AlbumSort::Artist;

    // Every song from both copies, a dot for each copy a song has, and the
    // copies' formats under the facts.
    let vanguard = accounts::ItemKey::new(crate::test_tracks::account(), album_id(0)).to_string();
    reset(h, FULL, Route::Album(vanguard.clone()));
    s.shoot(h, "sources-combined-album");

    // By album, last first: Sunday at the Village Vanguard (songs on both,
    // one only on each), then Kind of Blue (one disc on both, the bonus
    // disc only on the folder).
    reset(h, FULL, Route::Songs);
    app_mut(h).ui.songs.sort = super::super::songs::Column::Album;
    app_mut(h).ui.songs.descending = true;
    s.shoot(h, "sources-combined-songs");

    // Bill Evans Trio's albums under each source's heading, the combined
    // one once, under the server's, where its lead is.
    reset(h, FULL, Route::Artist { name: "Bill Evans Trio".into(), id: None });
    s.shoot(h, "sources-combined-artist");

    reset(h, FULL, Route::Albums);
    app_mut(h).actions.push(Action::OpenPalette);
    h.run_steps(2);
    {
        let app = app_mut(h);
        if let (Some(p), Some(index)) = (app.ui.palette.as_mut(), app.search.index.as_mut()) {
            p.state.query = "gloria".into();
            p.refresh(index);
        }
    }
    s.shoot(h, "sources-combined-search");

    // Only the folder's copies starred: the album's heart and the song's
    // are lit all the same.
    let folder = accounts::ItemKey::new(local_account(), album_id(0)).to_string();
    {
        let app = app_mut(h);
        let song = app.multi.data[&local_account()].tracks.album(&folder).and_then(|songs| songs.first().cloned());
        let starred = crate::favorites::Starred { songs: song.into_iter().collect(), albums: vec![folder.clone()], artists: Vec::new() };
        app.favorites.load_source(local_account(), starred, Vec::new());
        app.favorites.combine(&app.data, app.prefs_now().prefer_copy);
    }
    reset(h, FULL, Route::Album(vanguard.clone()));
    s.shoot(h, "sources-combined-loved");

    // Its menu offers Keep apart.
    reset(h, FULL, Route::Album(vanguard.clone()));
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "More").click();
    s.shoot(h, "sources-combined-menu");

    // Kept apart: the two copies side by side again, with the toast's Undo.
    reset(h, FULL, Route::Albums);
    app_mut(h).ui.sort = AlbumSort::Added;
    {
        let app = app_mut(h);
        app.apply(Action::KeepApart(vanguard.clone()));
        let deadline = testkit::Deadline::new();
        while app.multi.apart_pairs.is_empty() {
            assert!(!deadline.passed(), "the copies were never kept apart");
            if let Ok(msg) = app.inbox.recv_timeout(std::time::Duration::from_millis(10)) {
                app.handle(msg);
            }
        }
    }
    s.shoot(h, "sources-combined-apart");
    app_mut(h).ui.sort = AlbumSort::Artist;

    // Either copy's menu offers Combine again.
    reset(h, FULL, Route::Album(folder));
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "More").click();
    s.shoot(h, "sources-combined-apart-menu");
}

/// Settings with some set in managed.json (as the Home Manager module writes
/// it): their rows and the output picker locked, and the sign-in form.
#[test]
#[ignore = "renders PNGs for review; run with --ignored"]
fn managed_snapshots() {
    shoot_scenes(&[(egui::Theme::Light, "light"), (egui::Theme::Dark, "dark")], Some(MANAGED), managed_scenes);
}

fn managed_scenes(h: &mut Harness<'_, Option<OndApp>>, s: &Shooter) {
    for section in [
        settings_page::Section::Appearance,
        settings_page::Section::Playback,
        settings_page::Section::Output,
        settings_page::Section::Offline,
        settings_page::Section::Sources,
        settings_page::Section::About,
    ] {
        reset(h, FULL, Route::Recent);
        app_mut(h).actions.push(Action::OpenSettings(section));
        s.shoot(h, &format!("managed-settings-{}", section.label().to_lowercase()));
    }
    reset(h, FULL, Route::Recent);
    h.run_steps(2);
    h.get_by(|n| n.label().is_some_and(|l| l.starts_with("Output: UMC22"))).click();
    s.shoot(h, "managed-output-picker");
    egui::Popup::close_all(&h.ctx);
    reset(h, FULL, Route::Recent);
    app_mut(h).login = Some(super::session::LoginForm { server: "https://music.example.com".into(), username: "demo".into(), ..Default::default() });
    app_mut(h).onboarding = Some(Onboard { step: OnboardingStep::Form(0), ..Default::default() });
    s.shoot(h, "managed-sign-in");
    app_mut(h).login = None;
    app_mut(h).onboarding = None;
}

/// Zooms to `percent` as the keys do, without the toast.
fn zoom(h: &mut Harness<'_, Option<OndApp>>, percent: u16) {
    let app = app_mut(h);
    app.apply(Action::Prefs(PrefsAction::Zoom(percent)));
    app.ui.toasts = shell::kit::Toasts::default();
}
