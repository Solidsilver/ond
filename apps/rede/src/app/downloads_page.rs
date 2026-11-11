//! The Downloads place, and how each book's download stands wherever a book
//! is drawn.
//!
//! [`BookDownload`] is one book's download as the interface shows it:
//! preparing (its details are being fetched), on its way, here, or given up
//! on. The map of them is worked out once per change to the store's snapshot
//! or the books being prepared ([`Books::downloads`](super::Books::downloads)),
//! so a cover looks its book up rather than searching the pins.
//!
//! The page lists what's on its way first, then what's here, each with its
//! cover, size and state; the storage used against the limit; and pause,
//! resume and retry for the lot. The list is laid out only where it's on
//! screen. Its progress moves when the store says something changed, never
//! on a timer.
//!
//! While the account is offline, a book that isn't all here can't play: the
//! covers dim it, and its page says "Not downloaded" where Play would be.

use super::Action;
use super::library::{COVER_RADIUS, author_of, stand_in};
use super::nav::Route;
use super::view_ctx::ViewCtx;
use super::virtual_list::VirtualList;
use crate::widgets::elide;
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use offline::Pin;
use shell::icons::{self, Icon};
use shell::kit::{self, DownloadMark};
use shell::theme::Tokens;
use sources::ItemKey;
use std::collections::HashMap;

/// A row on the page: the cover, the title over the author and state.
const ROW_H: f32 = 60.0;
const ROW_COVER: f32 = 44.0;
/// The … at a row's right end, and its menu's width.
const MORE: f32 = 28.0;
const MENU_W: f32 = 200.0;
/// How far a book that can't play now fades: enough to read as out of
/// reach, not so much that its title can't be read.
pub const DIMMED: f32 = 0.45;

/// One book's download, as the covers, the page and the menus say it.
#[derive(Debug, Clone, PartialEq)]
pub enum BookDownload {
    /// The book's details are being fetched before its files can queue.
    Preparing,
    /// In the store and on its way: how much is here, by bytes when the
    /// server said its sizes.
    Downloading { fraction: f32, done: u64, total: u64 },
    /// Every file is here.
    Done { bytes: u64 },
    /// It couldn't be prepared (with why), or every file still missing gave
    /// up.
    Failed { error: Option<String> },
}

impl BookDownload {
    pub fn mark(&self) -> DownloadMark {
        match self {
            Self::Preparing => DownloadMark::Progress(0.0),
            Self::Downloading { fraction, .. } => DownloadMark::Progress(*fraction),
            Self::Done { .. } => DownloadMark::Done,
            Self::Failed { .. } => DownloadMark::Failed,
        }
    }

    /// Still to come (or given up on), rather than here.
    fn active(&self) -> bool {
        !matches!(self, Self::Done { .. })
    }
}

/// Every book's download, by the book's own id: the store's book pins, then
/// those being prepared, then those whose preparing failed.
pub fn book_downloads(state: &crate::downloads::State) -> HashMap<String, BookDownload> {
    let mut out = HashMap::new();
    for p in &state.snapshot.pins {
        let Pin::Book(key) = &p.pin else { continue };
        let Ok(key) = ItemKey::parse(key) else { continue };
        let download = match DownloadMark::of_counts(p.done, p.failed, p.total, p.downloading, p.fraction()) {
            DownloadMark::Done => BookDownload::Done { bytes: p.bytes_done },
            DownloadMark::Failed => BookDownload::Failed { error: None },
            DownloadMark::Progress(fraction) => BookDownload::Downloading { fraction, done: p.bytes_done, total: p.bytes_total },
        };
        out.insert(key.item, download);
    }
    for key in state.preparing.keys() {
        out.insert(key.item.clone(), BookDownload::Preparing);
    }
    for (key, error) in &state.errors {
        out.insert(key.item.clone(), BookDownload::Failed { error: Some(error.clone()) });
    }
    out
}

/// One row of the page: the book's id, its index in the library if it's
/// still there, and the name the store kept for it.
#[derive(Debug, Clone)]
pub struct DownloadRow {
    pub id: String,
    pub book: Option<usize>,
    pub label: String,
}

