//! The seek line shared by the now-playing display, the Player and Mini: click
//! or drag to seek, the downloaded part shaded, a pulse ahead of the position
//! while buffering, and a knob while it's held or hovered.

use super::Action;
use super::view_ctx::ViewCtx;
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
        let wave = if s.wave_h > 0.0 && self.experimental().waveform {
            let id = self.player.queue.current.as_ref().map(|c| c.track.id.clone());
            id.and_then(|id| self.waveforms.bars(&id, ((bar.width() / 3.0) as usize).max(8)))
        } else {
            None
        };
        let line = if grabbed && wave.is_none() { bar.expand2(vec2(0.0, s.grow)) } else { bar };
        let x = line.left() + line.width() * dur.map_or(0.0, |d| (shown / d).clamp(0.0, 1.0) as f32);
        if let Some(bars) = wave {
            paint_wave(painter, line, &bars, x, s);
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
            painter.rect_filled(Rect::from_min_max(pos2(x, line.top()), pos2((x + s.pulse_len).min(line.right()), line.bottom())), s.radius, s.pulse.gamma_multiply(alpha));
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

/// The waveform in place of the line: a bar per value, mirrored about the
/// line's centre, played (left of `x`) in the fill color and the rest faint.
fn paint_wave(painter: &egui::Painter, line: Rect, bars: &[f32], x: f32, s: &SeekStyle) {
    let step = line.width() / bars.len().max(1) as f32;
    let half_w = (step * 0.6).max(1.0) / 2.0;
    let (cy, rest) = (line.center().y, s.fill.gamma_multiply(0.3));
    let mut mesh = egui::Mesh::default();
    for (i, v) in bars.iter().enumerate() {
        let cx = line.left() + step * (i as f32 + 0.5);
        // Quiet passages stay visible as a sliver; loud ones fill the height.
        let h = s.wave_h * (0.12 + 0.88 * v.powf(0.8));
        mesh.add_colored_rect(Rect::from_min_max(pos2(cx - half_w, cy - h), pos2(cx + half_w, cy + h)), if cx <= x { s.fill } else { rest });
    }
    painter.add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

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
