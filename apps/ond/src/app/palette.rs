//! ⌘K: search the library and act on a result without leaving the keyboard.
//! ⏎ opens · ⌘⏎ plays · ⌥⏎ plays next · ⇧⏎ adds to the queue.
//!
//! The sheet is the kit's (`kit::palette`); this says what the rows are and
//! what the keys do with them.

use super::OfflineAction;
use super::view_ctx::ViewCtx;
use super::{Action, QueueAction};
use crate::nav::Route;
use crate::search::{ArtistEntry, Hit, Results, SearchIndex};
use crate::source::artists::normalize_name;
use egui::Rect;
use shell::kit::{self, PaletteRow, PaletteState};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct Palette {
    /// What's typed, and the row selected.
    pub(super) state: PaletteState,
    pub(super) results: Results,
    /// Generation of `SearchState` when `results` was last scored while stale.
    stale_generation: u64,
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
        let Some(palette) = self.ui.palette.as_mut() else { return };
        let current = self.search.is_current();
        let search_generation = self.search.generation;
        let Some(index) = self.search.index.as_mut() else {
            self.ui.palette = None;
            return;
        };
        if !current && palette.stale_generation != search_generation && !palette.state.query.is_empty() {
            palette.results = search_in_scope(index, self.data, &self.session.scope, &palette.state.query);
            palette.state.selected = palette.state.selected.min(palette.results.len().saturating_sub(1));
            palette.stale_generation = search_generation;
        }
        let mut verb = palette_keys(ctx);
        // Borrowed from the library, not from `self`, so rows can paint art.
        let dots = super::source_look::mixed(self.session).then(|| self.ui.prefs.sources.clone());
        let mut hits = Hits { art: &mut *self.art, data: self.data, scope: &self.session.scope, current, index, results: &mut palette.results, dots };
        let out = kit::palette(ctx, "palette", &TEXT, &mut palette.state, &mut hits);
        if let Some((_, play)) = out.clicked {
            verb = Some(if play { Verb::Play } else { Verb::Open });
        }
        if !current {
            palette.stale_generation = search_generation;
        }
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
        let current = self.search.is_current();
        match hit {
            Hit::Artist(i) => {
                let entry = &index.artists[i];
                let (name, known) = if current {
                    (entry.name.clone(), entry.id.clone())
                } else if let Some(resolved) = resolve_artist(entry, self.data, &self.session.scope) {
                    resolved
                } else {
                    return;
                };
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
                self.actions.push(Action::Artist(name, known, action));
            }
            Hit::Album(i) => {
                let id = index.albums[i].id.clone();
                if !current && (!id_in_scope(&self.session.scope, &id) || self.data.album(&id).is_none()) {
                    return;
                }
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
                if !current && !id_in_scope(&self.session.scope, &id) {
                    return;
                }
                let Some(tr) = self.data.tracks.album(&album_id).and_then(|ts| ts.iter().find(|t| t.id == id)).cloned() else { return };
                match verb {
                    Verb::Open => self.actions.push(Action::Go(Route::Album(album_id))),
                    // Plays the album from this track, as double-clicking it there would.
                    Verb::Play => self.play_from_album(&tr, QueueAction::Play { start: 0 }),
                    Verb::PlayNext => self.play_from_album(&tr, QueueAction::PlayNext),
                    Verb::Add => self.play_from_album(&tr, QueueAction::Add),
                    Verb::Download => {
                        let kept = self.track_pin(&tr).is_some_and(|p| self.downloads.snap.pin(&p).is_some());
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
    scope: &'h accounts::Scope,
    current: bool,
    index: &'h mut SearchIndex,
    results: &'h mut Results,
    /// Each source's mark, while the library mixes sources.
    dots: Option<super::source_look::SourceMarks>,
}

impl Hits<'_> {
    fn dot(&self, id: &str) -> Option<accounts::SourceColor> {
        self.dots.as_ref()?.get(crate::source::identity::account(id)?).map(|m| m.color)
    }
}

impl kit::PaletteRows for Hits<'_> {
    fn search(&mut self, query: &str) {
        *self.results = if self.current { self.index.search(query) } else { search_in_scope(self.index, self.data, self.scope, query) };
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
        let Some(hit) = self.results.hit(n) else { return PaletteRow { title: String::new(), detail: String::new(), keys, dot: None } };
        let (title, detail, dot) = match hit {
            Hit::Artist(i) => {
                let a = &index.artists[i];
                super::artists::avatar(self.art, ui, tile, a.cover.as_deref(), &a.name, &a.album_covers);
                (a.name.clone(), format!("Artist · {}", crate::fmt::plural(a.albums, "album")), None)
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
                (a.title.clone(), sub, self.dot(&a.id))
            }
            Hit::Track(i) => {
                let tr = &index.tracks[i];
                match data.album(&tr.album_id) {
                    Some(al) => self.art.paint_as(ui, tile, al.cover_art.as_deref(), 4.0, &crate::art::album_cover(al)),
                    None => self.art.paint(ui, tile, None, 4.0),
                }
                (tr.title.clone(), format!("Song · {} · {}", tr.artist, tr.album), self.dot(&tr.id))
            }
        };
        PaletteRow { title, detail, keys, dot }
    }
}

fn id_in_scope(scope: &accounts::Scope, id: &str) -> bool {
    crate::source::identity::account(id).is_none_or(|a| scope.includes(a))
}

fn artist_member_id(member: &crate::source::artists::ArtistIdentity) -> Option<String> {
    (!member.key.item.starts_with('\0')).then(|| member.key.to_string())
}

/// Precomputed per-search artist membership so filtering stays O(1) per entry.
struct ArtistFilter<'a> {
    scope: &'a accounts::Scope,
    in_scope_ids: HashSet<&'a str>,
    in_scope_names: HashSet<String>,
    merged_in_scope: HashMap<String, bool>,
}

impl<'a> ArtistFilter<'a> {
    fn new(data: &'a super::Data, scope: &'a accounts::Scope) -> Self {
        let mut in_scope_ids = HashSet::new();
        let mut in_scope_names = HashSet::new();
        for artist in data.artists.iter() {
            if id_in_scope(scope, &artist.id) {
                in_scope_ids.insert(artist.id.as_str());
                in_scope_names.insert(normalize_name(&artist.name));
            }
        }
        for album in data.albums.iter() {
            if !id_in_scope(scope, &album.id) {
                continue;
            }
            if let Some(name) = album.artist.as_deref() {
                in_scope_names.insert(normalize_name(name));
            }
            for credit in &album.artists {
                in_scope_names.insert(normalize_name(&credit.name));
            }
        }
        let mut merged_in_scope = HashMap::new();
        for group in &data.merged_artists {
            let has_in_scope = group.members.iter().any(|m| scope.includes(m.key.account));
            for member in &group.members {
                if let Some(id) = artist_member_id(member) {
                    merged_in_scope.insert(id, has_in_scope);
                }
            }
        }
        Self { scope, in_scope_ids, in_scope_names, merged_in_scope }
    }

    fn allows(&self, entry: &ArtistEntry) -> bool {
        if let Some(id) = entry.id.as_deref() {
            if self.in_scope_ids.contains(id) {
                return true;
            }
            if let Some(&in_scope) = self.merged_in_scope.get(id) {
                return in_scope;
            }
            if !id_in_scope(self.scope, id) {
                return false;
            }
        }
        self.in_scope_names.contains(&normalize_name(&entry.name))
    }
}

fn search_in_scope(index: &mut SearchIndex, data: &super::Data, scope: &accounts::Scope, query: &str) -> Results {
    let artists = ArtistFilter::new(data, scope);
    index.search_where(query, |a| artists.allows(a), |al| id_in_scope(scope, &al.id) && data.album(&al.id).is_some(), |tr| id_in_scope(scope, &tr.id))
}

/// Resolves an artist entry from a stale index into the current scope's `(name, id)`,
/// or `None` when no member or album credit for that artist remains in scope.
fn resolve_artist(entry: &ArtistEntry, data: &super::Data, scope: &accounts::Scope) -> Option<(String, Option<String>)> {
    if let Some(id) = entry.id.as_deref() {
        if id_in_scope(scope, id) && data.artists.iter().any(|a| a.id == id) {
            return Some((entry.name.clone(), Some(id.to_owned())));
        }
        if let Some(group) = data.merged_artists.iter().find(|g| g.members.iter().any(|m| artist_member_id(m).as_deref() == Some(id))) {
            return group.members.iter().find(|m| scope.includes(m.key.account)).map(|m| (m.name.clone(), artist_member_id(m)));
        }
        if !id_in_scope(scope, id) {
            return None;
        }
    }
    let normalized = normalize_name(&entry.name);
    if let Some(artist) = data.artists.iter().find(|a| id_in_scope(scope, &a.id) && normalize_name(&a.name) == normalized) {
        return Some((artist.name.clone(), Some(artist.id.clone())));
    }
    data.albums.iter().any(|a| id_in_scope(scope, &a.id) && a.has_artist(None, &entry.name)).then(|| (entry.name.clone(), None))
}