/// The page's rows: what's still to come first, then what's here, each by
/// title.
pub fn download_rows(books: &[sources::ItemSummary], state: &crate::downloads::State, downloads: &HashMap<String, BookDownload>) -> Vec<DownloadRow> {
    let by_id: HashMap<&str, usize> = books.iter().enumerate().map(|(i, b)| (b.id.as_str(), i)).collect();
    let labels: HashMap<String, &str> = state
        .snapshot
        .pins
        .iter()
        .filter_map(|p| match &p.pin {
            Pin::Book(key) => ItemKey::parse(key).ok().map(|k| (k.item, p.label.as_str())),
            _ => None,
        })
        .collect();
    let mut rows: Vec<(bool, String, DownloadRow)> = downloads
        .iter()
        .map(|(id, d)| {
            let book = by_id.get(id.as_str()).copied();
            let label = book.map(|i| books[i].title.clone()).or_else(|| labels.get(id).map(|l| (*l).to_owned())).unwrap_or_else(|| id.clone());
            (!d.active(), label.to_lowercase(), DownloadRow { id: id.clone(), book, label })
        })
        .collect();
    rows.sort_by(|a, b| (a.0, &a.1, &a.2.id).cmp(&(b.0, &b.1, &b.2.id)));
    rows.into_iter().map(|(_, _, row)| row).collect()
}

/// What a book's download button says, and the state line under a row.
pub fn download_text(download: Option<&BookDownload>, paused: bool, offline: bool) -> String {
    let percent = |f: f32| (f * 100.0).floor() as u32;
    match download {
        None => "Download".into(),
        Some(BookDownload::Preparing) => "Preparing…".into(),
        Some(BookDownload::Downloading { fraction, .. }) if paused => format!("Paused · {}%", percent(*fraction)),
        Some(BookDownload::Downloading { fraction, .. }) if offline => format!("Waiting · {}%", percent(*fraction)),
        Some(BookDownload::Downloading { fraction, .. }) => format!("Downloading {}%", percent(*fraction)),
        Some(BookDownload::Done { .. }) => "Downloaded".into(),
        Some(BookDownload::Failed { .. }) => "Download failed".into(),
    }
}

/// "1.2 GB of 10 GB · 180 GB free", or "Nothing downloaded yet".
pub fn storage_line(snapshot: &offline::Snapshot) -> String {
    let mut text = match (snapshot.used_bytes, snapshot.limit) {
        (0, _) => "Nothing downloaded yet".to_owned(),
        (used, Some(limit)) => format!("{} of {}", crate::fmt::bytes(used), crate::fmt::bytes(limit)),
        (used, None) => format!("{} on this computer", crate::fmt::bytes(used)),
    };
    if let Some(free) = snapshot.free_bytes {
        text.push_str(&format!(" · {} free", crate::fmt::bytes(free)));
    }
    text
}

