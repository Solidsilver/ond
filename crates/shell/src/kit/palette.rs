//! The command palette both apps search with (⌘K): a sheet anchored near
//! the top of a dimmed window, the query field with its "esc" hint, the
//! results in titled groups, each a picture over two lines with the
//! selected one's keys at its right, and a strip of key hints along the
//! foot. And the [`search_field`] in a sidebar that opens it.
//!
//! The app supplies the rows ([`PaletteRows`]): it searches when the query
//! changes, says what groups there are, and paints each row's picture. Keys
//! other than ↑ and ↓ (⏎ and whatever goes with it) are the app's, read
//! before [`palette`] is drawn, since what they do is the app's.

use crate::icons::{self, Icon};
use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Sense, pos2, vec2};

use super::elide;

/// The sheet's width, in a window with room for it.
const WIDTH: f32 = 640.0;
const ROW_H: f32 = 44.0;
const QUERY_H: f32 = 52.0;
const HINTS_H: f32 = 34.0;
/// A group's title line.
const GROUP_H: f32 = 24.0;
/// Room left either side in a window narrower than the sheet.
const MARGIN: f32 = 16.0;

/// What a palette keeps between frames: what's typed and which row is
/// selected.
#[derive(Debug, Default, Clone)]
pub struct PaletteState {
    pub query: String,
    /// The row selected, counted down through the groups.
    pub selected: usize,
    /// The selection moved by keyboard: scroll it into view once.
    follow: bool,
    /// The query was replaced: the cursor goes to its end.
    to_end: bool,
}

impl PaletteState {
    /// Puts `query` in the field, as if typed, with the cursor at its end
    /// and the first row selected. The app searches for it itself.
    pub fn replace_query(&mut self, query: String) {
        self.query = query;
        self.selected = 0;
        self.follow = true;
        self.to_end = true;
    }
}

/// What a palette says: the field's placeholder, the keys along its foot,
/// and the line shown when a query finds nothing.
pub struct PaletteText<'a> {
    pub placeholder: &'a str,
    /// Written with the Mac glyphs; shown as the platform writes them.
    pub footer: &'a str,
    pub no_matches: &'a str,
}

/// One result's two lines, and the keys shown at its right while it's
/// selected (Mac glyphs, as the footer).
pub struct PaletteRow {
    pub title: String,
    pub detail: String,
    pub keys: &'static str,
}

/// The results an app shows in its palette.
pub trait PaletteRows {
    /// Searches again: the query changed.
    fn search(&mut self, query: &str);
    /// Each group's title and how many rows it has, in the order drawn.
    /// Groups with none are skipped.
    fn groups(&self) -> Vec<(&'static str, usize)>;
    /// Paints the `n`th row's picture (counted through the groups) in
    /// `tile`, and says what the row reads.
    fn row(&mut self, ui: &mut egui::Ui, tile: Rect, n: usize) -> PaletteRow;
}

/// What happened in the palette this frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PaletteOutcome {
    /// A row was clicked: which, and whether ⌘ was held. It's selected.
    pub clicked: Option<(usize, bool)>,
    /// Esc, or a click outside: the palette should close.
    pub close: bool,
}

