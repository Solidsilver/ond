//! How far something is on its way to this computer: the [`DownloadMark`]
//! the lists, tiles and sidebar paint with [`download_mark`] (a check in a
//! ring once it's here, a filling ring while it comes, a warning when it
//! couldn't), and the [`download_button`] a page offers beside Play.
//!
//! The kit doesn't know what a download is made of; each app says which mark
//! its pins and tracks get. Painted only, apart from the button: the row or
//! tile a mark sits on says the same in its name for a screen reader.

use super::focus_ring;
use crate::icons::{self, Icon};
use crate::theme::{Tokens, semibold};
use egui::{Align2, FontId, Rect, Response, Sense, Ui, vec2};

/// How far along something is on its way to this computer, as the lists,
/// tiles and sidebar mark it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DownloadMark {
    Done,
    /// 0..=1; 0 while waiting its turn.
    Progress(f32),
    Failed,
}

impl DownloadMark {
    /// Something made of `total` files, `done` of them here and `failed`
    /// given up on: done when all are here, failed once nothing is still
    /// coming and every file left gave up, else `fraction` of the way.
    pub fn of_counts(done: usize, failed: usize, total: usize, downloading: bool, fraction: f32) -> Self {
        if done == total {
            DownloadMark::Done
        } else if failed > 0 && !downloading && done + failed == total {
            DownloadMark::Failed
        } else {
            DownloadMark::Progress(fraction)
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            DownloadMark::Done => "Downloaded",
            DownloadMark::Progress(f) if f > 0.0 => "Downloading",
            DownloadMark::Progress(_) => "Waiting to download",
            DownloadMark::Failed => "Download failed",
        }
    }
}

/// The small glyph for a [`DownloadMark`], centred at `center`, about `size`
/// across.
pub fn download_mark(ui: &Ui, center: egui::Pos2, size: f32, m: DownloadMark) {
    let t = Tokens::of(ui);
    let r = size / 2.0;
    match m {
        // A check in a ring: "downloaded", where the Download icon's arrow reads as the action.
        DownloadMark::Done => {
            let stroke = egui::Stroke::new(1.2, t.muted);
            ui.painter().circle_stroke(center, r * 0.88, stroke);
            let p = |x: f32, y: f32| center + vec2(x, y) * r;
            ui.painter().add(egui::Shape::line(vec![p(-0.4, 0.0), p(-0.1, 0.32), p(0.42, -0.28)], stroke));
        }
        DownloadMark::Progress(f) => icons::progress_ring(ui.painter(), center, r * 0.8, f, t.accent, t.hairline),
        DownloadMark::Failed => {
            ui.painter().circle_stroke(center, r * 0.8, egui::Stroke::new(1.3, t.warn));
            ui.painter().text(center, Align2::CENTER_CENTER, "!", FontId::new(size * 0.7, semibold()), t.warn);
        }
    }
}

/// A page's download button, shaped as [`secondary`](super::secondary) so it
/// sits beside Play: the Download arrow and an accent label while there's
/// nothing here, else the item's [`DownloadMark`] and what it says ("42%",
/// "Downloaded", "Download failed" in the warning colour). Its menu, if it
/// opens one, is the caller's.
pub fn download_button(ui: &mut Ui, mark: Option<DownloadMark>, text: &str) -> Response {
    let t = Tokens::of(ui);
    let ink = match mark {
        None => t.accent,
        Some(DownloadMark::Failed) => t.warn,
        Some(_) => t.text,
    };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(13.0, semibold()), ink);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 18.0 + 28.0, 30.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
        ui.painter().rect_filled(rect, 6.0, if resp.hovered() || open { t.selected } else { t.field });
        focus_ring(ui, &resp, rect, 6.0);
        let glyph = egui::pos2(rect.left() + 14.0 + 5.0, rect.center().y);
        match mark {
            None => icons::paint(ui.painter(), Rect::from_center_size(glyph, vec2(11.0, 11.0)), Icon::Download, ink),
            Some(m) => download_mark(ui, glyph, 13.0, m),
        }
        ui.painter().galley(egui::pos2(rect.left() + 14.0 + 18.0, rect.center().y - galley.size().y / 2.0), galley, ink);
    }
    let (label, enabled) = (text.to_owned(), ui.is_enabled());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &label));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::themed;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn the_download_button_is_named_by_what_it_says() {
        let mut h = themed(0, |ui, clicks: &mut usize| {
            if download_button(ui, Some(DownloadMark::Progress(0.4)), "Downloading 40%").clicked() {
                *clicks += 1;
            }
        });
        h.run();
        h.get_by_label("Downloading 40%").click();
        h.run();
        assert_eq!(h.state().1, 1);
    }

    #[test]
    fn a_mark_says_waiting_until_something_has_arrived() {
        assert_eq!(DownloadMark::Progress(0.0).hint(), "Waiting to download");
        assert_eq!(DownloadMark::Progress(0.1).hint(), "Downloading");
        assert_eq!(DownloadMark::Done.hint(), "Downloaded");
    }
}
