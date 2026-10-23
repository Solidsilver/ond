//! The system's folder picker.

use std::path::{Path, PathBuf};

/// Asks for a folder, starting in `start`. Blocks until the user chooses or
/// cancels; call from the UI thread.
#[cfg(target_os = "macos")]
pub fn pick(message: &str, start: &Path) -> Option<PathBuf> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSOpenPanel;
    use objc2_foundation::{NSString, NSURL};

    let mtm = MainThreadMarker::new()?;
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(false);
    panel.setCanCreateDirectories(true);
    panel.setAllowsMultipleSelection(false);
    panel.setPrompt(Some(&NSString::from_str("Choose")));
    panel.setMessage(Some(&NSString::from_str(message)));
    let start = NSURL::fileURLWithPath(&NSString::from_str(&start.to_string_lossy()));
    panel.setDirectoryURL(Some(&start));
    // NSModalResponseOK
    if panel.runModal() != 1 {
        return None;
    }
    let path = panel.URL()?.path()?;
    Some(PathBuf::from(path.to_string()))
}

#[cfg(not(target_os = "macos"))]
pub fn pick(_: &str, _: &Path) -> Option<PathBuf> {
    None
}
