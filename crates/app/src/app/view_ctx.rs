//! What a view gets to work with: the library, playback, downloads and the
//! rest to read, the window's own state to change, and a list of [`Action`]s
//! to ask for everything else. A view that takes a `ViewCtx` can't start a
//! request or touch the engine behind the app's back; the app applies the
//! actions after the frame.

use super::{Action, Analysis, Data, Downloads, ListenerApp, SearchState, Session, SongStats, SyncState, UiState, diagnostics};
use crate::art::Art;
use crate::favorites::Favorites;
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
    pub search: &'a SearchState,
    pub song_stats: &'a HashMap<String, Option<SongStats>>,
    /// Artwork loads as it's first drawn.
    pub art: &'a mut Art,
    /// The diagnostics bar keeps its readings between frames.
    pub diagnostics: &'a mut diagnostics::Diagnostics,
    pub ui: &'a mut UiState,
    pub actions: &'a mut Vec<Action>,
}

impl ViewCtx<'_> {
    pub fn position_secs(&self) -> f64 {
        self.player.position_secs(self.engine)
    }

    pub fn duration_secs(&self) -> Option<f64> {
        self.player.duration_secs(self.engine)
    }

    /// A track's analysis, if there is one for the file as it is now.
    pub fn analysis_of(&self, t: &Track) -> Option<&analysis::TrackAnalysis> {
        self.analysis.of(t)
    }
}

impl ListenerApp {
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
            search: &self.search,
            song_stats: &self.song_stats,
            art: &mut self.art,
            diagnostics: &mut self.diagnostics,
            ui: &mut self.ui,
            actions: &mut self.actions,
        }
    }
}
