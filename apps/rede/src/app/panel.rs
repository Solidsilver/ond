//! The column at the right of the full window, in Ond's queue column's place:
//! the playing book's chapters, its bookmarks and its facts, under a switch
//! (Chapters · Bookmarks · Info). It follows playback; with nothing playing
//! each tab says so, since the book page beside it already lists the viewed
//! book's chapters and facts.
//!
//! Docked beside the page when the window has room for both, as Ond docks its
//! queue; in a narrower window the same column is a sheet over the page that
//! a click outside or Esc closes. Whether it's shown is a preference; the tab
//! and the sheet are the window's.

use super::book::{CHAPTER_ROW_H, chapter_row};
use super::view_ctx::ViewCtx;
use super::{Action, PrefsAction, RedeApp};
use crate::player::Control;
use crate::widgets::elide;
use egui::{FontId, Rect, RichText, Sense, Ui, vec2};
use shell::icons::Icon;
use shell::kit::{KV_VALUE, kv_section, kv_text};
use shell::theme::{Tokens, semibold};

/// The column's width: Ond's queue column at its default.
pub const PANEL_W: f32 = 320.0;
/// The page keeps at least this much beside a docked column, as Ond's
/// content does beside its queue.
const CONTENT_MIN: f32 = 564.0;
/// The Info tab's cover beside the title.
const INFO_COVER: f32 = 56.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Chapters,
    Bookmarks,
    Info,
}

impl Tab {
    const ALL: [Self; 3] = [Self::Chapters, Self::Bookmarks, Self::Info];

    fn label(self) -> &'static str {
        match self {
            Self::Chapters => "Chapters",
            Self::Bookmarks => "Bookmarks",
            Self::Info => "Info",
        }
    }
}

/// The column's state that isn't a preference.
#[derive(Debug, Clone, Copy, Default)]
pub struct PanelState {
    pub tab: Tab,
    /// The window is too narrow to dock the column: it's a sheet instead.
    pub narrow: bool,
    /// The sheet is open (narrow windows only).
    pub sheet: bool,
    /// Scroll the chapter playing into view on the next frame the list draws:
    /// set when the column opens, or the title bar's chapter asks for it.
    pub reveal: bool,
}

/// A change to the column, applied by [`RedeApp::apply_panel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelAction {
    /// Shown or hidden: docked, it's the preference; narrow, the sheet.
    Toggle,
    /// Shown, on this tab, with the chapter playing in view.
    Show(Tab),
    /// Another tab, in the column as it is.
    Tab(Tab),
    CloseSheet,
}

/// Whether a window `width` wide docks the column beside the page.
pub fn docks(width: f32) -> bool {
    width >= PANEL_W + CONTENT_MIN
}

impl RedeApp {
    /// Whether the column shows this frame, docked or as the sheet.
    pub(super) fn panel_shown(&self) -> bool {
        if self.ui.panel.narrow { self.ui.panel.sheet } else { self.shown.show_panel }
    }

    /// Applied between frames, so it reads the preference itself rather than
    /// the frame's copy, which an earlier action may have outdated.
    pub(super) fn apply_panel(&mut self, action: PanelAction) {
        let docked = self.prefs.get(|s| s.show_panel);
        let shown = |app: &Self| if app.ui.panel.narrow { app.ui.panel.sheet } else { app.prefs.get(|s| s.show_panel) };
        let was = shown(self);
        match action {
            PanelAction::Toggle => {
                if self.ui.panel.narrow {
                    self.ui.panel.sheet = !self.ui.panel.sheet;
                } else {
                    self.apply_prefs(PrefsAction::ShowPanel(!docked));
                }
            }
            PanelAction::Show(tab) => {
                self.ui.panel.tab = tab;
                if self.ui.panel.narrow {
                    self.ui.panel.sheet = true;
                } else if !docked {
                    self.apply_prefs(PrefsAction::ShowPanel(true));
                }
                self.ui.panel.reveal = true;
            }
            PanelAction::Tab(tab) => self.ui.panel.tab = tab,
            PanelAction::CloseSheet => self.ui.panel.sheet = false,
        }
        if !was && shown(self) {
            self.ui.panel.reveal = true;
        }
        self.ctx.request_repaint();
    }

