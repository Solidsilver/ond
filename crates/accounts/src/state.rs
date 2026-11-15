//! Live state of a source, shared across apps without knowing its kind.

/// Why a source is offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OfflineReason {
    /// The server didn't answer; the app keeps trying in the background.
    Unreachable,
    /// The user chose to work offline, so no network requests are made.
    ByChoice,
}

/// What a source is doing right now.
///
/// Live state (not saved to disk), shared across Ond and Rede so `shell::kit`
/// can draw source status in switchers, menus and settings rows without knowing
/// what kind of source it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SourceState {
    /// Connected and ready, or a local folder that is present.
    Online,
    /// Opening or checking the connection.
    Connecting,
    /// Fetching changes from a remote source, with an optional completion
    /// fraction in `0.0..=1.0` (`None` when the total is not yet known).
    Syncing(Option<f32>),
    /// Walking a local folder for changes.
    Scanning,
    /// Not reaching the source.
    Offline(OfflineReason),
    /// Credentials expired or were turned away; the user must sign in again.
    SignedOut,
    /// A configured local folder is missing or unreadable.
    FolderMissing,
}

impl SourceState {
    /// Whether the source needs user action (shown in place of the usual
    /// detail line in switchers and menus).
    pub fn needs_attention(&self) -> bool {
        matches!(self, Self::SignedOut | Self::FolderMissing)
    }

    /// A short label in the apps' plain voice.
    pub fn label(&self) -> String {
        match self {
            Self::Online => "Online".to_owned(),
            Self::Connecting => "Connecting…".to_owned(),
            Self::Syncing(Some(fraction)) if fraction.is_finite() => {
                format!("Syncing {:.0}%", fraction.clamp(0.0, 1.0) * 100.0)
            }
            Self::Syncing(_) => "Syncing…".to_owned(),
            Self::Scanning => "Scanning…".to_owned(),
            Self::Offline(OfflineReason::Unreachable) => "Offline".to_owned(),
            Self::Offline(OfflineReason::ByChoice) => "Offline mode".to_owned(),
            Self::SignedOut => "Signed out".to_owned(),
            Self::FolderMissing => "Folder missing".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_signed_out_and_folder_missing_need_attention() {
        let states = [
            (SourceState::Online, false),
            (SourceState::Connecting, false),
            (SourceState::Syncing(None), false),
            (SourceState::Syncing(Some(0.4)), false),
            (SourceState::Scanning, false),
            (SourceState::Offline(OfflineReason::Unreachable), false),
            (SourceState::Offline(OfflineReason::ByChoice), false),
            (SourceState::SignedOut, true),
            (SourceState::FolderMissing, true),
        ];
        for (state, expected) in states {
            assert_eq!(state.needs_attention(), expected, "{state:?}");
        }
    }

    #[test]
    fn labels_match_the_apps_plain_voice() {
        assert_eq!(SourceState::Online.label(), "Online");
        assert_eq!(SourceState::Connecting.label(), "Connecting…");
        assert_eq!(SourceState::Syncing(None).label(), "Syncing…");
        assert_eq!(SourceState::Syncing(Some(0.4)).label(), "Syncing 40%");
        assert_eq!(SourceState::Scanning.label(), "Scanning…");
        assert_eq!(SourceState::Offline(OfflineReason::Unreachable).label(), "Offline");
        assert_eq!(SourceState::Offline(OfflineReason::ByChoice).label(), "Offline mode");
        assert_eq!(SourceState::SignedOut.label(), "Signed out");
        assert_eq!(SourceState::FolderMissing.label(), "Folder missing");
    }

    #[test]
    fn syncing_clamps_out_of_range_progress_and_handles_non_finite_fractions() {
        assert_eq!(SourceState::Syncing(Some(-0.25)).label(), "Syncing 0%");
        assert_eq!(SourceState::Syncing(Some(1.5)).label(), "Syncing 100%");
        assert_eq!(SourceState::Syncing(Some(f32::NAN)).label(), "Syncing…");
        assert_eq!(SourceState::Syncing(Some(f32::INFINITY)).label(), "Syncing…");
    }
}
