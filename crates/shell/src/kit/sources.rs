//! Sources, as both apps show them: [`source_dot`], the small mark in a
//! source's colour (and [`source_dots`], several stacked, for All sources),
//! and [`source_switcher`], the sidebar's foot, which shows what's in scope
//! and opens the menu that changes it.
//!
//! The kit never learns what kinds of source exist. The app describes each
//! one in a [`SourceInfo`] (its name, its colour, its [`SourceState`], a
//! line of its own words), says which optional rows the menu offers, and
//! draws its own rows at the menu's foot. The switcher hands back what was
//! picked as a [`SourcePick`], which the app turns into its own actions.
//!
//! All sources narrowed by the filter chips to some of them shows as those
//! sources: their names ("Home server, Music folder", or "2 of 3 sources"
//! where that doesn't fit) over their dots. An app that can show some
//! sources together asks for [`SourceSwitcher::toggles`], a box on each
//! source's row in the menu, ticked as its chip is lit, that adds it to the
//! set or takes it out; the row itself still picks that source alone.
//!
//! With one source nothing mentions sources: the switcher is the account
//! row each app drew before there could be several, in the app's
//! [`SwitcherLook`] (Ond's one line beside its buttons, Rede's two lines
//! under an initial), and its menu holds only the app's rows unless the app
//! offers Add or Manage. Dots appear once there's a second source.

use super::focus_ring;
use super::menus::{POPOVER_PAD, menu_row, popover_at, popover_rule};
use super::text::elide;
use crate::icons::{self, Icon};
use crate::theme::{self, Tokens, semibold};
use accounts::{AccountId, OfflineReason, Scope, SourceColor, SourceState};
use egui::{Color32, FontId, Pos2, Rect, Response, Sense, Ui, pos2, vec2};
use std::time::Duration;

/// A source as the app describes it to the kit.
#[derive(Debug, Clone, Copy)]
pub struct SourceInfo<'a> {
    pub id: AccountId,
    /// What the user calls it: "Home server", "Music folder".
    pub name: &'a str,
    /// What sort of source it is: "Navidrome", "Local folder".
    pub kind: &'a str,
    pub icon: Icon,
    pub color: SourceColor,
    pub state: &'a SourceState,
    /// The line under the name, in the app's words: "Synced 2 min ago", a
    /// server's address, how far a sync has got. Without one the state's
    /// own label is shown; a source that needs attention shows its state
    /// whatever this says.
    pub detail: Option<&'a str>,
}

impl SourceInfo<'_> {
    /// What's said about the source under its name: its state when it needs
    /// attention, else the app's detail, else its state.
    pub fn line(&self) -> String {
        match self.detail {
            Some(detail) if !self.state.needs_attention() => detail.to_owned(),
            _ => self.state.label(),
        }
    }
}

/// What was picked in the switcher's menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourcePick<R> {
    /// One source, to show alone.
    Source(AccountId),
    /// Every source together.
    All,
    /// A source's box in the menu: add it to the sources shown together,
    /// or take it out (the filter chips' set). The menu stays open.
    Toggle(AccountId),
    /// Add source…
    Add,
    /// Manage sources…
    Manage,
    /// What the app's own rows returned.
    App(R),
}

/// How the switcher's row is laid out, which is how the app drew its
/// account row before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitcherLook {
    /// One line, 28 points tall, beside the footer's buttons: the state's
    /// glyph leads, then the source's line.
    Line,
    /// Two lines, 40 points tall: the name's initial in a disc, then the
    /// name over the state's glyph and the source's line.
    Card,
}

/// Where the switcher is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SwitcherPlace {
    /// At the foot of the full sidebar, this wide.
    Foot { width: f32 },
    /// In the folded rail: a button with the mark alone.
    Rail,
}

/// What the switcher shows and offers.
#[derive(Debug, Clone, Copy)]
pub struct SourceSwitcher<'a> {
    /// Every source, in the order the menu lists them.
    pub sources: &'a [SourceInfo<'a>],
    /// What the app is showing. A scope on a source that isn't listed reads
    /// as All sources.
    pub scope: &'a Scope,
    pub look: SwitcherLook,
    /// Offer Add source… in the menu.
    pub add: bool,
    /// Offer Manage sources… in the menu.
    pub manage: bool,
    /// Give each source in the menu a box that adds it to or takes it from
    /// the sources shown together, for an app that can show some of them
    /// at once. Without it the menu checks the one in scope.
    pub toggles: bool,
    /// What hovering the switcher says, in the app's words, when it has
    /// more to say than the row shows.
    pub tooltip: Option<&'a str>,
}

/// The menu's width: Ond's server popover's, which Rede's account menu took.
pub const SWITCHER_MENU_W: f32 = 250.0;
/// A source dot's radius.
pub const DOT_R: f32 = 3.5;
/// How far each stacked dot sits to the right of the one before.
const DOT_STEP: f32 = 4.5;
/// The most dots a stack shows; more sources than this add nothing new to
/// see.
const MAX_DOTS: usize = 4;
const LINE_H: f32 = 28.0;
const CARD_H: f32 = 40.0;
/// The initial's disc in the [`SwitcherLook::Card`] row.
const AVATAR: f32 = 24.0;
/// A source's row in the menu: its name over its line.
const MENU_SOURCE_H: f32 = 36.0;
/// Where a menu row's dot sits, from the row's left edge: past the check's
/// gutter.
const MENU_DOT_X: f32 = POPOVER_PAD + 20.0 + DOT_R;
/// Where a menu row's text starts: past the widest stack of dots.
const MENU_TEXT_X: f32 = POPOVER_PAD + 20.0 + 24.0;

/// A spinner's frame interval.
const SPIN_FRAME: Duration = Duration::from_millis(50);
/// How long a spinner turns before it goes still: a sync that stalls, or a
/// connect that hangs, mustn't hold the window at 20 frames a second.
const SPIN_SECS: f64 = 30.0;

/// The small mark in a source's colour, centred on `center`. The app draws
/// it only when there's more than one source.
pub fn source_dot(ui: &Ui, center: Pos2, color: SourceColor) {
    ui.painter().circle_filled(center, DOT_R, dot_color(ui, color));
}

/// The room a dot takes in front of a line of text: the dot, and the gap
/// between it and the first letter.
pub const DOT_LEAD: f32 = 2.0 * DOT_R + 5.0;

/// A source's dot leading a line of text that starts at `x` and is centred
/// on `cy` (an artist under a cover, a row's second line). Returns where the
/// text starts.
pub fn source_dot_lead(ui: &Ui, x: f32, cy: f32, color: SourceColor) -> f32 {
    source_dot(ui, pos2(x + DOT_R, cy), color);
    x + DOT_LEAD
}

