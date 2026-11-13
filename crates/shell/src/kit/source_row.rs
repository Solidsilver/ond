//! A source as Settings' Sources tab lists it: [`source_row`], its dot,
//! name and kind over its state and the app's line about it, and a menu of
//! what can be done to it (rename it in place, change its colour with the
//! [`source_color_picker`], sync it, sign out, remove it). The app says
//! which of these apply in a [`SourceRowMenu`], and gets back what was
//! chosen as a [`SourceRowEvent`]; asking before signing out or removing,
//! and doing either, are the app's.
//!
//! A source set outside the app (Ond's managed.json) is `locked`: the row
//! carries the app's words for why, as a managed setting's row does, and
//! its menu has no Rename or Remove.
//!
//! Which row is being renamed, and what's typed so far, live in egui's
//! memory as a text field's cursor does: they're the field's, not the
//! app's, and only the finished name goes anywhere.

use super::focus_ring;
use super::menus::{POPOVER_PAD, menu_row, popover_at, popover_rule};
use super::rows::{ROW_PAD, divider};
use super::sources::{Glyph, SourceInfo, dot_color, forget_spin, glyph, source_dot, spins};
use super::text::elide;
use crate::icons::{self, Icon};
use crate::theme::{self, Tokens};
use accounts::SourceColor;
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};

/// What a source's menu offers, beyond what every source has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceRowMenu<'a> {
    pub rename: bool,
    pub color: bool,
    /// The app's word for fetching what changed ("Sync now", "Rescan"), or
    /// `None` when it can't now.
    pub sync: Option<&'a str>,
    pub sign_out: bool,
    pub remove: bool,
}

/// A source's row in Settings.
#[derive(Debug, Clone, Copy)]
pub struct SourceRow<'a> {
    pub source: SourceInfo<'a>,
    pub menu: SourceRowMenu<'a>,
    /// Why it can't be changed here, in the app's words ("Set in your Nix
    /// config"), when it's set outside the app. The row says so, and its
    /// menu offers neither Rename nor Remove.
    pub locked: Option<&'a str>,
}

/// What was chosen on a [`source_row`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceRowEvent {
    /// The name was changed in place to this (trimmed, never empty, never
    /// the name it had).
    Rename(String),
    Color(SourceColor),
    Sync,
    /// Sign out…: the app asks first.
    SignOut,
    /// Remove…: the app asks first, and says what goes with it.
    Remove,
}

/// The row's height: two lines of text and the padding a setting row has.
const ROW_H: f32 = 34.0 + 2.0 * ROW_PAD;
/// Where the dot sits, from the row's left edge.
const DOT_X: f32 = 6.0;
/// Where the text starts.
const TEXT_X: f32 = 20.0;
/// The … at the row's right end.
const MORE: f32 = 24.0;
/// The menu's width: room for the colours in a row.
const MENU_W: f32 = 220.0;
/// A colour's swatch in the picker, and the room between two.
const SWATCH: f32 = 18.0;
const SWATCH_GAP: f32 = 6.0;

/// A rename in progress: what's typed, and whether the field has yet to
/// take the focus.
#[derive(Clone)]
struct Renaming {
    draft: String,
    fresh: bool,
}

