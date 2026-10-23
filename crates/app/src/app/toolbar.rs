//! The toolbar: transport on the left, the now-playing display in the middle,
//! output / volume / queue on the right.

use super::widgets::elide;
use super::{Action, ListenerApp, mmss};
use crate::favorites::StarKind;
use crate::icons::{self, Icon};
use crate::nav::Route;
use crate::theme::{Tokens, semibold};
use audio::{Command, PlayState};
use egui::{Align, FontId, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};

const LEFT_W: f32 = 120.0;
const RIGHT_W: f32 = 370.0;
const LCD_MAX_W: f32 = 520.0;
const LCD_H: f32 = 38.0;
/// Below this window width the toolbar tightens up.
const NARROW_W: f32 = 900.0;

impl ListenerApp {
    pub(super) fn toolbar(&mut self, ui: &mut Ui) {
        let full = ui.available_rect_before_wrap();
        let left = Rect::from_min_max(full.min, pos2(full.left() + LEFT_W, full.bottom()));
        // Narrow windows fold the volume slider behind its icon so the
        // now-playing display keeps room for the title.
        let narrow = full.width() < NARROW_W;
        let right_w = if narrow { RIGHT_W - 90.0 } else { RIGHT_W };
        let right = Rect::from_min_max(pos2(full.right() - right_w, full.top()), full.max);
        let middle = Rect::from_min_max(pos2(left.right() + 12.0, full.top()), pos2(right.left() - 12.0, full.bottom()));

        ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)), |ui| self.transport(ui, None));
        let lcd_w = middle.width().clamp(160.0, LCD_MAX_W);
        let lcd = Rect::from_center_size(middle.center(), vec2(lcd_w, LCD_H));
        self.lcd(ui, lcd);
        ui.scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)), |ui| self.toolbar_right(ui, narrow));
    }

    /// Previous · play/pause (a spinner while buffering) · next. `color`
    /// overrides the icons' color (e.g. over artwork).
    pub(super) fn transport(&mut self, ui: &mut Ui, color: Option<egui::Color32>) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.x = 2.0;
        let state = self.engine.status.state();
        if icons::sized_button(ui, Icon::Previous, "Previous", vec2(32.0, 32.0), 13.0, color).clicked() {
            self.actions.push(Action::Previous);
        }
        let waiting = matches!(state, PlayState::Buffering | PlayState::Loading) || self.pending_seek.is_some();
        let (icon, label) = match state {
            _ if waiting => (Icon::Spinner, "Buffering (click to pause)"),
            PlayState::Playing => (Icon::Pause, "Pause"),
            _ => (Icon::Play, "Play"),
        };
        if icons::sized_button(ui, icon, label, vec2(36.0, 32.0), 17.0, Some(color.unwrap_or(t.text))).clicked() {
            self.actions.push(Action::PlayPause);
        }
        if icons::sized_button(ui, Icon::Next, "Next", vec2(32.0, 32.0), 13.0, color).clicked() {
            self.actions.push(Action::Next);
        }
    }

    /// The now-playing display: art, title, "artist — album", and a thin
    /// progress line along the bottom that you can click or drag to seek.
    /// Hovering shows the times; clicking elsewhere opens the album.
    pub(super) fn lcd(&mut self, ui: &mut Ui, rect: Rect) {
        let t = Tokens::of(ui);
        let painter = ui.painter().clone();
        painter.rect(rect, 7.0, t.sidebar, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
        let body = ui.interact(rect, ui.scope_id().with("lcd"), Sense::click());
        let Some(item) = self.queue.current.clone() else {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Not playing", FontId::proportional(12.0), t.muted);
            return;
        };
        let art = Rect::from_min_size(rect.min, vec2(rect.height(), rect.height()));
        let album = self.data.album(&item.track.album_id).cloned();
        let cover = item.track.cover_art.clone().or_else(|| album.as_ref().and_then(|a| a.cover_art.clone()));
        self.art.paint(ui, art.shrink(1.0), cover.as_deref(), 6.0);

        let text_rect = Rect::from_min_max(pos2(art.right() + 8.0, rect.top()), pos2(rect.right() - 8.0, rect.bottom()));
        let title = elide(ui, &item.track.title, FontId::new(12.5, semibold()), t.text, text_rect.width() - 80.0);
        let sub_text = match (&item.track.artist, &album) {
            (Some(a), Some(al)) => format!("{a} — {}", al.name),
            (Some(a), None) => a.clone(),
            (None, Some(al)) => al.name.clone(),
            _ => String::new(),
        };
        let sub = elide(ui, &sub_text, FontId::proportional(11.0), t.muted, text_rect.width() - 80.0);
        let block_h = title.size().y + sub.size().y;
        let y0 = text_rect.center().y - block_h / 2.0 - 1.0;
        painter.galley(pos2(text_rect.center().x - title.size().x / 2.0, y0), title, t.text);
        painter.galley(pos2(text_rect.center().x - sub.size().x / 2.0, y0 + block_h - sub.size().y), sub, t.muted);

        // Progress along the bottom edge of the text area.
        let state = self.engine.status.state();
        let dur = self.duration_secs().filter(|d| *d > 0.0);
        let pos = self.position_secs();
        let bar = Rect::from_min_max(pos2(text_rect.left() - 8.0, rect.bottom() - 3.0), pos2(rect.right() - 1.0, rect.bottom() - 1.0));
        let strip = Rect::from_min_max(pos2(bar.left(), rect.bottom() - 10.0), rect.max);
        let seekable = dur.is_some() && matches!(state, PlayState::Playing | PlayState::Paused | PlayState::Buffering);
        let strip_resp = ui.interact(strip, ui.scope_id().with("lcd-seek"), if seekable { Sense::click_and_drag() } else { Sense::hover() });
        let frac_at = |x: f32| f64::from(((x - bar.left()) / bar.width()).clamp(0.0, 1.0));
        if let Some(d) = dur {
            if (strip_resp.dragged() || strip_resp.drag_started())
                && let Some(p) = strip_resp.interact_pointer_pos()
            {
                self.seek_preview = Some(frac_at(p.x) * d);
            }
            if strip_resp.drag_stopped() || strip_resp.clicked() {
                if let Some(to) = strip_resp.interact_pointer_pos().map(|p| frac_at(p.x) * d).or(self.seek_preview) {
                    self.actions.push(Action::Seek(to));
                }
                self.seek_preview = None;
            }
        }
        painter.rect_filled(bar, 0.0, t.hairline);
        if let Some(f) = self.engine.status.fetched_fraction() {
            painter.rect_filled(Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * f.clamp(0.0, 1.0), bar.bottom())), 0.0, t.selected);
        }
        let shown = self.seek_preview.unwrap_or(pos);
        let x = bar.left() + bar.width() * dur.map_or(0.0, |d| (shown / d).clamp(0.0, 1.0) as f32);
        painter.rect_filled(Rect::from_min_max(bar.min, pos2(x, bar.bottom())), 0.0, t.muted);
        let buffering = state == PlayState::Buffering || self.pending_seek.is_some();
        if buffering {
            let tm = ui.input(|i| i.time) as f32;
            let alpha = 0.35 + 0.35 * (tm * std::f32::consts::TAU / 1.2).sin();
            painter.rect_filled(Rect::from_min_max(pos2(x, bar.top()), pos2((x + 40.0).min(bar.right()), bar.bottom())), 0.0, t.accent.gamma_multiply(alpha));
        }
        let hovering = body.hovered() || strip_resp.hovered() || self.seek_preview.is_some();
        // Love: the heart at the right end, shown when loved or on hover.
        let loved = self.favorites.is_on(StarKind::Song, &item.track.id);
        let heart = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y - 1.0), vec2(22.0, 22.0));
        let heart_resp = ui.interact(heart, ui.scope_id().with("lcd-heart"), if loved || hovering { Sense::click() } else { Sense::hover() });
        if loved || hovering || heart_resp.hovered() {
            let (icon, color) = if loved { (Icon::HeartFilled, t.accent) } else { (Icon::Heart, if heart_resp.hovered() { t.text } else { t.muted }) };
            icons::paint(&painter, Rect::from_center_size(heart.center(), vec2(11.0, 11.0)), icon, color);
        }
        let love_label = if loved { "Unlove (⇧⌘L)" } else { "Love (⇧⌘L)" };
        heart_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, love_label));
        if heart_resp.clicked() {
            self.actions.push(Action::ToggleStar { kind: StarKind::Song, id: item.track.id.clone(), track: Some(Box::new(item.track.clone())), name: None });
        }
        if hovering {
            let small = FontId::proportional(10.5);
            let y = rect.top() + 5.0;
            painter.text(pos2(text_rect.left(), y), egui::Align2::LEFT_TOP, mmss(shown), small.clone(), t.muted);
            let right = dur.map(|d| format!("−{}", mmss(d - shown))).unwrap_or_default();
            painter.text(pos2(heart.left() - 2.0, y), egui::Align2::RIGHT_TOP, right, small, t.muted);
            if strip_resp.hovered() || self.seek_preview.is_some() {
                painter.circle_filled(pos2(x, bar.center().y), 4.0, t.text);
            }
        }
        let label = format!("Now playing: {} — {sub_text}", item.track.title);
        body.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        if body.clicked() && !strip_resp.hovered() && !heart_resp.hovered() {
            self.actions.push(Action::Go(Route::Album(item.track.album_id.clone())));
        }
        if !heart_resp.hovered() {
            body.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        heart_resp.on_hover_text(love_label);
    }

    /// Queue toggle · volume · output chip (laid out right to left).
    fn toolbar_right(&mut self, ui: &mut Ui, narrow: bool) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.x = 10.0;
        // In narrow windows the queue is a sheet: the button shows whether it's open.
        let shown = if self.queue_narrow { self.queue_sheet } else { self.show_queue };
        let queue_icon = icons::sized_button(ui, Icon::Queue, if shown { "Hide queue (⌥⌘U)" } else { "Show queue (⌥⌘U)" }, vec2(28.0, 28.0), 14.0, Some(if shown { t.accent } else { t.muted }));
        if shown {
            ui.painter().rect_filled(queue_icon.rect, 6.0, t.accent.gamma_multiply(0.12));
        }
        if queue_icon.clicked() {
            self.actions.push(Action::ToggleQueue);
        }
        if icons::sized_button(ui, Icon::MiniPlayer, "Mini player (⇧⌘M)", vec2(28.0, 28.0), 14.0, Some(t.muted)).clicked() {
            self.actions.push(Action::ToggleMini);
        }
        let mut volume = |ui: &mut Ui, width: f32| {
            let resp = ui
                .add_enabled_ui(!self.bit_perfect, |ui| super::widgets::thin_slider(ui, &mut self.volume, width, "Volume"))
                .inner
                .on_disabled_hover_text("Bit-perfect: volume is controlled by your DAC or the system");
            if resp.changed() {
                self.engine.send(Command::SetVolume(self.volume));
            }
        };
        if narrow {
            let button = icons::sized_button(ui, Icon::Volume, "Volume", vec2(28.0, 28.0), 14.0, Some(t.muted));
            egui::Popup::menu(&button).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| volume(ui, 140.0));
        } else {
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                volume(ui, 80.0);
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Volume, t.muted);
            });
        }
        self.output.picker_ui(ui, &self.engine, &self.paths, &mut self.bit_perfect);
    }
}
