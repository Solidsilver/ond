//! Mini, the smallest window mode: the artwork with a thin progress line,
//! square however it's resized; hovering shows the title and transport over
//! it. "Up next" grows it into the Player (see `now_playing_view`), whose
//! bottom status line lives here too.

use super::widgets::elide;
use super::seek::SeekStyle;
use super::{Action, OndApp};
use crate::icons::{self, Icon};
use crate::theme::{Tokens, semibold};
use crate::visuals::backdrop::Blurred;
use crate::visuals::over_art::{self, OnArt};
use crate::window::WindowMode;
use egui::{Align, Color32, FontId, Layout, Rect, Sense, Ui, UiBuilder, ViewportCommand, pos2, vec2};

/// Text and icons drawn over artwork on a scrim, whatever the theme.
const ON_ART: Color32 = over_art::LIGHT;
const ON_ART_MUTED: Color32 = Color32::from_rgb(0xC9, 0xCC, 0xD2);

/// Mini's hover panel over the foot of the artwork: its height; the seek
/// line's centre below the top of its text; the transport row's centre above
/// its bottom (both inside its padding).
const PANEL_H: f32 = 112.0;
const PANEL_SEEK_Y: f32 = 46.0;
const PANEL_TRANSPORT_Y: f32 = 15.0;

pub(super) fn scrim() -> Color32 {
    Color32::from_rgba_unmultiplied(12, 13, 15, 204)
}

/// What part of the cover filling `whole` lies under `part`, from the
/// cover's blurred copy: its darkest and brightest colors. `None` until the
/// cover's look is worked out.
fn under(blurred: Option<&Blurred>, part: Rect, whole: Rect) -> Option<[Color32; 2]> {
    let crop = crate::art::crop_uv(whole.width() / whole.height().max(1.0));
    blurred.map(|b| over_art::region(&b.image, over_art::uv_of(part, whole, crop)))
}

