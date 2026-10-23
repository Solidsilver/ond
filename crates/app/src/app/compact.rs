//! Mini, the smallest window mode: the artwork with a thin progress line,
//! square however it's resized; hovering shows the title and transport over
//! it. "Up next" grows it into the Player (see `now_playing_view`), whose
//! bottom status line lives here too.

use super::widgets::elide;
use super::{Action, ListenerApp, mmss};
use crate::icons::{self, Icon};
use crate::theme::{Tokens, semibold};
use crate::window::WindowMode;
use audio::PlayState;
use egui::{Align, Color32, FontId, Layout, Rect, Sense, Ui, UiBuilder, ViewportCommand, pos2, vec2};
use library::Track;

/// Text and icons drawn over artwork, whatever the theme.
const ON_ART: Color32 = Color32::from_rgb(0xE8, 0xE9, 0xEC);
const ON_ART_MUTED: Color32 = Color32::from_rgb(0xC9, 0xCC, 0xD2);

pub(super) fn scrim() -> Color32 {
    Color32::from_rgba_unmultiplied(12, 13, 15, 204)
}

/// Buttons over artwork: light icons, a faint light hover.
pub(super) fn on_art(ui: &mut Ui) {
    let w = &mut ui.visuals_mut().widgets;
    w.hovered.weak_bg_fill = Color32::from_white_alpha(28);
    w.active.weak_bg_fill = Color32::from_white_alpha(44);
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
}

impl ListenerApp {
    fn cover_of(&self, track: &Track) -> Option<String> {
        track.cover_art.clone().or_else(|| self.data.album(&track.album_id).and_then(|a| a.cover_art.clone()))
    }

