//! The Offline place: what's on this computer, what's still on its way, and
//! how much room it takes. Also the one-line download status shown on album
//! and playlist pages.

use super::browse::grid_metrics;
use super::widgets::{Mark, danger, elide, link, secondary};
use super::view_ctx::ViewCtx;
use super::{Action, QueueAction};
use crate::icons::{self, Icon};
use crate::nav::Route;
use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use offline::{Pin, PinSummary, SetAside, Snapshot};

/// Storage caps offered in the limit menu, in GB.
const LIMITS: [u64; 6] = [10, 25, 50, 100, 250, 500];

/// "4.2 GB on this computer · 180 GB free".
pub(super) fn storage_used(snap: &Snapshot) -> String {
    let mut text = format!("{} on this computer", crate::fmt::bytes(snap.used_bytes));
    if let Some(free) = snap.free_bytes {
        text.push_str(&format!(" · {} free", crate::fmt::bytes(free)));
    }
    text
}

/// The storage limit's choices, as radio items in a menu.
pub(super) fn limit_choices(ui: &mut Ui, current: Option<u64>, actions: &mut Vec<Action>) {
    if ui.radio(current.is_none(), "No limit").clicked() {
        actions.push(Action::SetOfflineLimit(None));
    }
    for gb in LIMITS {
        let bytes = gb * 1_000_000_000;
        if ui.radio(current == Some(bytes), format!("{gb} GB")).clicked() {
            actions.push(Action::SetOfflineLimit(Some(bytes)));
        }
    }
}

/// "12 files (3.4 GB)".
fn files_and_bytes(a: &SetAside) -> String {
    format!("{} ({})", crate::fmt::plural(a.unknown, "file"), crate::fmt::bytes(a.unknown_bytes))
}

/// The notice's first lines: what happened and what's left in the folder.
pub(super) fn set_aside_text(a: &SetAside) -> (&'static str, String) {
    let title = "Ond couldn't read its list of downloads";
    let body = match a.unknown {
        0 => "None of the files it listed are left in the downloads folder.".to_owned(),
        1 => format!("{} it no longer knows is kept in the downloads folder.", files_and_bytes(a)),
        _ => format!("{} it no longer knows are kept in the downloads folder.", files_and_bytes(a)),
    };
    (title, body)
}

/// After the downloads list couldn't be read (see [`offline::SetAside`]): says
/// so, and offers to show the folder or remove the files it no longer knows,
/// after asking. `place` keeps each place's "are you sure" apart.
pub(super) fn set_aside_notice(ui: &mut Ui, snap: &Snapshot, actions: &mut Vec<Action>, place: &str) {
    let Some(a) = &snap.set_aside else { return };
    let t = Tokens::of(ui);
    let confirm_id = egui::Id::unique("set-aside-confirm").with(place);
    let mut confirming = ui.data(|d| d.get_temp::<bool>(confirm_id).unwrap_or(false)) && !a.removing;
    // Outlined, not filled: the buttons inside are filled with `field`.
    egui::Frame::new().stroke(egui::Stroke::new(1.0, t.hairline)).corner_radius(8).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
        ui.set_max_width(560.0);
        let (title, body) = set_aside_text(a);
        ui.label(egui::RichText::new(title).font(FontId::new(13.0, semibold())).color(t.text));
        ui.add_space(2.0);
        ui.add(egui::Label::new(egui::RichText::new(body).size(12.0).color(t.muted)).wrap());
        ui.add_space(10.0);
        if a.removing {
            ui.label(egui::RichText::new("Removing…").size(12.0).color(t.muted));
        } else if confirming {
            let ask = format!("Delete {} from the downloads folder? This can't be undone.", files_and_bytes(a));
            ui.add(egui::Label::new(egui::RichText::new(ask).size(12.0).color(t.text)).wrap());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if danger(ui, "Delete").clicked() {
                    actions.push(Action::RemoveSetAside);
                    confirming = false;
                }
                ui.add_space(4.0);
                if secondary(ui, "Cancel").clicked() {
                    confirming = false;
                }
            });
        } else {
            ui.horizontal(|ui| {
                if a.unknown > 0 {
                    if secondary(ui, &format!("Show in {}", crate::folder::FILE_MANAGER)).clicked() {
                        actions.push(Action::RevealSetAside);
                    }
                    ui.add_space(4.0);
                    if secondary(ui, "Remove them…").clicked() {
                        confirming = true;
                    }
                } else if secondary(ui, "Dismiss").clicked() {
                    // Nothing to delete but the unreadable list itself.
                    actions.push(Action::RemoveSetAside);
                }
            });
            ui.add_space(6.0);
            let why = if a.unknown > 0 { "Until then, Ond doesn't tidy up that folder." } else { "Until then, Ond doesn't tidy up the downloads folder." };
            ui.label(egui::RichText::new(why).size(12.0).color(t.muted));
        }
    });
    ui.data_mut(|d| d.insert_temp(confirm_id, confirming));
}

