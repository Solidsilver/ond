//! A playlist: its mosaic, name and length, Play / Shuffle / ⋯, then its
//! tracks in the shared track list. The user's own can be renamed, deleted,
//! reordered and trimmed.

use super::track_list::ListOpts;
use super::view_ctx::ViewCtx;
use super::{Action, Keep, OfflineAction, Overlay};
use crate::icons::{self, Icon};
use crate::queue::Source;
use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use library::Track;

impl ViewCtx<'_> {
    /// A playlist: a mosaic of its first albums' covers, name, length, Play /
    /// Shuffle / ⋯, then its tracks. The user's own playlists can be renamed,
    /// deleted, reordered by dragging (or ⌥↑/↓) and trimmed (⌫).
    pub(super) fn playlist_page(&mut self, ui: &mut Ui, id: &str, name: &str) {
        let t = Tokens::of(ui);
        // Borrowed from the library, not from `self`, so drawing can take `&mut self`.
        let data = self.data;
        let info = data.playlists.iter().find(|p| p.id == id);
        let tracks = data.playlist_tracks.get(id);
        let mine = info.is_some_and(|p| self.owns(p));
        let source = Source::Playlist { id: id.to_owned(), name: name.to_owned() };
        let pin = offline::Pin::Playlist(id.to_owned());
        let kept = self.downloads.snap.pin(&pin).is_some();
        if self.download_key(ui) {
            self.actions.push(Action::Offline(OfflineAction::Keep(Keep::Playlist(id.to_owned()), !kept)));
        }

        let art_side = (ui.available_width() * 0.3).clamp(120.0, 180.0);
        ui.horizontal(|ui| {
            let (art, _) = ui.allocate_exact_size(vec2(art_side, art_side), Sense::hover());
            self.mosaic(ui, art, tracks.map_or(&[][..], |ts| ts.as_slice()));
            ui.add_space(20.0);
            ui.vertical(|ui| {
                ui.add_space((art_side - 118.0).max(0.0));
                ui.label(egui::RichText::new("Playlist").font(FontId::new(11.0, semibold())).color(t.muted));
                ui.add(egui::Label::new(egui::RichText::new(name).font(FontId::new(26.0, semibold())).color(t.text)).truncate());
                // The loaded songs are newer than the playlist list's counts
                // (an edit since the last refresh).
                let counts = match (tracks, info) {
                    (Some(ts), _) => Some((ts.len(), ts.iter().filter_map(|t| t.duration_secs).sum::<u32>())),
                    (None, Some(p)) => Some((p.song_count as usize, p.duration)),
                    (None, None) => None,
                };
                let sub = match counts {
                    Some((n, secs)) if n > 0 => format!("{} · {}", crate::fmt::plural(n, "song"), crate::fmt::span(secs)),
                    Some(_) => "Empty".to_owned(),
                    None => String::new(),
                };
                ui.label(egui::RichText::new(sub).size(12.0).color(t.muted));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let has_tracks = tracks.is_some_and(|t| !t.is_empty());
                    ui.add_enabled_ui(has_tracks, |ui| {
                        if let Some(a) = super::browse::play_shuffle(ui) {
                            self.actions.push(Action::Playlist(id.to_owned(), a));
                        }
                    });
                    let more = icons::sized_button(ui, Icon::More, "More", vec2(30.0, 30.0), 13.0, Some(t.accent));
                    egui::Popup::menu(&more).show(|ui| {
                        let c = super::browse::Collection::Playlist(id.to_owned());
                        super::browse::collection_menu(ui, &c, false, kept, Some("⇧⌘D"), self.actions);
                        if mine {
                            ui.separator();
                            self.playlist_edit_items(ui, id, name);
                        }
                    });
                    ui.add_space(6.0);
                    self.pin_status(ui, &pin);
                });
            });
        });
        ui.add_space(20.0);
        let Some(tracks) = tracks else {
            crate::widgets::empty_state(ui, Icon::List, "Loading…", "");
            return;
        };
        if tracks.is_empty() {
            let hint = if mine {
                "Drag songs onto this playlist in the sidebar, or use “Add to playlist” on any song."
            } else {
                "Its owner hasn’t added any songs."
            };
            crate::widgets::empty_state(ui, Icon::Note, "This playlist is empty", hint);
            return;
        }
        let scroll = egui::ScrollArea::vertical().id_salt(("playlist", id)).auto_shrink(false).content_margin(super::layout::gutter());
        super::layout::into_gutter(ui, |ui| {
            self.track_list_rows(ui, scroll, tracks, &source, ListOpts { numbers: false, artist: true, playlist: mine.then(|| id.to_owned()) })
        });
    }

    /// Rename… and Delete… for one of the user's playlists.
    pub(super) fn playlist_edit_items(&mut self, ui: &mut Ui, id: &str, name: &str) {
        if ui.button("Rename…").clicked() {
            self.actions.push(Action::Overlay(Overlay::Rename { id: id.to_owned(), name: name.to_owned() }));
        }
        if ui.button("Delete…").clicked() {
            self.actions.push(Action::Overlay(Overlay::Delete { id: id.to_owned(), name: name.to_owned() }));
        }
    }

    /// Playlists the user can change (others' public ones are read-only).
    pub(super) fn owns(&self, p: &subsonic::Playlist) -> bool {
        super::sidebar::owns(self.session, p)
    }

    /// Up to four album covers in a 2×2 square (one fills it; none: a note glyph).
    pub(super) fn mosaic(&mut self, ui: &mut Ui, rect: Rect, tracks: &[Track]) {
        let t = Tokens::of(ui);
        let data = self.data;
        let mut covers: Vec<&str> = Vec::with_capacity(4);
        for tr in tracks {
            if let Some(c) = data.cover_for(tr)
                && !covers.contains(&c)
            {
                covers.push(c);
                if covers.len() == 4 {
                    break;
                }
            }
        }
        match covers.len() {
            0 => {
                ui.painter().rect_filled(rect, 6.0, t.field);
                icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(rect.width() * 0.28, rect.width() * 0.28)), Icon::Note, t.muted);
            }
            1..=3 => self.art.paint(ui, rect, Some(covers[0]), 6.0),
            _ => {
                let half = rect.width() / 2.0;
                for (i, c) in covers.iter().enumerate() {
                    let at = pos2(rect.left() + half * (i % 2) as f32, rect.top() + half * (i / 2) as f32);
                    // Only the mosaic's outer corners are rounded.
                    let r = 6;
                    let corners = match i {
                        0 => egui::CornerRadius { nw: r, ..Default::default() },
                        1 => egui::CornerRadius { ne: r, ..Default::default() },
                        2 => egui::CornerRadius { sw: r, ..Default::default() },
                        _ => egui::CornerRadius { se: r, ..Default::default() },
                    };
                    self.art.paint(ui, Rect::from_min_size(at, vec2(half, half)), Some(c), corners);
                }
            }
        }
    }
}
