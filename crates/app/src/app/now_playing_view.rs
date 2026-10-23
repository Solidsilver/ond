//! The Player window mode: the playing track large (artwork, links, a big
//! seek bar and the transport) with the queue underneath, and the lyrics in a
//! drawer that widens the window to the right, so the player never gets
//! squeezed. Synced lyrics follow the music and seek when clicked. The same
//! lyrics view, smaller, is the full window's sidebar "Lyrics" tab. A quiet
//! details strip folds out under the transport.

use super::compact::{on_art, scrim};
use super::widgets::{elide, heart_button, link};
use super::{Action, ListenerApp, mmss};
use crate::favorites::StarKind;
use crate::icons::{self, Icon};
use crate::lyrics::{self, Fetch, Lyrics};
use crate::nav::Route;
use crate::theme::{Tokens, semibold};
use crate::window::WindowMode;
use audio::PlayState;
use egui::{Align, Color32, FontId, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use library::Track;

/// Height of everything under the artwork in the player column.
const BELOW_ART_H: f32 = 272.0;
/// Room the open details strip takes.
const DETAILS_H: f32 = 110.0;
/// The queue keeps at least this much of the Player window.
const QUEUE_MIN_H: f32 = 250.0;
/// Lyric text size in the Player's drawer and the full window's sidebar.
const DRAWER_LYRIC: f32 = 17.0;
const SIDEBAR_LYRIC: f32 = 15.5;

impl ListenerApp {
    pub(super) fn player_ui(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        self.player_status_bar(ui);
        if self.windows.lyrics_open {
            egui::Panel::right("lyrics-drawer")
                .exact_size(crate::window::DRAWER_W)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.panel).inner_margin(egui::Margin { left: 20, right: 14, top: 14, bottom: 10 }))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Lyrics").font(FontId::new(12.0, semibold())).color(t.muted));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icons::sized_button(ui, Icon::Close, "Close lyrics (⌥⌘L)", vec2(22.0, 22.0), 9.0, Some(t.muted)).clicked() {
                                self.actions.push(Action::SetLyricsDrawer(false));
                            }
                        });
                    });
                    ui.add_space(6.0);
                    self.lyrics_pane(ui, DRAWER_LYRIC);
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin { left: 20, right: 20, top: 16, bottom: 4 }))
            .show(ui, |ui| {
                let full = ui.available_rect_before_wrap();
                let below = BELOW_ART_H + if self.np_details { DETAILS_H } else { 0.0 } + QUEUE_MIN_H;
                let side = full.width().min(full.height() - below).max(120.0).floor();
                match self.queue.current.as_ref().map(|c| c.track.clone()) {
                    Some(track) => {
                        self.ensure_lyrics(&track);
                        self.player_block(ui, &track, Some(side), true);
                    }
                    None => {
                        ui.vertical_centered(|ui| {
                            let (art, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
                            self.art.paint(ui, art, None, 8.0);
                            self.art_corner_buttons(ui, art);
                            ui.add_space(16.0);
                            ui.label(egui::RichText::new("Not playing").size(14.0).color(t.muted));
                        });
                    }
                }
                ui.add_space(8.0);
                let top = ui.cursor().top();
                ui.painter().hline(full.x_range(), top, egui::Stroke::new(1.0, t.hairline));
                let rest = Rect::from_min_max(pos2(full.left() - 8.0, top + 10.0), pos2(full.right() + 8.0, full.bottom()));
                ui.scope_builder(UiBuilder::new().max_rect(rest).layout(Layout::top_down(Align::Min)), |ui| self.queue_panel(ui, false));
            });
    }

    /// Full window and Mini, over the Player's artwork while it's hovered.
    fn art_corner_buttons(&mut self, ui: &mut Ui, art: Rect) {
        if !ui.rect_contains_pointer(art) {
            return;
        }
        let mut corner = ui.new_child(UiBuilder::new().max_rect(art.shrink(8.0)).layout(Layout::right_to_left(Align::Min)));
        on_art(&mut corner);
        corner.spacing_mut().item_spacing.x = 4.0;
        for (icon, label, mode) in [(Icon::Expand, "Full window (⇧⌘N)", WindowMode::Full), (Icon::MiniPlayer, "Mini player (⌥⌘3)", WindowMode::Mini)] {
            let (r, _) = corner.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
            corner.painter().circle_filled(r.center(), 13.0, scrim());
            let mut b = corner.new_child(UiBuilder::new().max_rect(r));
            if icons::sized_button(&mut b, icon, label, r.size(), 10.0, Some(Color32::from_rgb(0xE8, 0xE9, 0xEC))).clicked() {
                self.actions.push(Action::SetMode(mode));
            }
        }
    }

    /// Asks the server for `track`'s lyrics the first time something shows them.
    fn ensure_lyrics(&mut self, track: &Track) {
        if !self.lyrics.contains_key(&track.id) && self.client.is_some() {
            self.lyrics.insert(track.id.clone(), Fetch::Loading);
            self.bridge.fetch_lyrics(self.client.clone(), track.id.clone(), track.artist.clone(), track.title.clone());
        }
    }

    /// The playing track's lyrics at `size`, filling the rest of `ui`: synced
    /// lines following playback, plain text, or a quiet line saying why not.
    pub(super) fn lyrics_pane(&mut self, ui: &mut Ui, size: f32) {
        let t = Tokens::of(ui);
        let note = |ui: &mut Ui, text: &str| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(text).size(12.5).color(t.muted));
        };
        let Some(track) = self.queue.current.as_ref().map(|c| c.track.clone()) else {
            note(ui, "Nothing playing.");
            return;
        };
        self.ensure_lyrics(&track);
        match self.lyrics.get(&track.id) {
            Some(Fetch::Done(Some(l))) => {
                let l = l.clone();
                let bg = t.panel;
                self.lyrics_block(ui, &track, &l, size, bg);
            }
            Some(Fetch::Loading) => note(ui, "Looking for lyrics…"),
            _ if self.client.is_none() => note(ui, "Lyrics come from your server when it's connected."),
            _ => note(ui, "No lyrics for this song."),
        }
    }

    /// The sidebar's Lyrics tab.
    pub(super) fn sidebar_lyrics(&mut self, ui: &mut Ui) {
        self.lyrics_pane(ui, SIDEBAR_LYRIC);
    }

    /// Artwork (`side` px square, or a small one beside the title when `None`),
    /// title and links, the seek bar, transport, and the details strip. In the
    /// Player window (`window`) the artwork carries Full / Mini buttons and the
    /// details row a lyrics toggle.
    fn player_block(&mut self, ui: &mut Ui, track: &Track, side: Option<f32>, window: bool) {
        let width = ui.available_width();
        let album = self.data.album(&track.album_id).cloned();
        let cover = track.cover_art.clone().or_else(|| album.as_ref().and_then(|a| a.cover_art.clone()));
        let art = |this: &mut Self, ui: &mut Ui, side: f32| {
            let (art, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
            let dark = ui.visuals().dark_mode;
            let shadow = egui::Shadow { offset: [0, (side / 40.0) as i8], blur: (side / 14.0).min(28.0) as u8, spread: 0, color: Color32::from_black_alpha(if dark { 110 } else { 46 }) };
            ui.painter().add(shadow.as_shape(art, 8));
            this.art.paint(ui, art, cover.as_deref(), if side < 120.0 { 6.0 } else { 8.0 });
            if window && side >= 120.0 {
                this.art_corner_buttons(ui, art);
            }
        };

        let compact = side.is_none();
        if let Some(side) = side {
            ui.vertical_centered(|ui| art(self, ui, side));
            ui.add_space(20.0);
        }
        ui.horizontal(|ui| {
            if compact {
                art(self, ui, 88.0);
                ui.add_space(8.0);
            }
            ui.vertical(|ui| {
                ui.add_space(if compact { 18.0 } else { 0.0 });
                self.title_links(ui, track, album.as_ref(), if compact { width - 96.0 } else { width });
            });
        });
        ui.add_space(if compact { 12.0 } else { 18.0 });
        self.big_seek(ui, width);
        ui.add_space(if compact { 4.0 } else { 10.0 });
        self.big_transport(ui);
        if !compact {
            ui.add_space(14.0);
            self.details_strip(ui, track, window);
        }
    }

    /// The title with the heart at its right; "artist — album" as links.
    fn title_links(&mut self, ui: &mut Ui, track: &Track, album: Option<&library::Album>, width: f32) {
        let t = Tokens::of(ui);
        let loved = self.favorites.is_on(StarKind::Song, &track.id);
        ui.horizontal(|ui| {
            let title = elide(ui, &track.title, FontId::new(20.0, semibold()), t.text, width - 36.0);
            let (r, _) = ui.allocate_exact_size(vec2(width - 32.0, title.size().y), Sense::hover());
            ui.painter().galley(r.min, title, t.text);
            if heart_button(ui, loved, 26.0, 13.0).clicked() {
                self.actions.push(Action::ToggleStar { kind: StarKind::Song, id: track.id.clone(), track: Some(Box::new(track.clone())), name: None });
            }
        });
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let font = FontId::proportional(14.0);
            if let Some(artist) = &track.artist
                && link(ui, artist, font.clone(), t.accent).clicked()
            {
                let id = track.artists.first().and_then(|a| a.id.clone()).or_else(|| track.artist_ref.clone());
                self.actions.push(Action::Go(Route::Artist { name: track.artists.first().map_or_else(|| artist.clone(), |a| a.name.clone()), id }));
            }
            if let Some(al) = album {
                if track.artist.is_some() {
                    ui.label(egui::RichText::new("  —  ").font(font.clone()).color(t.muted));
                }
                if link(ui, &al.name, font, t.muted).clicked() {
                    self.reveal = Some(track.id.clone());
                    self.actions.push(Action::Go(Route::Album(al.id.clone())));
                }
            }
        });
    }

    /// A thick seek bar: drag or click to seek, the downloaded part shaded,
    /// elapsed and remaining underneath.
    fn big_seek(&mut self, ui: &mut Ui, width: f32) {
        let t = Tokens::of(ui);
        let state = self.engine.status.state();
        let dur = self.duration_secs().filter(|d| *d > 0.0);
        let seekable = dur.is_some() && matches!(state, PlayState::Playing | PlayState::Paused | PlayState::Buffering);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 16.0), if seekable { Sense::click_and_drag() } else { Sense::hover() });
        let bar = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
        let frac_at = |x: f32| f64::from(((x - bar.left()) / bar.width()).clamp(0.0, 1.0));
        if let Some(d) = dur {
            if (resp.dragged() || resp.drag_started())
                && let Some(p) = resp.interact_pointer_pos()
            {
                self.seek_preview = Some(frac_at(p.x) * d);
            }
            if resp.drag_stopped() || resp.clicked() {
                if let Some(to) = resp.interact_pointer_pos().map(|p| frac_at(p.x) * d).or(self.seek_preview) {
                    self.actions.push(Action::Seek(to));
                }
                self.seek_preview = None;
            }
        }
        let painter = ui.painter();
        painter.rect_filled(bar, 2.0, t.hairline);
        if let Some(f) = self.engine.status.fetched_fraction() {
            painter.rect_filled(Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * f.clamp(0.0, 1.0), bar.bottom())), 2.0, t.selected);
        }
        let shown = self.seek_preview.unwrap_or_else(|| self.position_secs());
        let x = bar.left() + bar.width() * dur.map_or(0.0, |d| (shown / d).clamp(0.0, 1.0) as f32);
        painter.rect_filled(Rect::from_min_max(bar.min, pos2(x, bar.bottom())), 2.0, t.text);
        if state == PlayState::Buffering || self.pending_seek.is_some() {
            let tm = ui.input(|i| i.time) as f32;
            let alpha = 0.35 + 0.35 * (tm * std::f32::consts::TAU / 1.2).sin();
            painter.rect_filled(Rect::from_min_max(pos2(x, bar.top()), pos2((x + 48.0).min(bar.right()), bar.bottom())), 2.0, t.accent.gamma_multiply(alpha));
        }
        if resp.hovered() || self.seek_preview.is_some() {
            painter.circle_filled(pos2(x, bar.center().y), 6.0, t.text);
        }
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, seekable, "Position"));
        let small = FontId::proportional(11.5);
        ui.horizontal(|ui| {
            // Line the times up with the bar's ends.
            ui.set_max_width(width);
            ui.label(egui::RichText::new(mmss(shown)).font(small.clone()).color(t.muted));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                let left = dur.map(|d| format!("−{}", mmss((d - shown).max(0.0)))).unwrap_or_default();
                ui.label(egui::RichText::new(left).font(small).color(t.muted));
            });
        });
    }

    /// Previous · a large round play/pause · next, centered.
    fn big_transport(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let state = self.engine.status.state();
        let waiting = matches!(state, PlayState::Buffering | PlayState::Loading) || self.pending_seek.is_some();
        let (icon, label) = match state {
            _ if waiting => (Icon::Spinner, "Buffering (click to pause)"),
            PlayState::Playing => (Icon::Pause, "Pause"),
            _ => (Icon::Play, "Play"),
        };
        let row_w = 44.0 + 24.0 + 52.0 + 24.0 + 44.0;
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - row_w) / 2.0).max(0.0));
            ui.spacing_mut().item_spacing.x = 24.0;
            if icons::sized_button(ui, Icon::Previous, "Previous", vec2(44.0, 44.0), 17.0, Some(t.text)).clicked() {
                self.actions.push(Action::Previous);
            }
            let (r, resp) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::click());
            let fill = if resp.hovered() { t.text.gamma_multiply(0.85) } else { t.text };
            ui.painter().circle_filled(r.center(), 26.0, fill);
            let glyph = if matches!(icon, Icon::Play) { Rect::from_center_size(r.center() + vec2(1.5, 0.0), vec2(20.0, 20.0)) } else { Rect::from_center_size(r.center(), vec2(20.0, 20.0)) };
            icons::paint(ui.painter(), glyph, icon, t.bg);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.actions.push(Action::PlayPause);
            }
            if icons::sized_button(ui, Icon::Next, "Next", vec2(44.0, 44.0), 17.0, Some(t.text)).clicked() {
                self.actions.push(Action::Next);
            }
        });
    }

    /// "Details ›": source format, what reaches the device, ReplayGain, and the
    /// loudness analysis when there is one. Folded away by default.
    fn details_strip(&mut self, ui: &mut Ui, track: &Track, lyrics_toggle: bool) {
        let t = Tokens::of(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let resp = link(ui, "Details", FontId::proportional(12.0), t.muted);
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 12.0), Sense::hover());
            icons::paint(ui.painter(), r.shrink(1.5), if self.np_details { Icon::ChevronDown } else { Icon::ChevronRight }, t.muted);
            if resp.clicked() {
                self.np_details = !self.np_details;
            }
            if lyrics_toggle {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let open = self.windows.lyrics_open;
                    let color = if open { t.accent } else { t.muted };
                    let label = if open { "Hide lyrics (⌥⌘L)" } else { "Show lyrics (⌥⌘L)" };
                    let b = icons::sized_button(ui, Icon::Lyrics, label, vec2(28.0, 24.0), 14.0, Some(color));
                    if open {
                        ui.painter().rect_filled(b.rect, 6.0, t.accent.gamma_multiply(0.12));
                    }
                    if b.clicked() {
                        self.actions.push(Action::SetLyricsDrawer(!open));
                    }
                });
            }
        });
        if !self.np_details {
            return;
        }
        ui.add_space(8.0);
        let rows = details_rows(track, self.output.signal().map(|s| s.summary()), self.analysis_of(track));
        egui::Grid::new("np-details").num_columns(2).spacing(vec2(16.0, 6.0)).show(ui, |ui| {
            for (k, v) in rows {
                ui.label(egui::RichText::new(k).size(12.0).color(t.muted));
                ui.add(egui::Label::new(egui::RichText::new(v).size(12.0).color(t.text)).wrap());
                ui.end_row();
            }
        });
    }

    /// The lyrics column, as stanzas under small section labels. Synced lines
    /// follow playback (the sung line bright and a touch larger, kept about a
    /// third of the way down) and seek when clicked; scrolling by hand pauses
    /// following for a few seconds or until "Follow". Plain lyrics are calm
    /// text with a quiet "Not synced" note.
    fn lyrics_block(&mut self, ui: &mut Ui, track: &Track, l: &Lyrics, size: f32, bg: Color32) {
        let t = Tokens::of(ui);
        let rect = ui.available_rect_before_wrap();
        let now = ui.input(|i| i.time);
        let pos_ms = (self.position_secs() * 1000.0) as i64;
        let width = (rect.width() - 12.0).min(size * 30.0);
        // Hand-scrolling over the lyrics pauses following.
        let wheel = ui.input(|i| i.smooth_scroll_delta.y != 0.0);
        if wheel && ui.rect_contains_pointer(rect) {
            self.lyrics_follow.scrolled_by_hand(now);
        }
        let following = self.lyrics_follow.following(now);
        let id = egui::Id::unique("np-lyrics").with(&track.id);

        match l {
            Lyrics::Synced(lines) => {
                let rows = lyrics::rows_synced(lines);
                let cur = lyrics::current(lines, pos_ms);
                // The line last followed, and the pass it was seen on: a stale
                // entry (the view was closed meanwhile) re-places it on reopening.
                let pass = ui.ctx().cumulative_pass_nr();
                let last: Option<(Option<usize>, u64)> = ui.data(|d| d.get_temp(id));
                let changed = last.is_none_or(|(c, seen)| c != cur || seen + 1 < pass);
                let view_h = rect.height();
                let mut seek_to = None;
                egui::ScrollArea::vertical().id_salt(id).auto_shrink(false).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add_space((view_h * 0.3).max(24.0));
                    let mut first = true;
                    for row in &rows {
                        match row {
                            lyrics::Row::Section(s) => section_label(ui, s, size, !first),
                            lyrics::Row::Break => ui.add_space((size * 0.8).round()),
                            lyrics::Row::Text { text, line: Some(i) } => {
                                let i = *i;
                                let sung = Some(i) == cur;
                                // A pause is a gap, or a quiet "• • •" while it's the sung line.
                                if text.is_empty() && !sung {
                                    continue;
                                }
                                let past = cur.is_some_and(|c| i < c);
                                let (px, color) = if sung {
                                    ((size * 1.06).round(), t.text)
                                } else if past {
                                    (size, t.muted.gamma_multiply(0.55))
                                } else {
                                    (size, t.muted)
                                };
                                let shown = if text.is_empty() { "•  •  •" } else { text.as_str() };
                                let galley = lyric_galley(ui, shown, FontId::new(px, semibold()), color, width);
                                let (r, resp) = ui.allocate_exact_size(vec2(width, galley.size().y + 6.0), Sense::click());
                                if resp.hovered() && !sung {
                                    ui.painter().rect_filled(r.expand2(vec2(6.0, 0.0)), 6.0, t.hover);
                                }
                                ui.painter().galley(r.min + vec2(0.0, 3.0), galley, color);
                                let label = format!("{} — {}", mmss(lines[i].start_ms as f64 / 1000.0), text);
                                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
                                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    seek_to = Some(lines[i].start_ms as f64 / 1000.0);
                                }
                                if sung && following && changed {
                                    // The sung line sits about a third of the way down.
                                    let lead = view_h * 0.3;
                                    let target = Rect::from_min_size(pos2(r.left(), r.top() - lead), vec2(r.width(), lead + r.height()));
                                    ui.scroll_to_rect(target, Some(Align::Min));
                                }
                                ui.add_space((size * 0.3).round());
                            }
                            lyrics::Row::Text { line: None, .. } => {}
                        }
                        first = false;
                    }
                    ui.add_space(view_h * 0.6);
                });
                if following {
                    ui.data_mut(|d| d.insert_temp(id, (cur, pass)));
                }
                if let Some(s) = seek_to {
                    self.lyrics_follow.resume();
                    // Re-place the clicked line once playback gets there.
                    ui.data_mut(|d| d.remove::<(Option<usize>, u64)>(id));
                    self.actions.push(Action::Seek(s));
                }
                edge_fades(ui, rect, bg, 32.0, true);
                if !following {
                    self.follow_button(ui, rect, id);
                }
                // Wake for the next line (and to resume following), not every frame.
                if self.engine.status.state() == PlayState::Playing
                    && let Some(ms) = lyrics::until_next(lines, pos_ms)
                {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(ms.clamp(16, 60_000) as u64));
                }
                if let Some(s) = self.lyrics_follow.resumes_in(now) {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(s));
                }
            }
            Lyrics::Plain(lines) => {
                let rows = lyrics::rows_plain(lines);
                let font = FontId::proportional(size);
                egui::ScrollArea::vertical().id_salt(id).auto_shrink(false).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Not synced").size(11.0).color(t.muted.gamma_multiply(0.8)))
                        .on_hover_text("These lyrics have no timing, so they don't follow the music");
                    ui.add_space((size * 1.0).round());
                    let mut first = true;
                    for row in &rows {
                        match row {
                            lyrics::Row::Section(s) => section_label(ui, s, size, !first),
                            lyrics::Row::Break => ui.add_space((size * 0.8).round()),
                            lyrics::Row::Text { text, .. } => {
                                let galley = lyric_galley(ui, text, font.clone(), t.text, width);
                                let (r, _) = ui.allocate_exact_size(vec2(width, galley.size().y), Sense::hover());
                                ui.painter().galley(r.min, galley, t.text);
                            }
                        }
                        first = false;
                    }
                    ui.add_space(32.0);
                });
                edge_fades(ui, rect, bg, 28.0, false);
            }
        }
    }

    /// The pill that brings following back after a manual scroll.
    fn follow_button(&mut self, ui: &mut Ui, area: Rect, id: egui::Id) {
        let t = Tokens::of(ui);
        let galley = ui.painter().layout_no_wrap("Follow lyrics".into(), FontId::proportional(12.0), t.text);
        let size = galley.size() + vec2(22.0, 10.0);
        let r = Rect::from_min_size(pos2(area.right() - size.x - 12.0, area.bottom() - size.y - 12.0), size);
        let resp = ui.interact(r, id.with("follow"), Sense::click());
        let fill = if resp.hovered() { t.hover } else { t.panel };
        ui.painter().rect(r, r.height() / 2.0, fill, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
        ui.painter().galley(r.center() - galley.size() / 2.0, galley, t.text);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Follow lyrics"));
        if resp.clicked() {
            self.lyrics_follow.resume();
            ui.data_mut(|d| d.remove::<(Option<usize>, u64)>(id));
        }
    }
}