    pub(super) fn mini_ui(&mut self, ui: &mut Ui) {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| {
            let rect = ui.max_rect();
            let cover = self.queue.current.as_ref().and_then(|i| self.cover_of(&i.track));
            self.art.paint(ui, rect, cover.as_deref(), 0.0);
            if self.queue.current.is_none() {
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
            if shown > 0.0 {
                self.mini_overlay(ui, rect, shown);
            }
            let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 3.0), rect.max);
            self.progress_line(ui, bar, Color32::from_white_alpha(46), ON_ART);
        });
    }

    /// Title, transport and "Up next" over the bottom of the art; close and
    /// full-window buttons in the top corners (Mini has no title bar).
    fn mini_overlay(&mut self, ui: &mut Ui, rect: Rect, alpha: f32) {
        let mut ui = ui.new_child(UiBuilder::new().max_rect(rect));
        ui.multiply_opacity(alpha);
        on_art(&mut ui);
        let panel = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 88.0), rect.max);
        ui.painter().rect_filled(panel, 0.0, scrim());
        let inner = panel.shrink2(vec2(12.0, 10.0));
        if let Some(item) = &self.queue.current {
            let title = elide(&ui, &item.track.title, FontId::new(13.0, semibold()), ON_ART, inner.width() - 4.0);
            let artist = elide(&ui, item.track.artist.as_deref().unwrap_or(""), FontId::proportional(11.5), ON_ART_MUTED, inner.width() - 4.0);
            let title_h = title.size().y;
            ui.painter().galley(pos2(inner.left() + 4.0, inner.top()), title, ON_ART);
            ui.painter().galley(pos2(inner.left() + 4.0, inner.top() + title_h + 1.0), artist, ON_ART_MUTED);
        }
        let row = Rect::from_min_max(pos2(inner.left() - 4.0, inner.bottom() - 32.0), inner.max);
        ui.scope_builder(UiBuilder::new().max_rect(row).layout(Layout::left_to_right(Align::Center)), |ui| {
            self.transport(ui, Some(ON_ART));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let up_next = egui::Button::new(egui::RichText::new("Up next").size(11.0).color(ON_ART))
                    .corner_radius(11.0)
                    .stroke(egui::Stroke::new(1.0, Color32::from_white_alpha(64)));
                if ui.add(up_next).on_hover_text("The player, with the queue (⌥⌘2)").clicked() {
                    self.actions.push(Action::SetMode(WindowMode::Player));
                }
                if let (Some(d), Some(_)) = (self.duration_secs(), &self.queue.current) {
                    ui.label(egui::RichText::new(format!("−{}", mmss(d - self.position_secs()))).size(11.0).color(ON_ART_MUTED));
                }
            });
        });
        let corner = |at: egui::Pos2| Rect::from_center_size(at, vec2(24.0, 24.0));
        for (r, icon, label, action) in [
            (corner(rect.left_top() + vec2(18.0, 18.0)), Icon::Close, "Close", None),
            (corner(rect.right_top() + vec2(-18.0, 18.0)), Icon::Expand, "Full window (⌥⌘1)", Some(Action::SetMode(WindowMode::Full))),
        ] {
            ui.painter().circle_filled(r.center(), 12.0, scrim());
            let mut b = ui.new_child(UiBuilder::new().max_rect(r));
            if icons::sized_button(&mut b, icon, label, r.size(), 9.0, Some(ON_ART)).clicked() {
                match action {
                    Some(a) => self.actions.push(a),
                    None => ui.ctx().send_viewport_cmd(ViewportCommand::Close),
                }
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
                    if self.output.picker_ui(ui, &self.engine, &self.paths, &mut self.bit_perfect) {
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
                        if !self.online && self.settings.is_some() {
                            let (g, resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                            icons::paint(ui.painter(), g, Icon::Offline, t.muted);
                            resp.on_hover_text("Can't reach the server. Downloaded music still plays.");
                        }
                    });
                });
            });
    }

    /// A thin progress line you can click or drag to seek, with a taller
    /// invisible strip to grab. A pulse sits ahead of the position while buffering.
    fn progress_line(&mut self, ui: &mut Ui, bar: Rect, track: Color32, fill: Color32) {
        let state = self.engine.status.state();
        let dur = self.duration_secs().filter(|d| *d > 0.0);
        let seekable = dur.is_some() && matches!(state, PlayState::Playing | PlayState::Paused | PlayState::Buffering);
        let strip = Rect::from_min_max(pos2(bar.left(), bar.bottom() - 10.0), bar.max);
        let resp = ui.interact(strip, ui.scope_id().with(("progress", bar.min.y as i32)), if seekable { Sense::click_and_drag() } else { Sense::hover() });
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
        let grabbed = resp.hovered() || self.seek_preview.is_some();
        let bar = if grabbed { Rect::from_min_max(pos2(bar.left(), bar.bottom() - 5.0), bar.max) } else { bar };
        painter.rect_filled(bar, 0.0, track);
        if let Some(f) = self.engine.status.fetched_fraction() {
            painter.rect_filled(Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * f.clamp(0.0, 1.0), bar.bottom())), 0.0, fill.gamma_multiply(0.3));
        }
        let shown = self.seek_preview.unwrap_or_else(|| self.position_secs());
        let x = bar.left() + bar.width() * dur.map_or(0.0, |d| (shown / d).clamp(0.0, 1.0) as f32);
        painter.rect_filled(Rect::from_min_max(bar.min, pos2(x, bar.bottom())), 0.0, fill);
        if state == PlayState::Buffering || self.pending_seek.is_some() {
            let tm = ui.input(|i| i.time) as f32;
            let alpha = 0.35 + 0.35 * (tm * std::f32::consts::TAU / 1.2).sin();
            painter.rect_filled(Rect::from_min_max(pos2(x, bar.top()), pos2((x + 40.0).min(bar.right()), bar.bottom())), 0.0, fill.gamma_multiply(alpha));
        }
        if grabbed && seekable {
            resp.on_hover_text_at_pointer(format!("{} / {}", mmss(shown), dur.map(mmss).unwrap_or_default()));
        }
    }
}
