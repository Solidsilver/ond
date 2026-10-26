//! What a view gets to work with: the library, playback, downloads and the
//! rest to read, the window's own state to change, and a list of [`Action`]s
//! to ask for everything else. A view that takes a `ViewCtx` can't start a
//! request or touch the engine behind the app's back; the app applies the
//! actions after the frame.

use super::{Action, Analysis, Data, Downloads, OndApp, SearchState, Session, SongStats, SyncState, UiState, diagnostics};
use crate::art::Art;
use crate::favorites::Favorites;
use crate::memo::Memo;
use crate::output::Outputs;
use crate::player::Player;
use audio::EngineHandle;
use library::Track;
use std::collections::HashMap;

pub struct ViewCtx<'a> {
    pub data: &'a Data,
    pub player: &'a Player,
    pub engine: &'a EngineHandle,
    pub favorites: &'a Favorites,
    pub downloads: &'a Downloads,
    pub analysis: &'a Analysis,
    pub output: &'a Outputs,
    pub session: &'a Session,
    pub sync: &'a SyncState,
    /// The palette scores with the index's own scratch space.
    pub search: &'a mut SearchState,
    /// Lyrics by track id, as fetched so far.
    pub lyrics: &'a HashMap<String, crate::lyrics::Fetch>,
    pub song_stats: &'a HashMap<String, Option<SongStats>>,
    /// Artwork loads as it's first drawn.
    pub art: &'a mut Art,
    /// The diagnostics bar keeps its readings between frames.
    pub diagnostics: &'a mut diagnostics::Diagnostics,
    pub ui: &'a mut UiState,
    pub actions: &'a mut Vec<Action>,
    /// Track waveforms, for the seek bar (experimental).
    pub waveforms: &'a mut crate::visuals::waveform::Waveforms,
}

/// What views work out from versioned data, kept between frames so a frame
/// only draws (see [`crate::memo::Memo`]).
#[derive(Default)]
pub struct Memos {
    /// The album page's summary, credits and footer.
    pub(super) album: Memo<super::album_page::AlbumKey, super::album_page::AlbumFacts>,
    /// The playing track's lyrics as rows (stanzas, labels, breaks).
    pub(super) lyrics: Memo<super::now_playing_view::LyricsKey, Vec<crate::lyrics::Row>>,
    /// The Info tab's rows for the playing song.
    pub(super) info: Memo<super::info_panel::InfoKey, super::info_panel::InfoFacts>,
}

/// A string's hash, for memo keys that shouldn't copy the string each frame.
pub fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

impl<'a> ViewCtx<'a> {
    pub fn position_secs(&self) -> f64 {
        self.player.position_secs(self.engine)
    }

    pub fn duration_secs(&self) -> Option<f64> {
        self.player.duration_secs(self.engine)
    }

    /// Whether `track` can play right now: downloaded, or the server is there.
    pub fn playable(&self, track: &Track) -> bool {
        super::playable(self.session, self.downloads, track)
    }

    /// This frame's spectrum of what's playing, for a visual on screen;
    /// `None` unless playing. Not only a read: while playing, asking for it
    /// keeps frames coming at the meters' rate (so call it only for a visual
    /// that's drawn). The returned `Frame` is a copy, but its waveform is
    /// shared, not copied.
    pub fn listen(&mut self, ui: &egui::Ui) -> Option<crate::visuals::spectrum::Frame> {
        if self.engine.status.state() != audio::PlayState::Playing {
            return None;
        }
        self.ui.live.want(crate::visuals::METER);
        let (pass, now) = (ui.ctx().cumulative_pass_nr(), ui.input(|i| i.time));
        Some(self.ui.listener.frame(&self.engine.status.tap, pass, now).clone())
    }

    /// The album's download, when it's kept offline (found without making a
    /// `Pin` to compare with: the grid asks for every tile, every frame).
    pub fn album_pin(&self, id: &str) -> Option<&'a offline::PinSummary> {
        let downloads: &'a Downloads = self.downloads;
        downloads.snap.pins.iter().find(|p| matches!(&p.pin, offline::Pin::Album(a) if a == id))
    }

    /// A track's analysis, if there is one for the file as it is now.
    pub fn analysis_of(&self, t: &Track) -> Option<&analysis::TrackAnalysis> {
        self.analysis.of(t)
    }
}

impl OndApp {
    /// Lends the parts a view works with.
    pub(super) fn view(&mut self) -> ViewCtx<'_> {
        ViewCtx {
            data: &self.data,
            player: &self.player,
            engine: &self.engine,
            favorites: &self.favorites,
            downloads: &self.downloads,
            analysis: &self.analysis,
            output: &self.output,
            session: &self.session,
            sync: &self.sync,
            search: &mut self.search,
            lyrics: &self.lyrics,
            song_stats: &self.song_stats,
            art: &mut self.art,
            diagnostics: &mut self.diagnostics,
            ui: &mut self.ui,
            actions: &mut self.actions,
            waveforms: &mut self.waveforms,
        }
    }
}
