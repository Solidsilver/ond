//! The toolbar: transport on the left, the now-playing display centred on the
//! window, output / volume / queue on the right.

use super::widgets::elide;
use super::seek::SeekStyle;
use super::view_ctx::ViewCtx;
use super::{Action, OndApp};
use crate::favorites::StarKind;
use crate::icons::{self, Icon};
use crate::nav::Route;
use crate::theme::{Tokens, semibold};
use audio::PlayState;
use egui::{Align, FontId, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};

/// The toolbar's height; on macOS the traffic lights sit centred in it.
pub const TOOLBAR_H: f32 = 52.0;
/// Width of prev · play/pause · next.
const TRANSPORT_W: f32 = 108.0;
/// Without the traffic lights (full screen, other platforms), the transport's
/// smallest inset from the window edge.
const TRANSPORT_INSET: f32 = 14.0;
const RIGHT_W: f32 = 370.0;
const LCD_MAX_W: f32 = 540.0;
const LCD_MIN_W: f32 = 200.0;
const LCD_H: f32 = 44.0;
/// Below this window width the toolbar tightens up.
const NARROW_W: f32 = 900.0;

/// Where prev/play/next go: centred in the free space between `free_left`
/// (past the traffic lights) and the now-playing display, never left of
/// `free_left`.
pub(super) fn transport_x(free_left: f32, lcd_left: f32) -> f32 {
    let centred = (free_left + lcd_left) / 2.0 - TRANSPORT_W / 2.0;
    centred.max(free_left)
}

/// Where the now-playing display goes: centred on the window (`full`'s
/// centre), as wide as the room on its tighter side allows; when even that is
/// too little, centred in whatever room there is between the two sides.
pub(super) fn lcd_rect(full: Rect, left_end: f32, right_start: f32) -> Rect {
    const GAP: f32 = 12.0;
    let cx = full.center().x;
    let half = (cx - left_end).min(right_start - cx) - GAP;
    let y = full.center().y;
    if half * 2.0 >= LCD_MIN_W {
        return Rect::from_center_size(pos2(cx, y), vec2((half * 2.0).min(LCD_MAX_W), LCD_H));
    }
    let room = Rect::from_min_max(pos2(left_end + GAP, full.top()), pos2(right_start - GAP, full.bottom()));
    Rect::from_center_size(pos2(room.center().x, y), vec2(room.width().clamp(140.0, LCD_MAX_W), LCD_H))
}

impl OndApp {
    pub(super) fn toolbar(&mut self, ui: &mut Ui) {
        let mut full = ui.available_rect_before_wrap();
        crate::csd::set_bar(ui.ctx(), full, None);
        // The bar is the window's title bar too: its empty parts move the
        // window, and a double-click zooms it. Widgets drawn after this take
        // the pointer where they are.
        let whole = full.expand2(vec2(12.0, 0.0));
        let bg = ui.interact(whole, ui.scope_id().with("titlebar"), Sense::click_and_drag());
        if bg.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if bg.double_clicked() {
            crate::chrome::title_double_click(ui.ctx());
        }
        // On macOS the traffic lights sit at the left of the bar; on Linux our
        // own window buttons at whichever end the desktop puts them.
        use crate::csd::{Side, reserved};
        full.max.x -= reserved(ui.ctx(), Side::Right);
        let free_left = if crate::chrome::folded(ui.ctx()) {
            whole.left() + crate::chrome::LIGHTS_W
        } else {
            (whole.left() + reserved(ui.ctx(), Side::Left)).max(full.left() + TRANSPORT_INSET)
        };
        // Narrow windows fold the volume slider behind its icon so the
        // now-playing display keeps room for the title.
        let narrow = full.width() < NARROW_W;
        let right_w = if narrow { RIGHT_W - 90.0 } else { RIGHT_W };
        let right = Rect::from_min_max(pos2(full.right() - right_w, full.top()), full.max);
        let right_start = ui
            .scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)), |ui| {
                self.toolbar_right(ui, narrow);
                ui.min_rect().left()
            })
            .inner;
        let lcd = lcd_rect(full, free_left + TRANSPORT_W, right_start);
        let x = transport_x(free_left, lcd.left());
        let left = Rect::from_min_max(pos2(x, full.top()), pos2(x + TRANSPORT_W, full.bottom()));
        ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)), |ui| self.transport(ui, None));
        if self.view().lcd_display(ui, lcd) {
            self.output.request_open();
        }
    }

    /// Previous · play/pause (a spinner while buffering) · next. `color`
    /// overrides the icons' color (e.g. over artwork).
    pub(super) fn transport(&mut self, ui: &mut Ui, color: Option<egui::Color32>) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.x = 4.0;
        let state = self.engine.status.state();
        if icons::sized_button(ui, Icon::Previous, "Previous", vec2(32.0, 32.0), 13.0, color).clicked() {
            self.actions.push(Action::Previous);
        }
        let waiting = matches!(state, PlayState::Buffering | PlayState::Loading) || self.player.pending_seek.is_some();
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

    /// Queue toggle · volume · output chip (laid out right to left).
    fn toolbar_right(&mut self, ui: &mut Ui, narrow: bool) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.x = 10.0;
        // In narrow windows the queue is a sheet: the button shows whether it's open.
        let shown = if self.ui.queue_narrow { self.ui.queue_sheet } else { self.show_queue() };
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
            let mut level = self.volume();
            let resp = ui
                .add_enabled_ui(!self.bit_perfect, |ui| super::widgets::thin_slider(ui, &mut level, width, "Volume"))
                .inner
                .on_disabled_hover_text("In bit-perfect mode, use your DAC's or the system's volume");
            if resp.changed() {
                self.set_volume(level);
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
        if self.output.picker_ui(ui, &self.engine, &mut self.bit_perfect) {
            self.actions.push(Action::OpenSettings(super::settings_page::Section::Playback));
        }
    }
}

