//! An album: header with links for artist / genre / year / label, then the
//! track list (composer and actions appear on hover), then "Album details":
//! credits gathered from the tracks' OpenSubsonic contributors and the
//! per-track measurements, folded away until asked for. Playlists reuse the
//! track list.

use super::widgets::{Mark, chip, elide, heart_button, link};
use super::{Action, DragTracks, Focus, Keep, ListenerApp, Overlay, PlaylistAction, QueueAction};
use crate::favorites::StarKind;
use crate::icons::{self, Icon};
use crate::nav::{FilterKind, Route};
use crate::queue::Source;
use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use analysis::TrackAnalysis;
use library::Track;

/// One line of credits: a role and the people in it, in first-seen order.
#[derive(Debug, Clone, PartialEq)]
pub struct Credit {
    pub role: String,
    /// (name, artist id)
    pub people: Vec<(String, Option<String>)>,
}

/// Groups the tracks' contributors by role. Performers come first, labelled by
/// instrument ("Piano"); then writers, then production. The main artists are
/// already in the header, so the "artist" roles are left out.
pub fn credits(tracks: &[Track]) -> Vec<Credit> {
    let mut out: Vec<Credit> = Vec::new();
    for t in tracks {
        for c in &t.contributors {
            let role = c.role.to_lowercase();
            if matches!(role.as_str(), "artist" | "albumartist") {
                continue;
            }
            let label = match (&role[..], c.sub_role.as_deref()) {
                ("performer", Some(sub)) if !sub.is_empty() => crate::fmt::capitalize(sub),
                _ => crate::fmt::capitalize(&role),
            };
            let entry = match out.iter_mut().position(|e| e.role == label) {
                Some(i) => &mut out[i],
                None => {
                    out.push(Credit { role: label, people: Vec::new() });
                    out.last_mut().expect("just pushed")
                }
            };
            if !entry.people.iter().any(|(n, _)| n == &c.name) {
                entry.people.push((c.name.clone(), c.artist_id.clone()));
            }
        }
    }
    let rank = |role: &str| match role.to_lowercase().as_str() {
        "composer" => 1,
        "lyricist" | "arranger" => 2,
        "conductor" => 3,
        "producer" => 4,
        "engineer" | "mixer" | "remixer" | "djmixer" => 5,
        _ => 0, // instruments and other performer roles
    };
    out.sort_by_key(|c| rank(&c.role));
    out
}


/// `FLAC 16/44.1` when every track shares it.
fn shared_format(tracks: &[Track]) -> Option<String> {
    let first = crate::fmt::quality(tracks.first()?);
    tracks.iter().all(|t| crate::fmt::quality(t) == first).then_some(first)
}

/// Per-track loudness, peak and dynamic range, with any finding the album as a
/// whole doesn't already show. The title column takes what's left.
fn analysis_table(ui: &mut Ui, tracks: &[Track], found: &[Option<TrackAnalysis>], album_verdict: Option<&analysis::Verdict>) {
    let t = Tokens::of(ui);
    let small = |s: String, c| egui::RichText::new(s).size(12.0).color(c);
    let title_w = (ui.available_width() - 360.0).clamp(90.0, 320.0);
    egui::Grid::new("album-analysis").num_columns(6).spacing(vec2(14.0, 6.0)).show(ui, |ui| {
        for h in ["", "", "Loudness", "Peak", "DR", ""] {
            ui.label(small(h.to_owned(), t.muted));
        }
        ui.end_row();
        for (i, (track, a)) in tracks.iter().zip(found).enumerate() {
            ui.label(small(track.track.unwrap_or(i as u32 + 1).to_string(), t.muted));
            let g = elide(ui, &track.title, FontId::proportional(12.0), t.text, title_w);
            let (r, _) = ui.allocate_exact_size(vec2(title_w, g.size().y), Sense::hover());
            ui.painter().galley(r.min, g, t.text);
            match a {
                Some(a) => {
                    let dash = || "–".to_owned();
                    ui.label(small(a.integrated_lufs.map_or_else(dash, |l| crate::fmt::level(l, "LUFS")), t.text));
                    ui.label(small(a.true_peak_dbtp.map_or_else(dash, |p| crate::fmt::level(p, "dBTP")), t.text));
                    ui.label(small(a.dr_rounded().map_or_else(dash, |d| format!("DR{d}")), t.text));
                    match a.verdicts.iter().find(|v| album_verdict.is_none_or(|av| !av.same_kind(v))) {
                        Some(v) => {
                            chip(ui, &v.label(), t.warn).on_hover_text(v.evidence());
                        }
                        None => {
                            ui.label("");
                        }
                    }
                }
                None => {
                    ui.label(small("not analysed".into(), t.muted));
                    for _ in 0..3 {
                        ui.label("");
                    }
                }
            }
            ui.end_row();
        }
    });
}

#[derive(Clone)]
pub(super) struct ListOpts {
    pub numbers: bool,
    pub artist: bool,
    /// The user's own playlist being shown: rows can be removed and dragged
    /// to reorder.
    pub playlist: Option<String>,
}