/// The room a dot takes after a line of text: the gap after the last
/// letter, and the dot.
pub const DOT_TRAIL: f32 = 5.0 + 2.0 * DOT_R;

/// A source's dot after a line of text that ends at `x` and is centred on
/// `cy`, so the line still starts where the title over it does.
pub fn source_dot_after(ui: &Ui, x: f32, cy: f32, color: SourceColor) {
    source_dot(ui, pos2(x + DOT_TRAIL - DOT_R, cy), color);
}

/// One line of text (its words, font and ink) from `at` (its top left),
/// cut to `width`, then the
/// dots of the sources it comes from when there are any: one source's dot,
/// or several stacked on `ground` (a playlist holding songs from more than
/// one). The text gives way to the dots, which always show. Returns how
/// wide the text and its dots are.
pub fn text_then_dots(ui: &Ui, at: Pos2, (text, font, ink): (&str, FontId, Color32), width: f32, dots: &[SourceColor], ground: Color32) -> f32 {
    let trail = if dots.is_empty() { 0.0 } else { DOT_TRAIL - 2.0 * DOT_R + dots_width(dots.len()) };
    let g = elide(ui, text, font, ink, (width - trail).max(0.0));
    let (text_w, cy) = (g.size().x, at.y + g.size().y / 2.0);
    ui.painter().galley(at, g, ink);
    match dots {
        [] => {}
        [one] => source_dot_after(ui, at.x + text_w, cy, *one),
        many => source_dots(ui, pos2(at.x + text_w + DOT_TRAIL - 2.0 * DOT_R + dots_width(many.len()) / 2.0, cy), many, ground),
    }
    text_w + trail
}

/// A source named inline, its dot then its name in `font` and `ink`, laid
/// out as one item (the line of facts under an album's title, a heading
/// over one source's part of a page). Not a control: it only says where
/// something comes from.
pub fn source_label(ui: &mut Ui, color: SourceColor, name: &str, font: FontId, ink: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(name.to_owned(), font, ink);
    let size = vec2(DOT_LEAD + galley.size().x, galley.size().y);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let x = source_dot_lead(ui, rect.left(), rect.center().y, color);
        ui.painter().galley(pos2(x, rect.top()), galley, ink);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, name));
    resp
}

/// Several sources' dots overlapping in a row centred on `center`, the
/// first on top, each cut out of the one under it by a ring of `ground`
/// (what they're drawn on). All sources' mark.
pub fn source_dots(ui: &Ui, center: Pos2, colors: &[SourceColor], ground: Color32) {
    let shown = &colors[..colors.len().min(MAX_DOTS)];
    let left = center.x - dots_width(shown.len()) / 2.0 + DOT_R;
    for (i, &color) in shown.iter().enumerate().rev() {
        let c = pos2(left + i as f32 * DOT_STEP, center.y);
        if i + 1 < shown.len() {
            ui.painter().circle_filled(c, DOT_R + 1.2, ground);
        }
        source_dot(ui, c, color);
    }
}

/// How wide [`source_dots`] draws `n` dots.
fn dots_width(n: usize) -> f32 {
    let n = n.clamp(1, MAX_DOTS);
    2.0 * DOT_R + (n - 1) as f32 * DOT_STEP
}

pub(super) fn dot_color(ui: &Ui, color: SourceColor) -> Color32 {
    theme::source_color(color, ui.visuals().dark_mode)
}

