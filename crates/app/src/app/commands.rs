//! What keys and the menu bar do (each [`Cmd`] stands for an [`Action`]), and
//! hearts, ratings and playlist edits.

use super::{Action, OndApp, OutputAction, PrefsAction, WindowAction, sidebar};
use std::sync::Arc;
use crate::bridge::PlaylistEdit;
use crate::favorites::StarKind;
use crate::keys::{Cmd, EditCmd};
use crate::nav::Route;
use crate::window::WindowMode;
use library::Track;

/// A change to a playlist, from a view.
#[derive(Debug, Clone)]
pub enum PlaylistAction {
    Create { name: String, tracks: Vec<Track> },
    Add { id: String, tracks: Vec<Track> },
    Rename { id: String, name: String },
    Delete { id: String },
    /// Remove the track at `index`.
    Remove { id: String, index: usize },
    /// Move the track at `from` to `to` (both indices into the list as it is).
    Move { id: String, from: usize, to: usize },
}

/// Moves `v[from]` to `to`, shifting the others; out-of-range is a no-op.
pub fn move_item<T>(v: &mut Vec<T>, from: usize, to: usize) -> bool {
    if from >= v.len() || from == to {
        return false;
    }
    let item = v.remove(from);
    v.insert(to.min(v.len()), item);
    true
}

/// How far ⌘↑ / ⌘↓ move the volume.
const VOLUME_STEP: f32 = 0.1;
/// The volume keys' step on a device's own volume (bit-perfect), in dB.
const DEVICE_VOLUME_STEP_DB: f32 = 2.0;
/// How far ⌥⌘→ / ⌥⌘← skip.
const SKIP_SECS: f64 = 10.0;

impl Cmd {
    /// What this key or menu command stands for, as things are now; `None`
    /// when it has nothing to act on (seeking with nothing playing, say).
    pub(super) fn action(self, app: &OndApp) -> Option<Action> {
        let current = app.player.queue.current.as_ref().map(|c| &c.track);
        Some(match self {
            Cmd::PlayPause => Action::PlayPause,
            Cmd::Next => Action::Next,
            Cmd::Previous => Action::Previous,
            Cmd::VolumeUp | Cmd::VolumeDown => {
                let up = self == Cmd::VolumeUp;
                // Bit-perfect: the device's own volume, when it has one.
                if app.bit_perfect {
                    let v = app.output.device_volume()?;
                    return Some(Action::Output(OutputAction::DeviceVolume(v.db + if up { DEVICE_VOLUME_STEP_DB } else { -DEVICE_VOLUME_STEP_DB })));
                }
                let step = if up { VOLUME_STEP } else { -VOLUME_STEP };
                Action::Output(OutputAction::Volume(app.prefs_now().volume + step))
            }
            Cmd::SeekForward | Cmd::SeekBack => {
                current?;
                let by = if self == Cmd::SeekForward { SKIP_SECS } else { -SKIP_SECS };
                let end = app.duration_secs().unwrap_or(f64::MAX);
                Action::Seek((app.position_secs() + by).clamp(0.0, end))
            }
            Cmd::Love => {
                let t = current?;
                Action::ToggleStar { kind: StarKind::Song, id: t.id.clone(), track: Some(Box::new(t.clone())), name: None }
            }
            Cmd::Reveal => {
                let t = current?;
                Action::Reveal { track: t.id.clone(), album: t.album_id.clone() }
            }
            Cmd::Search => Action::OpenPalette,
            Cmd::Back => Action::Back,
            Cmd::Forward => Action::Forward,
            Cmd::Place(n) => Action::Go(sidebar::PLACES.get(usize::from(n))?.1.clone()),
            Cmd::ToggleQueue => Action::Window(WindowAction::ToggleQueue),
            Cmd::Lyrics => Action::Lyrics,
            Cmd::Info => Action::Info,
            Cmd::TogglePlayer => Action::Window(WindowAction::TogglePlayer),
            Cmd::Full => Action::Window(WindowAction::SetMode(WindowMode::Full)),
            Cmd::Player => Action::Window(WindowAction::SetMode(WindowMode::Player)),
            Cmd::Mini => Action::Window(WindowAction::SetMode(WindowMode::Mini)),
            Cmd::ToggleMini => Action::Window(WindowAction::ToggleMini),
            Cmd::Settings => Action::OpenSettings(app.ui.settings_section),
            Cmd::Shortcuts => Action::ToggleShortcuts,
            Cmd::NewPlaylist => Action::NewPlaylist,
            Cmd::ToggleBitPerfect => Action::Output(OutputAction::ToggleBitPerfect),
            Cmd::ToggleDiagnostics => Action::Prefs(PrefsAction::Diagnostics(!app.prefs_now().diagnostics_bar)),
            Cmd::Visualizer => Action::ToggleVisualizer,
            Cmd::Appearance(pref) => Action::Prefs(PrefsAction::Theme(pref)),
            Cmd::Edit(e) => Action::Edit(e),
        })
    }
}