    /// The column docked at the right. Drawn before the page, so the page
    /// takes what's left.
    pub(super) fn side_panel(&mut self, ui: &mut Ui) {
        if self.ui.panel.narrow || !self.shown.show_panel {
            return;
        }
        let t = Tokens::of(ui);
        egui::Panel::right("side-panel")
            .exact_size(PANEL_W)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.panel).inner_margin(egui::Margin { left: 8, right: 8, top: 12, bottom: 8 }))
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                self.view().panel(ui);
            });
        self.ui.panel.reveal = false;
    }

    /// The column as a sheet over the right of the page (narrow windows).
    pub(super) fn panel_sheet(&mut self, ctx: &egui::Context, content: Rect) {
        if !self.ui.panel.narrow || !self.ui.panel.sheet {
            return;
        }
        let t = Tokens::of_ctx(ctx);
        let w = PANEL_W.min(content.width() - 24.0);
        let id = egui::Id::unique("panel-sheet");
        let area = egui::Modal::default_area(id).anchor(egui::Align2::RIGHT_TOP, vec2(0.0, content.top()));
        let shadow = egui::Shadow {
            offset: [-6, 0],
            blur: 24,
            spread: 0,
            color: egui::Color32::from_black_alpha(if ctx.global_style().visuals.dark_mode { 120 } else { 40 }),
        };
        let frame = egui::Frame::new().fill(t.panel).shadow(shadow).stroke(egui::Stroke::new(1.0, t.hairline)).inner_margin(egui::Margin {
            left: 8,
            right: 8,
            top: 12,
            bottom: 8,
        });
        let modal = egui::Modal::new(id).area(area).backdrop_color(egui::Color32::from_black_alpha(18)).frame(frame).show(ctx, |ui| {
            ui.set_width(w - 16.0);
            ui.set_height(content.height() - 20.0);
            self.view().panel(ui);
        });
        self.ui.panel.reveal = false;
        if modal.should_close() {
            self.actions.push(Action::Panel(PanelAction::CloseSheet));
        }
    }
}

