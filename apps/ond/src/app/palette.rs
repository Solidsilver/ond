//! ⌘K: search the library and act on a result without leaving the keyboard.
//! ⏎ opens · ⌘⏎ plays · ⌥⏎ plays next · ⇧⏎ adds to the queue.
//!
//! The sheet is the kit's (`kit::palette`); this says what the rows are and
//! what the keys do with them.

use super::OfflineAction;
use super::view_ctx::ViewCtx;
use super::{Action, QueueAction};
use crate::nav::Route;
use crate::search::{Hit, Results, SearchIndex};
use egui::Rect;
use shell::kit::{self, PaletteRow, PaletteState};

#[derive(Default)]
pub struct Palette {
    /// What's typed, and the row selected.
    pub(super) state: PaletteState,
    results: Results,
}

impl Palette {
    /// Re-runs the query after the index changed.
    pub fn refresh(&mut self, index: &mut SearchIndex) {
        self.results = index.search(&self.state.query);
        self.state.selected = self.state.selected.min(self.results.len().saturating_sub(1));
    }
}

#[derive(Clone, Copy)]
enum Verb {
    Open,
    Play,
    PlayNext,
    Add,
    /// Toggle keeping it offline.
    Download,
}

const TEXT: kit::PaletteText = kit::PaletteText {
    placeholder: "Search artists, albums and songs",
    footer: "⏎ open    ⌘⏎ play    ⌥⏎ play next    ⇧⏎ add to queue    ⇧⌘D download",
    no_matches: "No matches",
};

impl ViewCtx<'_> {
    pub(super) fn palette_ui(&mut self, ctx: &egui::Context) {
        if self.ui.palette.is_none() {
            return;
        }
        if self.search.index.is_none() {
            self.ui.palette = None;
            return;
        }
        let mut verb = palette_keys(ctx);
        let (Some(palette), Some(index)) = (self.ui.palette.as_mut(), self.search.index.as_mut()) else { return };
        // Borrowed from the library, not from `self`, so rows can paint art.
        let mut hits = Hits { art: &mut *self.art, data: self.data, index, results: &mut palette.results };
        let out = kit::palette(ctx, "palette", &TEXT, &mut palette.state, &mut hits);
        if let Some((_, play)) = out.clicked {
            verb = Some(if play { Verb::Play } else { Verb::Open });
        }
        let Some(palette) = self.ui.palette.as_ref() else { return };
        match (verb, palette.results.hit(palette.state.selected)) {
            (Some(v), Some(hit)) => {
                self.ui.palette = None;
                self.run_hit(hit, v);
            }
            _ if out.close => self.ui.palette = None,
            _ => {}
        }
    }

    fn run_hit(&mut self, hit: Hit, verb: Verb) {
        let Some(index) = self.search.index.as_ref() else { return };
        match hit {
            Hit::Artist(i) => {
                let name = index.artists[i].name.clone();
                let known = index.artists[i].id.clone();
                let action = match verb {
                    Verb::Open => {
                        let id =
                            known.or_else(|| self.data.albums.iter().find(|a| a.artist.as_deref() == Some(name.as_str())).and_then(|a| a.artist_ref.clone()));
                        self.actions.push(Action::Go(Route::Artist { name, id }));
                        return;
                    }
                    // Playing an artist from here shuffles across their albums.
                    Verb::Play => QueueAction::Shuffle,
                    Verb::PlayNext => QueueAction::PlayNext,
                    Verb::Add => QueueAction::Add,
                    // A whole artist is a lot to download by accident; do it per album.
                    Verb::Download => return,
                };
                self.actions.push(Action::Artist(name, action));
            }
            Hit::Album(i) => {
                let id = index.albums[i].id.clone();
                let kept = self.downloads.snap.pin(&offline::Pin::Album(id.clone())).is_some();
                self.actions.push(match verb {
                    Verb::Open => Action::Go(Route::Album(id)),
                    Verb::Play => Action::Album(id, QueueAction::Play { start: 0 }),
                    Verb::PlayNext => Action::Album(id, QueueAction::PlayNext),
                    Verb::Add => Action::Album(id, QueueAction::Add),
                    Verb::Download => Action::Offline(OfflineAction::Keep(super::Keep::Album(id), !kept)),
                });
            }
            Hit::Track(i) => {
                let (id, album_id) = (index.tracks[i].id.clone(), index.tracks[i].album_id.clone());
                let Some(tr) = self.data.tracks.album(&album_id).and_then(|ts| ts.iter().find(|t| t.id == id)).cloned() else { return };
                match verb {
                    Verb::Open => self.actions.push(Action::Go(Route::Album(album_id))),
                    // Plays the album from this track, as double-clicking it there would.
                    Verb::Play => self.play_from_album(&tr, QueueAction::Play { start: 0 }),
                    Verb::PlayNext => self.play_from_album(&tr, QueueAction::PlayNext),
                    Verb::Add => self.play_from_album(&tr, QueueAction::Add),
                    Verb::Download => {
                        let kept = self.downloads.snap.pin(&offline::Pin::Track(tr.id.clone())).is_some();
                        self.actions.push(Action::Offline(OfflineAction::Keep(super::Keep::Tracks(vec![tr]), !kept)));
                    }
                }
            }
        }
    }
}

