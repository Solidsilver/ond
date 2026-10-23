//! An album: header with links for artist / genre / year / label, the track
//! list (composer and actions appear on hover), and credits gathered from the
//! tracks' OpenSubsonic contributors. Playlists reuse the track list.

use super::widgets::{chip, elide, link, primary, secondary};
use super::{Action, Focus, ListenerApp, QueueAction, mmss};
use crate::icons::{self, Icon};
use crate::nav::{FilterKind, Route};
use crate::queue::Source;
use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use analysis::TrackAnalysis;
use library::Track;

/// Credit roles shown before "Show all credits".
const CREDITS_FOLDED: usize = 6;

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
                ("performer", Some(sub)) if !sub.is_empty() => capitalize(sub),
                _ => capitalize(&role),
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

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// "−11.2" with a real minus sign.
fn signed(v: f64) -> String {
    format!("{v:.1}").replace('-', "\u{2212}")
}

/// `FLAC 16/44.1` when every track shares it.
fn shared_format(tracks: &[Track]) -> Option<String> {
    let first = tracks.first()?.quality_label();
    tracks.iter().all(|t| t.quality_label() == first).then_some(first)
}

#[derive(Clone, Copy)]
struct ListOpts {
    numbers: bool,
    artist: bool,
}

impl ListenerApp {
    pub(super) fn album_page(&mut self, ui: &mut Ui, id: &str) {
        let t = Tokens::of(ui);
        let Some(album) = self.data.album(id).cloned() else {
            ui.label(egui::RichText::new("Loading…").color(t.muted));
            return;
        };
        let tracks = self.data.tracks.get(id).cloned().unwrap_or_default();
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
                            ui.label(egui::RichText::new(format!("{} LUFS", signed(lufs))).size(12.0).color(t.muted))
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
                        if primary(ui, Some(Icon::Play), "Play").clicked() {
                            self.actions.push(Action::Album(album.id.clone(), QueueAction::Play { start: 0 }));
                        }
                        ui.add_space(4.0);
                        if secondary(ui, "Shuffle").clicked() {
                            self.actions.push(Action::Album(album.id.clone(), QueueAction::Shuffle));
                        }
                        let more = icons::sized_button(ui, Icon::More, "More", vec2(30.0, 30.0), 13.0, Some(t.accent));
                        egui::Popup::menu(&more).show(|ui| {
                            if ui.button("Play next").clicked() {
                                self.actions.push(Action::Album(album.id.clone(), QueueAction::PlayNext));
                            }
                            if ui.button("Add to queue").clicked() {
                                self.actions.push(Action::Album(album.id.clone(), QueueAction::Add));
                            }
                            if !summary.complete() && analysing == 0 && !tracks.is_empty() {
                                ui.separator();
                                if ui.button("Analyze album").on_hover_text("Loudness, dynamic range, and a check for upsampled or lossy-sourced files").clicked() {
                                    self.actions.push(Action::Analyze(tracks.clone()));
                                }
                            }
                        });
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

            let credits = credits(&tracks);
            let wide = ui.available_width() >= 860.0;
            let source = Source::Album { id: album.id.clone(), name: album.name.clone() };
            let list_w = if wide && !credits.is_empty() { ui.available_width() - 320.0 } else { ui.available_width() };
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(list_w);
                    self.track_list(ui, &tracks, &source, ListOpts { numbers: true, artist: false });
                    ui.add_space(14.0);
                    let mins = tracks.iter().filter_map(|t| t.duration_secs).sum::<u32>().div_ceil(60);
                    let n = tracks.len();
                    ui.label(egui::RichText::new(format!("{n} song{}, {mins} minutes", if n == 1 { "" } else { "s" })).size(12.0).color(t.muted));
                    if summary.analyzed > 0 {
                        ui.add_space(14.0);
                        self.analysis_ui(ui, &tracks, &found, summary.verdict.as_ref().map(|(v, _)| v));
                    }
                    if !wide {
                        ui.add_space(20.0);
                        self.credits_ui(ui, &credits);
                    }
                });
                if wide && !credits.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical(|ui| {
                        ui.set_width(280.0);
                        self.credits_ui(ui, &credits);
                    });
                }
            });
            ui.add_space(24.0);
        });
    }

    /// Per-track measurements, folded away until asked for.
    fn analysis_ui(&mut self, ui: &mut Ui, tracks: &[Track], found: &[Option<TrackAnalysis>], album_verdict: Option<&analysis::Verdict>) {
        let t = Tokens::of(ui);
        let label = if self.analysis_open { "Hide analysis" } else { "Show analysis" };
        if link(ui, label, FontId::proportional(12.0), t.accent).clicked() {
            self.analysis_open = !self.analysis_open;
        }
        if !self.analysis_open {
            return;
        }
        ui.add_space(8.0);
        let small = |s: String, c| egui::RichText::new(s).size(12.0).color(c);
        let title_w = (ui.available_width() - 330.0).clamp(120.0, 360.0);
        egui::Grid::new("analysis").num_columns(6).spacing(vec2(16.0, 6.0)).show(ui, |ui| {
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
                        ui.label(small(a.integrated_lufs.map_or_else(dash, |l| format!("{} LUFS", signed(l))), t.text));
                        ui.label(small(a.true_peak_dbtp.map_or_else(dash, |p| format!("{} dBTP", signed(p))), t.text));
                        ui.label(small(a.dr_rounded().map_or_else(dash, |d| format!("DR{d}")), t.text));
                        match a.verdicts.iter().find(|v| album_verdict.is_none_or(|av| !av.same_kind(v))) {
                            Some(v) => {
                                ui.label(small(v.label(), t.warn)).on_hover_text(v.evidence());
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

    fn credits_ui(&mut self, ui: &mut Ui, credits: &[Credit]) {
        if credits.is_empty() {
            return;
        }
        let t = Tokens::of(ui);
        super::widgets::section(ui, "Credits");
        ui.add_space(4.0);
        let shown = if self.credits_open { credits.len() } else { credits.len().min(CREDITS_FOLDED) };
        egui::Grid::new("credits").num_columns(2).spacing(vec2(12.0, 7.0)).min_col_width(80.0).show(ui, |ui| {
            for c in &credits[..shown] {
                ui.label(egui::RichText::new(&c.role).size(13.0).color(t.muted));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (i, (name, id)) in c.people.iter().enumerate() {
                        if i > 0 {
                            ui.label(egui::RichText::new(", ").color(t.muted));
                        }
                        if link(ui, name, FontId::proportional(13.0), t.accent).clicked() {
                            self.actions.push(Action::Go(Route::Artist { name: name.clone(), id: id.clone() }));
                        }
                    }
                });
                ui.end_row();
            }
        });
        if credits.len() > CREDITS_FOLDED {
            ui.add_space(6.0);
            let label = if self.credits_open { "Show fewer".to_owned() } else { format!("Show all credits ({})", credits.len()) };
            if link(ui, &label, FontId::proportional(12.0), t.accent).clicked() {
                self.credits_open = !self.credits_open;
            }
        }
    }

    pub(super) fn playlist_page(&mut self, ui: &mut Ui, id: &str, name: &str) {
        let t = Tokens::of(ui);
        let info = self.data.playlists.iter().find(|p| p.id == id).cloned();
        let tracks = self.data.playlist_tracks.get(id).cloned().unwrap_or_default();
        let sub = info.as_ref().map(|p| format!("{} songs · {}", p.song_count, mmss(f64::from(p.duration))));
        self.heading(ui, name, sub);
        ui.horizontal(|ui| {
            if primary(ui, Some(Icon::Play), "Play").clicked() {
                self.actions.push(Action::Playlist(id.to_owned(), QueueAction::Play { start: 0 }));
            }
            ui.add_space(4.0);
            if secondary(ui, "Shuffle").clicked() {
                self.actions.push(Action::Playlist(id.to_owned(), QueueAction::Shuffle));
            }
        });
        ui.add_space(16.0);
        if tracks.is_empty() {
            ui.label(egui::RichText::new("Loading…").color(t.muted));
            return;
        }
        let source = Source::Playlist { id: id.to_owned(), name: name.to_owned() };
        egui::ScrollArea::vertical().id_salt(("playlist", id)).auto_shrink(false).show(ui, |ui| {
            self.track_list(ui, &tracks, &source, ListOpts { numbers: false, artist: true });
        });
    }

    /// Rows of tracks. Click selects, double-click (or ⏎) plays from there;
    /// ⌥⏎ plays next, ⇧⏎ adds to the queue. Hovering a row shows its composer
    /// and the queue actions.
    fn track_list(&mut self, ui: &mut Ui, tracks: &[Track], source: &Source, opts: ListOpts) {
        let t = Tokens::of(ui);
        self.track_list_keys(ui, tracks, source);
        let current = self.queue.current.as_ref().map(|c| c.track.id.clone());
        let discs = tracks.iter().filter_map(|t| t.disc).max().unwrap_or(1);
        let mut last_disc = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, tr) in tracks.iter().enumerate() {
            if discs > 1 && tr.disc != last_disc {
                last_disc = tr.disc;
                ui.add_space(if i == 0 { 0.0 } else { 12.0 });
                ui.label(egui::RichText::new(format!("Disc {}", tr.disc.unwrap_or(1))).font(FontId::new(12.0, semibold())).color(t.muted));
                ui.add_space(4.0);
            }
            let h = if opts.artist { 44.0 } else { 38.0 };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
            let playing = current.as_deref() == Some(tr.id.as_str());
            let selected = self.focus == Focus::Main && self.track_sel == Some(i);
            let hovered = resp.hovered() || resp.contains_pointer();
            if ui.is_rect_visible(rect) {
                let fill = if selected {
                    t.selected
                } else if hovered {
                    t.hover
                } else {
                    egui::Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, 6.0, fill);
                let mut x = rect.left() + 10.0;
                let cy = rect.center().y;
                let color = if playing { t.accent } else { t.text };
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
                let dur = tr.duration_secs.map(|d| mmss(f64::from(d))).unwrap_or_default();
                ui.painter().text(pos2(rect.right() - 10.0, cy), egui::Align2::RIGHT_CENTER, dur, FontId::proportional(12.0), t.muted);
                let actions_w = if hovered { 150.0 } else { 0.0 };
                let text_w = rect.right() - 60.0 - actions_w - x;
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
                    // Composer, on hover only.
                    if hovered && let Some(composer) = composer_of(tr) {
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
                    let r = Rect::from_min_max(pos2(rect.right() - 60.0 - actions_w, rect.top()), pos2(rect.right() - 56.0, rect.bottom()));
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::right_to_left(egui::Align::Center)));
                    child.spacing_mut().item_spacing.x = 4.0;
                    let more = icons::sized_button(&mut child, Icon::More, "More", vec2(24.0, 22.0), 11.0, Some(t.muted));
                    egui::Popup::menu(&more).show(|ui| self.track_menu(ui, tr, source));
                    if child.add(egui::Button::new(egui::RichText::new("Play next").size(11.0)).corner_radius(11.0).stroke(egui::Stroke::new(1.0, t.hairline))).clicked() {
                        self.actions.push(Action::Tracks { tracks: vec![tr.clone()], source: source.clone(), action: QueueAction::PlayNext });
                    }
                }
            }
            let label = format!("{}{}", tr.title, if playing { ", playing" } else { "" });
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListBoxOption, true, selected, &label));
            if resp.double_clicked() {
                self.actions.push(Action::Tracks { tracks: tracks.to_vec(), source: source.clone(), action: QueueAction::Play { start: i } });
            } else if resp.clicked() {
                self.track_sel = Some(i);
                self.focus = Focus::Main;
            }
            resp.context_menu(|ui| self.track_menu(ui, tr, source));
        }
    }

    pub(super) fn track_menu(&mut self, ui: &mut Ui, tr: &Track, source: &Source) {
        if ui.button("Play next").clicked() {
            self.actions.push(Action::Tracks { tracks: vec![tr.clone()], source: source.clone(), action: QueueAction::PlayNext });
        }
        if ui.button("Add to queue").clicked() {
            self.actions.push(Action::Tracks { tracks: vec![tr.clone()], source: source.clone(), action: QueueAction::Add });
        }
        if let Some(artist) = &tr.artist {
            ui.separator();
            if ui.button(format!("Go to {artist}")).clicked() {
                self.actions.push(Action::Go(Route::Artist { name: artist.clone(), id: tr.artist_ref.clone() }));
            }
        }
        if !matches!(source, Source::Album { .. }) && ui.button("Go to album").clicked() {
            self.actions.push(Action::Go(Route::Album(tr.album_id.clone())));
        }
    }

    /// ↑/↓ select, ⏎ play from the selection, ⌥⏎ play next, ⇧⏎ add.
    fn track_list_keys(&mut self, ui: &Ui, tracks: &[Track], source: &Source) {
        if self.focus != Focus::Main || self.palette.is_some() || ui.ctx().memory(|m| m.focused().is_some()) || tracks.is_empty() {
            return;
        }
        use egui::{Key, Modifiers};
        let (up, down, enter, next, add) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::ALT, Key::Enter),
                i.consume_key(Modifiers::SHIFT, Key::Enter),
            )
        });
        let last = tracks.len() - 1;
        if down {
            self.track_sel = Some(self.track_sel.map_or(0, |s| (s + 1).min(last)));
        }
        if up {
            self.track_sel = Some(self.track_sel.map_or(0, |s| s.saturating_sub(1)));
        }
        let Some(sel) = self.track_sel.filter(|s| *s <= last) else { return };
        if enter {
            self.actions.push(Action::Tracks { tracks: tracks.to_vec(), source: source.clone(), action: QueueAction::Play { start: sel } });
        }
        if next {
            self.actions.push(Action::Tracks { tracks: vec![tracks[sel].clone()], source: source.clone(), action: QueueAction::PlayNext });
        }
        if add {
            self.actions.push(Action::Tracks { tracks: vec![tracks[sel].clone()], source: source.clone(), action: QueueAction::Add });
        }
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
}
