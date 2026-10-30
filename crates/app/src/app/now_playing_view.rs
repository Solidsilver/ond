//! The Player window mode: the playing track large (artwork, links, a big
//! seek bar and the transport) with the queue underneath, and the lyrics in a
//! drawer that widens the window to the right, so the player never gets
//! squeezed. Synced lyrics follow the music and seek when clicked. The same
//! lyrics view, smaller, is the full window's sidebar "Lyrics" tab. A quiet
//! details strip folds out under the transport.

use super::WindowAction;
use super::seek::SeekStyle;
use super::view_ctx::ViewCtx;
use super::widgets::{elide, heart_button, link, link_line};
use super::{Action, OndApp};
use crate::favorites::StarKind;
use crate::icons::{self, Icon};
use crate::lyrics::{self, Fetch, Lyrics};
use crate::nav::Route;
use crate::theme::{Tokens, semibold};
use crate::visuals::over_art::OnArt;
use crate::window::WindowMode;
use audio::PlayState;
use egui::{Align, Color32, FontId, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use library::Track;

/// Height of everything under the artwork in the player column.
const BELOW_ART_H: f32 = 272.0;
/// The compact player block (no room for the big artwork): the artwork's
/// side beside the title, and the gap between them.
const COMPACT_ART: f32 = 88.0;
const COMPACT_ART_GAP: f32 = 8.0;
/// Room the waveform seek bar adds, when that's on.
const WAVE_EXTRA: f32 = 16.0;
/// Room the open details strip takes.
const DETAILS_H: f32 = 110.0;
/// The queue keeps at least this much of the Player window.
const QUEUE_MIN_H: f32 = 250.0;
/// Lyric text size in the Player's drawer and the full window's sidebar.
const DRAWER_LYRIC: f32 = 17.0;
const SIDEBAR_LYRIC: f32 = 15.5;
/// The Player's top bar (its title bar): the traffic lights centre in it.
pub const PLAYER_BAR_H: f32 = 40.0;

impl ViewCtx<'_> {
    /// The Player's top bar, which is also its title bar: the traffic lights
    /// on the left (macOS), Full window and Mini on the right (and on Linux
    /// the window's own buttons). Its empty part moves the window; a
    /// double-click zooms it. `on_art`: how to draw the buttons over the
    /// blurred backdrop, when that's behind the bar.
    pub(super) fn player_bar(&mut self, ui: &mut Ui, on_art: Option<OnArt>) {
        let t = Tokens::of(ui);
        let rect = ui.max_rect();
        let bg = ui.interact(rect, ui.scope_id().with("player-titlebar"), Sense::click_and_drag());
        if bg.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if bg.double_clicked() {
            crate::chrome::title_double_click(ui.ctx());
        }
        crate::csd::set_bar(ui.ctx(), rect, on_art);
        let fg = on_art.map_or(t.muted, |o| o.fg);
        let mut right = rect.shrink2(vec2(10.0, 0.0));
        // Our own window buttons (Linux) take the very end.
        right.max.x -= crate::csd::reserved(ui.ctx(), crate::csd::Side::Right);
        ui.scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if let Some(o) = on_art {
                o.apply(ui);
            }
            for (icon, label, mode) in [(Icon::Expand, "Full window (⇧⌘N)", WindowMode::Full), (Icon::MiniPlayer, "Mini player (⇧⌘M)", WindowMode::Mini)]
            {
                let r = ui.available_rect_before_wrap();
                let slot = Rect::from_min_max(pos2(r.right() - 28.0, r.center().y - 14.0), pos2(r.right(), r.center().y + 14.0));
                if let Some(scrim) = on_art.and_then(|o| o.scrim) {
                    ui.painter().circle_filled(slot.center(), 13.0, scrim);
                }
                if icons::sized_button(ui, icon, label, vec2(28.0, 28.0), 12.0, Some(fg)).clicked() {
                    self.actions.push(Action::Window(WindowAction::SetMode(mode)));
                }
            }
        });
    }

    /// Asks the server for `track`'s lyrics the first time a lyrics view
    /// needs them.
    pub(super) fn ensure_lyrics(&mut self, track: &Track) {
        if !self.lyrics.contains_key(&track.id) && self.session.client.is_some() {
            self.actions.push(Action::NeedLyrics(Box::new(track.clone())));
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
        // Borrowed from the player and the lyrics, not from `self`, so drawing
        // can take `&mut self`.
        let (player, fetched) = (self.player, self.lyrics);
        let Some(track) = player.queue.current.as_ref().map(|c| &c.track) else {
            note(ui, "Nothing playing.");
            return;
        };
        self.ensure_lyrics(track);
        match fetched.get(&track.id) {
            Some(Fetch::Done(Some(l))) => self.lyrics_block(ui, track, l, size, t.panel),
            Some(Fetch::Loading) => note(ui, "Looking for lyrics…"),
            _ if self.session.client.is_none() => note(ui, "Lyrics load when the server is connected."),
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
    pub(super) fn player_block(&mut self, ui: &mut Ui, track: &Track, side: Option<f32>, window: bool) {
        let width = ui.available_width();
        let data = self.data;
        let album = data.album(&track.album_id);
        let cover = data.cover_for(track);
        let art = |this: &mut Self, ui: &mut Ui, side: f32| {
            let (art, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
            let dark = ui.visuals().dark_mode;
            let shadow = egui::Shadow {
                offset: [0, (side / 40.0) as i8],
                blur: (side / 14.0).min(28.0) as u8,
                spread: 0,
                color: Color32::from_black_alpha(if dark { 110 } else { 46 }),
            };
            ui.painter().add(shadow.as_shape(art, 8));
            this.art.paint(ui, art, cover, if side < 120.0 { 6.0 } else { 8.0 });
            // The live spectrum along the foot of the artwork (experimental).
            if side >= 120.0
                && this.ui.prefs.experimental.spectrum
                && let Some(f) = this.listen(ui)
            {
                let inset = (side * 0.05).round();
                let area = Rect::from_min_max(pos2(art.left() + inset, art.bottom() - side * 0.2), pos2(art.right() - inset, art.bottom() - inset * 0.6));
                let n = ((area.width() / 9.0) as usize).clamp(8, crate::visuals::spectrum::BANDS);
                // Light bars over a dark foot of the cover, dark over a light one;
                // see-through either way (a meter, not a control).
                let under = cover.and_then(|c| this.art.look(c)).and_then(|l| l.backdrop.clone());
                let uv = |b: &crate::visuals::backdrop::Blurred| {
                    crate::visuals::over_art::region(&b.image, crate::visuals::over_art::uv_of(area, art, crate::art::crop_uv(1.0)))
                };
                let on = under.map_or(OnArt::plain(Color32::WHITE), |b| OnArt::pick(uv(&b), Color32::WHITE, crate::visuals::over_art::GRAPHIC));
                crate::visuals::paint_bars(ui.painter(), area, &crate::visuals::spectrum::grouped(&f.bands, n), on.fg.gamma_multiply(0.47));
            }
        };

        let compact = side.is_none();
        if let Some(side) = side {
            ui.vertical_centered(|ui| art(self, ui, side));
            ui.add_space(20.0);
        }
        ui.horizontal(|ui| {
            if compact {
                art(self, ui, COMPACT_ART);
                ui.add_space(COMPACT_ART_GAP);
            }
            ui.vertical(|ui| {
                ui.add_space(if compact { 18.0 } else { 0.0 });
                self.title_links(ui, track, album, if compact { width - (COMPACT_ART + COMPACT_ART_GAP) } else { width });
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
    pub(super) fn title_links(&mut self, ui: &mut Ui, track: &Track, album: Option<&library::Album>, width: f32) {
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
        // One line, so a long name can't push everything below it down: when
        // both don't fit, the album gives way first, the artist keeps at least half.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let font = FontId::proportional(14.0);
            let natural = |ui: &Ui, text: &str| ui.painter().layout_no_wrap(text.to_owned(), font.clone(), t.text).size().x;
            let sep = "  ·  ";
            let sep_w = if track.artist.is_some() && album.is_some() { natural(ui, sep) } else { 0.0 };
            let artist_w = track.artist.as_deref().map_or(0.0, |a| natural(ui, a));
            let album_w = album.map_or(0.0, |al| natural(ui, &al.name));
            let room = width - sep_w;
            let artist_max = if artist_w + album_w <= room { artist_w } else { artist_w.min((room - album_w).max(room * 0.5)) };
            if let Some(artist) = &track.artist
                && link_line(ui, artist, font.clone(), t.accent, artist_max).clicked()
                && let Some(route) = Route::artist_of(track)
            {
                self.actions.push(Action::Go(route));
            }
            if let Some(al) = album {
                if track.artist.is_some() {
                    ui.label(egui::RichText::new(sep).font(font.clone()).color(t.muted));
                }
                let left = (width - (ui.cursor().min.x - ui.min_rect().min.x)).max(24.0);
                if link_line(ui, &al.name, font, t.muted, left).clicked() {
                    self.ui.reveal = Some(track.id.clone());
                    self.actions.push(Action::Go(Route::Album(al.id.clone())));
                }
            }
        });
    }

    /// A thick seek bar: drag or click to seek, the downloaded part shaded,
    /// elapsed and remaining underneath.
    pub(super) fn big_seek(&mut self, ui: &mut Ui, width: f32) {
        let t = Tokens::of(ui);
        // The waveform needs the room; the layout keeps it while that's on,
        // so nothing moves when the waveform arrives.
        let wave = self.ui.prefs.waveform;
        let (rect, _) = ui.allocate_exact_size(vec2(width, if wave { 16.0 + WAVE_EXTRA } else { 16.0 }), Sense::hover());
        let bar = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
        let style = SeekStyle {
            track: t.hairline,
            fetched: t.selected,
            fill: t.text,
            pulse: t.accent,
            pulse_len: 48.0,
            radius: 2.0,
            grow: 0.0,
            knob: t.text,
            knob_r: 6.0,
            wave_h: if wave { 13.0 } else { 0.0 },
        };
        let (shown, _) = self.seek_line(ui, bar, rect, "player-seek", &style);
        let (elapsed, remaining) = self.seek_times(shown);
        let small = FontId::proportional(11.5);
        ui.horizontal(|ui| {
            // Line the times up with the bar's ends.
            ui.set_max_width(width);
            ui.label(egui::RichText::new(elapsed).font(small.clone()).color(t.muted));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.label(egui::RichText::new(remaining).font(small).color(t.muted));
            });
        });
    }

    /// Previous · a large round play/pause · next, centered.
    pub(super) fn big_transport(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let state = self.engine.status.state();
        let waiting = matches!(state, PlayState::Buffering | PlayState::Loading) || self.player.pending_seek.is_some() || self.player.held;
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
            let glyph = if matches!(icon, Icon::Play) {
                Rect::from_center_size(r.center() + vec2(1.5, 0.0), vec2(20.0, 20.0))
            } else {
                Rect::from_center_size(r.center(), vec2(20.0, 20.0))
            };
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
    pub(super) fn details_strip(&mut self, ui: &mut Ui, track: &Track, lyrics_toggle: bool) {
        let t = Tokens::of(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let resp = link(ui, "Details", FontId::proportional(12.0), t.muted);
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 12.0), Sense::hover());
            icons::paint(ui.painter(), r.shrink(1.5), if self.ui.np_details { Icon::ChevronDown } else { Icon::ChevronRight }, t.muted);
            if resp.clicked() {
                self.ui.np_details = !self.ui.np_details;
            }
            if lyrics_toggle {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let open = self.ui.prefs.lyrics_open;
                    let color = if open { t.accent } else { t.muted };
                    let label = if open { "Hide lyrics (⌥⌘L)" } else { "Show lyrics (⌥⌘L)" };
                    let b = icons::sized_button(ui, Icon::Lyrics, label, vec2(28.0, 24.0), 14.0, Some(color));
                    if open {
                        ui.painter().rect_filled(b.rect, 6.0, t.accent.gamma_multiply(0.12));
                    }
                    if b.clicked() {
                        self.actions.push(Action::Window(WindowAction::SetLyricsDrawer(!open)));
                    }
                });
            }
        });
        if !self.ui.np_details {
            return;
        }
        ui.add_space(8.0);
        let rows = details_rows(track, self.output.signal().map(|s| s.summary()), self.analysis_of(track));
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (k, v) in rows {
                super::widgets::kv_text(ui, k, &v);
            }
        });
    }

    /// The lyrics column, as stanzas under small section labels. Synced lines
    /// follow playback (the sung line bright and a touch larger, kept about a
    /// third of the way down) and seek when clicked; scrolling by hand pauses
    /// following for a few seconds or until "Follow". Plain lyrics are calm
    /// text with a quiet "Not synced" note.
    pub(super) fn lyrics_block(&mut self, ui: &mut Ui, track: &Track, l: &Lyrics, size: f32, bg: Color32) {
        let t = Tokens::of(ui);
        let rect = ui.available_rect_before_wrap();
        let now = ui.input(|i| i.time);
        let pos_ms = (self.position_secs() * 1000.0) as i64;
        let width = (rect.width() - 12.0).min(size * 30.0);
        // Hand-scrolling over the lyrics pauses following.
        let wheel = ui.input(|i| i.smooth_scroll_delta.y != 0.0);
        if wheel && ui.rect_contains_pointer(rect) {
            self.ui.lyrics_follow.scrolled_by_hand(now);
        }
        let following = self.ui.lyrics_follow.following(now);
        let id = egui::Id::unique("np-lyrics").with(&track.id);

        match l {
            Lyrics::Synced(lines) => {
                let rows = self.ui.memos.lyrics.get(lyrics_key(track, lines.as_ptr() as usize, lines.len(), true), || lyrics::rows_synced(lines));
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
                    for row in rows.iter() {
                        match row {
                            lyrics::Row::Section(s) => section_label(ui, s, size, !first),
                            lyrics::Row::Break => ui.add_space((size * 0.8).round()),
                            lyrics::Row::Text { text, line: Some(i) } => {
                                let i = *i;
                                let sung = Some(i) == cur;
                                // A pause is a gap, or a quiet "· · ·" while it's the sung line.
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
                                let shown = if text.is_empty() { "·  ·  ·" } else { text.as_str() };
                                let galley = lyric_galley(ui, shown, FontId::new(px, semibold()), color, width);
                                let (r, resp) = ui.allocate_exact_size(vec2(width, galley.size().y + 6.0), Sense::click());
                                if resp.hovered() && !sung {
                                    ui.painter().rect_filled(r.expand2(vec2(6.0, 0.0)), 6.0, t.hover);
                                }
                                ui.painter().galley(r.min + vec2(0.0, 3.0), galley, color);
                                resp.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::Role::Button,
                                        true,
                                        format!("{text}, at {}", crate::fmt::clock(lines[i].start_ms as f64 / 1000.0)),
                                    )
                                });
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
                    self.ui.lyrics_follow.resume();
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
                if let Some(s) = self.ui.lyrics_follow.resumes_in(now) {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(s));
                }
            }
            Lyrics::Plain(lines) => {
                let rows = self.ui.memos.lyrics.get(lyrics_key(track, lines.as_ptr() as usize, lines.len(), false), || lyrics::rows_plain(lines));
                let font = FontId::proportional(size);
                egui::ScrollArea::vertical().id_salt(id).auto_shrink(false).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Not synced").size(11.0).color(t.muted.gamma_multiply(0.8)))
                        .on_hover_text("These lyrics have no timing, so they can't follow the music");
                    ui.add_space((size * 1.0).round());
                    let mut first = true;
                    for row in rows.iter() {
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
    pub(super) fn follow_button(&mut self, ui: &mut Ui, area: Rect, id: egui::Id) {
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
            self.ui.lyrics_follow.resume();
            ui.data_mut(|d| d.remove::<(Option<usize>, u64)>(id));
        }
    }
}

/// The lyrics rows' memo key: the track, the lines' address and count (a
/// fetch replaces them), and whether they're synced.
pub(super) type LyricsKey = (u64, usize, usize, bool);

fn lyrics_key(track: &Track, lines: usize, len: usize, synced: bool) -> LyricsKey {
    (super::view_ctx::hash_str(&track.id), lines, len, synced)
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

/// How strongly the backdrop is washed out under the Player's text: enough
/// for `text` and `muted` to read over every part of every layer.
fn backdrop_wash(layers: &[crate::visuals::backdrop::Layer], wash: Color32, text: Color32, muted: Color32) -> f32 {
    layers.iter().map(|l| crate::visuals::backdrop::wash_alpha(l.blurred.extremes, wash, text, muted)).fold(0.0, f32::max)
}

/// The wash around the artwork, where only the window buttons sit (they
/// pick their own colors for what's under them; see `over_art`).
fn light_wash(alpha: f32) -> f32 {
    alpha.min(0.4)
}

/// The blurred cover `layers` (oldest first) filling `rect`, under a wash of
/// `wash`: light around the artwork (down to `art_bottom`), then by
/// `text_top` at `alpha`, the strength the text needs.
fn backdrop_shape(rect: Rect, layers: &[crate::visuals::backdrop::Layer], wash: Color32, alpha: f32, art_bottom: f32, text_top: f32) -> egui::Shape {
    let uv = crate::art::crop_uv(rect.width() / rect.height().max(1.0));
    let mut shapes = Vec::new();
    for l in layers {
        shapes.push(egui::epaint::RectShape::filled(rect, 0.0, Color32::WHITE.gamma_multiply(l.opacity)).with_texture(l.blurred.texture.id(), uv).into());
    }
    let light = light_wash(alpha);
    let mut mesh = egui::Mesh::default();
    for (y, a) in [(rect.top(), light), (art_bottom.min(text_top), light), (text_top, alpha), (rect.bottom(), alpha)] {
        let c = wash.gamma_multiply(a);
        mesh.colored_vertex(pos2(rect.left(), y), c);
        mesh.colored_vertex(pos2(rect.right(), y), c);
    }
    for k in 0..3 {
        let i = 2 * k;
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 3, i + 2);
    }
    shapes.push(egui::Shape::mesh(mesh));
    egui::Shape::Vec(shapes)
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
    let format =
        egui::TextFormat { font_id: FontId::new((size * 0.66).round().max(10.0), semibold()), color: t.muted, extra_letter_spacing: 0.9, ..Default::default() };
    let galley = ui.painter().layout_job(egui::text::LayoutJob::single_section(label.to_uppercase(), format));
    let (r, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(r.min, galley, t.muted);
    ui.add_space((size * 0.45).round());
}

/// The details strip's rows: only the ones there is something to say about.
/// The Info tab's file facts minus what the Player already shows (length on
/// the seek bar; channels), then the analysis in one line.
fn details_rows(track: &Track, signal: Option<String>, analysis: Option<&analysis::TrackAnalysis>) -> Vec<(&'static str, String)> {
    let mut rows: Vec<_> = crate::fmt::track_facts(track, signal).into_iter().filter(|(k, _)| !matches!(*k, "Length" | "Channels")).collect();
    if let Some(line) = analysis.and_then(crate::fmt::loudness_line) {
        rows.push(("Loudness", line));
    }
    rows
}

impl ViewCtx<'_> {
    /// The Player's artwork, title, seek bar and controls for what's playing,
    /// or an empty sleeve.
    fn player_main(&mut self, ui: &mut Ui, side: f32) {
        let player = self.player;
        match player.queue.current.as_ref().map(|c| &c.track) {
            Some(track) => {
                self.ensure_lyrics(track);
                self.player_block(ui, track, Some(side), true);
            }
            None => {
                let t = Tokens::of(ui);
                ui.vertical_centered(|ui| {
                    let (art, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
                    self.art.paint(ui, art, None, 8.0);
                    ui.add_space(16.0);
                    ui.label(egui::RichText::new("Not playing").size(14.0).color(t.muted));
                });
            }
        }
    }
}

impl OndApp {
    pub(super) fn player_ui(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        // The background, tinted from the album art when that's on, or the
        // blurred cover under a wash of it.
        let mut stage = crate::theme::tint_of(ui).map_or(t.bg, |x| x.stage);
        let layers = self.backdrop_layers(ui.ctx());
        let window = ui.max_rect();
        // Drawn under everything, once the artwork's place is known.
        let under = ui.painter().clone();
        let backdrop = (!layers.is_empty()).then(|| under.add(egui::Shape::Noop));
        let wash = stage;
        let alpha = backdrop_wash(&layers, wash, t.text, t.muted);
        // The window buttons over the backdrop: colors for what's under them.
        let bar = Rect::from_min_size(window.min, vec2(window.width(), PLAYER_BAR_H));
        let crop = crate::art::crop_uv(window.width() / window.height().max(1.0));
        let bar_art = crate::visuals::backdrop::seen(&layers, crate::visuals::over_art::uv_of(bar, window, crop), wash, light_wash(alpha))
            .map(|under| OnArt::pick(under, t.muted, crate::visuals::over_art::ICON));
        if backdrop.is_some() {
            stage = Color32::TRANSPARENT;
        }
        egui::Panel::top("player-bar")
            .exact_size(PLAYER_BAR_H)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(stage))
            .show(ui, |ui| self.view().player_bar(ui, bar_art));
        self.player_status_bar(ui);
        if self.ui.prefs.lyrics_open {
            egui::Panel::right("lyrics-drawer")
                .exact_size(crate::window::DRAWER_W)
                .resizable(false)
                .frame(egui::Frame::new().fill(t.panel).inner_margin(egui::Margin { left: 20, right: 14, top: 14, bottom: 10 }))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Lyrics").font(FontId::new(12.0, semibold())).color(t.muted));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icons::sized_button(ui, Icon::Close, "Close lyrics (⌥⌘L)", vec2(22.0, 22.0), 9.0, Some(t.muted)).clicked() {
                                self.actions.push(Action::Window(WindowAction::SetLyricsDrawer(false)));
                            }
                        });
                    });
                    ui.add_space(6.0);
                    self.view().lyrics_pane(ui, DRAWER_LYRIC);
                });
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(stage).inner_margin(egui::Margin { left: 20, right: 20, top: 4, bottom: 4 })).show(
            ui,
            |ui| {
                let full = ui.available_rect_before_wrap();
                let wave = if self.ui.prefs.waveform { WAVE_EXTRA } else { 0.0 };
                let below = BELOW_ART_H + wave + if self.ui.np_details { DETAILS_H } else { 0.0 } + QUEUE_MIN_H;

                let side = full.width().min(full.height() - below).max(120.0).floor();
                if let Some(slot) = backdrop {
                    let art_bottom = full.top() + if self.player.queue.current.is_some() { side } else { 0.0 };
                    under.set(slot, backdrop_shape(window, &layers, wash, alpha, art_bottom - side * 0.2, art_bottom + 18.0));
                }

                self.view().player_main(ui, side);
                ui.add_space(8.0);
                let top = ui.cursor().top();
                ui.painter().hline(full.x_range(), top, egui::Stroke::new(1.0, t.hairline));
                let rest = Rect::from_min_max(pos2(full.left() - 8.0, top + 10.0), pos2(full.right() + 8.0, full.bottom()));
                ui.scope_builder(UiBuilder::new().max_rect(rest).layout(Layout::top_down(Align::Min)), |ui| self.view().queue_panel(ui, false));
            },
        );
    }
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
        assert_eq!(rows[0], ("Format", "FLAC 24/96".into()));
        assert_eq!(rows[1], ("ReplayGain", "track −6.3 dB".into()));
        assert_eq!(rows[2], ("Playback", "24/96 · bit-perfect".into()));
    }
}