/// The palette over the window, under `id`. ↑ and ↓ move the selection;
/// typing searches through `rows`.
pub fn palette(ctx: &egui::Context, id: &str, text: &PaletteText, state: &mut PaletteState, rows: &mut impl PaletteRows) -> PaletteOutcome {
    use egui::{Key, Modifiers};
    let (up, down) = ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::ArrowUp), i.consume_key(Modifiers::NONE, Key::ArrowDown)));
    let n: usize = rows.groups().iter().map(|(_, n)| n).sum();
    if n > 0 {
        if down {
            state.selected = (state.selected + 1) % n;
            state.follow = true;
        }
        if up {
            state.selected = (state.selected + n - 1) % n;
            state.follow = true;
        }
    }

    // Anchored near the top so the field stays put while results grow and
    // shrink, as in Spotlight.
    let screen = ctx.content_rect();
    let top = (screen.height() * 0.12).clamp(24.0, 120.0);
    let list_max = (screen.height() * 0.72 - QUERY_H - HINTS_H).max(ROW_H * 3.0);
    // A narrow window keeps a margin either side.
    let width = WIDTH.min(screen.width() - 2.0 * MARGIN).max(240.0);
    let id = egui::Id::unique(id);
    let area = egui::Modal::default_area(id).anchor(egui::Align2::CENTER_TOP, vec2(0.0, top));
    let modal = egui::Modal::new(id).area(area).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(0).corner_radius(12)).show(ctx, |ui| {
        let t = Tokens::of(ui);
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 0.0;
        query_field(ui, &t, text.placeholder, width, state, rows);
        ui.painter().hline(ui.min_rect().x_range(), ui.min_rect().bottom(), egui::Stroke::new(1.0, t.hairline));
        let groups = rows.groups();
        let clicked = if groups.iter().all(|(_, n)| *n == 0) {
            if !state.query.trim().is_empty() {
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    ui.label(egui::RichText::new(text.no_matches).color(t.muted));
                });
                ui.add_space(14.0);
            }
            None
        } else {
            results_list(ui, &t, width, &groups, rows, state, list_max)
        };
        hints_bar(ui, &t, width, text.footer);
        clicked
    });
    if let Some((n, _)) = modal.inner {
        // A click acts on what was clicked, not on the keyboard selection.
        state.selected = n;
    }
    PaletteOutcome { clicked: modal.inner, close: modal.should_close() }
}

/// The search field, which keeps the keyboard; searches as it changes.
fn query_field(ui: &mut egui::Ui, t: &Tokens, placeholder: &str, width: f32, state: &mut PaletteState, rows: &mut impl PaletteRows) {
    ui.horizontal(|ui| {
        ui.set_height(QUERY_H);
        ui.add_space(16.0);
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        icons::paint(ui.painter(), r, Icon::Search, t.muted);
        ui.add_space(4.0);
        let mut out = egui::TextEdit::singleline(&mut state.query)
            .hint_text(placeholder)
            .font(FontId::proportional(17.0))
            .frame(egui::Frame::NONE)
            .desired_width(width - 90.0)
            .show(ui);
        if std::mem::take(&mut state.to_end) {
            let end = egui::text::CCursor::new(state.query.chars().count());
            out.state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
            out.state.store(ui.ctx(), out.response.id);
        }
        let edit = out.response;
        edit.request_focus();
        if edit.changed() {
            rows.search(&state.query);
            state.selected = 0;
            state.follow = true;
        }
        ui.label(egui::RichText::new("esc").size(11.0).color(t.muted));
    });
}

/// The results, grouped, in a scroll area tall enough for all of them up
/// to `list_max`. Returns a clicked row, and whether ⌘ was held.
fn results_list(
    ui: &mut egui::Ui,
    t: &Tokens,
    width: f32,
    groups: &[(&str, usize)],
    rows: &mut impl PaletteRows,
    state: &mut PaletteState,
    list_max: f32,
) -> Option<(usize, bool)> {
    let follow = std::mem::take(&mut state.follow);
    // Tall enough for everything when it fits, so groups aren't
    // hidden behind a scroll; scrolls only past ~70% of the window.
    let shown = groups.iter().filter(|(_, n)| *n > 0);
    let content_h = 6.0 + shown.map(|(_, n)| GROUP_H + ROW_H * *n as f32 + 6.0).sum::<f32>();
    let mut clicked = None;
    egui::ScrollArea::vertical().min_scrolled_height(content_h.min(list_max)).max_height(list_max).show(ui, |ui| {
        ui.add_space(6.0);
        let mut row = 0;
        for &(title, n) in groups {
            if n == 0 {
                continue;
            }
            let (head, _) = ui.allocate_exact_size(vec2(width, GROUP_H), Sense::hover());
            ui.painter().text(pos2(head.left() + 18.0, head.bottom() - 4.0), egui::Align2::LEFT_BOTTOM, title, FontId::new(11.0, semibold()), t.muted);
            for _ in 0..n {
                let selected = row == state.selected;
                let resp = result_row(ui, t, width, rows, row, selected);
                if selected && follow {
                    resp.scroll_to_me(None);
                }
                if resp.clicked() {
                    clicked = Some((row, ui.input(|i| i.modifiers.command)));
                }
                row += 1;
            }
            ui.add_space(6.0);
        }
    });
    clicked
}

