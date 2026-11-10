//! The Bookmarks tab of the column at the right: the playing book's
//! bookmarks in book order, set out as the chapters tab sets its rows (a
//! glyph where the chapter number goes, the title, the time at the right) so
//! the two tabs read as one list in two cuts. The bookmark at or just behind
//! where the book is gets its glyph in the accent: the one you'd go back to.
//!
//! A click jumps to a bookmark. Its menu (the … that shows on the row, or a
//! right-click) renames it in place, Enter keeping the new title and Esc the
//! old, or deletes it, with Undo in the toast that says so. Add bookmark at
//! the top does what ⌘B does.
//!
//! Which row is being renamed, and what's typed so far, live in egui's memory
//! as a text field's cursor does: they're the field's, not the app's, and
//! only the finished title goes anywhere, as an action.

use super::Action;
use super::book::{CHAPTER_NUMBER_RIGHT, CHAPTER_ROW_H, CHAPTER_TITLE_X};
use super::view_ctx::ViewCtx;
use crate::widgets::elide;
use egui::{FontId, Rect, RichText, Sense, Ui, pos2, vec2};
use shell::icons::{self, Icon};
use shell::kit;
use shell::theme::Tokens;
use sources::Bookmark;

/// The row menu's width.
const MENU_W: f32 = 150.0;
/// The … at the row's right end.
const MORE: f32 = 22.0;

/// A rename in progress: the row (its time's bits, which can't change), what's
/// typed, and whether the field has yet to take the focus.
#[derive(Clone)]
struct Renaming {
    at: u64,
    draft: String,
    fresh: bool,
}

/// What a row's menu chose.
enum Choice {
    Rename,
    Delete,
}

