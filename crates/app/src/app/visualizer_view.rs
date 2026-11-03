//! The visualizer (experimental, ⌘T): the whole window given over to light
//! moving with the music. Three to choose from (see `visuals::visualizer`):
//! ← and → switch, and the choice is kept (Settings › Experimental can also
//! shuffle them, a different one each track). F or a double-click goes full
//! screen; Esc leaves. The playing track's cover and names show in the corner
//! for a few seconds when it starts. Drawn on the GPU (see `visuals::gpu`),
//! or with the painter where there's no wgpu renderer to hand.

use super::widgets::elide_job;
use super::{Action, OndApp, WindowAction};
use crate::visuals::visualizer::{Dot, Input, Kind, Pick, Picture, Rng, Scene, caption_alpha, name_alpha};
use crate::visuals::{EVERY_FRAME, gpu};
use egui::{Align2, Color32, FontId, Galley, Key, Modifiers, Rect, Ui, pos2, vec2};
use std::collections::VecDeque;
use std::sync::Arc;

/// Earlier frames the painter version keeps, for its trails.
const TRAIL: usize = 8;
/// How long the key hint shows after opening.
const HINT_SECS: f64 = 4.0;
/// How long a new scene takes to fade in.
const FADE_IN: f64 = 0.5;

/// The open visualizer.
pub struct VisualizerView {
    scene: Scene,
    /// Seeds new scenes and picks shuffled ones.
    rng: Rng,
    /// Frame clock when it opened, and of the last frame drawn.
    opened: f64,
    last: f64,
    /// When the scene showing was switched to (its name shows for a moment).
    switched: f64,
    /// The queue item whose cover and names are shown, and since when.
    track: Option<u64>,
    track_since: f64,
    /// The cover whose colors the scene has, and those colors.
    cover: Option<String>,
    colors: Vec<Color32>,
    /// It made the window full screen (and so undoes that on leaving).
    fullscreen: bool,
    /// The next GPU frame starts from black.
    fresh: bool,
    /// Every frame advances the scene by exactly this many seconds, whatever
    /// the clock says (for snapshots).
    pub fixed_step: Option<f32>,
    /// The painter version's recent frames (each light and its color), newest last.
    trails: VecDeque<Vec<(Dot, [f32; 3])>>,
}

impl VisualizerView {
    fn new(pick: Pick, now: f64, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let kind = pick.kind().unwrap_or_else(|| Kind::ALL[rng.below(Kind::ALL.len())]);
        let scene = Scene::new(kind, rng.next_u64());
        Self {
            scene,
            rng,
            opened: now,
            last: now,
            switched: now,
            track: None,
            track_since: now,
            cover: None,
            colors: Vec::new(),
            fullscreen: false,
            fresh: true,
            fixed_step: None,
            trails: VecDeque::new(),
        }
    }

    /// Which visualizer is showing.
    pub fn kind(&self) -> Kind {
        self.scene.kind()
    }

    /// The scene, to set up a snapshot.
    #[cfg(test)]
    pub fn scene(&mut self) -> &mut Scene {
        &mut self.scene
    }

    /// Starts again with `seed` (for snapshots): the same scene from the start.
    #[cfg(test)]
    pub fn reseed(&mut self, seed: u64) {
        self.rng = Rng::new(seed);
        self.show(self.kind(), self.last);
    }

    /// Switches to `kind`, from black.
    fn show(&mut self, kind: Kind, now: f64) {
        self.scene = Scene::new(kind, self.rng.next_u64());
        self.scene.set_colors(&self.colors);
        self.switched = now;
        self.fresh = true;
        self.trails.clear();
    }

    /// Another visualizer than the one showing, at random.
    fn shuffled(&mut self) -> Kind {
        self.kind().step(1 + self.rng.below(Kind::ALL.len() - 1) as i32)
    }

