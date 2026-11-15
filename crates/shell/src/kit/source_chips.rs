//! The filter chips over a library page in All sources: [`source_chips`],
//! All sources and then a chip per source (its dot and name), which narrow
//! the page to the sources lit. They read an [`accounts::Scope`] and hand
//! back what was clicked as a [`SourceChip`], for the app to apply with
//! [`Scope::toggle_chip`] or [`Scope::clear_chips`]. With one source, or
//! with the switcher on one source, there's nothing to narrow, and they
//! draw nothing.

use super::focus_ring;
use super::sources::{DOT_R, SourceInfo, source_dot};
use crate::theme::{Tokens, semibold};
use accounts::{AccountId, Scope};
use egui::{FontId, Sense, Ui, pos2, vec2};

/// What was clicked among the [`source_chips`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceChip {
    /// All sources: every chip off.
    All,
    /// A source's chip, to turn on or off.
    Toggle(AccountId),
}

/// A chip's height, and the room between two.
const CHIP_H: f32 = 26.0;
const GAP: f32 = 6.0;

/// All sources, lit while no chip narrows the page, then each source's
/// chip, lit while it's one of those shown; they wrap when the row is full.
/// A source that needs attention has its name in the warning colour, and
/// its state in its tooltip and its name for a screen reader.
pub fn source_chips(ui: &mut Ui, sources: &[SourceInfo<'_>], scope: &Scope) -> Option<SourceChip> {
    if sources.len() < 2 || !scope.is_all() {
        return None;
    }
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
        if chip(ui, None, "All sources", !scope.has_chips(), false).clicked() {
            picked = Some(SourceChip::All);
        }
        for s in sources {
            let trouble = s.state.needs_attention();
            let resp = chip(ui, Some(s), s.name, scope.chip_selected(s.id), trouble);
            let resp = if trouble { resp.on_hover_text(s.state.label()) } else { resp };
            if resp.clicked() {
                picked = Some(SourceChip::Toggle(s.id));
            }
        }
    });
    picked
}

