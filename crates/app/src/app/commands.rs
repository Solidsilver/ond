//! What keys, the menu bar, hearts, ratings and playlist edits do. Views push
//! [`Action`]s; these apply them.

use super::{Action, OndApp, sidebar};
use std::sync::Arc;
use crate::bridge::PlaylistEdit;
use crate::favorites::StarKind;
use crate::keys::Cmd;
use crate::nav::Route;
use crate::window::WindowMode;
use audio::PlaybackMode;
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
/// How far ⌥⌘→ / ⌥⌘← skip.
const SKIP_SECS: f64 = 10.0;

impl OndApp {
    pub(super) fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::PlayPause => self.actions.push(Action::PlayPause),
            Cmd::Next => self.actions.push(Action::Next),
            Cmd::Previous => self.actions.push(Action::Previous),
            Cmd::VolumeUp | Cmd::VolumeDown => {
                // Bit-perfect leaves volume to the DAC.
                if !self.bit_perfect {
                    let step = if cmd == Cmd::VolumeUp { VOLUME_STEP } else { -VOLUME_STEP };
                    self.set_volume(self.volume() + step);
                }
            }
            Cmd::SeekForward | Cmd::SeekBack => {
                if self.player.queue.current.is_some() {
                    let by = if cmd == Cmd::SeekForward { SKIP_SECS } else { -SKIP_SECS };
                    let end = self.duration_secs().unwrap_or(f64::MAX);
                    self.actions.push(Action::Seek((self.position_secs() + by).clamp(0.0, end)));
                }
            }
            Cmd::Love => {
                if let Some(t) = self.player.queue.current.as_ref().map(|c| c.track.clone()) {
                    self.actions.push(Action::ToggleStar { kind: StarKind::Song, id: t.id.clone(), track: Some(Box::new(t)), name: None });
                }
            }
            Cmd::Reveal => {
                if let Some(t) = self.player.queue.current.as_ref().map(|c| c.track.clone()) {
                    self.ui.reveal = Some(t.id.clone());
                    self.actions.push(Action::Go(Route::Album(t.album_id)));
                }
            }
            Cmd::Search => self.actions.push(Action::OpenPalette),
            Cmd::Back => self.actions.push(Action::Back),
            Cmd::Forward => self.actions.push(Action::Forward),
            Cmd::Place(n) => {
                if let Some((_, route, _)) = sidebar::PLACES.get(usize::from(n)) {
                    self.actions.push(Action::Go(route.clone()));
                }
            }
            Cmd::ToggleQueue => self.actions.push(Action::ToggleQueue),
            Cmd::Lyrics => self.actions.push(Action::Lyrics),
            Cmd::Info => self.actions.push(Action::Info),
            Cmd::TogglePlayer => self.actions.push(Action::TogglePlayer),
            Cmd::Full => self.actions.push(Action::SetMode(WindowMode::Full)),
            Cmd::Player => self.actions.push(Action::SetMode(WindowMode::Player)),
            Cmd::Mini => self.actions.push(Action::SetMode(WindowMode::Mini)),
            Cmd::ToggleMini => self.actions.push(Action::ToggleMini),
            Cmd::Settings => self.actions.push(Action::OpenSettings(self.ui.settings_section)),
            Cmd::Shortcuts => {
                self.set_mode(Some(WindowMode::Full));
                self.ui.overlay = match self.ui.overlay {
                    Some(super::dialogs::Overlay::Shortcuts) => None,
                    _ => Some(super::dialogs::Overlay::Shortcuts),
                };
            }
            Cmd::NewPlaylist => {
                self.set_mode(Some(WindowMode::Full));
                self.ui.overlay = Some(super::dialogs::Overlay::new_playlist(Vec::new()));
            }
            Cmd::ToggleBitPerfect => {
                self.bit_perfect = !self.bit_perfect;
                let mode = if self.bit_perfect { PlaybackMode::BitPerfect } else { PlaybackMode::Normal };
                self.output.set_mode_in_use(mode, &self.engine);
            }
            Cmd::ToggleDiagnostics => self.actions.push(Action::SetDiagnostics(!self.diagnostics_shown())),
            Cmd::Visualizer => self.toggle_visualizer(),
            Cmd::Appearance(pref) => self.actions.push(Action::SetTheme(pref)),
            Cmd::Edit(e) => {
                // Handed to egui as input next frame, so the focused field gets it.
                let events = e.events(clipboard_text);
                self.ui.edit_input.extend(events);
                self.ctx.request_repaint();
            }
        }
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