/// One source in Settings: its dot, its name with its kind beside it, and
/// under them its state's glyph and line (the state itself when it needs
/// attention, in the warning colour). A click, or the … at its end, opens
/// its menu. Under a [`group`](super::group) it has the hairlines a setting
/// row has.
pub fn source_row(ui: &mut Ui, row: &SourceRow<'_>) -> Option<SourceRowEvent> {
    let t = Tokens::of(ui);
    let s = &row.source;
    let menu = SourceRowMenu { rename: row.menu.rename && row.locked.is_none(), remove: row.menu.remove && row.locked.is_none(), ..row.menu };
    let offers = menu.rename || menu.color || menu.sync.is_some() || menu.sign_out || menu.remove;
    divider(ui);
    let rename_id = egui::Id::unique("kit-source-rename").with(s.id);
    let mut renaming: Option<Renaming> = ui.data(|d| d.get_temp(rename_id));
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), if offers && renaming.is_none() { Sense::click() } else { Sense::hover() });
    let line = s.line();
    let mut picked = None;
    let text_x = rect.left() + TEXT_X;
    let room = rect.right() - text_x - if offers { MORE + 8.0 } else { 0.0 };
    let name_y = rect.top() + ROW_PAD + 1.0;
    if ui.is_rect_visible(rect) {
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
        if (resp.hovered() || open) && offers && renaming.is_none() {
            ui.painter().rect_filled(rect.expand2(vec2(4.0, -2.0)), 6.0, t.hover);
        }
        focus_ring(ui, &resp, rect.shrink2(vec2(-2.0, 2.0)), 6.0);
        let name_g = elide(ui, s.name, FontId::proportional(13.0), t.text, room);
        source_dot(ui, pos2(rect.left() + DOT_X, name_y + name_g.size().y / 2.0), s.color);
        let name_w = name_g.size().x;
        if renaming.is_none() {
            ui.painter().galley(pos2(text_x, name_y), name_g.clone(), t.text);
        }
        // The kind, then the lock's words, after the name in smaller type.
        let mut x = text_x + name_w + 8.0;
        for (text, ink) in [(Some(s.kind), t.muted), (row.locked, t.accent)] {
            let Some(text) = text.filter(|text| !text.is_empty() && renaming.is_none()) else { continue };
            let g = elide(ui, text, FontId::proportional(11.0), ink, (text_x + room - x).max(0.0));
            ui.painter().galley(pos2(x, name_y + (name_g.size().y - g.size().y) / 2.0 + 0.5), g.clone(), ink);
            x += g.size().x + 8.0;
        }
        let ink = if s.state.needs_attention() { t.warn } else { t.muted };
        let below = elide(ui, &line, FontId::proportional(11.5), ink, room - 12.0);
        let below_y = name_y + name_g.size().y + 3.0;
        glyph(ui, pos2(text_x + 4.0, below_y + below.size().y / 2.0), s.id, s.state, Glyph::Inline);
        ui.painter().galley(pos2(text_x + 12.0, below_y), below, ink);
        if !spins(s.state) {
            forget_spin(ui);
        }
        if offers && renaming.is_none() {
            let more = Rect::from_center_size(pos2(rect.right() - MORE / 2.0, rect.center().y), vec2(MORE, MORE));
            let ink = if resp.hovered() || open { t.text } else { t.muted };
            icons::paint(ui.painter(), Rect::from_center_size(more.center(), vec2(14.0, 14.0)), Icon::More, ink);
        }
    }
    if let Some(edit) = renaming.as_mut() {
        let field = Rect::from_min_max(pos2(text_x - 6.0, rect.top() + ROW_PAD - 3.0), pos2(rect.right(), rect.top() + ROW_PAD + 20.0));
        let done = rename_field(ui, field, s.name, edit);
        match done {
            Some(name) => {
                ui.data_mut(|d| d.remove::<Renaming>(rename_id));
                picked = name.map(SourceRowEvent::Rename);
            }
            None => {
                ui.data_mut(|d| d.insert_temp(rename_id, edit.clone()));
            }
        }
    }
    let name = match row.locked {
        Some(why) => format!("{}, {}, {line}, {why}", s.name, s.kind),
        None => format!("{}, {}, {line}", s.name, s.kind),
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(if offers { egui::Role::Button } else { egui::Role::Label }, true, &name));
    if !offers {
        return picked;
    }
    let chosen = popover_at(&resp, MENU_W, egui::RectAlign::BOTTOM_END, |ui| row_menu(ui, s.color, &menu)).flatten();
    match chosen {
        Some(Choice::Rename) => {
            ui.data_mut(|d| d.insert_temp(rename_id, Renaming { draft: s.name.to_owned(), fresh: true }));
        }
        Some(Choice::Event(event)) => picked = Some(event),
        None => {}
    }
    picked
}

/// What the menu chose: renaming happens in the row, the rest goes to the
/// app.
enum Choice {
    Rename,
    Event(SourceRowEvent),
}