impl OndApp {
    /// Applies a key or menu command now.
    pub(super) fn command(&mut self, cmd: Cmd) {
        if let Some(action) = cmd.action(self) {
            self.apply(action);
        }
    }

    /// Hands an Edit menu command to egui as input next frame, so the
    /// focused field pastes, copies or undoes as if the keys had reached it.
    pub(super) fn edit_input(&mut self, e: EditCmd) {
        let events = e.events(clipboard_text);
        self.ui.edit_input.extend(events);
        self.ctx.request_repaint();
    }

    /// Flips a heart now and tells the server; a refusal puts it back.
    pub(super) fn toggle_star(&mut self, kind: StarKind, id: String, track: Option<Track>, name: Option<String>) {
        let (on, seq) = self.favorites.toggle(kind, &id, track.as_ref(), name.as_deref());
        self.bridge.set_starred(self.session.client.clone(), kind, id, on, seq);
    }

    pub(super) fn rate(&mut self, id: String, rating: u8) {
        let (previous, seq) = self.favorites.rate(&id, rating);
        self.bridge.set_rating(self.session.client.clone(), id, rating, previous, seq);
    }

    pub(super) fn star_done(&mut self, kind: StarKind, id: String, on: bool, seq: u64, error: Option<String>) {
        match error {
            None => self.favorites.settled(kind, &id, seq),
            Some(e) => {
                let track = self.favorites.view.songs.iter().find(|t| t.id == id).cloned().or_else(|| self.find_track(&id));
                self.favorites.failed(kind, &id, on, seq, track.as_ref(), None);
                self.show_error(format!("Couldn't save the heart: {e}"));
            }
        }
    }

    pub(super) fn rating_done(&mut self, id: String, previous: u8, seq: u64, error: Option<String>) {
        match error {
            None => self.favorites.rating_settled(&id, seq),
            Some(e) => {
                self.favorites.rating_failed(&id, previous, seq);
                self.show_error(format!("Couldn't save the rating: {e}"));
            }
        }
    }

    /// A track we know of, from any album or playlist loaded.
    fn find_track(&self, id: &str) -> Option<Track> {
        self.data.track(id).or_else(|| self.data.playlist_tracks.values().flat_map(|v| v.iter()).find(|t| t.id == id)).cloned()
    }