/// The sidebar's foot: what's in scope, and a menu to change it. The menu
/// lists each source (when there's more than one) and All sources, then Add
/// source… and Manage sources… when the switcher offers them, then the
/// app's own rows, which `rows` draws and whose answer comes back as
/// [`SourcePick::App`]. Draws nothing without a source.
pub fn source_switcher<R>(ui: &mut Ui, switcher: &SourceSwitcher<'_>, place: SwitcherPlace, rows: impl FnOnce(&mut Ui) -> Option<R>) -> Option<SourcePick<R>> {
    if switcher.sources.is_empty() {
        return None;
    }
    let shown = Shown::of(switcher);
    let label = shown.label(switcher);
    let resp = match place {
        SwitcherPlace::Foot { width } => foot(ui, switcher, &shown, width),
        SwitcherPlace::Rail => rail(ui, switcher, &shown),
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    let tip = match (place, switcher.tooltip) {
        (SwitcherPlace::Rail, Some(tip)) => format!("{label}\n{tip}"),
        (SwitcherPlace::Rail, None) => label,
        (SwitcherPlace::Foot { .. }, Some(tip)) => tip.to_owned(),
        (SwitcherPlace::Foot { .. }, None) => label,
    };
    let resp = resp.on_hover_text(tip);
    // Up from the foot of the window, where there's room for it.
    popover_at(&resp, SWITCHER_MENU_W, egui::RectAlign::TOP_START, |ui| menu(ui, switcher, &shown, rows)).flatten()
}

/// What the switcher's row stands for.
enum Shown<'s, 'a> {
    /// The only source there is.
    Single(&'s SourceInfo<'a>),
    /// One of several, in scope.
    One(&'s SourceInfo<'a>),
    /// Some of several, narrowed by the filter chips: the ones included,
    /// fewer than all.
    Some(Vec<&'s SourceInfo<'a>>),
    /// All of several.
    All,
}

impl<'s, 'a> Shown<'s, 'a> {
    fn of(switcher: &'s SourceSwitcher<'a>) -> Self {
        match switcher.sources {
            [only] => Shown::Single(only),
            many => match switcher.scope.single().and_then(|id| many.iter().find(|s| s.id == id)) {
                Some(one) => Shown::One(one),
                None => {
                    let included: Vec<_> = many.iter().filter(|s| switcher.scope.includes(s.id)).collect();
                    if switcher.scope.has_chips() && !included.is_empty() && included.len() < many.len() { Shown::Some(included) } else { Shown::All }
                }
            },
        }
    }

    /// The row's name: the source's, the included ones' names, or All
    /// sources.
    fn title(&self) -> String {
        match self {
            Shown::Single(s) | Shown::One(s) => s.name.to_owned(),
            Shown::Some(some) => names(some),
            Shown::All => "All sources".to_owned(),
        }
    }

    /// The row's name when [`title`](Self::title) doesn't fit: how many
    /// of the sources are included.
    fn short_title(&self, sources: &[SourceInfo<'_>]) -> Option<String> {
        match self {
            Shown::Some(some) => Some(format!("{} of {} sources", some.len(), sources.len())),
            _ => None,
        }
    }

    /// The row's second line (the [`SwitcherLook::Card`]) or its only one
    /// (the [`SwitcherLook::Line`]).
    fn line(&self, switcher: &SourceSwitcher<'_>) -> String {
        match (self, switcher.look) {
            (Shown::Single(s), _) | (Shown::One(s), SwitcherLook::Card) => s.line(),
            (Shown::One(s), SwitcherLook::Line) if matches!(s.state, SourceState::Online) => s.name.to_owned(),
            (Shown::One(s), SwitcherLook::Line) => format!("{} · {}", s.name, s.line()),
            (Shown::All, SwitcherLook::Card) => all_line(switcher.sources.iter()),
            // Trouble is flagged at the row's end; there's no room to say it too.
            (Shown::All, SwitcherLook::Line) => "All sources".to_owned(),
            (Shown::Some(some), SwitcherLook::Line) => names(some),
            (Shown::Some(some), SwitcherLook::Card) if troubled(some.iter().copied()).next().is_some() => all_line(some.iter().copied()),
            (Shown::Some(_), SwitcherLook::Card) => self.short_title(switcher.sources).unwrap_or_default(),
        }
    }

    /// What a screen reader calls the switcher, whichever its look: the
    /// name, then its line unless that only says the name again.
    fn label(&self, switcher: &SourceSwitcher<'_>) -> String {
        let line = match self {
            Shown::Single(s) | Shown::One(s) => s.line(),
            Shown::Some(some) if troubled(some.iter().copied()).next().is_some() => all_line(some.iter().copied()),
            Shown::Some(_) => self.short_title(switcher.sources).unwrap_or_default(),
            Shown::All => all_line(switcher.sources.iter()),
        };
        let title = self.title();
        if line == title { line } else { format!("{title}, {line}") }
    }

    /// The state whose glyph the row shows, if any.
    fn state(&self, sources: &[SourceInfo<'a>]) -> Option<(AccountId, SourceState)> {
        match self {
            Shown::Single(s) | Shown::One(s) => Some((s.id, *s.state)),
            Shown::Some(some) => troubled(some.iter().copied()).next().map(|s| (s.id, *s.state)),
            Shown::All => troubled(sources.iter()).next().map(|s| (s.id, *s.state)),
        }
    }

    fn colors(&self, sources: &[SourceInfo<'_>]) -> Vec<SourceColor> {
        match self {
            Shown::Single(_) => Vec::new(),
            Shown::One(s) => vec![s.color],
            Shown::Some(some) => some.iter().map(|s| s.color).collect(),
            Shown::All => sources.iter().map(|s| s.color).collect(),
        }
    }
}

/// Whether the switcher flags a source: it needs the user (signed out, a
/// folder missing), or it's a server that can't be reached. Offline by
/// choice isn't trouble. An unreachable server keeps the app's own line
/// (which can say more than "Offline"); only what needs the user replaces
/// it.
pub fn flagged(state: &SourceState) -> bool {
    state.needs_attention() || matches!(state, SourceState::Offline(OfflineReason::Unreachable))
}

/// The sources the switcher flags.
fn troubled<'s, 'a: 's>(sources: impl Iterator<Item = &'s SourceInfo<'a>>) -> impl Iterator<Item = &'s SourceInfo<'a>> {
    sources.filter(|s| flagged(s.state))
}

/// Several sources' line: which needs attention, or how many there are.
fn all_line<'s, 'a: 's>(sources: impl Iterator<Item = &'s SourceInfo<'a>> + Clone) -> String {
    let trouble: Vec<_> = troubled(sources.clone()).collect();
    match trouble.as_slice() {
        [] => format!("{} sources", sources.count()),
        [one] => format!("{} · {}", one.name, one.state.label()),
        many => format!("{} need attention", many.len()),
    }
}

/// The included sources' names, in the menu's order.
fn names(some: &[&SourceInfo<'_>]) -> String {
    some.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")
}

/// `full` when it fits in `room` in `font`, else `short` (cut to fit if
/// even that doesn't).
fn fitted(ui: &Ui, full: &str, short: Option<String>, font: FontId, ink: Color32, room: f32) -> std::sync::Arc<egui::Galley> {
    let g = ui.painter().layout_no_wrap(full.to_owned(), font.clone(), ink);
    match short {
        Some(short) if g.size().x > room => elide(ui, &short, font, ink, room),
        _ => elide(ui, full, font, ink, room),
    }
}

/// The row at the foot of the full sidebar.
fn foot(ui: &mut Ui, switcher: &SourceSwitcher<'_>, shown: &Shown<'_, '_>, width: f32) -> Response {
    let t = Tokens::of(ui);
    let height = match switcher.look {
        SwitcherLook::Line => LINE_H,
        SwitcherLook::Card => CARD_H,
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let fill = if resp.is_pointer_button_down_on() || open {
        t.selected
    } else if resp.hovered() {
        t.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 6.0, fill);
    focus_ring(ui, &resp, rect, 6.0);
    let line = shown.line(switcher);
    let state = shown.state(switcher.sources);
    let colors = shown.colors(switcher.sources);
    match switcher.look {
        SwitcherLook::Line => {
            let lead = pos2(rect.left() + 12.0, rect.center().y);
            let mut text_x = rect.left() + 24.0;
            match shown {
                Shown::Single(s) => glyph(ui, lead, s.id, s.state, Glyph::Lead),
                _ => {
                    let w = dots_width(colors.len());
                    source_dots(ui, pos2(rect.left() + 12.0 - DOT_R + w / 2.0, lead.y), &colors, t.sidebar);
                    text_x = text_x.max(rect.left() + 12.0 - DOT_R + w + 6.0);
                }
            }
            // Several sources with one in trouble: a flag at the row's end,
            // the menu and the tooltip say which and why.
            let flag = !matches!(shown, Shown::Single(_) | Shown::One(_)) && troubled(switcher.sources.iter()).next().is_some();
            let mut right = rect.right() - 6.0;
            if flag {
                flag_mark(ui, pos2(rect.right() - 14.0, lead.y));
                right -= 20.0;
            }
            let g = fitted(ui, &line, shown.short_title(switcher.sources), FontId::proportional(11.5), t.muted, right - text_x);
            ui.painter().galley(pos2(text_x, rect.center().y - g.size().y / 2.0), g, t.muted);
        }
        SwitcherLook::Card => {
            let lead = pos2(rect.left() + 8.0 + AVATAR / 2.0, rect.center().y);
            match shown {
                Shown::Single(s) => avatar(ui, lead, s.name, None),
                Shown::One(s) => avatar(ui, lead, s.name, Some(s.color)),
                Shown::Some(_) | Shown::All => source_dots(ui, lead, &colors, t.sidebar),
            }
            let x = lead.x + AVATAR / 2.0 + 8.0;
            let room = rect.right() - x - 6.0;
            let name = fitted(ui, &shown.title(), shown.short_title(switcher.sources), FontId::new(12.5, semibold()), t.text, room);
            let indent = if state.is_some() { 12.0 } else { 0.0 };
            let line = elide(ui, &line, FontId::proportional(11.0), t.muted, room - indent);
            let top = rect.center().y - (name.size().y + line.size().y) / 2.0;
            let line_y = top + name.size().y;
            ui.painter().galley(pos2(x, top), name, t.text);
            if let Some((id, state)) = &state {
                glyph(ui, pos2(x + 4.0, line_y + line.size().y / 2.0), *id, state, Glyph::Inline);
            }
            ui.painter().galley(pos2(x + indent, line_y), line, t.muted);
        }
    }
    if state.is_none_or(|(_, s)| !spins(&s)) {
        forget_spin(ui);
    }
    resp
}

/// The switcher in the folded rail: with one source the row's lead (the
/// state's glyph, or the initial with the glyph as a badge), else the dots
/// alone.
fn rail(ui: &mut Ui, switcher: &SourceSwitcher<'_>, shown: &Shown<'_, '_>) -> Response {
    let t = Tokens::of(ui);
    let size = match switcher.look {
        SwitcherLook::Line => vec2(32.0, 28.0),
        SwitcherLook::Card => vec2(32.0, 32.0),
    };
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, t.hover);
    }
    focus_ring(ui, &resp, rect, 6.0);
    let c = rect.center();
    match (shown, switcher.look) {
        (Shown::Single(s), SwitcherLook::Line) => glyph(ui, c, s.id, s.state, Glyph::Lead),
        (Shown::Single(s), SwitcherLook::Card) => {
            avatar(ui, c, s.name, None);
            let badge = c + vec2(AVATAR / 2.0 - 1.0, AVATAR / 2.0 - 1.0);
            ui.painter().circle_filled(badge, 5.5, t.sidebar);
            glyph(ui, badge, s.id, s.state, Glyph::Inline);
        }
        _ => source_dots(ui, c, &shown.colors(switcher.sources), t.sidebar),
    }
    if !matches!(shown, Shown::Single(s) if spins(s.state)) {
        forget_spin(ui);
    }
    resp
}

/// The menu: the sources and All sources when there are several, Add and
/// Manage when offered, then the app's rows.
fn menu<R>(ui: &mut Ui, switcher: &SourceSwitcher<'_>, shown: &Shown<'_, '_>, rows: impl FnOnce(&mut Ui) -> Option<R>) -> Option<SourcePick<R>> {
    let mut picked = None;
    let several = !matches!(shown, Shown::Single(_));
    if several {
        let ground = ui.visuals().window_fill;
        for s in switcher.sources {
            let on = match shown {
                Shown::One(one) => one.id == s.id,
                // A box is ticked as its filter chip is lit.
                _ => switcher.toggles && switcher.scope.chip_selected(s.id),
            };
            match source_menu_row(ui, s, on, switcher.toggles) {
                Some(RowPick::Row) => picked = Some(SourcePick::Source(s.id)),
                Some(RowPick::Toggle) => picked = Some(SourcePick::Toggle(s.id)),
                None => {}
            }
        }
        let colors: Vec<SourceColor> = switcher.sources.iter().map(|s| s.color).collect();
        if all_menu_row(ui, &colors, matches!(shown, Shown::All), ground) {
            picked = Some(SourcePick::All);
        }
    }
    if switcher.add || switcher.manage {
        if several {
            popover_rule(ui, 6.0);
        }
        if switcher.add && menu_row(ui, "Add source…") {
            picked = Some(SourcePick::Add);
        }
        if switcher.manage && menu_row(ui, "Manage sources…") {
            picked = Some(SourcePick::Manage);
        }
    }
    if several || switcher.add || switcher.manage {
        popover_rule(ui, 6.0);
    }
    if let Some(r) = rows(ui) {
        picked = picked.or(Some(SourcePick::App(r)));
    }
    picked
}

/// What was clicked on a source's row in the menu.
enum RowPick {
    /// The row: show that source alone. The menu closes.
    Row,
    /// Its box: add it to the sources shown together, or take it out.
    Toggle,
}

/// A source in the menu: a check when it's the one in scope (or, with
/// `toggles`, a box at its leading edge, ticked when it's included), its
/// dot, its name, and its line under it (in the warning colour when it's
/// [`flagged`]).
fn source_menu_row(ui: &mut Ui, s: &SourceInfo<'_>, on: bool, toggles: bool) -> Option<RowPick> {
    let t = Tokens::of(ui);
    let line = s.line();
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), MENU_SOURCE_H), Sense::click());
    // Interacted after the row, so it's on top of it: the gutter is the box's.
    let tick = toggles.then(|| {
        let hit = Rect::from_min_max(rect.left_top(), pos2(rect.left() + MENU_DOT_X - DOT_R - 2.0, rect.bottom()));
        ui.interact(hit, resp.id.with("toggle"), Sense::click())
    });
    if ui.is_rect_visible(rect) {
        if resp.hovered() || tick.as_ref().is_some_and(|b| b.hovered()) {
            ui.painter().rect_filled(rect, 5.0, t.hover);
        }
        focus_ring(ui, &resp, rect.shrink(2.0), 3.0);
        match &tick {
            Some(tick) => tick_box(ui, tick, rect, on),
            None if on => check(ui, rect),
            None => {}
        }
        source_dot(ui, pos2(rect.left() + MENU_DOT_X, rect.center().y), s.color);
        let x = rect.left() + MENU_TEXT_X;
        let room = rect.right() - x - POPOVER_PAD;
        let name = elide(ui, s.name, FontId::proportional(13.0), t.text, room);
        let ink = if flagged(s.state) { t.warn } else { t.muted };
        let below = elide(ui, &line, FontId::proportional(11.0), ink, room);
        let top = rect.center().y - (name.size().y + below.size().y) / 2.0;
        let below_y = top + name.size().y;
        ui.painter().galley(pos2(x, top), name, t.text);
        ui.painter().galley(pos2(x, below_y), below, ink);
    }
    let label = format!("{}, {line}", s.name);
    if let Some(tick) = &tick {
        let name = format!("Include {}", s.name);
        tick.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, &name));
        // The row picks the source alone; its box says whether it's ticked.
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::MenuItem, true, &label));
        if tick.clicked() {
            return Some(RowPick::Toggle);
        }
    } else {
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::MenuItem, true, on, &label));
    }
    if resp.clicked() {
        ui.close();
        return Some(RowPick::Row);
    }
    None
}