impl ViewCtx<'_> {
    /// The switch, then the tab it's on.
    pub(super) fn panel(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let mut tab = self.panel.tab;
        // The book's time left goes beside the switch on the chapters, at the
        // speed it depends on; below it when there's no room.
        let left = self.player.book.as_ref().filter(|_| tab == Tab::Chapters).map(|book| {
            let (_, book_left) = book.timeline.remaining(self.player.position, self.player.speed);
            crate::timeline::left(book_left, self.player.speed)
        });
        let mut below = None;
        ui.horizontal(|ui| {
            if crate::widgets::choice(ui, &Tab::ALL, &mut tab, |tab| tab.label().into()) {
                self.actions.push(Action::Panel(PanelAction::Tab(tab)));
            }
            if let Some(left) = left {
                let w = ui.painter().layout_no_wrap(left.clone(), FontId::proportional(11.0), t.muted).size().x;
                if ui.available_width() >= w + 16.0 {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        ui.label(RichText::new(left).size(11.0).color(t.muted));
                    });
                } else {
                    below = Some(left);
                }
            }
        });
        if let Some(left) = below {
            ui.add_space(4.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(left).size(11.0).color(t.muted));
            });
        }
        ui.add_space(8.0);
        match tab {
            Tab::Chapters => self.chapters_tab(ui, &t),
            Tab::Bookmarks => shell::widgets::empty_state(ui, Icon::Bookmark, "No bookmarks yet", "Bookmarks come with the next update."),
            Tab::Info => self.info_tab(ui, &t),
        }
    }

    /// The playing book's chapters, the one playing marked with how far into
    /// it, the ones heard dimmed. A click goes to a chapter's start. Only the
    /// rows on screen are laid out: a book can have a few hundred.
    fn chapters_tab(&mut self, ui: &mut Ui, t: &Tokens) {
        let Some(book) = &self.player.book else {
            shell::widgets::empty_state(ui, Icon::Headphones, "Nothing playing", "Play a book to see its chapters here.");
            return;
        };
        let chapters = &book.timeline.chapters;
        if chapters.is_empty() {
            shell::widgets::empty_state(ui, Icon::Headphones, "No chapters", "This book plays as one long track.");
            return;
        }
        let at = self.player.position;
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut area = egui::ScrollArea::vertical().id_salt("panel-chapters").auto_shrink(false);
        if self.panel.reveal
            && let Some(current) = book.timeline.chapter(at)
        {
            // The chapter playing a third of the way down, so what comes
            // next is in view too.
            let offset = current as f32 * CHAPTER_ROW_H - ui.available_height() / 3.0;
            area = area.vertical_scroll_offset(offset.max(0.0));
        }
        area.show_rows(ui, CHAPTER_ROW_H, chapters.len(), |ui, rows| {
            for n in rows {
                let chapter = &chapters[n];
                let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), CHAPTER_ROW_H), Sense::click());
                let length = crate::fmt::clock((chapter.end - chapter.start).max(0.0));
                let current = at >= chapter.start && at < chapter.end;
                resp.widget_info(|| {
                    let state = if current { ", playing" } else { "" };
                    egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{}, {length}{state}", chapter.title))
                });
                if resp.hovered() && !current {
                    ui.painter().rect_filled(row, 4.0, t.hover);
                }
                shell::widgets::focus_ring(ui, &resp, row, 4.0);
                chapter_row(ui, t, row, n, chapter, Some(at));
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.actions.push(Action::Playback(Control::Seek(chapter.start)));
                }
            }
        });
    }

    /// The playing book's facts as quiet label and value rows, as Ond's Info
    /// tab sets a song's, then its description.
    fn info_tab(&mut self, ui: &mut Ui, t: &Tokens) {
        // A copy of the reference, so the description memo can be borrowed
        // mutably while the book is read.
        let player = self.player;
        let Some(book) = &player.book else {
            shell::widgets::empty_state(ui, Icon::Headphones, "Nothing playing", "Play a book to see its details here.");
            return;
        };
        let item = &book.item;
        let summary = &item.summary;
        egui::ScrollArea::vertical().id_salt("panel-info").auto_shrink(false).show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            let width = ui.available_width();
            ui.add_space(4.0);
            ui.horizontal_top(|ui| {
                let (cover, _) = ui.allocate_exact_size(vec2(INFO_COVER, INFO_COVER), Sense::hover());
                self.art.paint(ui, cover, summary.cover.as_deref(), 4.0);
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let text_w = width - INFO_COVER - 18.0;
                    ui.add_space(4.0);
                    let title = elide(ui, &summary.title, FontId::new(14.0, semibold()), t.text, text_w);
                    let (r, _) = ui.allocate_exact_size(title.size(), Sense::hover());
                    ui.painter().galley(r.min, title, t.text);
                    let authors = elide(ui, &super::library::authors_line(summary), FontId::proportional(KV_VALUE), t.muted, text_w);
                    let (r, _) = ui.allocate_exact_size(authors.size(), Sense::hover());
                    ui.painter().galley(r.min, authors, t.muted);
                });
            });
            kv_section(ui, "Book");
            if !summary.narrators.is_empty() {
                kv_text(ui, "Read by", &summary.narrators.join(", "));
            }
            for series in &summary.series {
                let value = match &series.sequence {
                    Some(n) => format!("{} #{n}", series.name),
                    None => series.name.clone(),
                };
                kv_text(ui, "Series", &value);
            }
            kv_text(ui, "Length", &crate::fmt::span(book.timeline.duration));
            if !book.timeline.chapters.is_empty() {
                kv_text(ui, "Chapters", &book.timeline.chapters.len().to_string());
            }
            if let Some(year) = item.published_year.as_deref().filter(|y| !y.is_empty()) {
                kv_text(ui, "Published", year);
            }
            if let Some(publisher) = item.publisher.as_deref().filter(|p| !p.is_empty()) {
                kv_text(ui, "Publisher", publisher);
            }
            if !item.genres.is_empty() {
                kv_text(ui, if item.genres.len() == 1 { "Genre" } else { "Genres" }, &item.genres.join(", "));
            }
            if let Some(description) = item.description.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                kv_section(ui, "About");
                // Audiobookshelf keeps descriptions as HTML; the book page's
                // parse is memoised by book, so this reuses it.
                let blocks = self.books.description(&summary.id, description);
                let style = shell::rich::Style { font: FontId::proportional(KV_VALUE), line_height: 19.0, ..shell::rich::Style::reading(ui) };
                shell::rich::draw(ui, &blocks, &style);
            }
            ui.add_space(12.0);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_column_docks_only_where_the_page_keeps_its_room() {
        assert!(docks(1100.0), "the default window has room for both");
        assert!(docks(PANEL_W + CONTENT_MIN));
        assert!(!docks(PANEL_W + CONTENT_MIN - 1.0));
        assert!(!docks(crate::MIN_SIZE[0]), "the smallest window gets the sheet");
    }

    #[test]
    fn the_tabs_say_what_they_hold() {
        assert_eq!(Tab::ALL.map(Tab::label), ["Chapters", "Bookmarks", "Info"]);
        assert_eq!(Tab::default(), Tab::Chapters);
    }
}
