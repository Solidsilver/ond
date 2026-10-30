//! The signal path, from the format pill: where the song comes from, its
//! format, what changes on the way, and the output.

use super::playing::Origin;
use super::widgets::kv_text;
use crate::theme::Tokens;
use audio::{BitPerfectStatus, SignalPath};
use library::Track;

const WIDTH: f32 = 300.0;

/// The pill's hover: where from, then the terse signal line.
pub(super) fn hover(origin: Option<&Origin>, path: Option<&SignalPath>, detail: &str) -> String {
    let line = path.map_or_else(|| detail.lines().next().unwrap_or_default().to_owned(), SignalPath::summary);
    match origin {
        Some(o) => format!("{} · {line}", o.short()),
        None => line,
    }
}

/// Opens under `resp` when it's clicked.
pub(super) fn popover(resp: &egui::Response, origin: Option<&Origin>, track: &Track, path: Option<&SignalPath>, underruns: u64) {
    egui::Popup::menu(resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).width(WIDTH).show(|ui| {
        ui.set_width(WIDTH);
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.add_space(4.0);
        if let Some(o) = origin {
            kv_text(ui, "Source", &o.source_text(track));
        }
        let Some(p) = path else { return };
        kv_text(ui, "Format", &format(p, track));
        if let Some(c) = changes(p) {
            kv_text(ui, "Changes", &c);
        }
        kv_text(ui, "Output", &p.output);
        let t = Tokens::of(ui);
        let color = match p.bit_perfect {
            BitPerfectStatus::Verified => t.ok,
            BitPerfectStatus::Unavailable { .. } => t.warn,
            _ => t.muted,
        };
        super::widgets::kv_row(ui, "", |ui| {
            ui.add(egui::Label::new(egui::RichText::new(crate::output::verdict_text(p)).size(11.0).color(color)).wrap());
        });
        if underruns > 0 {
            kv_text(ui, "Dropouts", &crate::fmt::thousands(underruns as usize));
        }
    });
}

/// The file's format: bit depth and rate, channels, and the bit rate when
/// it isn't converted (a conversion's rate is in the Source row).
fn format(p: &SignalPath, track: &Track) -> String {
    let rate = audio::khz(p.source.sample_rate);
    let mut parts = vec![match p.source.bits_per_sample {
        Some(b) => format!("{b}-bit / {rate} kHz"),
        None => format!("{rate} kHz"),
    }];
    parts.push(match p.source.channels {
        1 => "Mono".to_owned(),
        2 => "Stereo".to_owned(),
        n => format!("{n} channels"),
    });
    if p.transcoded.is_none()
        && let Some(k) = track.bit_rate_kbps
    {
        parts.push(format!("{} kbps", crate::fmt::thousands(k as usize)));
    }
    parts.join(" · ")
}

/// What happens between the file and the device, if anything.
fn changes(p: &SignalPath) -> Option<String> {
    let mut parts = Vec::new();
    let narrower = matches!((p.device_bits, p.source.bits_per_sample), (Some(d), Some(s)) if d < s);
    if p.resampled || narrower {
        parts.push(format!("Plays at {}", audio::describe(p.device_bits, p.device_rate)));
    }
    if p.resampled {
        parts.push("resampled".to_owned());
    }
    if p.dithered {
        parts.push("dithered".to_owned());
    }
    parts.extend(p.channels.label());
    parts.extend(p.gain.label());
    (!parts.is_empty()).then(|| parts.join(" · "))
}
