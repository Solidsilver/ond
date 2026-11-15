//! Where things come from, once more than one source is live: each source's
//! name, colour and state as the kit draws them, and the rule for when a
//! dot is drawn at all.
//!
//! Dots go on covers and rows only while the library on screen mixes
//! sources: more than one is live and the scope shows more than one of them.
//! With one source, or the switcher on one, nothing says where anything
//! comes from, and every page looks as it did before there could be
//! several.
//!
//! Where a list has columns (Albums as a list, Songs, the track lists of
//! playlists and Favorites), the source is a column of its own, a dot and
//! a name, narrow and the first to go when the window is. Under a title (a
//! tile's artist, a queue row's) the dot follows the line, so the lines
//! under a title start where it does.
//!
//! With matching albums combined, an album or song more than one source
//! holds is one item with a copy on each: it gets a dot per copy, stacked,
//! in the sources' order, and its Source cell names them all. Everything
//! else still gets its one dot.
//!
//! What's said about a source comes in two lengths. The switcher's menu and
//! the sidebar's tooltip get the short one ("Synced 9:41 AM", "3 files
//! skipped"); Settings › Sources gets the long one (the server's label, a
//! folder source's folders). Either gives way to the state when the source
//! is busy or can't be used, so trouble reads the same in both places.

use super::session::{Session, SourceSession};
use super::view_ctx::ViewCtx;
use super::{Action, PrefsAction};
use crate::settings::{Settings, SourceEntry, SourceKind};
use crate::source::identity;
use accounts::{AccountId, OfflineReason, SourceColor, SourceState};
use shell::kit;

/// Each configured source's name, colour and kind, read with the frame's
/// preferences. Empty while there's only one source, so a frame with one
/// copies nothing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceMarks(Vec<Mark>);

/// One source, as a dot and a name.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub id: AccountId,
    pub name: String,
    pub color: SourceColor,
    pub kind: SourceKind,
}

impl SourceMarks {
    pub fn read(s: &Settings) -> Self {
        if s.sources.len() < 2 {
            return Self::default();
        }
        Self(s.sources.iter().map(|e| Mark { id: e.id, name: name_of(e), color: e.color, kind: e.kind }).collect())
    }

    pub fn get(&self, id: AccountId) -> Option<&Mark> {
        self.0.iter().find(|m| m.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Mark> {
        self.0.iter()
    }
}

/// What a source is called: the name it was given, else its host or its
/// first folder's name.
pub fn name_of(entry: &SourceEntry) -> String {
    entry.name.clone().unwrap_or_else(|| super::sidebar::default_name(entry))
}

/// Whether the library on screen mixes sources, which is when dots are
/// drawn.
pub fn mixed(session: &Session) -> bool {
    session.accounts.len() > 1 && session.scope.is_all() && session.accounts.keys().filter(|a| session.scope.includes(**a)).count() > 1
}

/// The dot for something from `account` that may come from outside the
/// scope (the playing song): drawn whenever more than one source is
/// configured, since the library on screen doesn't say where it's from.
pub fn configured_dot(marks: &SourceMarks, account: AccountId) -> Option<SourceColor> {
    marks.get(account).map(|m| m.color)
}

/// [`ViewCtx::playing_dot`], for what draws without a view (Mini). Every
/// configured source has a live session (signed out or not), so "more than
/// one configured" is more than one in the session; a configuration the
/// session hasn't caught up with yet draws nothing.
pub fn playing_dot(session: &Session, marks: &SourceMarks, player: &crate::player::Player, track: &library::Track) -> Option<SourceColor> {
    if session.accounts.len() < 2 {
        return None;
    }
    let queued = player.queue.current.as_ref().filter(|c| c.track.id == track.id).map(|c| c.key.account);
    configured_dot(marks, queued.or_else(|| identity::account(&track.id))?)
}

/// Whether the items `ids` come from more than one source. Compares the
/// account part of each id as text, so a long list costs no parsing.
pub fn spans<'i>(mut ids: impl Iterator<Item = &'i str>) -> bool {
    let account = |id: &'i str| id.split_once('/').map(|(a, _)| a);
    let Some(first) = ids.next().map(account) else { return false };
    ids.any(|id| account(id) != first)
}

/// How much is said about a source under its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// A few words, for the switcher's menu.
    Short,
    /// Its address or its folders, for Settings › Sources.
    Long,
}

/// A source described for the kit, owning what [`kit::SourceInfo`]
/// borrows.
pub struct Described {
    pub id: AccountId,
    pub name: String,
    pub kind: crate::source::Kind,
    pub color: SourceColor,
    pub state: SourceState,
    pub detail: Option<String>,
}