    /// Keeps up with what's playing: a new track (`uid`) shows its cover and
    /// names, and shuffled, another visualizer; a new `cover` lends the scene
    /// its colors once its `look` is worked out.
    fn follow(&mut self, uid: Option<u64>, cover: Option<&str>, look: Option<&crate::art::Look>, shuffle: bool, now: f64) {
        if uid != self.track {
            if shuffle && self.track.is_some() {
                let kind = self.shuffled();
                self.show(kind, now);
            }
            self.track = uid;
            self.track_since = now;
        }
        if cover != self.cover.as_deref() && (look.is_some() || cover.is_none()) {
            self.colors = look.and_then(|l| l.palette.as_ref()).map_or_else(Vec::new, |p| p.colors.clone());
            self.scene.set_colors(&self.colors);
            self.cover = cover.map(str::to_owned);
        }
    }

    /// Moves the scene on to `now` (`half`: the view's half extents) and
    /// returns what to draw, fading in after a switch.
    fn step(&mut self, now: f64, half: [f32; 2], audio: Option<&crate::visuals::spectrum::Frame>) -> Picture {
        let dt = self.fixed_step.unwrap_or((now - self.last) as f32);
        self.last = now;
        let mut picture = self.scene.step(&Input { dt, half, audio });
        picture.bright = ((now - self.switched) / FADE_IN).clamp(0.0, 1.0) as f32;
        picture
    }

    /// Draws `picture` over `rect`: on the GPU when there's a `gpu` target,
    /// else with the painter (keeping its last frames for the trails).
    fn draw(&mut self, painter: &egui::Painter, rect: Rect, gpu: Option<gpu::Target>, half: [f32; 2], picture: Picture) {
        match gpu {
            Some(target) => {
                let ppp = painter.pixels_per_point();
                let size_px = [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32];
                painter.add(gpu::shape(rect, gpu::VisualizerCallback { target, size_px, half, picture, clear: self.fresh }));
                self.fresh = false;
            }
            None => {
                self.trails.push_back(picture.dots.iter().chain(&picture.front).map(|d| (*d, picture.shown(d))).collect());
                while self.trails.len() > TRAIL {
                    self.trails.pop_front();
                }
                paint_trails(painter, rect, half, &picture, &self.trails);
            }
        }
    }
}

impl OndApp {
    /// Opens the visualizer, or closes it if it's open (⌘T). Only while that
    /// experiment is on.
    pub(super) fn toggle_visualizer(&mut self) {
        if self.ui.visualizer.is_some() {
            self.close_visualizer();
        } else if self.prefs_now().experimental.visualizer {
            let now = self.ctx.input(|i| i.time);
            self.ui.visualizer = Some(VisualizerView::new(self.prefs_now().experimental.visualizer_pick, now, now.to_bits()));
            self.ctx.request_repaint();
        }
    }