impl ListenerApp {
    pub(super) fn album_page(&mut self, ui: &mut Ui, id: &str) {
        let t = Tokens::of(ui);
        let Some(album) = self.data.album(id).cloned() else {
            ui.label(egui::RichText::new("Loading…").color(t.muted));
            return;
        };
        let tracks = self.data.tracks.album(id).cloned().unwrap_or_default();
        if self.download_key(ui) {
            let kept = self.offline_snap.pin(&offline::Pin::Album(id.to_owned())).is_some();
            self.actions.push(Action::KeepOffline(Keep::Album(id.to_owned()), !kept));
        }
        let found: Vec<Option<TrackAnalysis>> = tracks.iter().map(|t| self.analysis_of(t).cloned()).collect();
        let summary = analysis::summarize(&found.iter().flatten().collect::<Vec<_>>(), tracks.len());
        let analysing = self.analyzer.pending(tracks.iter().map(|t| t.id.as_str()));
        egui::ScrollArea::vertical().id_salt(("album", id)).auto_shrink(false).show(ui, |ui| {
            // Header: art beside the text, or above it when there's no room.
            let avail = ui.available_width();
            let stacked = avail < 460.0;
            let art_side = if stacked { avail.min(220.0) } else { (avail * 0.3).clamp(150.0, 220.0) };
            let layout = if stacked { egui::Layout::top_down(egui::Align::Min) } else { egui::Layout::left_to_right(egui::Align::Min) };
            ui.with_layout(layout, |ui| {
                let (art, _) = ui.allocate_exact_size(vec2(art_side, art_side), Sense::hover());
                self.art.paint(ui, art, album.cover_art.as_deref(), 6.0);
                ui.add_space(if stacked { 16.0 } else { 20.0 });
                // The text block sits on the artwork's bottom edge. Its height
                // (it wraps) is known after drawing, so use last frame's and
                // redo the frame when it changed.
                let measured = ui.scope_id().with(("album-header-h", id));
                let last_h = ui.data(|d| d.get_temp::<f32>(measured)).unwrap_or(150.0);
                let block = ui.vertical(|ui| {
                    if !stacked {
                        ui.add_space((art_side - last_h).max(0.0));
                    }
                    let top = ui.cursor().top();
                    ui.add(egui::Label::new(egui::RichText::new(&album.name).font(FontId::new(26.0, semibold())).color(t.text)).wrap());
                    if let Some(artist) = &album.artist
                        && link(ui, artist, FontId::proportional(18.0), t.accent).clicked()
                    {
                        self.actions.push(Action::Go(Route::Artist { name: artist.clone(), id: album.artist_ref.clone() }));
                    }
                    ui.add_space(2.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let small = FontId::proportional(12.0);
                        let mut first = true;
                        let mut sep = |ui: &mut Ui| {
                            if !first {
                                ui.label(egui::RichText::new("  ·  ").size(12.0).color(t.muted));
                            }
                            first = false;
                        };
                        for g in album.all_genres() {
                            sep(ui);
                            if link(ui, g, small.clone(), t.muted).clicked() {
                                self.actions.push(Action::Go(Route::Filter { kind: FilterKind::Genre, value: g.to_owned() }));
                            }
                        }
                        if let Some(y) = album.year {
                            sep(ui);
                            if link(ui, &y.to_string(), small.clone(), t.muted).clicked() {
                                self.actions.push(Action::Go(Route::Filter { kind: FilterKind::Year, value: y.to_string() }));
                            }
                        }
                        for l in &album.labels {
                            sep(ui);
                            if link(ui, l, small.clone(), t.muted).clicked() {
                                self.actions.push(Action::Go(Route::Filter { kind: FilterKind::Label, value: l.clone() }));
                            }
                        }
                        if let Some(f) = shared_format(&tracks) {
                            sep(ui);
                            ui.label(egui::RichText::new(f).size(12.0).color(t.muted));
                        }
                        // Measurements once the whole album is known; a quiet
                        // count while it's being worked on.
                        if let (true, Some(dr), Some(lufs)) = (summary.complete(), summary.dr, summary.lufs) {
                            sep(ui);
                            ui.label(egui::RichText::new(format!("DR{dr}")).size(12.0).color(t.muted))
                                .on_hover_text("Dynamic range (DR meter): the album's average");
                            sep(ui);
                            ui.label(egui::RichText::new(crate::fmt::level(lufs, "LUFS")).size(12.0).color(t.muted))
                                .on_hover_text("Integrated loudness (EBU R128), across the album");
                        } else if analysing > 0 {
                            sep(ui);
                            ui.label(egui::RichText::new(format!("analysing {}/{}", summary.analyzed, tracks.len())).size(12.0).color(t.muted));
                        }
                        if let Some((v, n)) = &summary.verdict {
                            ui.add_space(10.0);
                            let tracks_note = if *n == summary.analyzed { "every analysed track".to_owned() } else { format!("{n} of {} analysed tracks", summary.analyzed) };
                            chip(ui, &v.label(), t.warn).on_hover_text(format!("{}\n\nSeen in {tracks_note}.", v.evidence()));
                        }
                    });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if let Some(a) = super::browse::play_shuffle(ui) {
                            self.actions.push(Action::Album(album.id.clone(), a));
                        }
                        let loved = self.favorites.is_on(StarKind::Album, &album.id);
                        if heart_button(ui, loved, 30.0, 14.0).clicked() {
                            self.actions.push(Action::ToggleStar { kind: StarKind::Album, id: album.id.clone(), track: None, name: None });
                        }
                        let more = icons::sized_button(ui, Icon::More, "More", vec2(30.0, 30.0), 13.0, Some(t.accent));
                        let pin = offline::Pin::Album(album.id.clone());
                        let kept = self.offline_snap.pin(&pin).is_some();
                        egui::Popup::menu(&more).show(|ui| {
                            let c = super::browse::Collection::Album(album.id.clone());
                            super::browse::collection_menu(ui, &c, false, kept, Some("⇧⌘D"), &mut self.actions);
                            if !summary.complete() && analysing == 0 && !tracks.is_empty() {
                                ui.separator();
                                if ui.button("Analyze album").on_hover_text("Loudness, dynamic range, and a check for upsampled or lossy-sourced files").clicked() {
                                    self.actions.push(Action::Analyze(tracks.to_vec()));
                                }
                            }
                            if !tracks.is_empty() {
                                ui.separator();
                                self.add_to_playlist_menu(ui, &tracks);
                            }
                        });
                        ui.add_space(6.0);
                        self.pin_status(ui, &pin);
                    });
                    ui.min_rect().bottom() - top
                });
                let h = block.inner.round();
                if (h - last_h).abs() > 0.5 {
                    ui.data_mut(|d| d.insert_temp(measured, h));
                    ui.ctx().request_discard("album header height changed");
                }
            });
            ui.add_space(24.0);
            if tracks.is_empty() {
                ui.label(egui::RichText::new("Loading tracks…").color(t.muted));
                return;
            }