impl ViewCtx<'_> {
    /// The book's download, if it has one.
    pub(super) fn download(&mut self, id: &str) -> Option<BookDownload> {
        self.books.downloads(self.downloads).get(id).cloned()
    }

    /// The book can't play now: the account is offline and not all of it is
    /// here, by the same test playing makes. The book in the player is
    /// already open, and keeps its controls.
    pub(super) fn unplayable(&self, id: &str) -> bool {
        self.sync.offline
            && !self.player.book.as_ref().is_some_and(|b| b.key.item == id)
            && !self.item_key(id).and_then(|key| self.downloads.book(&key)).is_some_and(offline::PinSummary::complete)
    }

    pub(super) fn item_key(&self, id: &str) -> Option<ItemKey> {
        self.session.account.as_ref().map(|a| ItemKey::new(a.id, id))
    }

    /// The download's commands in a book's menus: Download, or Retry and
    /// Remove download, as apply. Downloading needs the server.
    pub(super) fn download_commands(&mut self, ui: &mut Ui, id: &str) -> Option<Action> {
        let key = self.item_key(id)?;
        let download = self.download(id);
        let mut chosen = None;
        match &download {
            None => {
                if !self.sync.offline && kit::menu_row(ui, "Download") {
                    chosen = Some(Action::DownloadBook(key));
                }
            }
            Some(d) => {
                if matches!(d, BookDownload::Failed { .. }) && !self.sync.offline && kit::menu_row(ui, "Retry download") {
                    chosen = Some(self.retry(key.clone()));
                }
                if kit::menu_row(ui, "Remove download") {
                    chosen = Some(Action::RemoveDownload(key));
                }
            }
        }
        chosen
    }

    /// A failed book tries again: a book that never got as far as the store
    /// is prepared again; files that gave up are retried with the rest.
    pub(super) fn retry(&self, key: ItemKey) -> Action {
        if self.downloads.errors.contains_key(&key) { Action::DownloadBook(key) } else { Action::RetryDownloads }
    }

    pub(super) fn downloads_page(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let downloads = self.books.downloads(self.downloads);
        let rows = self.books.download_rows(self.downloads);
        let snapshot = self.downloads.snapshot.clone();
        let done = downloads.values().filter(|d| !d.active()).count();
        let sub = match done {
            0 => None,
            1 => Some("1 book".to_owned()),
            n => Some(format!("{n} books")),
        };
        let active = downloads.values().filter(|d| matches!(d, BookDownload::Preparing | BookDownload::Downloading { .. })).count();
        let failed = downloads.values().any(|d| matches!(d, BookDownload::Failed { .. }));
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Downloads").font(FontId::new(24.0, shell::theme::semibold())).color(t.text));
            if let Some(sub) = sub {
                ui.label(egui::RichText::new(sub).size(13.0).color(t.muted));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if active > 0 || snapshot.paused {
                    let (label, paused) = if snapshot.paused { ("Resume all", false) } else { ("Pause all", true) };
                    if kit::secondary(ui, label).clicked() {
                        self.actions.push(Action::PauseDownloads(paused));
                    }
                }
                if failed && !self.sync.offline && kit::secondary(ui, "Retry failed").clicked() {
                    // A book that never reached the store is prepared again.
                    for key in self.downloads.errors.keys() {
                        self.actions.push(Action::DownloadBook(key.clone()));
                    }
                    self.actions.push(Action::RetryDownloads);
                }
            });
        });
        ui.add_space(10.0);
        if rows.is_empty() {
            shell::kit::empty_state(ui, Icon::Download, "Nothing downloaded", "Download a book from its page to listen without a connection.");
            return;
        }
        ui.label(egui::RichText::new(storage_line(&snapshot)).size(12.5).color(t.muted));
        let notice = if self.sync.offline {
            Some("Can't reach the server. Downloaded books still play; the rest wait for it.")
        } else if snapshot.paused {
            Some("Downloads are paused.")
        } else if snapshot.limit_reached {
            Some("The storage limit is reached. Some books wait for room; change the limit in Settings.")
        } else {
            None
        };
        if let Some(notice) = notice {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                icons::inline(ui, if self.sync.offline { Icon::Offline } else { Icon::Download }, t.muted);
                ui.label(egui::RichText::new(notice).size(12.5).color(t.muted));
            });
        }
        ui.add_space(12.0);
        VirtualList::grid("downloads", ROW_H, rows.len()).show(ui, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &rows[range] {
                let download = downloads.get(&row.id).cloned();
                self.download_row(ui, &t, row, download.as_ref(), snapshot.paused);
            }
        });
    }

    /// One book on the page: its cover, title, author and state, its mark,
    /// and the … with Retry and Remove download. A click opens the book.
    fn download_row(&mut self, ui: &mut Ui, t: &Tokens, row: &DownloadRow, download: Option<&BookDownload>, paused: bool) {
        let width = ui.available_width().min(kit::LIST_W);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
        let book = row.book.map(|i| self.books.all()[i].clone());
        let state = download_text(download, paused, self.sync.offline);
        let detail = match download {
            Some(BookDownload::Done { bytes }) => crate::fmt::bytes(*bytes),
            Some(BookDownload::Downloading { done, total, .. }) if *total > 0 => {
                format!("{state} · {} of {}", crate::fmt::bytes(*done), crate::fmt::bytes(*total))
            }
            Some(BookDownload::Failed { error: Some(error) }) => format!("{state} · {error}"),
            _ => state.clone(),
        };
        let author = book.as_ref().map(author_of).filter(|a| !a.is_empty()).unwrap_or("Unknown author").to_owned();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{} by {author}, {detail}", row.label)));
        let more_rect = Rect::from_center_size(pos2(rect.right() - MORE / 2.0 - 6.0, rect.center().y), vec2(MORE, MORE));
        let more = ui.interact(more_rect, resp.id.with("more"), Sense::click());
        more.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("More for {}", row.label)));
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&more));
        if ui.is_rect_visible(rect) {
            ui.painter().rect_filled(rect, 6.0, kit::row_fill(t, open, resp.hovered() || more.hovered()));
            kit::focus_ring(ui, &resp, rect, 6.0);
            let cover = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - ROW_COVER / 2.0), vec2(ROW_COVER, ROW_COVER));
            match &book {
                Some(b) => self.art.paint(ui, cover, b.cover.as_deref(), COVER_RADIUS / 2.0, &stand_in(b)),
                None => kit::generated_cover(ui, cover, COVER_RADIUS / 2.0, &kit::Generated { title: &row.label, byline: None, seed: &row.id }),
            }
            let x = cover.right() + 12.0;
            let mark_x = more_rect.left() - 14.0;
            let room = (mark_x - 14.0 - x).max(20.0);
            let title = elide(ui, &row.label, FontId::proportional(13.0), t.text, room);
            let title_h = title.size().y;
            let line_color = if matches!(download, Some(BookDownload::Failed { .. })) { t.warn } else { t.muted };
            let line = elide(ui, &format!("{author} · {detail}"), FontId::proportional(12.0), line_color, room);
            let top = rect.center().y - (title_h + 3.0 + line.size().y) / 2.0;
            ui.painter().galley(pos2(x, top), title, t.text);
            ui.painter().galley(pos2(x, top + title_h + 3.0), line, line_color);
            if let Some(d) = download {
                kit::download_mark(ui, pos2(mark_x, rect.center().y), 13.0, d.mark());
            }
            if more.hovered() || open {
                ui.painter().rect_filled(more_rect, 5.0, t.selected);
            }
            kit::focus_ring(ui, &more, more_rect, 5.0);
            icons::paint(ui.painter(), Rect::from_center_size(more_rect.center(), vec2(12.0, 12.0)), Icon::More, t.muted);
        }
        let mut chosen = None;
        kit::popover_at(&more, MENU_W, egui::RectAlign::BOTTOM_END, |ui| chosen = self.download_commands(ui, &row.id));
        resp.context_menu(|ui| {
            ui.set_width(MENU_W);
            if let Some(c) = self.download_commands(ui, &row.id) {
                chosen = Some(c);
            }
        });
        if let Some(action) = chosen {
            self.actions.push(action);
        } else if resp.clicked() && book.is_some() {
            self.actions.push(Action::Go(Route::Book(row.id.clone())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(key: &ItemKey, done: usize, failed: usize, total: usize, downloading: bool) -> offline::PinSummary {
        offline::PinSummary {
            pin: Pin::Book(key.to_string()),
            label: key.item.clone(),
            total,
            done,
            failed,
            bytes_done: done as u64 * 100,
            bytes_total: total as u64 * 100,
            downloading,
        }
    }

    #[test]
    fn each_book_reads_as_its_pin_with_preparing_and_errors_on_top() {
        let account = sources::AccountId::from_uuid(uuid::Uuid::from_u128(7));
        let key = |id: &str| ItemKey::new(account, id);
        let mut state = crate::downloads::State {
            snapshot: std::sync::Arc::new(offline::Snapshot {
                pins: vec![pin(&key("here"), 2, 0, 2, false), pin(&key("coming"), 1, 0, 4, true), pin(&key("gave-up"), 1, 1, 2, false)],
                ..Default::default()
            }),
            ..Default::default()
        };
        state.preparing.insert(key("asked"), 1);
        state.errors.insert(key("unreachable"), "Can't reach the server.".into());
        let all = book_downloads(&state);
        assert_eq!(all["here"], BookDownload::Done { bytes: 200 });
        assert_eq!(all["coming"], BookDownload::Downloading { fraction: 0.25, done: 100, total: 400 });
        assert_eq!(all["gave-up"], BookDownload::Failed { error: None });
        assert_eq!(all["asked"], BookDownload::Preparing);
        assert_eq!(all["unreachable"], BookDownload::Failed { error: Some("Can't reach the server.".into()) });
        let rows = download_rows(&[], &state, &all);
        let order: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(order, ["asked", "coming", "gave-up", "unreachable", "here"], "what's still to come first, then what's here");
    }

    #[test]
    fn the_button_says_where_the_download_stands() {
        let coming = BookDownload::Downloading { fraction: 0.426, done: 0, total: 0 };
        assert_eq!(download_text(None, false, false), "Download");
        assert_eq!(download_text(Some(&coming), false, false), "Downloading 42%");
        assert_eq!(download_text(Some(&coming), true, false), "Paused · 42%");
        assert_eq!(download_text(Some(&coming), false, true), "Waiting · 42%");
        assert_eq!(download_text(Some(&BookDownload::Done { bytes: 1 }), false, false), "Downloaded");
    }

    #[test]
    fn storage_reads_against_the_limit_when_there_is_one() {
        let mut snapshot = offline::Snapshot { used_bytes: 1_200_000_000, free_bytes: Some(180_000_000_000), ..Default::default() };
        assert_eq!(storage_line(&snapshot), "1.2 GB on this computer · 180 GB free");
        snapshot.limit = Some(10_000_000_000);
        assert_eq!(storage_line(&snapshot), "1.2 GB of 10 GB · 180 GB free");
        snapshot.used_bytes = 0;
        assert!(storage_line(&snapshot).starts_with("Nothing downloaded yet"));
    }
}
