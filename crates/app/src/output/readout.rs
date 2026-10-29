//! The few characters of signal path shown in the now-playing display and
//! on the panel's card: the formats, a dot for the bit-perfect verdict, and
//! the words behind them (the hover, and what's wrong when something is).

use super::Outputs;
use crate::icons::{self, Icon};
use crate::theme::Tokens;
use audio::{BitPerfectStatus, PlayState, SignalPath};

/// The few characters of signal path shown in the now-playing display:
/// `24/96 → 16/48` plus a dot for the bit-perfect verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct Readout {
    pub from: String,
    /// What reaches the device, when that differs from the file.
    pub to: Option<String>,
    pub verdict: Verdict,
    /// The full line and verdict, for the tooltip.
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Normal mode: nothing to report.
    None,
    /// Bit-perfect and verified: a green dot.
    Verified,
    /// Bit-perfect asked for but not confirmed: a hollow ring.
    Requested,
    /// Bit-perfect impossible (device limit, transcode, downmix): the warning colour.
    Unavailable,
}

impl Outputs {
    /// The readout for what's playing, if anything is.
    pub fn readout(&self, status: &audio::Status) -> Option<Readout> {
        let p = self.signal.as_ref()?;
        if status.state() == PlayState::Stopped {
            return None;
        }
        let mut detail = format!("{}\n{}", p.summary(), hover_text(p));
        let underruns = status.underruns();
        if underruns > 0 {
            detail.push_str(&format!("\n{underruns} dropouts"));
        }
        Some(readout_of(p, detail))
    }
}

pub(super) fn readout_of(p: &SignalPath, detail: String) -> Readout {
    let (from, to) = match &p.transcoded {
        Some(t) => (t.clone(), None),
        None => {
            let from = audio::describe(p.source.bits_per_sample, p.source.sample_rate);
            let changed = p.resampled || matches!((p.device_bits, p.source.bits_per_sample), (Some(d), Some(s)) if d < s);
            (from, changed.then(|| audio::describe(p.device_bits, p.device_rate)))
        }
    };
    let verdict = match &p.bit_perfect {
        BitPerfectStatus::Off => Verdict::None,
        BitPerfectStatus::Verified => Verdict::Verified,
        BitPerfectStatus::Requested { .. } => Verdict::Requested,
        BitPerfectStatus::Unavailable { .. } => Verdict::Unavailable,
    };
    Readout { from, to, verdict, detail }
}

/// The readout in a small pill: the format(s) and, after them, the verdict dot.
/// Right-aligned at `right` (vertically centred on it). Returns the pill's
/// rect; with `paint` false it only measures.
pub fn paint_readout(ui: &egui::Ui, right: egui::Pos2, r: &Readout, size: f32, paint: bool) -> egui::Rect {
    const PAD_X: f32 = 6.0;
    const PAD_Y: f32 = 2.5;
    let t = Tokens::of(ui);
    let painter = ui.painter();
    let font = egui::FontId::proportional(size);
    let from = painter.layout_no_wrap(r.from.clone(), font.clone(), t.muted);
    let to = r.to.as_ref().map(|s| painter.layout_no_wrap(s.clone(), font.clone(), t.muted));
    let arrow_w = size * 0.85;
    let gap = 3.0;
    let dot_w = if r.verdict == Verdict::None { 0.0 } else { 4.0 + 5.0 };
    let text_w = from.size().x + to.as_ref().map_or(0.0, |g| gap + arrow_w + gap + g.size().x);
    let h = from.size().y;
    let rect = egui::Rect::from_min_max(
        egui::pos2(right.x - text_w - dot_w - 2.0 * PAD_X, right.y - h / 2.0 - PAD_Y),
        egui::pos2(right.x, right.y + h / 2.0 + PAD_Y),
    );
    if !paint {
        return rect;
    }
    painter.rect(rect, rect.height() / 2.0, t.field, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
    let mut x = rect.left() + PAD_X;
    let top = right.y - h / 2.0;
    painter.galley(egui::pos2(x, top), from.clone(), t.muted);
    x += from.size().x;
    if let Some(to) = to {
        x += gap;
        let a = egui::Rect::from_center_size(egui::pos2(x + arrow_w / 2.0, right.y), egui::vec2(arrow_w, size * 0.7));
        icons::paint(painter, a, Icon::ArrowRight, t.muted);
        x += arrow_w + gap;
        painter.galley(egui::pos2(x, top), to.clone(), t.muted);
        x += to.size().x;
    }
    let c = egui::pos2(x + 4.0 + 2.5, right.y);
    match r.verdict {
        Verdict::None => {}
        Verdict::Verified => {
            painter.circle_filled(c, 2.6, t.ok);
        }
        Verdict::Unavailable => {
            painter.circle_filled(c, 2.6, t.warn);
        }
        Verdict::Requested => {
            painter.circle_stroke(c, 2.3, egui::Stroke::new(1.0, t.muted));
        }
    }
    rect
}

/// What's wrong with what's playing, if anything: a device limit, or why
/// bit-perfect isn't (verified). Verified, or Normal mode, says nothing.
pub(super) fn warning(path: &SignalPath, t: &Tokens) -> Option<(String, egui::Color32)> {
    let warn = t.warn;
    match &path.bit_perfect {
        BitPerfectStatus::Unavailable { device_limit: true, .. } => {
            Some((format!("Device limit: plays at {}", audio::describe(path.device_bits, path.device_rate)), warn))
        }
        BitPerfectStatus::Unavailable { reason, .. } => Some((format!("Not bit-perfect: {reason}"), warn)),
        BitPerfectStatus::Requested { reason } => Some((format!("Not verified: {reason}"), t.muted)),
        _ => None,
    }
}

pub(super) fn hover_text(p: &SignalPath) -> String {
    let verdict = match &p.bit_perfect {
        BitPerfectStatus::Off => "Normal mode, with volume, ReplayGain and resampling".to_owned(),
        BitPerfectStatus::Verified => "Bit-perfect, with exclusive access, the file's sample rate and untouched samples".to_owned(),
        BitPerfectStatus::Requested { reason } => format!("Bit-perfect requested, not verified: {reason}"),
        BitPerfectStatus::Unavailable { reason, .. } => format!("Not bit-perfect: {reason}"),
    };
    format!("{}\n{verdict}", p.output)
}