            let source = Source::Album { id: album.id.clone(), name: album.name.clone() };
            self.track_list(ui, &tracks, &source, ListOpts { numbers: true, artist: false, playlist: None });
            ui.add_space(14.0);
            let secs = tracks.iter().filter_map(|t| t.duration_secs).sum::<u32>();
            let mut footer = format!("{} · {}", crate::fmt::plural(tracks.len(), "song"), crate::fmt::span(secs));
            if let Some(y) = album.year {
                footer.push_str(&format!(" · {y}"));
            }
            if let Some(l) = album.labels.first() {
                footer.push_str(&format!(" · {l}"));
            }
            ui.label(egui::RichText::new(footer).size(12.0).color(t.muted));
            ui.add_space(10.0);
            self.album_details(ui, &tracks, &found, summary.verdict.as_ref().map(|(v, _)| v), analysing > 0);
            ui.add_space(24.0);
        });
    }

    /// "Album details ›": the album's credits and its per-track measurements,
    /// folded away until asked for (open stays open for the session).
    fn album_details(&mut self, ui: &mut Ui, tracks: &[Track], found: &[Option<TrackAnalysis>], album_verdict: Option<&analysis::Verdict>, analysing: bool) {
        let t = Tokens::of(ui);
        let open = self.album_details_open;
        let toggle = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let l = link(ui, "Album details", FontId::proportional(12.0), t.accent);
            let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            icons::paint(ui.painter(), rect, if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.accent);
            l
        });
        if toggle.inner.clicked() {
            self.album_details_open = !open;
        }
        if !open {
            return;
        }
        let width = ui.available_width().min(720.0);
        ui.allocate_ui_with_layout(vec2(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_max_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            let people = credits(tracks);
            if !people.is_empty() {
                super::widgets::kv_section(ui, "Credits");
                for c in &people {
                    let items = c.people.iter().map(|(name, id)| (name.clone(), Action::Go(Route::Artist { name: name.clone(), id: id.clone() }))).collect();
                    self.links_row(ui, &c.role, items);
                }
            }
            super::widgets::kv_section(ui, "Analysis");
            if found.iter().all(Option::is_none) {
                super::widgets::kv_row(ui, "Loudness", |ui| {
                    if analysing {
                        ui.label(egui::RichText::new("Analysing…").size(12.5).color(t.muted));
                    } else if link(ui, "Analyze album", FontId::proportional(12.5), t.accent)
                        .on_hover_text("Loudness, dynamic range, and a check for upsampled or lossy-sourced files")
                        .clicked()
                    {
                        self.actions.push(Action::Analyze(tracks.to_vec()));
                    }
                });
            } else {
                ui.spacing_mut().item_spacing.y = 6.0;
                analysis_table(ui, tracks, found, album_verdict);
            }
        });
    }

    /// A playlist: a mosaic of its first albums' covers, name, length, Play /
    /// Shuffle / ⋯, then its tracks. The user's own playlists can be renamed,
    /// deleted, reordered by dragging (or ⌥↑/↓) and trimmed (⌫).
    pub(super) fn playlist_page(&mut self, ui: &mut Ui, id: &str, name: &str) {
        let t = Tokens::of(ui);
        let info = self.data.playlists.iter().find(|p| p.id == id).cloned();
        let tracks = self.data.playlist_tracks.get(id).cloned();
        let mine = info.as_ref().is_some_and(|p| self.owns(p));
        let source = Source::Playlist { id: id.to_owned(), name: name.to_owned() };
        let pin = offline::Pin::Playlist(id.to_owned());
        let kept = self.offline_snap.pin(&pin).is_some();
        if self.download_key(ui) {
            self.actions.push(Action::KeepOffline(Keep::Playlist(id.to_owned()), !kept));
        }

        let art_side = (ui.available_width() * 0.3).clamp(120.0, 180.0);
        ui.horizontal(|ui| {
            let (art, _) = ui.allocate_exact_size(vec2(art_side, art_side), Sense::hover());
            self.mosaic(ui, art, tracks.as_deref().map_or(&[], Vec::as_slice));
            ui.add_space(20.0);
            ui.vertical(|ui| {
                ui.add_space((art_side - 118.0).max(0.0));
                ui.label(egui::RichText::new("Playlist").font(FontId::new(11.0, semibold())).color(t.muted));
                ui.add(egui::Label::new(egui::RichText::new(name).font(FontId::new(26.0, semibold())).color(t.text)).truncate());
                // The loaded songs are newer than the playlist list's counts
                // (an edit since the last refresh).
                let counts = match (&tracks, &info) {
                    (Some(ts), _) => Some((ts.len(), ts.iter().filter_map(|t| t.duration_secs).sum::<u32>())),
                    (None, Some(p)) => Some((p.song_count as usize, p.duration)),
                    (None, None) => None,
                };
                let sub = match counts {
                    Some((n, secs)) if n > 0 => format!("{} · {}", crate::fmt::plural(n, "song"), crate::fmt::span(secs)),
                    Some(_) => "Empty".to_owned(),
                    None => String::new(),
                };
                ui.label(egui::RichText::new(sub).size(12.0).color(t.muted));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let has_tracks = tracks.as_ref().is_some_and(|t| !t.is_empty());
                    ui.add_enabled_ui(has_tracks, |ui| {
                        if let Some(a) = super::browse::play_shuffle(ui) {
                            self.actions.push(Action::Playlist(id.to_owned(), a));
                        }
                    });
                    let more = icons::sized_button(ui, Icon::More, "More", vec2(30.0, 30.0), 13.0, Some(t.accent));
                    egui::Popup::menu(&more).show(|ui| {
                        let c = super::browse::Collection::Playlist(id.to_owned());
                        super::browse::collection_menu(ui, &c, false, kept, Some("⇧⌘D"), &mut self.actions);
                        if mine {
                            ui.separator();
                            self.playlist_edit_items(ui, id, name);
                        }
                    });
                    ui.add_space(6.0);
                    self.pin_status(ui, &pin);
                });
            });
        });
        ui.add_space(20.0);
        let Some(tracks) = tracks else {
            ui.label(egui::RichText::new("Loading…").color(t.muted));
            return;
        };
        if tracks.is_empty() {
            let hint = if mine { "Drag songs onto this playlist in the sidebar, or use “Add to Playlist” on any song." } else { "Its owner hasn’t added any songs." };
            super::widgets::empty_state(ui, Icon::Note, "This playlist is empty", hint);
            return;
        }
        egui::ScrollArea::vertical().id_salt(("playlist", id)).auto_shrink(false).show(ui, |ui| {
            self.track_list(ui, &tracks, &source, ListOpts { numbers: false, artist: true, playlist: mine.then(|| id.to_owned()) });
        });
    }

    /// Rename… and Delete… for one of the user's playlists.
    pub(super) fn playlist_edit_items(&mut self, ui: &mut Ui, id: &str, name: &str) {
        if ui.button("Rename…").clicked() {
            self.actions.push(Action::Overlay(Overlay::Rename { id: id.to_owned(), name: name.to_owned() }));
        }
        if ui.button("Delete…").clicked() {
            self.actions.push(Action::Overlay(Overlay::Delete { id: id.to_owned(), name: name.to_owned() }));
        }
    }

    /// Playlists the user can change (others' public ones are read-only).
    pub(super) fn owns(&self, p: &subsonic::Playlist) -> bool {
        match (&p.owner, &self.client) {
            (Some(owner), Some(c)) => owner == c.username(),
            _ => true,
        }
    }

    /// Up to four album covers in a 2×2 square (one fills it; none: a note glyph).
    fn mosaic(&mut self, ui: &mut Ui, rect: Rect, tracks: &[Track]) {
        let t = Tokens::of(ui);
        let mut covers: Vec<String> = Vec::new();
        for tr in tracks {
            let cover = self.data.cover_for(tr).map(str::to_owned);
            if let Some(c) = cover
                && !covers.contains(&c)
            {
                covers.push(c);
                if covers.len() == 4 {
                    break;
                }
            }
        }
        match covers.len() {
            0 => {
                ui.painter().rect_filled(rect, 6.0, t.field);
                icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(rect.width() * 0.28, rect.width() * 0.28)), Icon::Note, t.muted);
            }
            1..=3 => self.art.paint(ui, rect, Some(&covers[0]), 6.0),
            _ => {
                let half = rect.width() / 2.0;
                for (i, c) in covers.iter().enumerate() {
                    let at = pos2(rect.left() + half * (i % 2) as f32, rect.top() + half * (i / 2) as f32);
                    // Only the mosaic's outer corners are rounded.
                    let r = 6;
                    let corners = match i {
                        0 => egui::CornerRadius { nw: r, ..Default::default() },
                        1 => egui::CornerRadius { ne: r, ..Default::default() },
                        2 => egui::CornerRadius { sw: r, ..Default::default() },
                        _ => egui::CornerRadius { se: r, ..Default::default() },
                    };
                    self.art.paint(ui, Rect::from_min_size(at, vec2(half, half)), Some(c), corners);
                }
            }
        }
    }

    /// The Add to Playlist submenu: New Playlist…, then recently used, then the rest.
    pub(super) fn add_to_playlist_menu(&mut self, ui: &mut Ui, tracks: &[Track]) {
        ui.menu_button("Add to Playlist", |ui| {
            if ui.button("New Playlist…").clicked() {
                self.actions.push(Action::Overlay(Overlay::new_playlist(tracks.to_vec())));
            }
            let mine: Vec<subsonic::Playlist> = self.data.playlists.iter().filter(|p| self.owns(p)).cloned().collect();
            let recent: Vec<&subsonic::Playlist> = self.recent_playlists.iter().filter_map(|id| mine.iter().find(|p| &p.id == id)).collect();
            let mut rest: Vec<&subsonic::Playlist> = mine.iter().filter(|p| !self.recent_playlists.contains(&p.id)).collect();
            rest.sort_by_cached_key(|p| p.name.to_lowercase());
            for group in [recent, rest] {
                if group.is_empty() {
                    continue;
                }
                ui.separator();
                for p in group {
                    if ui.button(&p.name).clicked() {
                        self.actions.push(Action::PlaylistEdit(PlaylistAction::Add { id: p.id.clone(), tracks: tracks.to_vec() }));
                    }
                }
            }
        });
    }

    /// Rows of tracks. Click selects, double-click (or ⏎) plays from there;
    /// ⌥⏎ plays next, ⇧⏎ adds to the queue. Hovering a row shows its composer
    /// and the queue actions; a loved track keeps its heart. Rows drag onto
    /// a sidebar playlist, and within the user's own playlist, to reorder.
    pub(super) fn track_list(&mut self, ui: &mut Ui, tracks: &[Track], source: &Source, opts: ListOpts) {
        let t = Tokens::of(ui);
        self.track_list_keys(ui, tracks, source, opts.playlist.as_deref());
        let current = self.player.queue.current.as_ref().map(|c| c.track.id.clone());
        let discs = tracks.iter().filter_map(|t| t.disc).max().unwrap_or(1);
        let mut last_disc = None;
        // Where a reorder drag would land: (index to insert at, y of the line).
        let mut drop_at: Option<(usize, f32)> = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, tr) in tracks.iter().enumerate() {
            if discs > 1 && opts.playlist.is_none() && tr.disc != last_disc {
                last_disc = tr.disc;
                ui.add_space(if i == 0 { 0.0 } else { 12.0 });
                ui.label(egui::RichText::new(format!("Disc {}", tr.disc.unwrap_or(1))).font(FontId::new(12.0, semibold())).color(t.muted));
                ui.add_space(4.0);
            }
            let h = if opts.artist { 44.0 } else { 38.0 };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click_and_drag());
            let playing = current.as_deref() == Some(tr.id.as_str());
            if self.reveal.as_deref() == Some(tr.id.as_str()) {
                self.reveal = None;
                self.track_sel = Some(i);
                self.focus = Focus::Main;
                ui.scroll_to_rect(rect, Some(egui::Align::Center));
            }
            // Off screen: the space is kept, nothing else to do (long lists).
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let selected = self.focus == Focus::Main && self.track_sel == Some(i);
            let hovered = resp.hovered() || resp.contains_pointer();
            let loved = self.favorites.is_on(StarKind::Song, &tr.id);
            // Drag: the selection's track (or this one) to a playlist; within
            // one's own playlist, also to a new place in it.
            // (Built only when the drag starts, the one frame egui takes it: a
            // copy per row per frame adds up on long lists.)
            if resp.drag_started() {
                resp.dnd_set_drag_payload(DragTracks { tracks: vec![tr.clone()], from: opts.playlist.clone().map(|p| (p, i)) });
            }
            if let Some(pid) = &opts.playlist {
                if let Some(drag) = resp.dnd_hover_payload::<DragTracks>()
                    && drag.from.as_ref().is_some_and(|(p, _)| p == pid)
                    && let Some(p) = ui.ctx().pointer_hover_pos()
                {
                    let below = p.y > rect.center().y;
                    drop_at = Some((if below { i + 1 } else { i }, if below { rect.bottom() } else { rect.top() }));
                }
                if let Some(drag) = resp.dnd_release_payload::<DragTracks>()
                    && let Some((p, from)) = &drag.from
                    && p == pid
                    && let Some(pos) = ui.ctx().pointer_interact_pos()
                {
                    let mut to = if pos.y > rect.center().y { i + 1 } else { i };
                    if to > *from {
                        to -= 1;
                    }
                    self.actions.push(Action::PlaylistEdit(PlaylistAction::Move { id: pid.clone(), from: *from, to }));
                }
            }
            if ui.is_rect_visible(rect) {
                ui.painter().rect_filled(rect, 6.0, super::widgets::row_fill(&t, selected, hovered));
                let mut x = rect.left() + 10.0;
                let cy = rect.center().y;
                // Offline, what isn't downloaded is dimmed rather than hidden.
                let available = self.playable(tr);
                let color = super::widgets::track_color(&t, playing, available);
                if let Some(m) = Mark::of_track(self.offline_snap.track(&tr.id)) {
                    super::widgets::mark(ui, pos2(rect.right() - 48.0, cy), 10.0, m);
                }
                if opts.numbers {
                    if playing {
                        icons::paint(ui.painter(), Rect::from_center_size(pos2(x + 12.0, cy), vec2(9.0, 9.0)), Icon::Play, t.accent);
                    } else {
                        let n = tr.track.map(|n| n.to_string()).unwrap_or_default();
                        ui.painter().text(pos2(x + 18.0, cy), egui::Align2::RIGHT_CENTER, n, FontId::proportional(12.0), t.muted);
                    }
                    x += 34.0;
                } else if playing {
                    icons::paint(ui.painter(), Rect::from_center_size(pos2(x + 4.0, cy), vec2(8.0, 8.0)), Icon::Play, t.accent);
                    x += 16.0;
                }
                let dur = tr.duration_secs.map(|d| crate::fmt::clock(f64::from(d))).unwrap_or_default();
                ui.painter().text(pos2(rect.right() - 10.0, cy), egui::Align2::RIGHT_CENTER, dur, FontId::proportional(12.0), t.muted);
                // The heart sits just left of the duration: always when loved,
                // on hover otherwise.
                let heart = Rect::from_center_size(pos2(rect.right() - 70.0, cy), vec2(24.0, 24.0));
                if loved || hovered {
                    let label = if loved { "Unlove" } else { "Love" };
                    let hr = super::widgets::hover_heart(ui, heart, ui.scope_id().with(("heart", &tr.id, i)), loved, true, 12.0, label);
                    if hr.on_hover_text(label).clicked() {
                        self.actions.push(Action::ToggleStar { kind: StarKind::Song, id: tr.id.clone(), track: Some(Box::new(tr.clone())), name: None });
                    }
                }
                let actions_w = if hovered { 150.0 } else { 0.0 };
                let text_w = rect.right() - 86.0 - actions_w - x;
                let title_font = if playing { FontId::new(13.0, semibold()) } else { FontId::proportional(13.0) };
                if opts.artist {
                    let title = elide(ui, &tr.title, title_font, color, text_w);
                    ui.painter().galley(pos2(x, cy - title.size().y), title, color);
                    let sub = elide(ui, tr.artist.as_deref().unwrap_or(""), FontId::proportional(12.0), t.muted, text_w);
                    ui.painter().galley(pos2(x, cy + 1.0), sub, t.muted);
                } else {
                    let title = elide(ui, &tr.title, title_font, color, text_w);
                    let title_w = title.size().x;
                    ui.painter().galley(pos2(x, cy - title.size().y / 2.0), title, color);
                    if hovered && !available {
                        let g = ui.painter().layout_no_wrap("Not downloaded".into(), FontId::proportional(12.0), t.muted);
                        if title_w + 10.0 + g.size().x < text_w {
                            ui.painter().galley(pos2(x + title_w + 10.0, cy - g.size().y / 2.0), g, t.muted);
                        }
                    }
                    // Composer, on hover only.
                    else if hovered && let Some(composer) = composer_of(tr) {
                        let at = Rect::from_min_size(pos2(x + title_w + 10.0, cy - 8.0), vec2((text_w - title_w - 10.0).max(0.0), 16.0));
                        if at.width() > 40.0 {
                            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(at).layout(egui::Layout::left_to_right(egui::Align::Center)));
                            child.spacing_mut().item_spacing.x = 0.0;
                            child.label(egui::RichText::new("by ").size(12.0).color(t.muted));
                            if link(&mut child, &composer, FontId::proportional(12.0), t.muted).clicked() {
                                let id = tr.contributors.iter().find(|c| c.name == composer).and_then(|c| c.artist_id.clone());
                                self.actions.push(Action::Go(Route::Artist { name: composer.clone(), id }));
                            }
                        }
                    }
                }
                if hovered {
                    let r = Rect::from_min_max(pos2(rect.right() - 86.0 - actions_w, rect.top()), pos2(rect.right() - 84.0, rect.bottom()));
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::right_to_left(egui::Align::Center)));
                    child.spacing_mut().item_spacing.x = 4.0;
                    let more = icons::sized_button(&mut child, Icon::More, "More", vec2(24.0, 22.0), 11.0, Some(t.muted));
                    egui::Popup::menu(&more).show(|ui| self.track_row_menu(ui, tr, i, source, opts.playlist.as_deref()));
                    if child.add(egui::Button::new(egui::RichText::new("Play next").size(11.0)).corner_radius(11.0).stroke(egui::Stroke::new(1.0, t.hairline))).clicked() {
                        self.actions.push(Action::Tracks { tracks: vec![tr.clone()], source: source.clone(), action: QueueAction::PlayNext });
                    }
                }
            }
            let label = format!("{}{}{}", tr.title, if loved { ", loved" } else { "" }, if playing { ", playing" } else { "" });
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListBoxOption, true, selected, &label));
            if resp.double_clicked() {
                self.actions.push(Action::Tracks { tracks: tracks.to_vec(), source: source.clone(), action: QueueAction::Play { start: i } });
            } else if resp.clicked() {
                self.track_sel = Some(i);
                self.focus = Focus::Main;
            }
            resp.context_menu(|ui| self.track_row_menu(ui, tr, i, source, opts.playlist.as_deref()));
        }
        if let Some((_, y)) = drop_at {
            let r = ui.min_rect();
            ui.painter().line_segment([pos2(r.left() + 4.0, y), pos2(r.right() - 4.0, y)], egui::Stroke::new(2.0, t.accent));
        }
    }

    /// [`track_menu`](Self::track_menu), plus Remove from Playlist in the user's own.
    fn track_row_menu(&mut self, ui: &mut Ui, tr: &Track, index: usize, source: &Source, playlist: Option<&str>) {
        self.track_menu(ui, tr, source);
        if let Some(pid) = playlist {
            ui.separator();
            if ui.button("Remove from Playlist").clicked() {
                self.actions.push(Action::PlaylistEdit(PlaylistAction::Remove { id: pid.to_owned(), index }));
            }
        }
    }

    pub(super) fn track_menu(&mut self, ui: &mut Ui, tr: &Track, source: &Source) {
        if ui.button("Play next").clicked() {
            self.actions.push(Action::Tracks { tracks: vec![tr.clone()], source: source.clone(), action: QueueAction::PlayNext });
        }
        if ui.button("Add to queue").clicked() {
            self.actions.push(Action::Tracks { tracks: vec![tr.clone()], source: source.clone(), action: QueueAction::Add });
        }
        ui.separator();
        let loved = self.favorites.is_on(StarKind::Song, &tr.id);
        if ui.button(if loved { "Unlove" } else { "Love" }).clicked() {
            self.actions.push(Action::ToggleStar { kind: StarKind::Song, id: tr.id.clone(), track: Some(Box::new(tr.clone())), name: None });
        }
        ui.menu_button("Rating", |ui| {
            let current = self.favorites.rating(&tr.id);
            for r in 1..=5u8 {
                let stars = format!("{}{}", "★".repeat(r.into()), "☆".repeat((5 - r).into()));
                if ui.add(egui::Button::selectable(current == r, stars)).clicked() {
                    self.actions.push(Action::Rate(tr.id.clone(), r));
                }
            }
            ui.separator();
            if ui.add_enabled(current > 0, egui::Button::new("Clear Rating")).clicked() {
                self.actions.push(Action::Rate(tr.id.clone(), 0));
            }
        });
        self.add_to_playlist_menu(ui, std::slice::from_ref(tr));
        if let Some(artist) = &tr.artist {
            ui.separator();
            if ui.button(format!("Go to {artist}")).clicked()
                && let Some(route) = Route::artist_of(tr)
            {
                self.actions.push(Action::Go(route));
            }
        }
        if !matches!(source, Source::Album { .. }) && ui.button("Go to album").clicked() {
            self.actions.push(Action::Go(Route::Album(tr.album_id.clone())));
        }
        // Kept on its own: can be removed on its own. Kept with an album or
        // playlist: that's where it's removed.
        let alone = self.offline_snap.pin(&offline::Pin::Track(tr.id.clone())).is_some();
        if alone || !self.offline_snap.is_done(&tr.id) {
            ui.separator();
            super::browse::offline_item(ui, alone, None, || Keep::Tracks(vec![tr.clone()]), &mut self.actions);
        }
    }

    /// Whether a list may take keys this frame: no palette or sheet is open
    /// over it and no text field is being typed into.
    pub(super) fn list_keys_free(&self, ui: &Ui) -> bool {
        self.palette.is_none() && self.overlay.is_none() && !ui.ctx().memory(|m| m.focused().is_some())
    }

    /// ⇧⌘D, when nothing else has the keyboard.
    pub(super) fn download_key(&self, ui: &Ui) -> bool {
        if !self.list_keys_free(ui) {
            return false;
        }
        ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::D)))
    }

    /// The list keys pressed this frame, consumed, when the list on `focus`
    /// may take them (see [`list_keys_free`](Self::list_keys_free)); `None`
    /// otherwise. `edit` also takes ⌥↑/⌥↓ and ⌫ (lists the user can change);
    /// `escape` takes Esc.
    pub(super) fn list_keys(&self, ui: &Ui, focus: Focus, edit: bool, escape: bool) -> Option<ListKeys> {
        if self.focus != focus || !self.list_keys_free(ui) {
            return None;
        }
        use egui::{Key, Modifiers};
        Some(ui.input_mut(|i| {
            let mut k = ListKeys {
                up: i.consume_key(Modifiers::NONE, Key::ArrowUp),
                down: i.consume_key(Modifiers::NONE, Key::ArrowDown),
                enter: i.consume_key(Modifiers::NONE, Key::Enter),
                play_next: i.consume_key(Modifiers::ALT, Key::Enter),
                add: i.consume_key(Modifiers::SHIFT, Key::Enter),
                ..ListKeys::default()
            };
            if edit {
                k.move_up = i.consume_key(Modifiers::ALT, Key::ArrowUp);
                k.move_down = i.consume_key(Modifiers::ALT, Key::ArrowDown);
                k.remove = i.consume_key(Modifiers::NONE, Key::Backspace) | i.consume_key(Modifiers::NONE, Key::Delete);
            }
            if escape {
                k.escape = i.consume_key(Modifiers::NONE, Key::Escape);
            }
            k
        }))
    }

    /// ↑/↓ select, ⏎ play from the selection, ⌥⏎ play next, ⇧⏎ add. In the
    /// user's own playlist, ⌥↑/⌥↓ move the selected song and ⌫ removes it.
    fn track_list_keys(&mut self, ui: &Ui, tracks: &[Track], source: &Source, playlist: Option<&str>) {
        if tracks.is_empty() {
            return;
        }
        let Some(k) = self.list_keys(ui, Focus::Main, playlist.is_some(), false) else { return };
        let last = tracks.len() - 1;
        self.track_sel = k.step(self.track_sel, tracks.len());
        let Some(sel) = self.track_sel.filter(|s| *s <= last) else { return };
        if let Some(pid) = playlist {
            if k.move_up && sel > 0 {
                self.actions.push(Action::PlaylistEdit(PlaylistAction::Move { id: pid.to_owned(), from: sel, to: sel - 1 }));
            }
            if k.move_down && sel < last {
                self.actions.push(Action::PlaylistEdit(PlaylistAction::Move { id: pid.to_owned(), from: sel, to: sel + 1 }));
            }
            if k.remove {
                self.actions.push(Action::PlaylistEdit(PlaylistAction::Remove { id: pid.to_owned(), index: sel }));
            }
        }
        if k.enter {
            self.actions.push(Action::Tracks { tracks: tracks.to_vec(), source: source.clone(), action: QueueAction::Play { start: sel } });
        }
        if k.play_next {
            self.actions.push(Action::Tracks { tracks: vec![tracks[sel].clone()], source: source.clone(), action: QueueAction::PlayNext });
        }
        if k.add {
            self.actions.push(Action::Tracks { tracks: vec![tracks[sel].clone()], source: source.clone(), action: QueueAction::Add });
        }
    }
}

