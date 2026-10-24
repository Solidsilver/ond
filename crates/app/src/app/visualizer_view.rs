//! The visualizer (experimental, ⌘T): the whole window given over to light
//! moving with the music, in the album's colors. F or a double-click goes full
//! screen; Esc leaves. The playing track's title shows for a few seconds when
//! it starts. Drawn on the GPU (see `visuals::gpu`), or with the painter where
//! there's no wgpu renderer to hand.

use super::{Action, OndApp};
use crate::visuals::visualizer::{Light, Scene, caption_alpha};
use crate::visuals::{SMOOTH, gpu};
use egui::{Color32, FontId, Key, Modifiers, Rect, Ui, pos2, vec2};
use std::collections::VecDeque;

/// Earlier frames the painter version keeps, for its trails.
const TRAIL: usize = 10;
/// How long the key hint shows after opening.
const HINT_SECS: f64 = 4.0;

/// The open visualizer.
pub struct VisualizerView {
    scene: Scene,
    /// Frame clock when it opened, and of the last frame drawn.
    opened: f64,
    last: f64,
    /// The queue item whose title is captioned, and since when.
    track: Option<u64>,
    track_since: f64,
    /// The cover whose colors the scene has.
    cover: Option<String>,
    /// It made the window full screen (and so undoes that on leaving).
    fullscreen: bool,
    /// The next GPU frame starts from black.
    fresh: bool,
    /// The painter version's recent frames, newest last.
    trails: VecDeque<Vec<Light>>,
}

impl VisualizerView {
    fn new(now: f64) -> Self {
        Self { scene: Scene::default(), opened: now, last: now, track: None, track_since: now, cover: None, fullscreen: false, fresh: true, trails: VecDeque::new() }
    }
}

impl OndApp {
    /// Opens the visualizer, or closes it if it's open (⌘T). Only while that
    /// experiment is on.
    pub(super) fn toggle_visualizer(&mut self) {
        if self.ui.visualizer.is_some() {
            self.close_visualizer();
        } else if self.experimental().visualizer {
            self.ui.visualizer = Some(VisualizerView::new(self.ctx.input(|i| i.time)));
            self.ctx.request_repaint();
        }
    }