/// Soft fades at the top and bottom of a scrolling column, so lines dissolve
/// into the page instead of being cut.
fn edge_fades(ui: &Ui, rect: Rect, bg: Color32, h: f32, top_too: bool) {
    let clear = Color32::TRANSPARENT;
    for (top, a, b) in [(true, bg, clear), (false, clear, bg)] {
        if top && !top_too {
            continue;
        }
        let r = if top { Rect::from_min_size(rect.min, vec2(rect.width(), h)) } else { Rect::from_min_max(pos2(rect.left(), rect.bottom() - h), rect.max) };
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(r.left_top(), a);
        mesh.colored_vertex(r.right_top(), a);
        mesh.colored_vertex(r.left_bottom(), b);
        mesh.colored_vertex(r.right_bottom(), b);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 3, 2);
        ui.painter().add(egui::Shape::mesh(mesh));
    }
}

/// A lyric line laid out at `width` with comfortable leading (1.45×), so
/// wrapped lines read as one line continuing.
fn lyric_galley(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let line_height = Some((font.size * 1.45).round());
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), egui::TextFormat { font_id: font, color, line_height, ..Default::default() });
    job.wrap.max_width = width;
    ui.painter().layout_job(job)
}

/// "VERSE 1": small, spaced capitals above a stanza, with room above it
/// unless it opens the lyrics.
fn section_label(ui: &mut Ui, label: &str, size: f32, space_above: bool) {
    let t = Tokens::of(ui);
    if space_above {
        ui.add_space((size * 1.1).round());
    }
    let format = egui::TextFormat {
        font_id: FontId::new((size * 0.66).round().max(10.0), semibold()),
        color: t.muted,
        extra_letter_spacing: 0.9,
        ..Default::default()
    };
    let galley = ui.painter().layout_job(egui::text::LayoutJob::single_section(label.to_uppercase(), format));
    let (r, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(r.min, galley, t.muted);
    ui.add_space((size * 0.45).round());
}

/// The details strip's rows: only the ones there is something to say about.
fn details_rows(track: &Track, signal: Option<String>, analysis: Option<&analysis::TrackAnalysis>) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    let mut source = Vec::new();
    if let Some(s) = &track.suffix {
        source.push(s.to_uppercase());
    }
    if let Some(rate) = track.sample_rate {
        source.push(audio::describe(track.bit_depth, rate));
    }
    if let Some(k) = track.bit_rate_kbps {
        source.push(format!("{k} kbps"));
    }
    if let Some(b) = track.size {
        source.push(format!("{:.1} MB", b as f64 / 1_000_000.0));
    }
    if !source.is_empty() {
        rows.push(("File", source.join(" · ")));
    }
    if let Some(s) = signal {
        rows.push(("Playback", s));
    }
    let gain = |g: Option<f32>| g.map(|g| format!("{g:+.1} dB").replace('-', "−"));
    match (gain(track.track_gain), gain(track.album_gain)) {
        (Some(tg), Some(ag)) => rows.push(("ReplayGain", format!("track {tg} · album {ag}"))),
        (Some(tg), None) => rows.push(("ReplayGain", format!("track {tg}"))),
        (None, Some(ag)) => rows.push(("ReplayGain", format!("album {ag}"))),
        (None, None) => {}
    }
    if let Some(a) = analysis {
        let mut parts = Vec::new();
        if let Some(dr) = a.dr {
            parts.push(format!("DR{}", dr.round() as i64));
        }
        if let Some(l) = a.integrated_lufs {
            parts.push(format!("{l:.1} LUFS").replace('-', "−"));
        }
        if let Some(p) = a.true_peak_dbtp {
            parts.push(format!("peak {p:.1} dBTP").replace('-', "−"));
        }
        if !parts.is_empty() {
            rows.push(("Loudness", parts.join(" · ")));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_only_say_what_is_known() {
        let mut t = Track {
            id: "t".into(),
            album_id: "a".into(),
            title: "x".into(),
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
        };
        assert!(details_rows(&t, None, None).is_empty());
        t.suffix = Some("flac".into());
        t.bit_depth = Some(24);
        t.sample_rate = Some(96_000);
        t.track_gain = Some(-6.3);
        let rows = details_rows(&t, Some("24/96 · bit-perfect".into()), None);
        assert_eq!(rows[0], ("File", "FLAC · 24/96".into()));
        assert_eq!(rows[1], ("Playback", "24/96 · bit-perfect".into()));
        assert_eq!(rows[2].1, "track −6.3 dB");
    }
}