impl Described {
    pub fn info(&self) -> kit::SourceInfo<'_> {
        kit::SourceInfo {
            id: self.id,
            name: &self.name,
            kind: self.kind.name,
            icon: self.kind.icon,
            color: self.color,
            state: &self.state,
            detail: self.detail.as_deref(),
        }
    }
}

/// `entry` as it is now, from its live session (`None` before it has one,
/// which reads as signed out).
pub fn describe(entry: &SourceEntry, runtime: Option<&SourceSession>, work_offline: bool, detail: Detail) -> Described {
    let kind = crate::source::Kind::of(entry.kind);
    let state = runtime.map_or(SourceState::SignedOut, |s| s.state(work_offline));
    let detail = runtime.and_then(|s| line(entry, s, &state, kind, detail));
    Described { id: entry.id, name: name_of(entry), kind, color: entry.color, state, detail }
}

/// The line under a source's name. `None` leaves the state's own words.
fn line(entry: &SourceEntry, s: &SourceSession, state: &SourceState, kind: crate::source::Kind, detail: Detail) -> Option<String> {
    match state {
        SourceState::Syncing(_) | SourceState::Scanning => Some(progress(s, kind)),
        // "Offline" alone could be a choice; this one isn't.
        SourceState::Offline(OfflineReason::Unreachable) => Some("Can't reach the server".to_owned()),
        SourceState::Online => match (detail, kind.scans) {
            (Detail::Short, false) => s.sync.last.map(|at| format!("Synced {}", at.format("%-I:%M %p"))),
            (Detail::Short, true) if s.skipped > 0 => Some(skipped(s.skipped)),
            (Detail::Short, true) => s.sync.last.map(|at| format!("Scanned {}", at.format("%-I:%M %p"))),
            (Detail::Long, false) if !s.server_label.is_empty() => Some(s.server_label.clone()),
            (Detail::Long, false) => Some(super::diagnostics::server_host(&entry.server).to_owned()),
            (Detail::Long, true) => {
                let folders: Vec<String> = entry.folders.iter().map(|f| f.path.display().to_string()).collect();
                let mut line = folders.join(", ");
                if s.skipped > 0 {
                    line.push_str(&format!(" · {}", skipped(s.skipped)));
                }
                Some(line)
            }
        },
        _ => None,
    }
}

/// How far a sync or scan has got, as the one source's footer says it.
fn progress(s: &SourceSession, kind: crate::source::Kind) -> String {
    match s.sync.progress {
        Some(p) if kind.scans => format!("Scanning · {} tracks", crate::fmt::thousands(p.done)),
        Some(p) if p.total > 0 => format!("Syncing · {:.0}%", 100.0 * p.done as f32 / p.total as f32),
        _ if kind.scans => "Scanning…".to_owned(),
        _ => "Syncing library…".to_owned(),
    }
}

fn skipped(n: usize) -> String {
    format!("{} skipped", crate::fmt::plural(n, "file"))
}

/// The marks of the sources holding `copies` (an album's or a song's, the
/// lead first), in that order, while the library mixes sources.
fn marks_of_copies<'m>(session: &Session, marks: &'m SourceMarks, copies: &[accounts::ItemKey]) -> Vec<&'m Mark> {
    if !mixed(session) {
        return Vec::new();
    }
    copies.iter().filter_map(|k| marks.get(k.account)).collect()
}

/// The dots of `marks`, in their order.
pub fn colors(marks: &[&Mark]) -> Vec<SourceColor> {
    marks.iter().map(|m| m.color).collect()
}

/// A Source column's cell from `x`, centred on `cy`, in `width`: the dot
/// and name of each source in `marks`, stacked on `ground` when there are
/// several.
pub fn source_cell(ui: &egui::Ui, (x, cy, width): (f32, f32, f32), marks: &[&Mark], ground: egui::Color32, font: egui::FontId, ink: egui::Color32) {
    let names: Vec<&str> = marks.iter().map(|m| m.name.as_str()).collect();
    kit::sources_cell(ui, x, cy, width, &colors(marks), ground, (&names, font, ink));
}