    pub(super) fn close_visualizer(&mut self) {
        let Some(v) = self.ui.visualizer.take() else { return };
        if v.fullscreen {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        // Closed after this frame was drawn (a menu, a command): draw the
        // window again now, not at the next input.
        self.ctx.request_repaint();
    }

    /// Esc was pressed in the open visualizer (and is taken). Asked before
    /// the frame is drawn, so the window it closes to draws in this frame.
    pub(super) fn visualizer_escaped(&self, ctx: &egui::Context) -> bool {
        self.ui.visualizer.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
    }

    /// Switches to the next visualizer (`by` 1) or the previous one (-1), and
    /// keeps that as the choice (unless they're shuffled).
    pub(super) fn switch_visualizer(&mut self, by: i32) {
        let now = self.ctx.input(|i| i.time);
        let Some(v) = self.ui.visualizer.as_mut() else { return };
        let kind = v.kind().step(by);
        v.show(kind, now);
        if self.prefs_now().experimental.visualizer_pick != Pick::Shuffle {
            self.prefs.edit(|s| s.experimental.visualizer_pick = Pick::of(kind));
        }
    }

    pub(super) fn visualizer_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let (f, left, right) = ctx.input_mut(|i| {
            (i.consume_key(Modifiers::NONE, Key::F), i.consume_key(Modifiers::NONE, Key::ArrowLeft), i.consume_key(Modifiers::NONE, Key::ArrowRight))
        });
        if left != right {
            self.switch_visualizer(if right { 1 } else { -1 });
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
                self.ui.live.want(EVERY_FRAME);
            }
            let current = self.player.queue.current.as_ref().map(|c| (c.uid, &c.track));
            let album = current.and_then(|(_, t)| self.data.album(&t.album_id)).map(|a| a.name.as_str());
            let cover = current.and_then(|(_, t)| self.data.cover_for(t));
            let look = cover.and_then(|c| self.art.look(c));
            let shuffle = self.ui.prefs.experimental.visualizer_pick == Pick::Shuffle;
            let Some(v) = self.ui.visualizer.as_mut() else { return };
            v.follow(current.map(|(u, _)| u), cover, look.as_deref(), shuffle, now);
            let half = if rect.width() >= rect.height() { [rect.width() / rect.height().max(1.0), 1.0] } else { [1.0, rect.height() / rect.width().max(1.0)] };
            let picture = v.step(now, half, audio.as_ref());
            if picture.bright < 1.0 {
                ctx.request_repaint();
            }
            v.draw(ui.painter(), rect, self.gpu, half, picture);
            let (track_since, switched, opened, kind) = (v.track_since, v.switched, v.opened, v.kind());
            if let Some((_, track)) = current {
                let caption = caption_alpha(now - track_since);
                if caption > 0.0 {
                    let art = cover.and_then(|c| self.art.texture(c, 96.0));
                    paint_caption(ui.painter(), rect, art, &track.title, track.artist.as_deref(), album, caption);
                    if caption < 1.0 {
                        ctx.request_repaint();
                    }
                }
            }
            paint_titles(ui.painter(), rect, now, switched.max(opened), opened, kind);
            body.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, format!("Visualizer: {}", kind.label())));
        });
    }

    /// Whether `action` takes the window elsewhere, so the visualizer gives way.
    pub(super) fn leaves_visualizer(action: &Action) -> bool {
        matches!(
            action,
            Action::Go(_)
                | Action::OpenPalette
                | Action::Window(WindowAction::SetMode(_) | WindowAction::ToggleMini | WindowAction::TogglePlayer)
                | Action::OpenSettings(_)
                | Action::Overlay(_)
                | Action::Lyrics
                | Action::Info
        )
    }
}

/// `galley` in white with a soft dark edge, so it reads over bright light,
/// placed by `align` at `at`; `alpha` 0..=255 (clamped).
fn shadowed(painter: &egui::Painter, at: egui::Pos2, align: Align2, galley: Arc<Galley>, alpha: f32) {
    let alpha = alpha.clamp(0.0, 255.0);
    let min = align.anchor_size(at, galley.size()).min;
    painter.galley_with_override_text_color(min + vec2(0.0, 1.0), galley.clone(), Color32::from_black_alpha((alpha * 0.6) as u8));
    painter.galley_with_override_text_color(min, galley, Color32::from_white_alpha(alpha as u8));
}

/// [`shadowed`] text on one line, whole.
fn shadowed_text(painter: &egui::Painter, at: egui::Pos2, align: Align2, text: &str, font: FontId, alpha: f32) {
    shadowed(painter, at, align, painter.layout_no_wrap(text.to_owned(), font, Color32::WHITE), alpha);
}

/// The visualizer's name for a moment after it's shown (`shown`: when), and
/// the key hint for the first seconds after it opened; each asks for the
/// frames its fade needs.
fn paint_titles(painter: &egui::Painter, rect: Rect, now: f64, shown: f64, opened: f64, kind: Kind) {
    let ctx = painter.ctx();
    let name = name_alpha(now - shown);
    if name > 0.0 {
        let at = pos2(rect.center().x, rect.top() + 72.0);
        shadowed_text(painter, at, Align2::CENTER_CENTER, kind.label(), FontId::new(22.0, crate::theme::semibold()), 230.0 * name);
        ctx.request_repaint();
    }
    let since_open = now - opened;
    if since_open < HINT_SECS + 1.0 {
        let a = (1.0 - (since_open - HINT_SECS).max(0.0)) as f32;
        let hint = crate::keys::shown("← → to switch · F for full screen · Esc to leave").into_owned();
        shadowed_text(painter, pos2(rect.center().x, rect.bottom() - 28.0), Align2::CENTER_CENTER, &hint, FontId::proportional(12.0), 140.0 * a);
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((HINT_SECS + 1.0 - since_open).clamp(0.02, 1.0)));
    }
}