/// One chip: the field's fill, the accent's tint when lit, a dot first for
/// a source.
fn chip(ui: &mut Ui, source: Option<&SourceInfo<'_>>, text: &str, on: bool, trouble: bool) -> egui::Response {
    let t = Tokens::of(ui);
    let font = if on { FontId::new(12.0, semibold()) } else { FontId::proportional(12.0) };
    let ink = if trouble {
        t.warn
    } else if on {
        t.accent
    } else {
        t.text
    };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, ink);
    let lead = if source.is_some() { 2.0 * DOT_R + 6.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + lead + 22.0, CHIP_H), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = match (on, resp.hovered()) {
            (true, hot) => t.accent.gamma_multiply(if hot { 0.24 } else { 0.16 }),
            (false, true) => t.selected,
            (false, false) => t.field,
        };
        ui.painter().rect_filled(rect, CHIP_H / 2.0, fill);
        focus_ring(ui, &resp, rect, CHIP_H / 2.0);
        let x = rect.left() + 11.0;
        if let Some(s) = source {
            source_dot(ui, pos2(x + DOT_R, rect.center().y), s.color);
        }
        ui.painter().galley(pos2(x + lead, rect.center().y - galley.size().y / 2.0), galley, ink);
    }
    let name = match source {
        Some(s) if s.state.needs_attention() => format!("{text}, {}", s.state.label()),
        _ => text.to_owned(),
    };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), on, &name));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons::Icon;
    use crate::kit::{canvas, themed};
    use crate::theme;
    use accounts::{OfflineReason, SourceColor, SourceState};
    use egui_kittest::kittest::{NodeT, Queryable};

    fn id(n: u128) -> AccountId {
        format!("00000000-0000-4000-8000-{n:012x}").parse().unwrap()
    }

    const ONLINE: SourceState = SourceState::Online;
    const OUT: SourceState = SourceState::SignedOut;
    const AWAY: SourceState = SourceState::Offline(OfflineReason::Unreachable);

    fn three(office: &'static SourceState) -> Vec<SourceInfo<'static>> {
        let info = |n: u128, name: &'static str, color: u8, state: &'static SourceState| SourceInfo {
            id: id(n),
            name,
            kind: "Server",
            icon: Icon::Disc,
            color: SourceColor::new(color),
            state,
            detail: None,
        };
        vec![info(1, "Home server", 0, &ONLINE), info(2, "Office", 3, office), info(3, "Music folder", 5, &AWAY)]
    }

    /// The chips over `scope`, which the harness narrows as the app would.
    fn harness(sources: Vec<SourceInfo<'static>>, scope: Scope) -> egui_kittest::Harness<'static, (bool, Scope)> {
        themed(scope, move |ui, scope: &mut Scope| match source_chips(ui, &sources, scope) {
            Some(SourceChip::All) => scope.clear_chips(),
            Some(SourceChip::Toggle(id)) => scope.toggle_chip(id),
            None => {}
        })
    }

    fn lit(h: &egui_kittest::Harness<'_, (bool, Scope)>, name: &str) -> bool {
        h.get_by_label(name).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
    }

    #[test]
    fn a_chip_narrows_all_sources_and_all_sources_clears_them() {
        let mut h = harness(three(&ONLINE), Scope::all());
        h.run();
        assert!(lit(&h, "All sources") && !lit(&h, "Office"));
        h.get_by_label("Office").click();
        h.run();
        assert!(h.state().1.chip_selected(id(2)) && !h.state().1.includes(id(1)));
        assert!(lit(&h, "Office") && !lit(&h, "All sources"));
        h.get_by_label("Home server").click();
        h.run();
        assert_eq!(h.state().1.chips().count(), 2);
        h.get_by_label("All sources").click();
        h.run();
        assert!(!h.state().1.has_chips() && h.state().1.is_all());
    }

    #[test]
    fn one_source_or_the_switcher_on_one_draws_no_chips() {
        let mut h = harness(three(&ONLINE)[..1].to_vec(), Scope::all());
        h.run();
        assert!(h.query_by_label("All sources").is_none());
        let mut h = harness(three(&ONLINE), Scope::only(id(1)));
        h.run();
        assert!(h.query_by_label("All sources").is_none());
    }

    #[test]
    fn a_source_that_needs_attention_says_so_on_its_chip() {
        let mut h = harness(three(&OUT), Scope::all());
        h.run();
        h.get_by_label("Office, Signed out");
        h.get_by_label("Music folder");
    }

    /// Renders the chips with one source (nothing), three, three with one
    /// chip lit, and three with one signed out and two lit, to look at:
    /// `cargo test -p shell --release snapshots -- --ignored` writes
    /// `target/snapshots/kit-source-chips-*.png`.
    #[test]
    #[ignore = "writes pictures to look at"]
    fn snapshots() {
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
        std::fs::create_dir_all(&out).unwrap();
        let mut narrowed = Scope::all();
        narrowed.toggle_chip(id(2));
        let mut two = narrowed.clone();
        two.toggle_chip(id(3));
        let scenes: [(&str, Vec<SourceInfo<'static>>, Scope); 4] = [
            ("one", three(&ONLINE)[..1].to_vec(), Scope::all()),
            ("three", three(&ONLINE), Scope::all()),
            ("three-one", three(&ONLINE), narrowed),
            ("trouble", three(&OUT), two),
        ];
        for (theme, pref) in [("light", theme::ThemePref::Light), ("dark", theme::ThemePref::Dark)] {
            for (name, sources, scope) in &scenes {
                let (sources, scope) = (sources.clone(), scope.clone());
                let mut h = canvas(vec2(440.0, 52.0), pref, move |ui| {
                    let ground = Tokens::of(ui).bg;
                    ui.painter().rect_filled(egui::Rect::EVERYTHING, 0.0, ground);
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        source_chips(ui, &sources, &scope);
                    });
                });
                h.run();
                h.render().unwrap().save(out.join(format!("kit-source-chips-{name}-{theme}.png"))).unwrap();
            }
        }
    }
}
