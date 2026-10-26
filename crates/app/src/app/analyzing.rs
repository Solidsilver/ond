//! Library analysis (loudness, dynamic range, fake hi-res checks): the
//! stored results, the background runner, and the library-wide run that
//! resumes after a restart until it's done.

use super::{Action, OndApp};
use crate::analyze;
use crate::bridge::Msg;
use crate::prefs::Prefs;
use library::Track;
use std::collections::HashMap;

/// Library analysis: stored results and the background runner.
pub struct Analysis {
    /// Stored analysis results by track id, with the file stamp they're for.
    pub records: HashMap<String, analysis::Record>,
    /// Bumped whenever `records` changes, so counts over it know to recount.
    pub version: u64,
    /// How much of the library has no current analysis, per (records, tracks) version.
    pub unanalysed: crate::memo::Memo<(u64, u64), (usize, f64)>,
    /// The stored results have been read (until then everything looks unanalysed).
    pub loaded: bool,
    pub runner: analyze::Runner,
    /// This session queued the library-wide run (see `check_library_analysis`).
    pub library_run: bool,
}

impl Analysis {
    pub(super) fn new(runner: analyze::Runner) -> Self {
        Self { records: HashMap::new(), version: 0, unanalysed: Default::default(), loaded: false, runner, library_run: false }
    }

    /// Applies the analysis actions that need only the runner and preferences.
    pub(super) fn apply(&mut self, action: Action, prefs: &Prefs) {
        match action {
            Action::PauseAnalysis(paused) => self.runner.set_paused(paused),
            Action::StopAnalysis => {
                self.runner.clear();
                self.runner.set_paused(false);
                // A library-wide run isn't resumed at the next launch either.
                prefs.edit(|s| s.analysis_resume = false);
            }
            Action::SetAnalyzePlaying(on) => prefs.edit(|s| s.analyze_playing = on),
            _ => unreachable!("not an analysis action"),
        }
    }

    /// A track's analysis, if there is one for the file as it is now.
    pub fn of(&self, t: &Track) -> Option<&analysis::TrackAnalysis> {
        self.records.get(&t.id).filter(|r| r.stamp == analyze::stamp_of(t)).map(|r| &r.analysis)
    }
}

impl OndApp {
    /// A result came in, or the stored ones were read.
    pub(super) fn analysis_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Analyzed { track_id, record } => {
                self.analysis.records.insert(track_id, record);
                self.analysis.version += 1;
            }
            Msg::AnalysesLoaded(rows) => {
                self.analysis.records.extend(rows);
                self.analysis.version += 1;
                self.analysis.loaded = true;
                self.resume_library_analysis();
            }
            _ => unreachable!("not an analysis message"),
        }
    }

    /// Analyses `tracks` (those without a current result), unpausing the runner.
    pub(super) fn analyze(&mut self, tracks: &[Track]) {
        self.analysis.runner.set_paused(false);
        self.enqueue_analysis(tracks);
    }

    /// Starts the library-wide run; it resumes after a restart until done.
    pub(super) fn analyze_library(&mut self) {
        self.prefs.edit(|s| s.analysis_resume = true);
        self.analysis.runner.set_paused(false);
        self.resume_library_analysis();
    }

    /// A track's analysis, if there is one for the file as it is now.
    pub fn analysis_of(&self, t: &Track) -> Option<&analysis::TrackAnalysis> {
        self.analysis.of(t)
    }

    /// Queues the tracks that have no current analysis; returns how many.
    /// Downloaded copies are read straight from disk (and need no server).
    /// The stream URLs are made by the runner as it gets to each track.
    pub(super) fn enqueue_analysis(&self, tracks: &[Track]) -> usize {
        let online = self.session.client.is_some();
        let jobs: Vec<analyze::Job> = tracks
            .iter()
            .filter(|t| self.analysis_of(t).is_none())
            .filter_map(|t| {
                let local = self.downloads.local_path(&t.id);
                (online || local.is_some()).then(|| analyze::job_for(t, local))
            })
            .collect();
        self.analysis.runner.enqueue(jobs)
    }

    /// Queues the whole library, once the tracks, the stored results and the
    /// server are all there (so finished tracks aren't redone).
    pub(super) fn resume_library_analysis(&mut self) {
        if self.analysis_library() && self.analysis.loaded && self.session.client.is_some() && !self.data.tracks.all.is_empty() {
            let all = self.data.tracks.all.clone();
            self.enqueue_analysis(&all);
            self.analysis.library_run = true;
            if !self.analysis.runner.progress().active() {
                self.finish_library_analysis();
            }
        }
    }

    /// The library run, once queued, has nothing left to do: it's finished
    /// and isn't resumed at the next launch. Until it has been queued (the
    /// server not answered yet, or offline) there's nothing to finish.
    pub(super) fn check_library_analysis(&mut self) {
        if self.analysis.library_run && self.analysis_library() && !self.analysis.runner.progress().active() {
            self.finish_library_analysis();
        }
    }

    pub(super) fn finish_library_analysis(&mut self) {
        self.analysis.library_run = false;
        if self.analysis_library() {
            self.prefs.edit(|s| s.analysis_resume = false);
        }
    }

    /// A library-wide analysis should be running (resumed after a restart).
    pub(super) fn analysis_library(&self) -> bool {
        self.prefs.get(|s| s.analysis_resume)
    }

    /// Analyse each track as it plays (it's in the playback cache by then).
    pub fn analyze_playing(&self) -> bool {
        self.prefs.get(|s| s.analyze_playing)
    }

    /// Tracks in the library without a current analysis, and their total length.
    /// Counted once per change to either, not every frame Settings is open.
    pub fn unanalysed(&mut self) -> (usize, f64) {
        let key = (self.analysis.version, self.data.tracks_version);
        let (analyses, tracks) = (&self.analysis.records, &self.data.tracks.all);
        *self.analysis.unanalysed.get(key, || {
            tracks
                .iter()
                .filter(|t| !analyses.get(&t.id).is_some_and(|r| r.stamp == crate::analyze::stamp_of(t)))
                .fold((0, 0.0), |(n, secs), t| (n + 1, secs + f64::from(t.duration_secs.unwrap_or(0))))
        })
    }
}