/// The box in a source row's gutter: an outline, filled with the accent
/// and ticked when the source is included; ringed when focused.
fn tick_box(ui: &Ui, tick: &Response, row: Rect, on: bool) {
    let t = Tokens::of(ui);
    let r = Rect::from_center_size(pos2(row.left() + POPOVER_PAD + 6.0, row.center().y), vec2(13.0, 13.0));
    if on {
        ui.painter().rect_filled(r, 3.0, t.accent);
        icons::paint(ui.painter(), r.shrink(2.5), Icon::Check, t.on_accent);
    } else {
        let ink = if tick.hovered() { t.text } else { t.muted };
        ui.painter().rect_stroke(r, 3.0, egui::Stroke::new(1.2, ink), egui::StrokeKind::Inside);
    }
    focus_ring(ui, tick, r.expand(2.0), 4.0);
}

/// All sources in the menu: the stacked dots, checked when it's the scope.
fn all_menu_row(ui: &mut Ui, colors: &[SourceColor], on: bool, ground: Color32) -> bool {
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), LINE_H), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter().rect_filled(rect, 5.0, t.hover);
        }
        focus_ring(ui, &resp, rect.shrink(2.0), 3.0);
        if on {
            check(ui, rect);
        }
        let w = dots_width(colors.len());
        source_dots(ui, pos2(rect.left() + MENU_DOT_X - DOT_R + w / 2.0, rect.center().y), colors, ground);
        let at = pos2(rect.left() + MENU_TEXT_X, rect.center().y);
        ui.painter().text(at, egui::Align2::LEFT_CENTER, "All sources", FontId::proportional(13.0), t.text);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::MenuItem, true, on, "All sources"));
    if resp.clicked() {
        ui.close();
        return true;
    }
    false
}