/// Why a pin isn't finished, in a few words.
pub fn pin_text(snap: &Snapshot, s: &PinSummary) -> String {
    let count = format!("{} of {}", s.done, s.total);
    if s.complete() {
        return format!("Downloaded · {}", crate::fmt::bytes(s.bytes_done));
    }
    if snap.paused {
        format!("Paused · {count}")
    } else if s.failed > 0 && !s.downloading && s.done + s.failed == s.total {
        format!("{} couldn't download · {count}", s.failed)
    } else if !snap.online && !s.downloading {
        format!("Waiting for the server · {count}")
    } else if snap.limit_reached && !s.downloading {
        format!("Storage limit reached · {count}")
    } else {
        format!("Downloading · {count}")
    }
}

impl ViewCtx<'_> {
    /// A mark and a few words next to the Play buttons, only when `pin` is kept.
    pub(super) fn pin_status(&self, ui: &mut Ui, pin: &Pin) {
        let Some(s) = self.downloads.snap.pin(pin) else { return };
        let Some(m) = Mark::of_pin(Some(s)) else { return };
        let t = Tokens::of(ui);
        let (r, resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        super::widgets::mark(ui, r.center(), 12.0, m);
        resp.on_hover_text(m.hint());
        ui.label(egui::RichText::new(pin_text(&self.downloads.snap, s)).size(12.0).color(t.muted));
    }

    pub(super) fn offline_page(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let snap = self.downloads.snap.clone();
        let albums: Vec<(usize, &PinSummary)> =
            snap.pins.iter().filter_map(|p| if let Pin::Album(id) = &p.pin { self.data.by_id.get(id).map(|&i| (i, p)) } else { None }).collect();
        let playlists: Vec<&PinSummary> = snap.pins.iter().filter(|p| matches!(p.pin, Pin::Playlist(_))).collect();
        let songs: Vec<&PinSummary> = snap.pins.iter().filter(|p| matches!(p.pin, Pin::Track(_))).collect();

        let mut parts = Vec::new();
        for (n, word) in [(albums.len(), "album"), (playlists.len(), "playlist"), (songs.len(), "song")] {
            if n > 0 {
                parts.push(crate::fmt::plural(n, word));
            }
        }
        self.heading(ui, "Offline", (!parts.is_empty()).then(|| parts.join(" · ")));

        if !self.session.online && self.session.settings.is_some() {
            ui.horizontal(|ui| {
                icons::inline(ui, Icon::Offline, t.muted);
                ui.label(egui::RichText::new("Can't reach the server. Downloaded music still plays.").size(12.5).color(t.muted));
            });
            ui.add_space(10.0);
        }

        self.storage_line(ui, &snap);
        ui.add_space(14.0);

        if snap.set_aside.is_some() {
            set_aside_notice(ui, &snap, self.actions, "page");
            ui.add_space(18.0);
        }

        if snap.pins.is_empty() {
            super::widgets::empty_state(
                ui,
                Icon::Download,
                "Nothing downloaded yet",
                "Choose “Make available offline” from an album’s or playlist’s menu, or press ⇧⌘D on its page.",
            );
            return;
        }

        super::layout::into_gutter(ui, |ui| egui::ScrollArea::vertical().id_salt("offline").auto_shrink(false).content_margin(super::layout::gutter()).show(ui, |ui| {
            let in_progress: Vec<&PinSummary> = snap.pins.iter().filter(|p| !p.complete()).collect();
            if !in_progress.is_empty() {
                self.in_progress(ui, &snap, &in_progress);
                ui.add_space(22.0);
            }
            if !albums.is_empty() {
                super::widgets::section(ui, "Albums");
                ui.add_space(8.0);
                let (cols, tile) = grid_metrics(ui.available_width());
                let ids: Vec<usize> = albums.iter().map(|(i, _)| *i).collect();
                for chunk in ids.chunks(cols) {
                    self.grid_row(ui, chunk, tile);
                }
            }
            if !playlists.is_empty() {
                ui.add_space(6.0);
                super::widgets::section(ui, "Playlists");
                ui.add_space(6.0);
                for p in &playlists {
                    self.pin_row(ui, &snap, p);
                }
                ui.add_space(16.0);
            }
            if !songs.is_empty() {
                super::widgets::section(ui, "Songs");
                ui.add_space(6.0);
                for p in &songs {
                    self.pin_row(ui, &snap, p);
                }
            }
            ui.add_space(24.0);
        }));
    }

    /// The storage summary, then the limit and Finder.
    pub(super) fn storage_line(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let t = Tokens::of(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(egui::RichText::new(storage_used(snap)).size(12.0).color(t.muted));
            ui.label(egui::RichText::new("  ·  ").size(12.0).color(t.muted));
            let limit = match snap.limit {
                Some(b) => format!("Limit {}", crate::fmt::bytes(b)),
                None => "No limit".to_owned(),
            };
            let resp = link(ui, &limit, FontId::proportional(12.0), t.muted);
            egui::Popup::menu(&resp).show(|ui| {
                ui.label(egui::RichText::new("Storage limit").font(FontId::new(12.0, semibold())));
                limit_choices(ui, snap.limit, self.actions);
            });
            ui.label(egui::RichText::new("  ·  ").size(12.0).color(t.muted));
            let resp = link(ui, "Location", FontId::proportional(12.0), t.muted).on_hover_text(self.downloads.base.display().to_string());
            egui::Popup::menu(&resp).show(|ui| {
                ui.label(egui::RichText::new(self.downloads.base.display().to_string()).size(11.0).color(t.muted));
                if ui.button(format!("Show in {}", crate::folder::FILE_MANAGER)).clicked() {
                    self.actions.push(Action::RevealOffline);
                }
                if ui.button("Change…").clicked() {
                    self.actions.push(Action::MoveOffline);
                }
            });
        });
    }

    /// What's still downloading, with pause / resume and retry.
    pub(super) fn in_progress(&mut self, ui: &mut Ui, snap: &Snapshot, pins: &[&PinSummary]) {
        let t = Tokens::of(ui);
        ui.horizontal(|ui| {
            super::widgets::section(ui, "In progress");
            ui.add_space(10.0);
            let (label, pause) = if snap.paused { ("Resume", false) } else { ("Pause", true) };
            if link(ui, label, FontId::proportional(12.0), t.accent).clicked() {
                self.actions.push(Action::PauseDownloads(pause));
            }
            if snap.failed > 0 {
                ui.add_space(10.0);
                if link(ui, "Retry failed", FontId::proportional(12.0), t.accent).clicked() {
                    self.actions.push(Action::RetryDownloads);
                }
            }
        });
        ui.add_space(6.0);
        for p in pins {
            self.pin_row(ui, snap, p);
        }
    }

    /// One kept album / playlist / song: art, name, status, and a thin bar
    /// while it's on its way. Click opens it; right-click removes.
    pub(super) fn pin_row(&mut self, ui: &mut Ui, snap: &Snapshot, p: &PinSummary) {
        let t = Tokens::of(ui);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().min(760.0), 48.0), Sense::click());
        // Off screen: keep the space, skip the rest (thousands of kept songs).
        if !ui.is_rect_visible(rect) {
            return;
        }
        let (cover, name, open) = match &p.pin {
            Pin::Album(id) => {
                let a = self.data.album(id);
                (a.and_then(|a| a.cover_art.clone()), a.map_or_else(|| p.label.clone(), |a| a.name.clone()), Some(Route::Album(id.clone())))
            }
            Pin::Playlist(id) => (None, p.label.clone(), Some(Route::Playlist { id: id.clone(), name: p.label.clone() })),
            Pin::Track(id) => {
                let tr = self.data.track(id);
                let cover = tr.and_then(|t| self.data.cover_for(t)).map(str::to_owned);
                (cover, p.label.clone(), tr.map(|t| Route::Album(t.album_id.clone())))
            }
        };
        if ui.is_rect_visible(rect) {
            if resp.hovered() {
                ui.painter().rect_filled(rect, 6.0, t.hover);
            }
            let art = Rect::from_min_size(pos2(rect.left() + 6.0, rect.top() + 6.0), vec2(36.0, 36.0));
            match (&p.pin, cover) {
                (Pin::Playlist(_), _) => {
                    ui.painter().rect_filled(art, 4.0, t.field);
                    icons::paint(ui.painter(), art.shrink(11.0), Icon::List, t.muted);
                }
                (_, cover) => self.art.paint(ui, art, cover.as_deref(), 4.0),
            }
            let x = art.right() + 12.0;
            let right_w = 150.0;
            let w = rect.right() - x - right_w - 10.0;
            let g = elide(ui, &name, FontId::proportional(13.0), t.text, w);
            ui.painter().galley(pos2(x, rect.center().y - g.size().y), g, t.text);
            let status = pin_text(snap, p);
            let g = elide(ui, &status, FontId::proportional(12.0), t.muted, w);
            ui.painter().galley(pos2(x, rect.center().y + 1.0), g, t.muted);
            if !p.complete() {
                let bar = Rect::from_min_size(pos2(rect.right() - right_w, rect.center().y - 1.5), vec2(right_w - 34.0, 3.0));
                ui.painter().rect_filled(bar, 1.5, t.hairline);
                let mut fill = bar;
                fill.set_width(bar.width() * p.fraction());
                ui.painter().rect_filled(fill, 1.5, if snap.paused { t.muted } else { t.accent });
            }
            if let Some(m) = Mark::of_pin(Some(p)) {
                super::widgets::mark(ui, pos2(rect.right() - 14.0, rect.center().y), 12.0, m);
            }
        }
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
        if resp.double_clicked() {
            match &p.pin {
                Pin::Album(id) => self.actions.push(Action::Album(id.clone(), QueueAction::Play { start: 0 })),
                Pin::Playlist(id) => self.actions.push(Action::Playlist(id.clone(), QueueAction::Play { start: 0 })),
                Pin::Track(id) => {
                    if let Some(tr) = self.data.track(id).cloned() {
                        self.play_from_album(&tr, QueueAction::Play { start: 0 });
                    }
                }
            }
        } else if resp.clicked()
            && let Some(route) = open
        {
            self.actions.push(Action::Go(route));
        }
        resp.context_menu(|ui| {
            if ui.button("Remove download").clicked() {
                self.actions.push(Action::Unpin(p.pin.clone()));
            }
        });
    }
}