/// The palette's keys this frame, consumed: what ⏎ (with its modifier) or
/// ⇧⌘D asks for. ↑ and ↓ are the kit's.
fn palette_keys(ctx: &egui::Context) -> Option<Verb> {
    use egui::{Key, Modifiers};
    let keys = ctx.input_mut(|i| {
        [
            i.consume_key(Modifiers::COMMAND, Key::Enter),
            i.consume_key(Modifiers::ALT, Key::Enter),
            i.consume_key(Modifiers::SHIFT, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_shortcut(&egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::D)),
        ]
    });
    let [cmd_enter, alt_enter, shift_enter, enter, download] = keys;
    match (cmd_enter, alt_enter, shift_enter, enter) {
        (true, ..) => Some(Verb::Play),
        (_, true, ..) => Some(Verb::PlayNext),
        (_, _, true, _) => Some(Verb::Add),
        (.., true) => Some(Verb::Open),
        _ if download => Some(Verb::Download),
        _ => None,
    }
}

/// What the result rows draw from: covers, the library, and the index the
/// hits point into.
struct Hits<'h> {
    art: &'h mut crate::art::Art,
    data: &'h super::Data,
    index: &'h mut SearchIndex,
    results: &'h mut Results,
}

impl kit::PaletteRows for Hits<'_> {
    fn search(&mut self, query: &str) {
        *self.results = self.index.search(query);
    }

    /// Top hit, Artists, Albums, Songs.
    fn groups(&self) -> Vec<(&'static str, usize)> {
        let r = &*self.results;
        vec![("Top hit", usize::from(r.top.is_some())), ("Artists", r.artists.len()), ("Albums", r.albums.len()), ("Songs", r.tracks.len())]
    }

    /// A picture (a person, or the cover), and its name over what it is.
    fn row(&mut self, ui: &mut egui::Ui, tile: Rect, n: usize) -> PaletteRow {
        let keys = "⏎ open    ⌘⏎ play";
        let (index, data) = (&*self.index, self.data);
        let Some(hit) = self.results.hit(n) else { return PaletteRow { title: String::new(), detail: String::new(), keys } };
        let (title, detail) = match hit {
            Hit::Artist(i) => {
                let a = &index.artists[i];
                super::artists::avatar(self.art, ui, tile, a.cover.as_deref(), &a.name, &a.album_covers);
                (a.name.clone(), format!("Artist · {}", crate::fmt::plural(a.albums, "album")))
            }
            Hit::Album(i) => {
                let a = &index.albums[i];
                match data.album(&a.id) {
                    Some(al) => self.art.paint_as(ui, tile, al.cover_art.as_deref(), 4.0, &crate::art::album_cover(al)),
                    None => self.art.paint(ui, tile, None, 4.0),
                }
                let sub = match a.year {
                    Some(y) => format!("{} · {y}", a.artist),
                    None => a.artist.clone(),
                };
                (a.title.clone(), sub)
            }
            Hit::Track(i) => {
                let tr = &index.tracks[i];
                match data.album(&tr.album_id) {
                    Some(al) => self.art.paint_as(ui, tile, al.cover_art.as_deref(), 4.0, &crate::art::album_cover(al)),
                    None => self.art.paint(ui, tile, None, 4.0),
                }
                (tr.title.clone(), format!("Song · {} · {}", tr.artist, tr.album))
            }
        };
        PaletteRow { title, detail, keys }
    }
}