/// The check in a menu row's gutter, as [`check_row`](super::check_row)
/// draws it.
fn check(ui: &Ui, rect: Rect) {
    let mark = Rect::from_center_size(pos2(rect.left() + POPOVER_PAD + 6.0, rect.center().y), vec2(11.0, 11.0));
    icons::paint(ui.painter(), mark, Icon::Check, Tokens::of(ui).text);
}

/// The mark that says a source wants a look, whatever its trouble.
fn flag_mark(ui: &Ui, c: Pos2) {
    super::download_mark(ui, c, 12.0, super::DownloadMark::Failed);
}

/// The [`SwitcherLook::Card`]'s lead: the name's initial in a quiet disc,
/// in the accent, or in the source's colour when there are several.
fn avatar(ui: &Ui, c: Pos2, name: &str, color: Option<SourceColor>) {
    let ink = match color {
        Some(color) => dot_color(ui, color),
        None => Tokens::of(ui).accent,
    };
    ui.painter().circle_filled(c, AVATAR / 2.0, ink.gamma_multiply(0.18));
    let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    ui.painter().text(c, egui::Align2::CENTER_CENTER, initial, FontId::new(12.0, semibold()), ink);
}

/// Which size a state's glyph is drawn at.
#[derive(Clone, Copy)]
pub(super) enum Glyph {
    /// Leading a one-line row, or alone in the rail: online is a check.
    Lead,
    /// Beside a small line of text, or as a badge: online is a dot.
    Inline,
}

/// A state's small glyph at `c`: online, a ring or spinner while busy, a
/// cloud while offline, a warning when it needs the user.
pub(super) fn glyph(ui: &Ui, c: Pos2, id: AccountId, state: &SourceState, size: Glyph) {
    let t = Tokens::of(ui);
    let painter = ui.painter();
    let lead = matches!(size, Glyph::Lead);
    match state {
        SourceState::Online if lead => icons::status_mark(painter, c, 3.4, icons::Status::Good, None),
        SourceState::Online => {
            painter.circle_filled(c, 3.2, t.ok);
        }
        SourceState::Syncing(Some(f)) if f.is_finite() => icons::progress_ring(painter, c, if lead { 5.5 } else { 4.5 }, *f, t.accent, t.hairline),
        SourceState::Syncing(_) | SourceState::Connecting | SourceState::Scanning => {
            let side = if lead { 12.0 } else { 11.0 };
            icons::paint(painter, Rect::from_center_size(c, vec2(side, side)), Icon::Spinner, t.muted);
            spin(ui, id, state);
        }
        SourceState::Offline(_) => {
            let side = if lead { 13.0 } else { 12.0 };
            icons::paint(painter, Rect::from_center_size(c, vec2(side, side)), Icon::Offline, t.muted);
        }
        SourceState::SignedOut | SourceState::FolderMissing => super::download_mark(ui, c, if lead { 12.0 } else { 11.0 }, super::DownloadMark::Failed),
    }
}

pub(super) fn spins(state: &SourceState) -> bool {
    matches!(state, SourceState::Syncing(None) | SourceState::Connecting | SourceState::Scanning)
        || matches!(state, SourceState::Syncing(Some(f)) if !f.is_finite())
}

/// Keeps a spinner turning for [`SPIN_SECS`] from when this source started
/// in this state, then lets it go still. A change of source or state starts
/// the count again.
fn spin(ui: &Ui, id: AccountId, state: &SourceState) {
    let now = ui.input(|i| i.time);
    let key = (id, std::mem::discriminant(state));
    let since = ui.data_mut(|d| {
        let slot = d.get_temp_mut_or_insert_with(spin_id(), || (key, now));
        if slot.0 != key {
            *slot = (key, now);
        }
        slot.1
    });
    if now - since < SPIN_SECS {
        ui.ctx().request_repaint_after(SPIN_FRAME);
    }
}

/// Nothing is spinning: the next spinner counts from its own start.
pub(super) fn forget_spin(ui: &Ui) {
    ui.data_mut(|d| d.remove::<SpinSince>(spin_id()));
}

type SpinSince = ((AccountId, std::mem::Discriminant<SourceState>), f64);