/// The menu: Rename and the app's sync, the colours, then Sign out… and
/// Remove… apart from them.
fn row_menu(ui: &mut Ui, color: SourceColor, menu: &SourceRowMenu<'_>) -> Option<Choice> {
    let mut picked = None;
    if menu.rename && menu_row(ui, "Rename") {
        picked = Some(Choice::Rename);
    }
    if let Some(sync) = menu.sync
        && menu_row(ui, sync)
    {
        picked = Some(Choice::Event(SourceRowEvent::Sync));
    }
    if menu.color {
        if menu.rename || menu.sync.is_some() {
            popover_rule(ui, 6.0);
        }
        let t = Tokens::of(ui);
        ui.horizontal(|ui| {
            ui.add_space(POPOVER_PAD);
            ui.label(egui::RichText::new("Colour").size(11.0).color(t.muted));
        });
        ui.add_space(4.0);
        if let Some(c) = ui
            .horizontal(|ui| {
                ui.add_space(POPOVER_PAD);
                source_color_picker(ui, color)
            })
            .inner
        {
            ui.close();
            picked = Some(Choice::Event(SourceRowEvent::Color(c)));
        }
        ui.add_space(4.0);
    }
    if menu.sign_out || menu.remove {
        if menu.rename || menu.sync.is_some() || menu.color {
            popover_rule(ui, 6.0);
        }
        if menu.sign_out && menu_row(ui, "Sign out…") {
            picked = Some(Choice::Event(SourceRowEvent::SignOut));
        }
        if menu.remove && menu_row(ui, "Remove…") {
            picked = Some(Choice::Event(SourceRowEvent::Remove));
        }
    }
    picked
}

/// The eight source colours in a row, each a swatch named for a screen
/// reader, `current` ringed. The one clicked, if any.
pub fn source_color_picker(ui: &mut Ui, current: SourceColor) -> Option<SourceColor> {
    let t = Tokens::of(ui);
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = SWATCH_GAP;
        for color in SourceColor::ALL {
            let (rect, resp) = ui.allocate_exact_size(vec2(SWATCH, SWATCH), Sense::click());
            let on = color == current;
            if ui.is_rect_visible(rect) {
                let c = rect.center();
                ui.painter().circle_filled(c, SWATCH / 2.0 - 3.0, dot_color(ui, color));
                if on {
                    ui.painter().circle_stroke(c, SWATCH / 2.0 - 0.75, egui::Stroke::new(1.5, t.text));
                } else if resp.hovered() {
                    ui.painter().circle_stroke(c, SWATCH / 2.0 - 0.75, egui::Stroke::new(1.0, t.muted));
                }
                if resp.has_focus() {
                    ui.painter().circle_stroke(c, SWATCH / 2.0 + 1.0, egui::Stroke::new(1.5, t.accent));
                }
            }
            let name = theme::SOURCE_COLOR_NAMES[color.as_usize()];
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, name));
            if resp.on_hover_text(name).clicked() {
                picked = Some(color);
            }
        }
    });
    picked
}