/// One result: its picture, its name over what it is, and its keys while
/// it's selected.
fn result_row(ui: &mut egui::Ui, t: &Tokens, width: f32, rows: &mut impl PaletteRows, n: usize, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
    let inner = rect.shrink2(vec2(8.0, 1.0));
    if selected {
        ui.painter().rect_filled(inner, 8.0, t.selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(inner, 8.0, t.hover);
    }
    let tile = Rect::from_center_size(pos2(inner.left() + 22.0, inner.center().y), vec2(30.0, 30.0));
    let PaletteRow { title, detail, keys } = rows.row(ui, tile, n);
    let x = tile.right() + 12.0;
    let hint = crate::keys::shown(keys);
    let hint_w = if selected { ui.painter().layout_no_wrap(hint.to_string(), FontId::proportional(11.0), t.muted).size().x + 16.0 } else { 0.0 };
    let w = inner.right() - x - hint_w - 12.0;
    let g = elide(ui, &title, FontId::proportional(13.0), t.text, w);
    ui.painter().galley(pos2(x, inner.center().y - g.size().y), g, t.text);
    let g = elide(ui, &detail, FontId::proportional(12.0), t.muted, w);
    ui.painter().galley(pos2(x, inner.center().y + 1.0), g, t.muted);
    if selected {
        ui.painter().text(pos2(inner.right() - 12.0, inner.center().y), egui::Align2::RIGHT_CENTER, hint, FontId::proportional(11.0), t.muted);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListBoxOption, true, selected, &title));
    resp
}

/// A row's picture when it's a name rather than something with a cover (a
/// person, a series): `icon` in a quiet circle.
pub fn palette_icon_tile(ui: &egui::Ui, tile: Rect, icon: Icon) {
    let t = Tokens::of(ui);
    ui.painter().circle_filled(tile.center(), 15.0, t.field);
    icons::paint(ui.painter(), tile.shrink(8.0), icon, t.muted);
}

/// The strip of key hints along the bottom.
fn hints_bar(ui: &mut egui::Ui, t: &Tokens, width: f32, footer: &str) {
    let (bar, _) = ui.allocate_exact_size(vec2(width, HINTS_H), Sense::hover());
    ui.painter().rect_filled(bar, egui::CornerRadius { nw: 0, ne: 0, sw: 12, se: 12 }, t.sidebar);
    ui.painter().hline(bar.x_range(), bar.top(), egui::Stroke::new(1.0, t.hairline));
    let footer = crate::keys::shown(footer);
    let at = pos2(bar.left() + 18.0, bar.center().y);
    let room = width - 36.0;
    let whole = ui.painter().layout_no_wrap(footer.to_string(), FontId::proportional(11.0), t.muted);
    if whole.size().x <= room {
        ui.painter().text(at, egui::Align2::LEFT_CENTER, footer, FontId::proportional(11.0), t.muted);
    } else {
        // In a narrow window the hints that don't fit trail off.
        let g = elide(ui, &footer, FontId::proportional(11.0), t.muted, room);
        ui.painter().galley(pos2(at.x, at.y - g.size().y / 2.0), g, t.muted);
    }
}

/// The field at the top of a sidebar that opens the palette: "Search", with
/// the shortcut at its right. The palette is the real search; this only
/// says where it is.
pub fn search_field(ui: &mut egui::Ui, shortcut: &str) -> egui::Response {
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    ui.painter().rect_filled(rect, 6.0, if resp.hovered() { t.selected } else { t.field });
    icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(11.0, 11.0)), Icon::Search, t.muted);
    ui.painter().text(pos2(rect.left() + 26.0, rect.center().y), egui::Align2::LEFT_CENTER, "Search", FontId::proportional(13.0), t.muted);
    ui.painter().text(pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, crate::keys::shown(shortcut), FontId::proportional(11.0), t.muted);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, crate::keys::shown(&format!("Search ({shortcut})"))));
    resp
}