fn spin_id() -> egui::Id {
    egui::Id::unique("kit-source-spinning-since")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{canvas, themed};
    use egui_kittest::kittest::Queryable;

    /// An account id from its string form: the kit has no `uuid` of its own.
    fn id(n: u128) -> AccountId {
        format!("00000000-0000-4000-8000-{n:012x}").parse().unwrap()
    }

    const ONLINE: SourceState = SourceState::Online;
    const OUT: SourceState = SourceState::SignedOut;
    const AWAY: SourceState = SourceState::Offline(OfflineReason::Unreachable);
    const BY_CHOICE: SourceState = SourceState::Offline(OfflineReason::ByChoice);

    fn three(office: &'static SourceState) -> [SourceInfo<'static>; 3] {
        let info = |n: u128, name: &'static str, color: u8, state: &'static SourceState, detail: &'static str| SourceInfo {
            id: id(n),
            name,
            kind: "Server",
            icon: Icon::Disc,
            color: SourceColor::new(color),
            state,
            detail: Some(detail),
        };
        [
            info(1, "Home server", 0, &ONLINE, "Synced 2 min ago"),
            info(2, "Office", 3, office, "Synced yesterday"),
            info(3, "Music folder", 5, &ONLINE, "12 840 songs"),
        ]
    }

    /// What `switcher` returned on the last frame it was drawn, kept until a
    /// test reads it.
    type Seen = Option<SourcePick<&'static str>>;

    fn harness(sources: Vec<SourceInfo<'static>>, scope: Scope, look: SwitcherLook, place: SwitcherPlace) -> egui_kittest::Harness<'static, (bool, Seen)> {
        harness_with(sources, scope, look, place, false)
    }

    fn harness_with(
        sources: Vec<SourceInfo<'static>>,
        scope: Scope,
        look: SwitcherLook,
        place: SwitcherPlace,
        toggles: bool,
    ) -> egui_kittest::Harness<'static, (bool, Seen)> {
        themed(None, move |ui, seen: &mut Seen| {
            let switcher = SourceSwitcher { sources: &sources, scope: &scope, look, add: true, manage: true, toggles, tooltip: None };
            ui.add_space(300.0);
            let picked = source_switcher(ui, &switcher, place, |ui| menu_row(ui, "Settings…").then_some("settings"));
            if picked.is_some() {
                *seen = picked;
            }
        })
    }

    /// Every shape painted on the last frame, nested ones flattened.
    fn shapes(h: &egui_kittest::Harness<'_, (bool, Seen)>) -> Vec<egui::Shape> {
        fn flatten(shape: &egui::Shape, out: &mut Vec<egui::Shape>) {
            match shape {
                egui::Shape::Vec(inner) => inner.iter().for_each(|s| flatten(s, out)),
                other => out.push(other.clone()),
            }
        }
        let mut out = Vec::new();
        for clipped in &h.output().shapes {
            flatten(&clipped.shape, &mut out);
        }
        out
    }

    /// Circles filled in one of `colors`, in light mode.
    fn dots(h: &egui_kittest::Harness<'_, (bool, Seen)>, colors: &[SourceColor]) -> usize {
        let fills: Vec<Color32> = colors.iter().map(|&c| theme::source_color(c, false)).collect();
        shapes(h).iter().filter(|s| matches!(s, egui::Shape::Circle(c) if fills.contains(&c.fill))).count()
    }

    #[test]
    fn picking_a_source_in_the_menu_returns_it() {
        let mut h = harness(three(&ONLINE).to_vec(), Scope::all(), SwitcherLook::Card, SwitcherPlace::Foot { width: 200.0 });
        h.run();
        h.get_by_label("All sources, 3 sources").click();
        h.run();
        h.get_by_label("Office, Synced yesterday").click();
        h.run();
        assert_eq!(h.state().1, Some(SourcePick::Source(id(2))));
        h.get_by_label("All sources, 3 sources").click();
        h.run();
        h.get_by_label("Settings…").click();
        h.run();
        assert_eq!(h.state().1, Some(SourcePick::App("settings")), "the app's own rows answer through it");
    }

    #[test]
    fn the_menu_checks_the_source_in_scope_and_offers_add_and_manage() {
        use egui_kittest::kittest::NodeT;
        let mut h = harness(three(&ONLINE).to_vec(), Scope::only(id(3)), SwitcherLook::Line, SwitcherPlace::Foot { width: 200.0 });
        h.run();
        h.get_by_label_contains("Music folder").click();
        h.run();
        let checked = |h: &egui_kittest::Harness<'_, _>, label: &str| {
            h.get_by_role_and_label(egui::accesskit::Role::MenuItem, label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
        };
        assert!(checked(&h, "Music folder, 12 840 songs") && !checked(&h, "Home server, Synced 2 min ago") && !checked(&h, "All sources"));
        h.get_by_label("Manage sources…").click();
        h.run();
        assert_eq!(h.state().1, Some(SourcePick::Manage));
    }

    #[test]
    fn a_source_that_needs_attention_shows_its_state_in_the_row_and_the_menu() {
        let mut h = harness(three(&OUT).to_vec(), Scope::only(id(2)), SwitcherLook::Card, SwitcherPlace::Foot { width: 200.0 });
        h.run();
        assert!(h.query_by_label_contains("Synced yesterday").is_none(), "the detail gives way");
        h.get_by_label("Office, Signed out").click();
        h.run();
        assert_eq!(h.query_all_by_label("Office, Signed out").count(), 2, "the row and the menu both say so");
        // All sources says which one wants the user.
        let mut h = harness(three(&OUT).to_vec(), Scope::all(), SwitcherLook::Card, SwitcherPlace::Foot { width: 200.0 });
        h.run();
        h.get_by_label("All sources, Office · Signed out");
    }

    #[test]
    fn an_unreachable_server_flags_the_switcher_and_offline_by_choice_does_not() {
        let mut sources = three(&AWAY);
        sources[1].detail = Some("Can't reach the server");
        let flag = |h: &egui_kittest::Harness<'_, (bool, Seen)>| shapes(h).iter().any(|s| matches!(s, egui::Shape::Text(t) if t.galley.text() == "!"));
        let mut h = harness(sources.to_vec(), Scope::all(), SwitcherLook::Line, SwitcherPlace::Foot { width: 200.0 });
        h.run();
        assert!(flag(&h), "the row is flagged");
        h.get_by_label("All sources, Office · Offline").click();
        h.run();
        // The menu keeps the app's own words for it, in the warning colour.
        h.get_by_role_and_label(egui::accesskit::Role::MenuItem, "Office, Can't reach the server");
        let warn = Tokens::of_ctx(&h.ctx).warn;
        let warned = shapes(&h).iter().any(|s| match s {
            egui::Shape::Text(text) => text.galley.text() == "Can't reach the server" && text.fallback_color == warn,
            _ => false,
        });
        assert!(warned, "the line is in the warning colour");
        // Gone offline on purpose, it isn't trouble.
        assert!(!flagged(&BY_CHOICE) && flagged(&AWAY) && flagged(&OUT) && !flagged(&ONLINE));
        let mut h = harness(three(&BY_CHOICE).to_vec(), Scope::all(), SwitcherLook::Line, SwitcherPlace::Foot { width: 200.0 });
        h.run();
        h.get_by_label("All sources, 3 sources");
        assert!(!flag(&h));
    }

    /// All sources narrowed by the chips to Home server and Music folder.
    fn narrowed() -> Scope {
        let mut scope = Scope::all();
        scope.toggle_chip(id(1));
        scope.toggle_chip(id(3));
        scope
    }

    /// The words painted on the last frame.
    fn texts(h: &egui_kittest::Harness<'_, (bool, Seen)>) -> Vec<String> {
        shapes(h)
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_narrowed_scope_names_its_sources_or_counts_them_when_they_dont_fit() {
        let sources = three(&ONLINE);
        for look in [SwitcherLook::Line, SwitcherLook::Card] {
            let mut h = harness(sources.to_vec(), narrowed(), look, SwitcherPlace::Foot { width: 220.0 });
            h.run();
            h.get_by_label("Home server, Music folder, 2 of 3 sources");
            assert!(texts(&h).contains(&"Home server, Music folder".to_owned()), "{look:?}: {:?}", texts(&h));
            assert_eq!(dots(&h, &[sources[0].color, sources[2].color]), 2, "the included sources' dots");
            assert_eq!(dots(&h, &[sources[1].color]), 0, "and not the other's");
            let mut h = harness(sources.to_vec(), narrowed(), look, SwitcherPlace::Foot { width: 130.0 });
            h.run();
            assert!(texts(&h).contains(&"2 of 3 sources".to_owned()), "{look:?}: {:?}", texts(&h));
        }
        // Every chip lit is every source: All sources.
        let mut all = narrowed();
        all.toggle_chip(id(2));
        let mut h = harness(sources.to_vec(), all, SwitcherLook::Line, SwitcherPlace::Foot { width: 220.0 });
        h.run();
        h.get_by_label("All sources, 3 sources");
    }

    #[test]
    fn the_menus_boxes_tick_the_included_sources_and_toggle_them_without_picking() {
        use egui::accesskit::{Role, Toggled};
        use egui_kittest::kittest::NodeT;
        let mut h = harness_with(three(&ONLINE).to_vec(), narrowed(), SwitcherLook::Line, SwitcherPlace::Foot { width: 220.0 }, true);
        h.run();
        h.get_by_label("Home server, Music folder, 2 of 3 sources").click();
        h.run();
        let ticked =
            |h: &egui_kittest::Harness<'_, _>, name: &str| h.get_by_role_and_label(Role::CheckBox, name).accesskit_node().toggled() == Some(Toggled::True);
        assert!(ticked(&h, "Include Home server") && !ticked(&h, "Include Office") && ticked(&h, "Include Music folder"));
        h.get_by_role_and_label(Role::CheckBox, "Include Office").click();
        h.run();
        assert_eq!(h.state().1, Some(SourcePick::Toggle(id(2))));
        assert!(h.query_by_label("Office, Synced yesterday").is_some(), "the menu stays open");
        // The row itself still picks the source alone.
        h.get_by_role_and_label(Role::MenuItem, "Office, Synced yesterday").click();
        h.run();
        assert_eq!(h.state().1, Some(SourcePick::Source(id(2))));
        // Without the boxes (Rede), the one in scope is checked as before.
        let mut h = harness(three(&ONLINE).to_vec(), Scope::only(id(1)), SwitcherLook::Card, SwitcherPlace::Foot { width: 220.0 });
        h.run();
        h.get_by_label("Home server, Synced 2 min ago").click();
        h.run();
        assert!(h.query_by_role(Role::CheckBox).is_none());
    }

    #[test]
    fn the_folded_rail_draws_the_dots_alone() {
        let sources = three(&OUT);
        let colors: Vec<SourceColor> = sources.iter().map(|s| s.color).collect();
        let mut h = harness(sources.to_vec(), Scope::all(), SwitcherLook::Card, SwitcherPlace::Rail);
        h.run();
        assert_eq!(dots(&h, &colors), 3, "a dot for each source");
        assert!(!shapes(&h).iter().any(|s| matches!(s, egui::Shape::Text(_))), "and no words");
        h.get_by_label("All sources, Office · Signed out");
    }

    #[test]
    fn with_one_source_no_dot_is_drawn() {
        for look in [SwitcherLook::Line, SwitcherLook::Card] {
            for place in [SwitcherPlace::Foot { width: 200.0 }, SwitcherPlace::Rail] {
                let mut only = three(&ONLINE)[0];
                only.color = SourceColor::new(6);
                let mut h = harness(vec![only], Scope::all(), look, place);
                h.run();
                assert_eq!(dots(&h, &[only.color]), 0, "{look:?} {place:?}");
                h.get_by_label("Home server, Synced 2 min ago");
            }
        }
        // And its menu doesn't list it, only what the app and the switcher
        // offer.
        let mut h = harness(vec![three(&ONLINE)[0]], Scope::all(), SwitcherLook::Line, SwitcherPlace::Foot { width: 200.0 });
        h.run();
        h.get_by_label("Home server, Synced 2 min ago").click();
        h.run();
        assert!(h.query_by_label("All sources").is_none());
        h.get_by_label("Add source…").click();
        h.run();
        assert_eq!(h.state().1, Some(SourcePick::Add));
    }

    #[test]
    fn a_source_label_draws_its_dot_before_its_name() {
        let color = SourceColor::new(4);
        let mut h = themed((), move |ui, _| {
            source_label(ui, color, "Music folder", FontId::proportional(12.0), Tokens::of(ui).muted);
        });
        h.run();
        let found = h.get_by_label("Music folder");
        let dot = shapes_of(&h.output().shapes)
            .into_iter()
            .find_map(|s| match s {
                egui::Shape::Circle(c) if c.fill == theme::source_color(color, false) => Some(c.center),
                _ => None,
            })
            .expect("a dot in the source's colour");
        let rect = found.rect();
        assert!((dot.x - (rect.left() + DOT_R)).abs() < 0.5, "the dot leads the name");
        assert!(rect.width() > DOT_LEAD, "and the name follows it");
    }

    #[test]
    fn dots_after_a_line_follow_its_text_and_the_text_gives_way_to_them() {
        let (one, two) = (SourceColor::new(1), SourceColor::new(6));
        let mut h = themed((), move |ui, _| {
            let t = Tokens::of(ui);
            let font = FontId::proportional(12.0);
            text_then_dots(ui, pos2(10.0, 10.0), ("Bill Evans", font.clone(), t.muted), 200.0, &[one], t.sidebar);
            let w = text_then_dots(ui, pos2(10.0, 40.0), (&"Bill Evans Trio ".repeat(8), font, t.muted), 120.0, &[one, two], t.sidebar);
            assert!(w <= 120.0 + 0.5, "the dots fit in the width: {w}");
        });
        h.run();
        let shapes = shapes_of(&h.output().shapes);
        let text_right = |y: f32| {
            shapes.iter().find_map(|s| match s {
                egui::Shape::Text(t) if (t.pos.y - y).abs() < 0.5 => Some(t.pos.x + t.galley.size().x),
                _ => None,
            })
        };
        let dot_x = |color: SourceColor, below: f32| {
            shapes.iter().find_map(|s| match s {
                egui::Shape::Circle(c) if c.fill == theme::source_color(color, false) && c.center.y > below => Some(c.center.x),
                _ => None,
            })
        };
        let (first, second) = (text_right(10.0).unwrap(), text_right(40.0).unwrap());
        assert!((dot_x(one, 0.0).unwrap() - (first + DOT_TRAIL - DOT_R)).abs() < 0.5, "the dot follows the text");
        assert!(dot_x(two, 30.0).unwrap() > second, "both stacked dots follow the cut text");
        assert!(second < 10.0 + 120.0 - DOT_TRAIL, "and the text was cut to make room");
    }

    /// Every shape in `clipped`, nested ones flattened.
    fn shapes_of(clipped: &[egui::epaint::ClippedShape]) -> Vec<egui::Shape> {
        fn flatten(shape: &egui::Shape, out: &mut Vec<egui::Shape>) {
            match shape {
                egui::Shape::Vec(inner) => inner.iter().for_each(|s| flatten(s, out)),
                other => out.push(other.clone()),
            }
        }
        let mut out = Vec::new();
        for c in clipped {
            flatten(&c.shape, &mut out);
        }
        out
    }

    #[test]
    fn a_line_is_the_state_when_the_source_needs_attention_and_the_detail_otherwise() {
        let [home, office, _] = three(&OUT);
        assert_eq!(home.line(), "Synced 2 min ago");
        assert_eq!(office.line(), "Signed out");
        assert_eq!(SourceInfo { detail: None, ..home }.line(), "Online");
    }

    /// Renders the switcher with one source, three, and three with one in
    /// trouble, in both looks, at the foot and in the rail, then its menu,
    /// and the dots, to look at:
    /// `cargo test -p shell --release snapshots -- --ignored` writes
    /// `target/snapshots/kit-sources-*.png`.
    #[test]
    #[ignore = "writes pictures to look at"]
    fn snapshots() {
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
        std::fs::create_dir_all(&out).unwrap();
        let one = vec![SourceInfo { detail: Some("music.example.com"), ..three(&ONLINE)[0] }];
        let scenes: [(&str, Vec<SourceInfo<'static>>, Scope); 7] = [
            ("one", one.clone(), Scope::all()),
            ("three", three(&ONLINE).to_vec(), Scope::all()),
            ("three-one", three(&ONLINE).to_vec(), Scope::only(id(1))),
            ("trouble", three(&OUT).to_vec(), Scope::only(id(2))),
            ("trouble-all", three(&OUT).to_vec(), Scope::all()),
            ("unreachable", three(&AWAY).to_vec(), Scope::all()),
            ("narrowed", three(&ONLINE).to_vec(), narrowed()),
        ];
        for (theme, pref) in [("light", theme::ThemePref::Light), ("dark", theme::ThemePref::Dark)] {
            for (name, sources, scope) in &scenes {
                // The four ways it's drawn, side by side on the sidebar's ground.
                let (sources, scope) = (sources.clone(), scope.clone());
                let mut h = canvas(vec2(560.0, 64.0), pref, move |ui| {
                    ui.horizontal(|ui| {
                        for look in [SwitcherLook::Line, SwitcherLook::Card] {
                            let s = SourceSwitcher { sources: &sources, scope: &scope, look, add: false, manage: false, toggles: false, tooltip: None };
                            ui.allocate_ui(vec2(196.0, 48.0), |ui| source_switcher(ui, &s, SwitcherPlace::Foot { width: 196.0 }, |_| None::<()>));
                            ui.add_space(8.0);
                        }
                        for look in [SwitcherLook::Line, SwitcherLook::Card] {
                            let s = SourceSwitcher { sources: &sources, scope: &scope, look, add: false, manage: false, toggles: false, tooltip: None };
                            source_switcher(ui, &s, SwitcherPlace::Rail, |_| None::<()>);
                            ui.add_space(12.0);
                        }
                    });
                });
                h.run();
                h.render().unwrap().save(out.join(format!("kit-sources-{name}-{theme}.png"))).unwrap();
            }
            // The menu, with Add and Manage, over the app's own rows.
            for (name, sources, scope) in [
                ("one", &scenes[0].1, &scenes[0].2),
                ("three", &scenes[2].1, &scenes[2].2),
                ("trouble", &scenes[3].1, &scenes[3].2),
                ("unreachable", &scenes[5].1, &scenes[5].2),
                ("narrowed", &scenes[6].1, &scenes[6].2),
            ] {
                let (sources, scope) = (sources.clone(), scope.clone());
                let mut h = canvas(vec2(300.0, 400.0), pref, move |ui| {
                    ui.add_space(340.0);
                    let toggles = scope.has_chips();
                    let s = SourceSwitcher { sources: &sources, scope: &scope, look: SwitcherLook::Card, add: true, manage: true, toggles, tooltip: None };
                    source_switcher(ui, &s, SwitcherPlace::Foot { width: 216.0 }, |ui| {
                        menu_row(ui, "Settings…");
                        menu_row(ui, "Keyboard shortcuts");
                        None::<()>
                    });
                });
                h.run();
                let row = h.get_by_role(egui::accesskit::Role::Button);
                row.click();
                h.run_steps(4);
                h.event(egui::Event::PointerGone);
                h.run_steps(2);
                h.render().unwrap().save(out.join(format!("kit-sources-menu-{name}-{theme}.png"))).unwrap();
            }
            // Every colour as a dot, then stacks of one to five.
            let mut h = canvas(vec2(240.0, 60.0), pref, |ui| {
                let t = Tokens::of(ui);
                let y = ui.max_rect().top() + 20.0;
                for (i, &c) in SourceColor::ALL.iter().enumerate() {
                    source_dot(ui, pos2(24.0 + i as f32 * 14.0, y), c);
                }
                for n in 1..=5 {
                    source_dots(ui, pos2(24.0 + (n - 1) as f32 * 36.0, y + 20.0), &SourceColor::ALL[..n], t.sidebar);
                }
            });
            h.run();
            h.render().unwrap().save(out.join(format!("kit-sources-dots-{theme}.png"))).unwrap();
        }
    }
}