impl ViewCtx<'_> {
    /// The sources holding the album `id`, while the library mixes sources:
    /// its one source, or each with a copy of a combined album, in the
    /// sources' order.
    pub fn album_sources(&self, id: &str) -> Vec<&Mark> {
        marks_of_copies(self.session, &self.ui.prefs.sources, &self.data.copies_of_album(id))
    }

    /// The dots for the album `id`, as [`album_sources`](Self::album_sources).
    pub fn album_dots(&self, id: &str) -> Vec<SourceColor> {
        colors(&self.album_sources(id))
    }

    /// The sources holding the song `id`, while the library mixes sources:
    /// its one source, or each with a copy of a combined song.
    pub fn track_sources(&self, id: &str) -> Vec<&Mark> {
        marks_of_copies(self.session, &self.ui.prefs.sources, &self.data.copies_of_track(id))
    }

    /// Whether the library on screen mixes sources, so a list shows its
    /// Source column.
    pub fn mixed_sources(&self) -> bool {
        mixed(self.session)
    }

    /// The dots for a playlist, while the library mixes sources: its
    /// server's, or for one of Ond's own, those of the sources it holds
    /// songs from, in the sources' order.
    pub fn playlist_dots(&self, p: &crate::source::Playlist) -> Vec<SourceColor> {
        if !mixed(self.session) {
            return Vec::new();
        }
        let marks = &self.ui.prefs.sources;
        if let Some(account) = identity::account(&p.id) {
            return marks.get(account).map(|m| m.color).into_iter().collect();
        }
        let Some(tracks) = self.data.playlist_tracks.get(&p.id) else { return Vec::new() };
        // Each song's account as text, once each, before any is parsed.
        let mut seen: Vec<&str> = Vec::new();
        for t in tracks.iter() {
            if let Some((a, _)) = t.id.split_once('/')
                && !seen.contains(&a)
            {
                seen.push(a);
            }
        }
        let held: Vec<AccountId> = seen.iter().filter_map(|a| a.parse().ok()).collect();
        marks.iter().filter(|m| held.contains(&m.id)).map(|m| m.color).collect()
    }

    /// The dot for the song `track` playing, whenever more than one source
    /// is configured: it may come from outside the scope.
    pub fn playing_dot(&self, track: &library::Track) -> Option<SourceColor> {
        playing_dot(self.session, &self.ui.prefs.sources, self.player, track)
    }

    /// The dots for the queue's `item`, while the library mixes sources:
    /// one per copy of a combined song, else its own source's. The queue
    /// says which copy it holds, so a song with one copy takes its dot from
    /// there rather than from its id.
    pub fn queue_dots(&self, item: &crate::queue::Item) -> Vec<SourceColor> {
        if !mixed(self.session) {
            return Vec::new();
        }
        let copies = self.data.copies_of_track(&item.track.id);
        if copies.len() > 1 {
            return colors(&marks_of_copies(self.session, &self.ui.prefs.sources, &copies));
        }
        configured_dot(&self.ui.prefs.sources, item.key.account).into_iter().collect()
    }

    /// Whether a list of `tracks` shows its Source column: the library mixes
    /// sources and the list holds songs from more than one, or a song more
    /// than one source holds (an album, or a server's playlist, otherwise
    /// comes from one, and says so elsewhere).
    pub fn sources_column(&self, tracks: &[library::Track]) -> bool {
        let data = self.data;
        let combined = || !data.track_copies.is_empty() && tracks.iter().any(|t| data.track_copies.contains_key(data.track_lead(&t.id)));
        mixed(self.session) && (spans(tracks.iter().map(|t| t.id.as_str())) || combined())
    }

    /// The filter chips under a library page's heading, in All sources with
    /// more than one live; nothing otherwise.
    pub fn scope_chips(&mut self, ui: &mut egui::Ui) {
        if self.session.accounts.len() < 2 || !self.session.scope.is_all() {
            return;
        }
        let marks = &self.ui.prefs.sources;
        let states: Vec<SourceState> =
            marks.iter().map(|m| self.session.get(m.id).map_or(SourceState::SignedOut, |s| s.state(self.session.work_offline))).collect();
        let sources: Vec<kit::SourceInfo<'_>> = marks
            .iter()
            .zip(&states)
            .map(|(m, state)| {
                let kind = crate::source::Kind::of(m.kind);
                kit::SourceInfo { id: m.id, name: &m.name, kind: kind.name, icon: kind.icon, color: m.color, state, detail: None }
            })
            .collect();
        ui.add_space(4.0);
        let picked = kit::source_chips(ui, &sources, &self.session.scope);
        ui.add_space(12.0);
        if let Some(chip) = picked {
            self.actions.push(Action::Prefs(PrefsAction::Scope(super::all_sources::ScopeAction::Chip(chip))));
        }
    }
}
