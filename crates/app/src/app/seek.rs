//! The seek line shared by the now-playing display, the Player and Mini: click
//! or drag to seek, the downloaded part shaded, a pulse ahead of the position
//! while buffering, and a knob while it's held or hovered.

use super::Action;
use super::view_ctx::ViewCtx;
use crate::visuals::waveform::Bars;
use audio::PlayState;
use egui::{Color32, Rect, Response, Sense, Ui, pos2, vec2};

/// How one seek line looks.
pub(super) struct SeekStyle {
    /// The empty line.
    pub track: Color32,
    /// The part already downloaded.
    pub fetched: Color32,
    /// The part already played.
    pub fill: Color32,
    /// The buffering pulse ahead of the position.
    pub pulse: Color32,
    pub pulse_len: f32,
    pub radius: f32,
    /// How much taller the line gets while held or hovered.
    pub grow: f32,
    pub knob: Color32,
    pub knob_r: f32,
    /// How far the waveform reaches above and below the line, when the
    /// waveform seek bar is on (0: this line never shows one).
    pub wave_h: f32,
}

impl ViewCtx<'_> {
    /// Draws `bar` and handles seeking from `strip` (a taller area around it,
    /// easier to grab). Returns the position it shows (the drag preview while
    /// dragging) and the strip's response.
    pub fn seek_line(&mut self, ui: &mut Ui, bar: Rect, strip: Rect, id: &str, s: &SeekStyle) -> (f64, Response) {
        let state = self.engine.status.state();
        let dur = self.duration_secs().filter(|d| *d > 0.0);
        let seekable = dur.is_some() && matches!(state, PlayState::Playing | PlayState::Paused | PlayState::Buffering);
        let resp = ui.interact(strip, ui.scope_id().with(id), if seekable { Sense::click_and_drag() } else { Sense::hover() });
        let frac_at = |x: f32| f64::from(((x - bar.left()) / bar.width()).clamp(0.0, 1.0));
        if let Some(d) = dur {
            if (resp.dragged() || resp.drag_started())
                && let Some(p) = resp.interact_pointer_pos()
            {
                self.ui.seek_preview = Some(frac_at(p.x) * d);
            }
            if resp.drag_stopped() || resp.clicked() {
                if let Some(to) = resp.interact_pointer_pos().map(|p| frac_at(p.x) * d).or(self.ui.seek_preview) {
                    self.actions.push(Action::Seek(to));
                }
                self.ui.seek_preview = None;
            }
        }
        // A drag that ended somewhere else (the window changed mode under it)
        // leaves no preview behind.
        if self.ui.seek_preview.is_some() && !resp.dragged() && ui.ctx().dragged_id().is_none() {
            self.ui.seek_preview = None;
        }
        let painter = ui.painter();
        let grabbed = resp.hovered() || self.ui.seek_preview.is_some();
        let shown = self.ui.seek_preview.unwrap_or_else(|| self.position_secs());
        let look = self.ui.prefs.waveform_bars;
        let wave = if s.wave_h > 0.0 && self.ui.prefs.waveform {
            let grid = WaveGrid::along(bar, painter.pixels_per_point(), look);
            let id = self.player.queue.current.as_ref().map(|c| c.track.id.clone());
            id.and_then(|id| self.waveforms.bars(&id, grid.n)).map(|bars| (grid, bars))
        } else {
            None
        };
        let line = if grabbed && wave.is_none() { bar.expand2(vec2(0.0, s.grow)) } else { bar };
        let x = line.left() + line.width() * dur.map_or(0.0, |d| (shown / d).clamp(0.0, 1.0) as f32);
        if let Some((grid, bars)) = wave {
            paint_wave(painter, line, &grid, &bars, x, s.wave_h, s.fill);
        } else {
            painter.rect_filled(line, s.radius, s.track);
            if let Some(f) = self.engine.status.fetched_fraction() {
                painter.rect_filled(Rect::from_min_max(line.min, pos2(line.left() + line.width() * f.clamp(0.0, 1.0), line.bottom())), s.radius, s.fetched);
            }
            painter.rect_filled(Rect::from_min_max(line.min, pos2(x, line.bottom())), s.radius, s.fill);
        }
        if state == PlayState::Buffering || self.player.pending_seek.is_some() {
            let tm = ui.input(|i| i.time) as f32;
            let alpha = 0.35 + 0.35 * (tm * std::f32::consts::TAU / 1.2).sin();
            painter.rect_filled(
                Rect::from_min_max(pos2(x, line.top()), pos2((x + s.pulse_len).min(line.right()), line.bottom())),
                s.radius,
                s.pulse.gamma_multiply(alpha),
            );
        }
        if grabbed && seekable {
            painter.circle_filled(pos2(x, line.center().y), s.knob_r, s.knob);
        }
        // A screen reader hears where it is, in seconds, out of how long
        // ("2:21 of 6:23").
        resp.widget_info(|| egui::WidgetInfo::slider(seekable, shown, "Position"));
        if let Some(d) = dur {
            ui.ctx().accesskit_node_builder(resp.id, |node| {
                node.set_min_numeric_value(0.0);
                node.set_max_numeric_value(d);
                node.set_value(format!("{} of {}", crate::fmt::clock(shown), crate::fmt::clock(d)));
            });
        }
        (shown, resp)
    }

    /// The elapsed and remaining times for a line showing `shown`: "2:21",
    /// "−4:02" (empty while the length isn't known).
    pub fn seek_times(&self, shown: f64) -> (String, String) {
        seek_times(shown, self.duration_secs())
    }
}