impl OndApp {
    pub(super) fn mini_ui(&mut self, ui: &mut Ui) {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| {
            let rect = ui.max_rect();
            let cover = self.player.queue.current.as_ref().and_then(|i| self.data.cover_for(&i.track));
            self.art.paint(ui, rect, cover, 0.0);
            // What's under the controls drawn over the cover, so they can read on it.
            let blurred = cover.and_then(|c| self.art.look(c)).and_then(|l| l.backdrop.clone());
            if self.player.queue.current.is_none() {
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Not playing", FontId::proportional(12.0), ON_ART_MUTED);
            }
            // No title bar: the artwork moves the window, and a double-click goes back.
            let body = ui.interact(rect, ui.scope_id().with("mini-body"), Sense::click_and_drag());
            if body.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if body.double_clicked() {
                self.actions.push(Action::ToggleMini);
            }
            let shown = ui.ctx().animate_bool_with_time(ui.scope_id().with("mini-hover"), ui.rect_contains_pointer(rect), 0.15);
            // At rest: a thin line inset from the edges, just to show where the
            // song is. Seeking happens in the overlay, away from the window's
            // edges and resize zones.
            if shown < 1.0 && self.player.queue.current.is_some() {
                let line = Rect::from_min_max(pos2(rect.left() + 12.0, rect.bottom() - 9.0), pos2(rect.right() - 12.0, rect.bottom() - 7.0));
                let frac = self.duration_secs().filter(|d| *d > 0.0).map_or(0.0, |d| (self.position_secs() / d).clamp(0.0, 1.0) as f32);
                let fade = 1.0 - shown;
                let on = under(blurred.as_ref(), line.expand(4.0), rect).map_or(OnArt::plain(ON_ART), |u| OnArt::pick(u, ON_ART, over_art::GRAPHIC));
                if let Some(scrim) = on.scrim {
                    ui.painter().rect_filled(line.expand2(vec2(5.0, 3.0)), 4.0, scrim.gamma_multiply(fade));
                }
                ui.painter().rect_filled(line, 1.0, on.fg.gamma_multiply(0.22 * fade));
                ui.painter().rect_filled(Rect::from_min_max(line.min, pos2(line.left() + line.width() * frac, line.bottom())), 1.0, on.fg.gamma_multiply(0.9 * fade));
            }
            if shown > 0.0 {
                self.mini_overlay(ui, rect, shown, blurred.as_ref());
            }
        });
    }

    /// Over the bottom of the art: title, a seek bar with the times beside it,
    /// and the transport, on a scrim (or a frosted strip of the backdrop). In
    /// the top corners: the Player and the full window, in colors that read
    /// on what's under them (`blurred`: the cover's blurred copy, once known).
    fn mini_overlay(&mut self, ui: &mut Ui, rect: Rect, alpha: f32, blurred: Option<&Blurred>) {
        let mut ui = ui.new_child(UiBuilder::new().max_rect(rect));
        ui.multiply_opacity(alpha);
        OnArt::plain(ON_ART).apply(&mut ui);
        let panel = Rect::from_min_max(pos2(rect.left(), rect.bottom() - PANEL_H), rect.max);
        let layers = self.backdrop_layers(ui.ctx());
        if layers.is_empty() {
            ui.painter().rect_filled(panel, 0.0, scrim());
        } else {
            // Frosted: the blurred cover under the panel, lined up with the
            // artwork it covers, then only as much scrim as the text needs.
            let uv = Rect::from_min_max(pos2(0.0, (panel.top() - rect.top()) / rect.height()), pos2(1.0, 1.0));
            let wash = scrim().to_opaque();
            let mut alpha: f32 = 0.0;
            for l in &layers {
                ui.painter().add(egui::epaint::RectShape::filled(panel, 0.0, Color32::WHITE.gamma_multiply(l.opacity)).with_texture(l.blurred.texture.id(), uv));
                alpha = alpha.max(crate::visuals::backdrop::wash_alpha(l.blurred.extremes, wash, ON_ART, ON_ART_MUTED));
            }
            ui.painter().rect_filled(panel, 0.0, wash.gamma_multiply(alpha));
        }

        let inner = panel.shrink2(vec2(14.0, 10.0));
        if let Some(item) = &self.player.queue.current {
            let title = elide(&ui, &item.track.title, FontId::new(13.0, semibold()), ON_ART, inner.width());
            let artist = elide(&ui, item.track.artist.as_deref().unwrap_or(""), FontId::proportional(11.5), ON_ART_MUTED, inner.width());
            let title_h = title.size().y;
            ui.painter().galley(pos2(inner.left(), inner.top()), title, ON_ART);
            ui.painter().galley(pos2(inner.left(), inner.top() + title_h + 1.0), artist, ON_ART_MUTED);
        }
        // The seek bar, inset from every edge, with the times at its ends.
        let seek_y = inner.top() + PANEL_SEEK_Y;
        if self.player.queue.current.is_some() {
            let font = FontId::proportional(10.0);
            let time_w = ui.painter().layout_no_wrap("−88:88".into(), font.clone(), ON_ART_MUTED).size().x;
            let bar = Rect::from_min_max(pos2(inner.left() + time_w + 8.0, seek_y - 1.5), pos2(inner.right() - time_w - 8.0, seek_y + 1.5));
            let style = SeekStyle {
                track: Color32::from_white_alpha(56),
                fetched: ON_ART.gamma_multiply(0.3),
                fill: ON_ART,
                pulse: ON_ART,
                pulse_len: 30.0,
                radius: 2.0,
                grow: 1.0,
                knob: ON_ART,
                knob_r: 5.0,
                wave_h: 0.0,
            };
            let mut v = self.view();
            let (shown, _) = v.seek_line(&mut ui, bar, bar.expand2(vec2(4.0, 7.0)), "mini-seek", &style);
            let (elapsed, remaining) = v.seek_times(shown);
            ui.painter().text(pos2(bar.left() - 8.0, seek_y), egui::Align2::RIGHT_CENTER, elapsed, font.clone(), ON_ART_MUTED);
            ui.painter().text(pos2(bar.right() + 8.0, seek_y), egui::Align2::LEFT_CENTER, remaining, font, ON_ART_MUTED);
        }
        let row = Rect::from_center_size(pos2(inner.center().x, inner.bottom() - PANEL_TRANSPORT_Y), vec2(120.0, 32.0));
        ui.scope_builder(UiBuilder::new().max_rect(row).layout(Layout::left_to_right(Align::Center)), |ui| self.transport(ui, Some(ON_ART)));
        let corner = |at: egui::Pos2| Rect::from_center_size(at, vec2(26.0, 26.0));
        for (r, icon, label, mode) in [
            (corner(rect.left_top() + vec2(19.0, 19.0)), Icon::Player, "Player (⌥⌘2)", WindowMode::Player),
            (corner(rect.right_top() + vec2(-19.0, 19.0)), Icon::Expand, "Full window (⌥⌘1)", WindowMode::Full),
        ] {
            // Until the cover's look is known: the dark disc that suits any cover.
            let on = under(blurred, r, rect).map_or(OnArt { fg: ON_ART, scrim: Some(scrim()) }, |u| OnArt::pick(u, ON_ART, over_art::ICON));
            if let Some(scrim) = on.scrim {
                ui.painter().circle_filled(r.center(), 13.0, scrim);
            }
            let mut b = ui.new_child(UiBuilder::new().max_rect(r));
            on.apply(&mut b);
            if icons::sized_button(&mut b, icon, label, r.size(), 10.0, Some(on.fg)).clicked() {
                self.actions.push(Action::SetMode(mode));
            }
        }
    }

    /// The Player's bottom line: the output picker and the signal readout.
    pub(super) fn player_status_bar(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        egui::Panel::bottom("player-status")
            .exact_size(36.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.panel).inner_margin(egui::Margin::symmetric(10, 0)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if self.output.picker_ui(ui, &self.engine, &mut self.bit_perfect) {
                        self.actions.push(Action::OpenSettings(super::settings_page::Section::Playback));
                    }
                    // The same few characters of signal path as the full window's display.
                    let readout = self.output.readout(&self.engine.status);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(r) = readout {
                            let size = crate::output::paint_readout(ui, egui::Pos2::ZERO, &r, 10.5, false).size();
                            let (rect, resp) = ui.allocate_exact_size(size + vec2(4.0, 6.0), Sense::click());
                            crate::output::paint_readout(ui, rect.right_center() - vec2(2.0, 0.0), &r, 10.5, true);
                            if resp.clicked() {
                                self.output.request_open();
                            }
                            resp.on_hover_text(r.detail).on_hover_cursor(egui::CursorIcon::PointingHand);
                        }
                        if !self.session.online && self.session.settings.is_some() {
                            let (g, resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                            icons::paint(ui.painter(), g, Icon::Offline, t.muted);
                            resp.on_hover_text("Can't reach the server. Downloaded music still plays.");
                        }
                    });
                });
            });
    }
}
