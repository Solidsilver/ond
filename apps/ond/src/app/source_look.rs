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

/// The mark for the item `id` (an album, a track), while the library mixes
/// sources.
pub fn mark_of<'m>(session: &Session, marks: &'m SourceMarks, id: &str) -> Option<&'m Mark> {
    if !mixed(session) {
        return None;
    }
    marks.get(identity::account(id)?)
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

impl ViewCtx<'_> {
    /// The colour of the dot for the item `id`, while the library mixes
    /// sources.
    pub fn source_dot(&self, id: &str) -> Option<SourceColor> {
        mark_of(self.session, &self.ui.prefs.sources, id).map(|m| m.color)
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