/// Elapsed and remaining in whole seconds that add up to the length as the
/// lists show it (whole seconds too): "2:21" and "−4:02" for 6:23.
fn seek_times(shown: f64, duration: Option<f64>) -> (String, String) {
    let elapsed = shown.max(0.0).floor();
    let remaining = duration.filter(|d| *d > 0.0).map(|d| format!("−{}", crate::fmt::clock((d.floor() - elapsed).max(0.0)))).unwrap_or_default();
    (crate::fmt::clock(elapsed), remaining)
}

/// Where the waveform's bars go along a line: every bar the same whole number
/// of pixels wide with the same whole-pixel gap between, the run centred, so
/// none comes out a pixel wider or further apart than its neighbours; and
/// their shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WaveGrid {
    pub n: usize,
    /// The first bar's left edge, the distance from one bar to the next, and
    /// a bar's width, in points on the pixel grid.
    left: f32,
    step: f32,
    width: f32,
    /// Rounded bars rather than square ones.
    rounded: bool,
}

impl WaveGrid {
    pub fn along(line: Rect, ppp: f32, look: Bars) -> Self {
        let px = |pt: f32| (pt * ppp).round().max(1.0);
        let (w, gap) = (px(look.width_pt()), px(look.gap_pt()));
        let span = (line.width() * ppp).floor().max(w);
        let n = ((span + gap) / (w + gap)).floor().max(1.0);
        let used = n * (w + gap) - gap;
        let left = (line.left() * ppp + (span - used) / 2.0).round();
        Self { n: n as usize, left: left / ppp, step: (w + gap) / ppp, width: w / ppp, rounded: look.rounded }
    }
}

/// The waveform in place of the line: a bar per value, laid out by `grid`,
/// reaching up to `wave_h` above and below the line's centre, played (left of
/// `x`) in `fill` and the rest faint. Square bars stay crisp on the pixel
/// grid; rounded ones are anti-aliased.
pub(super) fn paint_wave(painter: &egui::Painter, line: Rect, grid: &WaveGrid, bars: &[f32], x: f32, wave_h: f32, fill: Color32) {
    let ppp = painter.pixels_per_point();
    let snap = |pt: f32| (pt * ppp).round() / ppp;
    let (cy, rest) = (snap(line.center().y), fill.gamma_multiply(0.3));
    let mut mesh = egui::Mesh::default();
    let mut shapes = Vec::new();
    for (i, v) in bars.iter().enumerate() {
        let left = grid.left + grid.step * i as f32;
        // Quiet passages stay visible as a sliver; loud ones fill the height.
        // Never shorter than the bar is wide, so a rounded one stays a pill.
        let h = snap(wave_h * (0.12 + 0.88 * v.powf(0.8))).max(grid.width / 2.0);
        let rect = Rect::from_min_max(pos2(left, cy - h), pos2(left + grid.width, cy + h));
        let color = if rect.center().x <= x { fill } else { rest };
        if grid.rounded {
            shapes.push(egui::Shape::rect_filled(rect, grid.width / 2.0, color));
        } else {
            mesh.add_colored_rect(rect, color);
        }
    }
    if grid.rounded {
        painter.extend(shapes);
    } else {
        painter.add(egui::Shape::mesh(mesh));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_bars_sit_on_whole_pixels_evenly() {
        for (ppp, left, width) in [(1.0, 10.0, 300.0), (2.0, 10.25, 301.3), (1.5, 33.7, 512.9), (2.0, 0.0, 3.0)] {
            let line = Rect::from_min_size(pos2(left, 0.0), vec2(width, 4.0));
            let g = WaveGrid::along(line, ppp, Bars::default());
            assert!(g.n >= 1);
            // Every edge on a pixel boundary: the same width and gap each time.
            for i in 0..g.n {
                let l = (g.left + g.step * i as f32) * ppp;
                let r = l + g.width * ppp;
                assert!((l - l.round()).abs() < 1e-3 && (r - r.round()).abs() < 1e-3, "bar {i} at {l}..{r} ({ppp}x)");
            }
            // Inside the line, centred to within a pixel.
            let end = g.left + g.step * (g.n - 1) as f32 + g.width;
            assert!(g.left >= line.left() - 0.5 / ppp && end <= line.right() + 0.5 / ppp, "{g:?} in {line:?}");
            assert!(((g.left - line.left()) - (line.right() - end)).abs() <= 1.0 / ppp + 1e-3, "{g:?} in {line:?}");
        }
        // The same look at any scale: 2 pt bars with 1 pt gaps by default.
        let line = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 4.0));
        let g = WaveGrid::along(line, 2.0, Bars::default());
        assert_eq!((g.n, g.step, g.width), (100, 3.0, 2.0));
        // Thicker and further apart, and out-of-range values kept in range.
        let g = WaveGrid::along(line, 2.0, Bars { rounded: true, width: 8, gap: 4 });
        assert_eq!((g.n, g.step, g.width), (50, 6.0, 4.0));
        let wild = WaveGrid::along(line, 2.0, Bars { rounded: false, width: 0, gap: 200 });
        assert_eq!((wild.step, wild.width), (4.0, 1.0));
    }

    #[test]
    fn elapsed_and_remaining_add_up_to_the_length() {
        // 6:23 long, part-way through a second: 2:21 + 4:02.
        assert_eq!(seek_times(141.6, Some(383.0)), ("2:21".to_owned(), "−4:02".to_owned()));
        assert_eq!(seek_times(141.0, Some(383.0)), ("2:21".to_owned(), "−4:02".to_owned()));
        // The engine's length can carry a fraction the lists don't show.
        assert_eq!(seek_times(0.4, Some(383.47)), ("0:00".to_owned(), "−6:23".to_owned()));
        assert_eq!(seek_times(383.2, Some(383.47)), ("6:23".to_owned(), "−0:00".to_owned()));
        assert_eq!(seek_times(12.0, None), ("0:12".to_owned(), String::new()));
    }
}