impl ViewCtx<'_> {
    /// The now-playing display. Art on the left; title and "artist — album"
    /// centred; along the bottom a thin seek line with the elapsed and
    /// remaining times at its ends; the format pill and the heart on the right.
    /// Returns whether the format pill was clicked (to open the output picker).
    fn lcd_display(&mut self, ui: &mut Ui, rect: Rect) -> bool {
        let t = Tokens::of(ui);
        let painter = ui.painter().clone();
        let fill = crate::theme::tint_of(ui).map_or(t.sidebar, |x| x.display);
        painter.rect(rect, 7.0, fill, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
        let body = ui.interact(rect, ui.scope_id().with("lcd"), Sense::click());
        // The live spectrum, faint behind the text (experimental).
        if self.experimental().spectrum
            && self.player.queue.current.is_some()
            && let Some(f) = self.listen(ui)
        {
            let area = Rect::from_min_max(pos2(rect.left() + rect.height() + 8.0, rect.top() + 5.0), pos2(rect.right() - 8.0, rect.bottom() - 13.0));
            let n = ((area.width() / 6.0) as usize).clamp(8, crate::visuals::spectrum::BANDS);
            crate::visuals::paint_bars(&painter, area, &crate::visuals::spectrum::grouped(&f.bands, n), t.accent.gamma_multiply(if ui.visuals().dark_mode { 0.16 } else { 0.12 }));
        }
        // Borrowed from the player and library, not from `self`, so drawing can
        // take `&mut self`.
        let (player, data) = (self.player, self.data);
        let Some(item) = player.queue.current.as_ref() else {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Not playing", FontId::proportional(12.0), t.muted);
            return false;
        };
        let art = Rect::from_min_size(rect.min, vec2(rect.height(), rect.height()));
        let album = data.album(&item.track.album_id);
        self.art.paint(ui, art.shrink(1.0), data.cover_for(&item.track), 6.0);

        // The right column: the heart at the end, the format pill just left of
        // it, both centred vertically.
        let heart = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(22.0, 22.0));
        let readout = self.output.readout(&self.engine.status);
        let readout_anchor = pos2(heart.left() - 2.0, rect.center().y);
        let readout_rect = readout.as_ref().map(|r| crate::output::paint_readout(ui, readout_anchor, r, 10.0, false));
        let col_right = readout_rect.map_or(heart.left(), |r| r.left()) - 10.0;
        let col_left = art.right() + 10.0;

        // Title and subtitle: centred on the display when the same margin fits
        // on both sides, else centred in the column between art and pill.
        let reserve = (col_left - rect.left()).max(rect.right() - col_right);
        let symmetric = rect.width() - 2.0 * reserve;
        let (center_x, text_w) = if symmetric >= 140.0 { (rect.center().x, symmetric) } else { ((col_left + col_right) / 2.0, (col_right - col_left).max(40.0)) };
        let title = elide(ui, &item.track.title, FontId::new(12.5, semibold()), t.text, text_w);
        let sub_text = match (&item.track.artist, album) {
            (Some(a), Some(al)) => format!("{a} · {}", al.name),
            (Some(a), None) => a.clone(),
            (None, Some(al)) => al.name.clone(),
            _ => String::new(),
        };
        let sub = elide(ui, &sub_text, FontId::proportional(11.0), t.muted, text_w);
        let y0 = rect.top() + 4.0;
        let title_h = title.size().y;
        painter.galley(pos2(center_x - title.size().x / 2.0, y0), title, t.text);
        painter.galley(pos2(center_x - sub.size().x / 2.0, y0 + title_h), sub, t.muted);

        // The seek line along the bottom of the column, the times at its ends.
        let time_font = FontId::proportional(9.5);
        // Sized for the widest time, so the line doesn't shift as digits change.
        let time_w = painter.layout_no_wrap("−88:88".into(), time_font.clone(), t.muted).size().x;
        let line_y = rect.bottom() - 7.5;
        let bar_left = col_left + time_w + 6.0;
        let bar = Rect::from_min_max(pos2(bar_left, line_y - 1.0), pos2((col_right - time_w - 6.0).max(bar_left + 20.0), line_y + 1.0));
        let strip = Rect::from_min_max(pos2(bar.left() - 4.0, line_y - 6.0), pos2(bar.right() + 4.0, rect.bottom()));
        let style = SeekStyle {
            track: t.hairline,
            fetched: t.selected,
            fill: t.muted,
            pulse: t.accent,
            pulse_len: 40.0,
            radius: 1.0,
            grow: 0.5,
            knob: t.text,
            knob_r: 3.5,
            wave_h: 4.0,
        };
        let (shown, strip_resp) = self.seek_line(ui, bar, strip, "lcd-seek", &style);
        let (elapsed, remaining) = self.seek_times(shown);
        painter.text(pos2(bar.left() - 6.0, line_y), egui::Align2::RIGHT_CENTER, elapsed, time_font.clone(), t.muted);
        painter.text(pos2(bar.right() + 6.0, line_y), egui::Align2::LEFT_CENTER, remaining, time_font, t.muted);

        let hovering = body.hovered() || strip_resp.hovered() || self.ui.seek_preview.is_some();
        // Love: the heart at the right end, shown when loved or on hover.
        let loved = self.favorites.is_on(StarKind::Song, &item.track.id);
        // The signal path in a few characters; hover for all of it, click for the picker.
        let readout_resp = readout.as_ref().zip(readout_rect).map(|(r, area)| {
            crate::output::paint_readout(ui, readout_anchor, r, 10.0, true);
            let resp = ui.interact(area, ui.scope_id().with("lcd-readout"), Sense::click());
            if resp.hovered() {
                painter.rect_stroke(area, area.height() / 2.0, egui::Stroke::new(1.0, t.muted.gamma_multiply(0.6)), egui::StrokeKind::Inside);
            }
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Signal path: {}", r.detail.lines().next().unwrap_or_default())));
            resp
        });
        let on_readout = readout_resp.as_ref().is_some_and(|r| r.hovered());
        let open_picker = readout_resp.as_ref().is_some_and(|r| r.clicked());
        // Always clickable: the heart sits over the display, so while the pointer
        // is on it the display itself doesn't count as hovered.
        let love_label = if loved { "Unlove (⇧⌘L)" } else { "Love (⇧⌘L)" };
        let heart_resp = super::widgets::hover_heart(ui, heart, ui.scope_id().with("lcd-heart"), loved, hovering, 11.0, love_label);
        if heart_resp.clicked() {
            self.actions.push(Action::ToggleStar { kind: StarKind::Song, id: item.track.id.clone(), track: Some(Box::new(item.track.clone())), name: None });
        }
        body.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Now playing: {}, {sub_text}", item.track.title)));
        // The artwork switches to the player last used (Player or Mini); the
        // rest of the display opens the album.
        let on_art = body.interact_pointer_pos().or(body.hover_pos()).is_some_and(|p| art.contains(p));
        if body.clicked() && !strip_resp.hovered() && !heart_resp.hovered() && !on_readout {
            self.actions.push(if on_art { Action::TogglePlayer } else { Action::Go(Route::Album(item.track.album_id.clone())) });
        }
        if on_art && body.hovered() {
            painter.rect_stroke(art.shrink(1.0), 6.0, egui::Stroke::new(1.5, t.accent.gamma_multiply(0.7)), egui::StrokeKind::Inside);
        }
        if !heart_resp.hovered() && !on_readout && !strip_resp.hovered() {
            body.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        heart_resp.on_hover_text(crate::keys::shown(love_label));
        if let (Some(resp), Some(r)) = (readout_resp, readout) {
            resp.on_hover_text(r.detail).on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        open_picker
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(w: f32) -> Rect {
        Rect::from_min_size(pos2(12.0, 0.0), vec2(w - 24.0, 52.0))
    }

    #[test]
    fn the_display_is_centred_on_the_window() {
        let f = full(1440.0);
        let r = lcd_rect(f, 26.0 + 112.0, f.right() - 330.0);
        assert!((r.center().x - 720.0).abs() < 0.5, "{r:?}");
        assert_eq!(r.width(), LCD_MAX_W);
    }

    #[test]
    fn a_tight_side_shrinks_it_but_keeps_it_centred() {
        let f = full(1000.0);
        let right_start = f.right() - 330.0;
        let r = lcd_rect(f, 138.0, right_start);
        assert!((r.center().x - 500.0).abs() < 0.5);
        assert!(r.right() <= right_start - 11.9, "{r:?} overlaps the right controls");
        assert!(r.width() < LCD_MAX_W);
    }

    #[test]
    fn the_transport_sits_midway_between_the_lights_and_the_display() {
        let x = transport_x(84.0, 450.0);
        assert!(((x + TRANSPORT_W / 2.0) - 267.0).abs() < 0.5);
        // No room to centre: it stays clear of the lights.
        assert_eq!(transport_x(84.0, 150.0), 84.0);
    }

    #[test]
    fn with_no_room_at_the_centre_it_fits_between_the_sides() {
        let f = full(640.0);
        let (left_end, right_start) = (138.0, f.right() - 300.0);
        let r = lcd_rect(f, left_end, right_start);
        assert!(r.left() >= left_end && r.right() <= right_start + 1.0 || r.width() == 140.0, "{r:?}");
    }
}