    pub(super) fn close_visualizer(&mut self) {
        if let Some(v) = self.ui.visualizer.take()
            && v.fullscreen
        {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }

    pub(super) fn visualizer_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let (esc, f) = ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Escape), i.consume_key(Modifiers::NONE, Key::F)));
        if esc {
            self.close_visualizer();
            return;
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| {
            let rect = ui.max_rect();
            let body = ui.interact(rect, ui.scope_id().with("visualizer"), egui::Sense::click());
            if (f || body.double_clicked())
                && let Some(v) = self.ui.visualizer.as_mut()
            {
                v.fullscreen = !v.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(v.fullscreen));
            }
            let now = ui.input(|i| i.time);
            let audio = self.view().listen(ui);
            if audio.is_some() {
                self.ui.live.want(SMOOTH);
            }
            // The album's colors, once its look is worked out.
            let current = self.player.queue.current.as_ref().map(|c| (c.uid, c.track.clone()));
            let cover = current.as_ref().and_then(|(_, t)| self.data.cover_for(t).map(str::to_owned));
            let look = cover.as_deref().and_then(|c| self.art.look(c));
            let Some(v) = self.ui.visualizer.as_mut() else { return };
            if cover != v.cover && (look.is_some() || cover.is_none()) {
                v.scene.set_colors(look.as_ref().and_then(|l| l.palette.as_ref()).map_or(&[][..], |p| &p.colors));
                v.cover = cover;
            }
            let dt = (now - v.last) as f32;
            v.last = now;
            let (lights, fb) = v.scene.step(dt, audio.as_ref());
            // The feedback is per 60th of a second; scale it to this frame's length.
            let n = (dt * 60.0).clamp(0.0, 3.0);
            let feedback = crate::visuals::visualizer::Feedback { fade: fb.fade.powf(n), zoom: fb.zoom.powf(n), turn: fb.turn * n, drift: fb.drift.map(|d| d * n) };
            match self.gpu {
                Some(format) => {
                    let size_px = [(rect.width() * ctx.pixels_per_point()).round() as u32, (rect.height() * ctx.pixels_per_point()).round() as u32];
                    ui.painter().add(gpu::shape(rect, gpu::VisualizerCallback { format, size_px, lights, feedback, clear: v.fresh }));
                    v.fresh = false;
                }
                None => {
                    v.trails.push_back(lights);
                    while v.trails.len() > TRAIL {
                        v.trails.pop_front();
                    }
                    paint_trails(ui.painter(), rect, &v.trails);
                }
            }
            // The track's title as it starts (and when the view opens).
            let uid = current.as_ref().map(|(u, _)| *u);
            if uid != v.track {
                v.track = uid;
                v.track_since = now;
            }
            let caption = caption_alpha(now - v.track_since);
            if let Some((_, track)) = &current
                && caption > 0.0
            {
                let painter = ui.painter();
                let at = pos2(rect.center().x, rect.top() + rect.height() * 0.78);
                painter.text(at, egui::Align2::CENTER_BOTTOM, &track.title, FontId::new(24.0, crate::theme::semibold()), Color32::from_white_alpha((235.0 * caption) as u8));
                if let Some(artist) = &track.artist {
                    painter.text(at + vec2(0.0, 8.0), egui::Align2::CENTER_TOP, artist, FontId::proportional(15.0), Color32::from_white_alpha((170.0 * caption) as u8));
                }
                if caption < 1.0 {
                    ctx.request_repaint();
                }
            }
            let since_open = now - v.opened;
            if since_open < HINT_SECS + 1.0 {
                let a = (1.0 - (since_open - HINT_SECS).max(0.0)) as f32;
                let hint = crate::keys::shown("Esc to leave · F for full screen").into_owned();
                ui.painter().text(pos2(rect.center().x, rect.bottom() - 28.0), egui::Align2::CENTER_CENTER, hint, FontId::proportional(12.0), Color32::from_white_alpha((120.0 * a) as u8));
                ctx.request_repaint_after(std::time::Duration::from_secs_f64((HINT_SECS + 1.0 - since_open).clamp(0.02, 1.0)));
            }
            body.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Visualizer"));
        });
    }

    /// Whether `action` takes the window elsewhere, so the visualizer gives way.
    pub(super) fn leaves_visualizer(action: &Action) -> bool {
        matches!(
            action,
            Action::Go(_) | Action::OpenPalette | Action::SetMode(_) | Action::ToggleMini | Action::TogglePlayer | Action::OpenSettings(_) | Action::Overlay(_) | Action::Lyrics | Action::Info
        )
    }
}

/// The painter version: each light as a soft dot, its last few positions
/// fading behind it.
fn paint_trails(painter: &egui::Painter, rect: Rect, frames: &VecDeque<Vec<Light>>) {
    let scale = rect.width().min(rect.height()) / 2.0;
    let to_screen = |l: &Light| rect.center() + vec2(l.x, -l.y) * scale;
    let n = frames.len();
    for (age, lights) in frames.iter().enumerate() {
        let keep = (age + 1) as f32 / n as f32;
        for l in lights {
            let c = Color32::from_rgb((l.color[0] * 255.0) as u8, (l.color[1] * 255.0) as u8, (l.color[2] * 255.0) as u8);
            let r = (l.radius * scale).max(1.0);
            painter.circle_filled(to_screen(l), r * 1.8, c.gamma_multiply(l.color[3] * keep * keep * 0.25));
            painter.circle_filled(to_screen(l), r * 0.7, c.gamma_multiply(l.color[3] * keep * keep));
        }
    }
}