    /// Applies a playlist change: locally first where the result is certain
    /// (so lists don't jump), then on the server, which sends back the truth.
    pub(super) fn playlist_action(&mut self, a: PlaylistAction) {
        let client = self.session.client.clone();
        match a {
            PlaylistAction::Create { name, tracks } => {
                let name = if name.trim().is_empty() { "Untitled playlist".to_owned() } else { name.trim().to_owned() };
                let ids = tracks.iter().map(|t| t.id.clone()).collect();
                self.bridge.create_playlist(client, name, ids);
            }
            PlaylistAction::Add { id, tracks } => {
                if tracks.is_empty() {
                    return;
                }
                self.note_recent_playlist(&id);
                if let Some(list) = self.data.playlist_tracks.get_mut(&id) {
                    Arc::make_mut(list).extend(tracks.iter().cloned());
                }
                if let Some(p) = self.data.playlists.iter_mut().find(|p| p.id == id) {
                    p.song_count += tracks.len() as u32;
                    p.duration += tracks.iter().filter_map(|t| t.duration_secs).sum::<u32>();
                }
                let n = tracks.len();
                let name = self.data.playlists.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
                self.show_notice(format!("Added {} to {name}", if n == 1 { tracks[0].title.clone() } else { format!("{n} songs") }));
                self.bridge.edit_playlist(client, id, PlaylistEdit::Add(tracks.into_iter().map(|t| t.id).collect()));
            }
            PlaylistAction::Rename { id, name } => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    return;
                }
                if let Some(p) = self.data.playlists.iter_mut().find(|p| p.id == id) {
                    p.name = name.clone();
                }
                self.ui.nav.rename_playlist(&id, &name);
                self.bridge.edit_playlist(client, id, PlaylistEdit::Rename(name));
            }
            PlaylistAction::Delete { id } => {
                self.data.playlists.retain(|p| p.id != id);
                self.data.playlist_tracks.remove(&id);
                if matches!(self.ui.nav.current(), Route::Playlist { id: cur, .. } if *cur == id) {
                    self.go(Route::Recent);
                }
                self.bridge.edit_playlist(client, id, PlaylistEdit::Delete);
            }
            PlaylistAction::Remove { id, index } => {
                let Some(list) = self.data.playlist_tracks.get_mut(&id) else { return };
                if index >= list.len() {
                    return;
                }
                let gone = Arc::make_mut(list).remove(index);
                if let Some(p) = self.data.playlists.iter_mut().find(|p| p.id == id) {
                    p.song_count = p.song_count.saturating_sub(1);
                    p.duration = p.duration.saturating_sub(gone.duration_secs.unwrap_or(0));
                }
                self.ui.track_sel = self.ui.track_sel.map(|s| s.min(list.len().saturating_sub(1))).filter(|_| !list.is_empty());
                self.bridge.edit_playlist(client, id, PlaylistEdit::Remove(vec![index]));
            }
            PlaylistAction::Move { id, from, to } => {
                let Some(list) = self.data.playlist_tracks.get_mut(&id) else { return };
                if !move_item(Arc::make_mut(list), from, to) {
                    return;
                }
                if self.ui.track_sel == Some(from) {
                    self.ui.track_sel = Some(to.min(list.len() - 1));
                }
                let ids = list.iter().map(|t| t.id.clone()).collect();
                self.bridge.edit_playlist(client, id, PlaylistEdit::Replace(ids));
            }
        }
    }

    /// Playlists added to lately come first in "Add to Playlist".
    fn note_recent_playlist(&mut self, id: &str) {
        self.ui.recent_playlists.retain(|p| p != id);
        self.ui.recent_playlists.insert(0, id.to_owned());
        self.ui.recent_playlists.truncate(3);
    }
}

/// The clipboard's text, for the Edit menu's Paste (only the macOS menu sends it).
fn clipboard_text() -> Option<String> {
    #[cfg(target_os = "macos")]
    return arboard::Clipboard::new().and_then(|mut c| c.get_text()).ok();
    #[cfg(not(target_os = "macos"))]
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_item_shifts_the_rest() {
        let mut v = vec!['a', 'b', 'c', 'd'];
        assert!(move_item(&mut v, 0, 2));
        assert_eq!(v, ['b', 'c', 'a', 'd']);
        assert!(move_item(&mut v, 3, 0));
        assert_eq!(v, ['d', 'b', 'c', 'a']);
        assert!(move_item(&mut v, 1, 9));
        assert_eq!(v, ['d', 'c', 'a', 'b']);
        assert!(!move_item(&mut v, 7, 0));
        assert!(!move_item(&mut v, 2, 2));
    }
}