/// The keys a list reacts to, as read (and consumed) by `list_keys`.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ListKeys {
    pub up: bool,
    pub down: bool,
    pub enter: bool,
    pub play_next: bool,
    pub add: bool,
    pub move_up: bool,
    pub move_down: bool,
    pub remove: bool,
    pub escape: bool,
}

impl ListKeys {
    /// The selection after ↑/↓ in a list of `len` rows: the first row when
    /// nothing was selected, clamped at both ends.
    pub fn step(&self, sel: Option<usize>, len: usize) -> Option<usize> {
        let last = len.checked_sub(1)?;
        let mut sel = sel;
        if self.down {
            sel = Some(sel.map_or(0, |s| (s + 1).min(last)));
        }
        if self.up {
            sel = Some(sel.map_or(0, |s| s.saturating_sub(1)));
        }
        sel
    }
}


fn composer_of(t: &Track) -> Option<String> {
    t.composer.clone().or_else(|| t.contributors.iter().find(|c| c.role.eq_ignore_ascii_case("composer")).map(|c| c.name.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::Contributor;

    fn track(contributors: &[(&str, Option<&str>, &str)]) -> Track {
        Track {
            id: "t".into(),
            album_id: "a".into(),
            title: "t".into(),
            artist: None,
            artist_ref: None,
            track: None,
            disc: None,
            duration_secs: None,
            suffix: Some("flac".into()),
            content_type: None,
            size: None,
            bit_rate_kbps: None,
            bit_depth: Some(16),
            sample_rate: Some(44_100),
            channels: None,
            track_gain: None,
            album_gain: None,
            track_peak: None,
            album_peak: None,
            composer: None,
            contributors: contributors
                .iter()
                .map(|(role, sub, name)| Contributor { role: (*role).into(), sub_role: sub.map(Into::into), name: (*name).into(), artist_id: None })
                .collect(),
            cover_art: None,
            artists: Vec::new(),
        }
    }

    #[test]
    fn credits_group_by_role_with_instruments_first() {
        let tracks = [
            track(&[("producer", None, "Orrin Keepnews"), ("composer", None, "Scott LaFaro"), ("performer", Some("piano"), "Bill Evans")]),
            track(&[("composer", None, "George Gershwin"), ("performer", Some("bass"), "Scott LaFaro"), ("albumartist", None, "Bill Evans Trio")]),
            track(&[("composer", None, "Scott LaFaro"), ("performer", Some("piano"), "Bill Evans")]),
        ];
        let c = credits(&tracks);
        let roles: Vec<&str> = c.iter().map(|c| c.role.as_str()).collect();
        assert_eq!(roles, ["Piano", "Bass", "Composer", "Producer"]);
        let composers: Vec<&str> = c[2].people.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(composers, ["Scott LaFaro", "George Gershwin"]);
        assert!(credits(&[track(&[])]).is_empty());
    }

    #[test]
    fn shared_format_only_when_uniform() {
        let a = track(&[]);
        let mut b = track(&[]);
        assert_eq!(shared_format(&[a.clone(), b.clone()]).as_deref(), Some("FLAC 16/44.1"));
        b.bit_depth = Some(24);
        assert_eq!(shared_format(&[a, b]), None);
        assert_eq!(shared_format(&[]), None);
    }

    #[test]
    fn arrow_keys_step_and_clamp() {
        let down = ListKeys { down: true, ..ListKeys::default() };
        let up = ListKeys { up: true, ..ListKeys::default() };
        assert_eq!(down.step(None, 3), Some(0), "the first press selects the first row");
        assert_eq!(down.step(Some(2), 3), Some(2), "stops at the end");
        assert_eq!(up.step(Some(0), 3), Some(0), "stops at the top");
        assert_eq!(up.step(Some(2), 3), Some(1));
        assert_eq!(ListKeys::default().step(Some(1), 3), Some(1));
        assert_eq!(down.step(None, 0), None, "an empty list has nothing to select");
    }
}