/// The name as a field in `field`. `Some` once it's done: with the new name
/// to keep (Enter, or a click elsewhere), or `None` to keep the old one
/// (Esc, nothing changed, or nothing typed).
fn rename_field(ui: &mut Ui, field: Rect, old: &str, edit: &mut Renaming) -> Option<Option<String>> {
    let t = Tokens::of(ui);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field));
    let out = egui::TextEdit::singleline(&mut edit.draft)
        .hint_text("Its name")
        .font(FontId::proportional(13.0))
        .margin(vec2(5.0, 3.0))
        .desired_width(field.width())
        .background_color(t.field)
        .show(&mut child);
    let resp = out.response;
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Source name"));
    if edit.fresh {
        // The whole name selected, so typing replaces it.
        edit.fresh = false;
        resp.request_focus();
        let mut state = out.state;
        state.cursor.set_char_range(Some(egui::text::CCursorRange::select_all_str(&edit.draft)));
        state.store(ui.ctx(), resp.id);
        return None;
    }
    if !resp.lost_focus() {
        // A field that went out of view (Settings closed, the tab changed)
        // lost its focus unseen: it comes back as a row.
        return (!resp.has_focus()).then_some(None);
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return Some(None);
    }
    let name = edit.draft.trim();
    Some((!name.is_empty() && name != old).then(|| name.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{canvas, group, themed};
    use accounts::{AccountId, OfflineReason, SourceState};
    use egui_kittest::kittest::{NodeT, Queryable};

    fn id(n: u128) -> AccountId {
        format!("00000000-0000-4000-8000-{n:012x}").parse().unwrap()
    }

    const ONLINE: SourceState = SourceState::Online;
    const OUT: SourceState = SourceState::SignedOut;
    const AWAY: SourceState = SourceState::Offline(OfflineReason::Unreachable);

    fn info(n: u128, name: &'static str, color: u8, state: &'static SourceState, detail: &'static str) -> SourceInfo<'static> {
        SourceInfo { id: id(n), name, kind: "Audiobookshelf", icon: Icon::Server, color: SourceColor::new(color), state, detail: Some(detail) }
    }

    const ALL: SourceRowMenu<'static> = SourceRowMenu { rename: true, color: true, sync: Some("Sync now"), sign_out: true, remove: true };

    type Seen = Vec<SourceRowEvent>;

    fn harness(rows: Vec<SourceRow<'static>>) -> egui_kittest::Harness<'static, (bool, Seen)> {
        let mut h = themed(Vec::new(), move |ui, seen: &mut Seen| {
            ui.set_width(420.0);
            group(ui, "Sources", |ui| {
                for row in &rows {
                    seen.extend(source_row(ui, row));
                }
            });
        });
        h.set_size(vec2(480.0, 480.0));
        h
    }

    #[test]
    fn the_menu_hands_back_what_was_chosen() {
        let mut h = harness(vec![SourceRow { source: info(1, "Home", 0, &ONLINE, "books.example.com"), menu: ALL, locked: None }]);
        h.run();
        for (row, want) in [("Sync now", SourceRowEvent::Sync), ("Sign out…", SourceRowEvent::SignOut), ("Remove…", SourceRowEvent::Remove)] {
            h.get_by_label("Home, Audiobookshelf, books.example.com").click();
            h.run();
            h.get_by_label(row).click();
            h.run();
            assert_eq!(h.state().1.last(), Some(&want), "{row}");
        }
        h.get_by_label("Home, Audiobookshelf, books.example.com").click();
        h.run();
        let checked = |h: &egui_kittest::Harness<'_, _>, name: &str| h.get_by_label(name).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True);
        assert!(checked(&h, "Blue") && !checked(&h, "Teal"), "the colour it has is the one ringed");
        h.get_by_label("Teal").click();
        h.run();
        assert_eq!(h.state().1.last(), Some(&SourceRowEvent::Color(SourceColor::new(5))));
    }

    #[test]
    fn rename_edits_the_name_in_place_and_enter_keeps_it() {
        let mut h = harness(vec![SourceRow { source: info(1, "Home", 0, &ONLINE, "books.example.com"), menu: ALL, locked: None }]);
        h.run();
        h.get_by_label("Home, Audiobookshelf, books.example.com").click();
        h.run();
        h.get_by_label("Rename").click();
        h.run_steps(2);
        // Typing replaces the whole name, which starts selected.
        h.get_by_label("Source name").type_text("  Office  ");
        h.run();
        h.key_press(egui::Key::Enter);
        h.run_steps(2);
        assert_eq!(h.state().1, vec![SourceRowEvent::Rename("Office".into())]);
        assert!(h.query_by_label("Source name").is_none(), "the row is a row again");
    }

    #[test]
    fn esc_or_an_unchanged_name_keeps_the_old_one() {
        let mut h = harness(vec![SourceRow { source: info(1, "Home", 0, &ONLINE, "books.example.com"), menu: ALL, locked: None }]);
        h.run();
        for finish in [egui::Key::Escape, egui::Key::Enter] {
            h.get_by_label("Home, Audiobookshelf, books.example.com").click();
            h.run();
            h.get_by_label("Rename").click();
            h.run_steps(2);
            if finish == egui::Key::Escape {
                h.get_by_label("Source name").type_text("Elsewhere");
                h.run();
            }
            h.key_press(finish);
            h.run_steps(2);
            assert!(h.state().1.is_empty(), "{finish:?}");
        }
    }

    #[test]
    fn a_locked_source_says_why_and_offers_no_rename_or_remove() {
        let mut h = harness(vec![SourceRow { source: info(1, "Home", 0, &ONLINE, "books.example.com"), menu: ALL, locked: Some("Set in your Nix config") }]);
        h.run();
        h.get_by_label("Home, Audiobookshelf, books.example.com, Set in your Nix config").click();
        h.run();
        assert!(h.query_by_label("Rename").is_none() && h.query_by_label("Remove…").is_none());
        h.get_by_label("Sync now");
        h.get_by_label("Sign out…");
    }

    #[test]
    fn a_source_that_needs_attention_shows_its_state_in_place_of_the_detail() {
        let mut h = harness(vec![SourceRow { source: info(2, "Office", 3, &OUT, "office.example.com"), menu: SourceRowMenu::default(), locked: None }]);
        h.run();
        h.get_by_label("Office, Audiobookshelf, Signed out");
        assert!(h.query_by_role(egui::accesskit::Role::Button).is_none(), "nothing to offer, so nothing to click");
    }

    /// Renders the rows with one source, three, three with one signed out and
    /// one offline, a locked one, and the open menu, to look at:
    /// `cargo test -p shell --release snapshots -- --ignored` writes
    /// `target/snapshots/kit-source-row-*.png`.
    #[test]
    #[ignore = "writes pictures to look at"]
    fn snapshots() {
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
        std::fs::create_dir_all(&out).unwrap();
        let home = info(1, "Home server", 0, &ONLINE, "Synced 2 min ago");
        let office = info(2, "Office", 3, &ONLINE, "Synced yesterday");
        let folder = SourceInfo { kind: "Local folder", ..info(3, "Music folder", 5, &ONLINE, "12 840 songs") };
        let scenes: [(&str, Vec<SourceRow<'static>>); 4] = [
            ("one", vec![SourceRow { source: home, menu: ALL, locked: None }]),
            ("three", [home, office, folder].map(|source| SourceRow { source, menu: ALL, locked: None }).to_vec()),
            (
                "trouble",
                [home, SourceInfo { state: &OUT, ..office }, SourceInfo { state: &AWAY, ..folder }]
                    .map(|source| SourceRow { source, menu: ALL, locked: None })
                    .to_vec(),
            ),
            ("locked", vec![SourceRow { source: SourceInfo { kind: "Navidrome", ..home }, menu: ALL, locked: Some("Set in your Nix config") }]),
        ];
        for (theme, pref) in [("light", theme::ThemePref::Light), ("dark", theme::ThemePref::Dark)] {
            for (name, rows) in &scenes {
                let rows = rows.clone();
                let mut h = canvas(vec2(460.0, 70.0 + 52.0 * rows.len() as f32), pref, move |ui| pane(ui, &rows));
                h.run();
                h.render().unwrap().save(out.join(format!("kit-source-row-{name}-{theme}.png"))).unwrap();
            }
            // The menu, open on the second of three.
            let rows = scenes[1].1.clone();
            let mut h = canvas(vec2(460.0, 380.0), pref, move |ui| pane(ui, &rows));
            h.run();
            h.get_by_label("Office, Audiobookshelf, Synced yesterday").click();
            h.run_steps(4);
            h.event(egui::Event::PointerGone);
            h.run_steps(2);
            h.render().unwrap().save(out.join(format!("kit-source-row-menu-{theme}.png"))).unwrap();
        }
    }

    /// `rows` in a group on the Settings pane's ground.
    fn pane(ui: &mut Ui, rows: &[SourceRow<'_>]) {
        let ground = ui.visuals().window_fill;
        ui.painter().rect_filled(Rect::EVERYTHING, 0.0, ground);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.vertical(|ui| {
                ui.set_width(400.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                group(ui, "Sources", |ui| {
                    for row in rows {
                        source_row(ui, row);
                    }
                });
            });
        });
    }
}