impl ViewCtx<'_> {
    pub(super) fn bookmarks_tab(&mut self, ui: &mut Ui, t: &Tokens) {
        // A copy of the reference, so the rows can push actions while the
        // book is read.
        let player = self.player;
        let Some(book) = &player.book else {
            kit::empty_state(ui, Icon::Headphones, "Nothing playing", "Play a book to see its bookmarks here.");
            return;
        };
        let bookmarks = self.bookmarks.clone();
        ui.horizontal(|ui| {
            ui.set_height(26.0);
            ui.add_space(6.0);
            if !bookmarks.is_empty() {
                let count = if bookmarks.len() == 1 { "1 bookmark".to_owned() } else { format!("{} bookmarks", bookmarks.len()) };
                ui.label(RichText::new(count).size(11.5).color(t.muted));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let add = kit::quiet_button(ui, Some(Icon::Plus), "Add bookmark").on_hover_text(shell::keys::shown("Bookmark this spot (⌘B)"));
                if add.clicked()
                    && let Some(action) = super::bookmarks::at_current_position(self.player)
                {
                    self.actions.push(action);
                }
            });
        });
        ui.add_space(4.0);
        if bookmarks.is_empty() {
            kit::empty_state(ui, Icon::Bookmark, "No bookmarks yet", "Press ⌘B to mark this spot.");
            return;
        }
        let at = player.position;
        // Half a second's grace: a bookmark made a moment ago is still "here".
        let nearest = bookmarks.iter().rposition(|b| b.position.secs() <= at + 0.5);
        let id = egui::Id::unique(("bookmark-rename", &book.key));
        let mut renaming: Option<Renaming> = ui.data(|d| d.get_temp(id));
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical().id_salt("panel-bookmarks").auto_shrink(false).show_rows(ui, CHAPTER_ROW_H, bookmarks.len(), |ui, rows| {
            for n in rows {
                let bookmark = &bookmarks[n];
                let chapter = book.timeline.chapter(bookmark.position.secs()).map(|i| book.timeline.chapters[i].title.as_str());
                let editing = renaming.as_mut().filter(|r| r.at == bookmark.position.secs().to_bits());
                let row = Row { bookmark, chapter, nearest: nearest == Some(n) };
                match editing {
                    Some(edit) => {
                        if let Some(done) = self.rename_row(ui, t, &row, edit) {
                            if let Some(title) = done {
                                self.actions.push(Action::RenameBookmark { item: bookmark.item.clone(), time: bookmark.position.secs(), title });
                            }
                            renaming = None;
                        }
                    }
                    None => match self.bookmark_row(ui, t, &row) {
                        Some(Choice::Rename) => {
                            renaming = Some(Renaming { at: bookmark.position.secs().to_bits(), draft: bookmark.title.clone(), fresh: true });
                        }
                        Some(Choice::Delete) => {
                            self.actions.push(Action::DeleteBookmark { item: bookmark.item.clone(), time: bookmark.position.secs() });
                        }
                        None => {}
                    },
                }
            }
        });
        ui.data_mut(|d| match renaming {
            Some(r) => {
                d.insert_temp(id, r);
            }
            None => d.remove::<Renaming>(id),
        });
    }

    /// One bookmark: the glyph, the title, the time, and on hover the … with
    /// its menu. A click jumps there.
    fn bookmark_row(&mut self, ui: &mut Ui, t: &Tokens, row: &Row<'_>) -> Option<Choice> {
        let b = row.bookmark;
        let time = b.position.secs();
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), CHAPTER_ROW_H), Sense::click());
        let clock = crate::fmt::clock(time);
        let name = row.name(&clock);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{name}, jump there")));
        // The … sits over the row's right end, so it takes the pointer there;
        // the row is still "under the pointer" while it's on the ….
        let more_rect = Rect::from_center_size(pos2(rect.right() - MORE / 2.0 - 2.0, rect.center().y), vec2(MORE, MORE));
        let more = ui.interact(more_rect, resp.id.with("more"), Sense::click());
        more.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("More for {}", b.title)));
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&more));
        let live = ui.rect_contains_pointer(rect) || resp.has_focus() || more.has_focus() || open;
        if ui.is_rect_visible(rect) {
            if live {
                ui.painter().rect_filled(rect, 4.0, t.hover);
            }
            kit::focus_ring(ui, &resp, rect, 4.0);
            row.paint_glyph(ui, t, rect);
            // The time steps left to make room for the … while it shows.
            let right = if live { more_rect.left() - 4.0 } else { rect.right() };
            let time_g = elide(ui, &clock, FontId::proportional(12.0), t.muted, 72.0);
            let time_w = time_g.size().x;
            ui.painter().galley(pos2(right - time_w, rect.center().y - time_g.size().y / 2.0), time_g, t.muted);
            let title = elide(ui, &b.title, FontId::proportional(13.0), t.text, right - rect.left() - CHAPTER_TITLE_X - time_w - 12.0);
            ui.painter().galley(pos2(rect.left() + CHAPTER_TITLE_X, rect.center().y - title.size().y / 2.0), title, t.text);
            if live {
                if more.hovered() || open {
                    ui.painter().rect_filled(more_rect, 5.0, t.selected);
                }
                kit::focus_ring(ui, &more, more_rect, 5.0);
                icons::paint(ui.painter(), Rect::from_center_size(more_rect.center(), vec2(12.0, 12.0)), Icon::More, t.muted);
            }
        }
        let mut choice = None;
        kit::popover_at(&more, MENU_W, egui::RectAlign::BOTTOM_END, |ui| choice = menu(ui));
        resp.context_menu(|ui| {
            ui.set_width(MENU_W);
            if let Some(c) = menu(ui) {
                choice = Some(c);
            }
        });
        let hover = row.chapter.map_or_else(|| clock.clone(), |chapter| format!("{chapter} · {clock}"));
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(hover).clicked() {
            self.actions.push(Action::JumpToBookmark { item: b.item.clone(), time });
        }
        choice
    }

    /// The row being renamed: its title as a field. `Some` once it's done:
    /// with the new title to keep (Enter, or a click elsewhere), or `None`
    /// to keep the old one (Esc, nothing changed, or nothing typed).
    fn rename_row(&mut self, ui: &mut Ui, t: &Tokens, row: &Row<'_>, edit: &mut Renaming) -> Option<Option<String>> {
        let b = row.bookmark;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), CHAPTER_ROW_H), Sense::hover());
        row.paint_glyph(ui, t, rect);
        let clock = crate::fmt::clock(b.position.secs());
        let time_g = elide(ui, &clock, FontId::proportional(12.0), t.muted, 72.0);
        let time_w = time_g.size().x;
        ui.painter().galley(pos2(rect.right() - time_w, rect.center().y - time_g.size().y / 2.0), time_g, t.muted);
        // The field's text lines up with the titles above and below it.
        let field = Rect::from_min_max(pos2(rect.left() + CHAPTER_TITLE_X - 6.0, rect.top() + 3.0), pos2(rect.right() - time_w - 10.0, rect.bottom() - 3.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field));
        let out = egui::TextEdit::singleline(&mut edit.draft)
            .hint_text("Give it a title")
            .font(FontId::proportional(13.0))
            .margin(vec2(5.0, 4.0))
            .desired_width(field.width())
            .background_color(t.field)
            .show(&mut child);
        let resp = out.response;
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Bookmark title"));
        if edit.fresh {
            // The whole title selected, so typing replaces it.
            edit.fresh = false;
            resp.request_focus();
            let mut state = out.state;
            state.cursor.set_char_range(Some(egui::text::CCursorRange::select_all_str(&edit.draft)));
            state.store(ui.ctx(), resp.id);
            return None;
        }
        if !resp.lost_focus() {
            // A field that went out of view (the column hidden, the tab
            // changed) lost its focus unseen: it comes back as a row.
            return (!resp.has_focus()).then_some(None);
        }
        let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        let title = edit.draft.trim();
        if esc {
            return Some(None);
        }
        if title.is_empty() {
            // A bookmark needs a title: Enter on an empty field keeps it open.
            if enter {
                resp.request_focus();
                return None;
            }
            return Some(None);
        }
        Some((title != b.title).then(|| title.to_owned()))
    }
}

/// Rename and Delete, as a menu's rows.
fn menu(ui: &mut Ui) -> Option<Choice> {
    if kit::menu_row(ui, "Rename") {
        return Some(Choice::Rename);
    }
    if kit::menu_row(ui, "Delete") {
        return Some(Choice::Delete);
    }
    None
}

/// What a row draws from.
struct Row<'a> {
    bookmark: &'a Bookmark,
    chapter: Option<&'a str>,
    /// The bookmark at or just behind where the book is.
    nearest: bool,
}

impl Row<'_> {
    /// What a screen reader hears: the title and the time, and the chapter
    /// when the title doesn't already say it.
    fn name(&self, clock: &str) -> String {
        let title = &self.bookmark.title;
        match self.chapter.filter(|c| !title.contains(c)) {
            Some(chapter) => format!("{title}, {clock}, {chapter}"),
            None => format!("{title}, {clock}"),
        }
    }

    fn paint_glyph(&self, ui: &Ui, t: &Tokens, rect: Rect) {
        let glyph = Rect::from_center_size(pos2(rect.left() + CHAPTER_NUMBER_RIGHT - 5.0, rect.center().y), vec2(10.0, 11.0));
        icons::paint(ui.painter(), glyph, Icon::Bookmark, if self.nearest { t.accent } else { t.muted });
    }
}
