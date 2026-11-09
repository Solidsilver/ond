//! A book's page: its cover, what the listing knows (title, authors,
//! narrators, series and number, length), the description and the chapters,
//! fetched with the book's details when the page opens.
//!
//! Playback isn't here yet (the engine work is on another branch), so the Play
//! button is drawn disabled and says so.

use super::library::authors_line;
use super::view_ctx::ViewCtx;
use super::{Action, Details};
use crate::fmt;
use crate::widgets::elide;
use egui::{FontId, RichText, Sense, Ui, pos2, vec2};
use shell::icons::Icon;
use shell::theme::{Tokens, semibold};
use sources::ItemSummary;

/// A square cover, matching the library tiles.
const COVER_W: f32 = 176.0;
/// What a disabled Play button says when hovered.
const PLAY_LATER: &str = "Playback comes next";

impl ViewCtx<'_> {
    pub(super) fn book_page(&mut self, ui: &mut Ui, id: &str) {
        let t = Tokens::of(ui);
        let back = if self.nav.can_back() { "← Back" } else { "← Library" };
        if crate::widgets::link(ui, back, FontId::proportional(12.0), t.muted).clicked() {
            self.actions.push(Action::Back);
        }
        ui.add_space(8.0);
        let Some(book) = self.books.by_id(id).cloned() else {
            shell::widgets::empty_state(ui, Icon::Headphones, "This book isn't here", "It may have been removed from the server. Sync again to see.");
            return;
        };
        egui::ScrollArea::vertical().id_salt(("book", id)).auto_shrink(false).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(COVER_W, COVER_W), Sense::hover());
                self.art.paint(ui, rect, book.cover.as_deref(), 6.0);
                ui.add_space(22.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&book.title).font(FontId::new(24.0, semibold())).color(t.text));
                    if let Some(subtitle) = &book.subtitle {
                        ui.label(RichText::new(subtitle).size(14.0).color(t.muted));
                    }
                    ui.add_space(2.0);
                    ui.label(RichText::new(authors_line(&book)).size(14.0).color(t.text));
                    let facts = facts_line(&book);
                    if !facts.is_empty() {
                        ui.add_space(2.0);
                        ui.label(RichText::new(facts).size(12.0).color(t.muted));
                    }
                    ui.add_space(14.0);
                    // Playback is the engine branch's work: the button shows
                    // where it will be, and says when it comes.
                    let play = egui::Button::new(RichText::new("Play").font(FontId::new(13.0, semibold())).color(t.on_accent))
                        .fill(t.accent)
                        .corner_radius(7)
                        .min_size(vec2(96.0, 32.0));
                    ui.add_enabled(false, play).on_disabled_hover_text(PLAY_LATER);
                });
            });
            ui.add_space(20.0);
            self.details(ui, id, &t);
            ui.add_space(10.0);
            if let Some(Details::Loaded(item)) = self.books.details.get(id).cloned() {
                if let Some(description) = item.description.as_deref().filter(|d| !d.is_empty()) {
                    ui.label(RichText::new("About this book").font(FontId::new(15.0, semibold())).color(t.text));
                    ui.add_space(4.0);
                    ui.scope(|ui| {
                        ui.set_max_width(ui.available_width().min(720.0));
                        ui.add(egui::Label::new(RichText::new(description).size(13.0).color(t.text)).wrap());
                    });
                    ui.add_space(18.0);
                }
                chapters(ui, &t, &item.chapters);
            }
            ui.add_space(24.0);
        });
    }

    /// What the page says while the details are on their way, or when they
    /// couldn't be had.
    fn details(&self, ui: &mut Ui, id: &str, t: &Tokens) {
        match self.books.details.get(id) {
            None | Some(Details::Loading) => {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(13.0, 13.0), Sense::hover());
                    shell::icons::paint(ui.painter(), rect, Icon::Spinner, t.muted);
                    ui.label(RichText::new("Loading the chapters…").size(12.0).color(t.muted));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                });
            }
            Some(Details::Failed(why)) => {
                ui.label(RichText::new(why).size(12.0).color(t.danger));
            }
            Some(Details::Loaded(_)) => {}
        }
    }
}

/// The line under the authors: narrators, series and number, and how long the
/// book is, each only when the server said.
pub fn facts_line(book: &ItemSummary) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !book.narrators.is_empty() {
        parts.push(format!("Read by {}", book.narrators.join(", ")));
    }
    for series in &book.series {
        parts.push(match &series.sequence {
            Some(n) => format!("{} #{n}", series.name),
            None => series.name.clone(),
        });
    }
    if let Some(duration) = book.duration {
        parts.push(fmt::span(duration));
    }
    parts.join(" · ")
}

/// The chapter list: each chapter's title and how long it runs.
fn chapters(ui: &mut Ui, t: &Tokens, chapters: &[sources::Chapter]) {
    if chapters.is_empty() {
        return;
    }
    ui.label(RichText::new("Chapters").font(FontId::new(15.0, semibold())).color(t.text));
    ui.add_space(6.0);
    // A column, like the description: a chapter's length belongs beside its
    // title, not at the far edge of a wide window.
    ui.scope(|ui| {
        ui.set_max_width(ui.available_width().min(720.0));
        // A book rarely has more than a few hundred chapters, and this list is
        // drawn only while the book's page is open.
        for (n, chapter) in chapters.iter().enumerate() {
            let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
            let length = fmt::clock((chapter.end - chapter.start).max(0.0));
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{} · {}", chapter.title, length)));
            let number = format!("{}", n + 1);
            ui.painter().text(pos2(row.left(), row.center().y), egui::Align2::LEFT_CENTER, number, FontId::proportional(12.0), t.muted);
            let title = elide(ui, &chapter.title, FontId::proportional(13.0), t.text, row.width() - 120.0);
            ui.painter().galley(pos2(row.left() + 34.0, row.center().y - title.size().y / 2.0), title, t.text);
            let time = elide(ui, &length, FontId::proportional(12.0), t.muted, 64.0);
            ui.painter().galley(pos2(row.right() - time.size().x, row.center().y - time.size().y / 2.0), time, t.muted);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sources::Series;

    fn book() -> ItemSummary {
        ItemSummary { id: "b".into(), title: "The Left Hand of Darkness".into(), authors: vec!["Ursula K. Le Guin".into()], ..ItemSummary::default() }
    }

    #[test]
    fn the_facts_line_says_only_what_the_server_said() {
        assert_eq!(facts_line(&book()), "", "no narrator, series or length: nothing to say");
        let mut b = book();
        b.narrators = vec!["George Guidall".into()];
        b.duration = Some(41_000.0);
        assert_eq!(facts_line(&b), "Read by George Guidall · 11 hr 24 min");
        b.series = vec![Series { name: "Hainish".into(), sequence: Some("2".into()) }];
        assert_eq!(facts_line(&b), "Read by George Guidall · Hainish #2 · 11 hr 24 min");
        b.series[0].sequence = None;
        assert!(facts_line(&b).contains("Hainish ·"), "a book with no number in its series says so plainly");
    }
}