/// The cover, title, artist and album in the bottom-left corner, at `alpha` (0..=1).
fn paint_caption(painter: &egui::Painter, rect: Rect, art: Option<egui::TextureHandle>, title: &str, artist: Option<&str>, album: Option<&str>, alpha: f32) {
    const SIZE: f32 = 72.0;
    const MARGIN: f32 = 32.0;
    let tile = Rect::from_min_size(pos2(rect.left() + MARGIN, rect.bottom() - MARGIN - SIZE), vec2(SIZE, SIZE));
    let tint = Color32::from_white_alpha((255.0 * alpha) as u8);
    match art {
        Some(t) => {
            painter.add(egui::epaint::RectShape::filled(tile, 4.0, tint).with_texture(t.id(), crate::art::crop_uv(1.0)));
        }
        None => {
            painter.rect_filled(tile, 4.0, Color32::from_white_alpha((30.0 * alpha) as u8));
        }
    }
    let x = tile.right() + 14.0;
    let lines = [
        (Some(title), FontId::new(16.0, crate::theme::semibold()), 240.0),
        (artist, FontId::proportional(14.0), 200.0),
        (album, FontId::proportional(14.0), 170.0),
    ];
    let shown: Vec<_> = lines.into_iter().filter_map(|(t, f, a)| t.map(|t| (t, f, a))).collect();
    let line_h = 20.0;
    let mut y = tile.center().y - line_h * shown.len() as f32 / 2.0 + line_h / 2.0;
    let room = (rect.right() - x - MARGIN).max(40.0);
    for (text, font, a) in shown {
        shadowed(painter, pos2(x, y), Align2::LEFT_CENTER, painter.layout_job(elide_job(text, font, Color32::WHITE, room)), a * alpha);
        y += line_h;
    }
}

/// The painter version: the backdrop, then each light as a soft dot or a
/// line, its last few frames fading behind it.
fn paint_trails(painter: &egui::Painter, rect: Rect, half: [f32; 2], picture: &Picture, frames: &VecDeque<Vec<(Dot, [f32; 3])>>) {
    if let crate::visuals::visualizer::Look::Lit { centre, .. } = picture.look {
        painter.rect_filled(rect, 0.0, color(centre, 1.0));
    }
    let scale = rect.width() / (2.0 * half[0]);
    let to_screen = |p: [f32; 2]| rect.center() + vec2(p[0], -p[1]) * scale;
    let n = frames.len();
    for (age, lights) in frames.iter().enumerate() {
        let keep = (age + 1) as f32 / n as f32;
        let keep = keep * keep * picture.bright;
        for (d, c) in lights {
            let r = (d.radius * scale).max(0.75);
            let a = (d.color[3] * keep).min(1.0);
            if d.axis != [0.0; 2] {
                let ends = [to_screen([d.at[0] - d.axis[0], d.at[1] - d.axis[1]]), to_screen([d.at[0] + d.axis[0], d.at[1] + d.axis[1]])];
                painter.line_segment(ends, egui::Stroke::new(r * 1.4, color(*c, a)));
            } else {
                painter.circle_filled(to_screen(d.at), r * 1.6, color(*c, a * 0.25));
                painter.circle_filled(to_screen(d.at), r * 0.7, color(*c, a));
            }
        }
    }
    for d in &picture.shades {
        painter.circle_filled(to_screen(d.at), d.radius * scale, Color32::BLACK);
    }
}

fn color(c: [f32; 3], alpha: f32) -> Color32 {
    let [r, g, b] = c.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
    Color32::from_rgb(r, g, b).gamma_multiply(alpha.clamp(0.0, 1.0))
}
