//! The system's folder picker and file manager.

use std::path::{Path, PathBuf};

/// Asks for a folder, starting in `start`, and hands the choice (`None` when
/// cancelled) to `done`. Call from the UI thread. On macOS and Windows the
/// dialog is modal and `done` runs before this returns; on Linux the portal's
/// dialog is another process, so it's waited for on a thread of its own and
/// the window keeps drawing (and answering the compositor) meanwhile.
pub fn pick(message: &str, start: &Path, done: impl FnOnce(Option<PathBuf>) + Send + 'static) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dialog = rfd::FileDialog::new().set_title(message).set_directory(start);
        let spawned = std::thread::Builder::new().name("folder-picker".into()).spawn(move || done(dialog.pick_folder()));
        if let Err(e) = spawned {
            tracing::warn!("folder picker: {e}");
        }
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    done(modal(message, start));
}

#[cfg(target_os = "macos")]
fn modal(message: &str, start: &Path) -> Option<PathBuf> {
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

/// The common item dialog.
#[cfg(windows)]
fn modal(message: &str, start: &Path) -> Option<PathBuf> {
    rfd::FileDialog::new().set_title(message).set_directory(start).pick_folder()
}

/// What the system's file manager is called, for "Show in ...".
pub const FILE_MANAGER: &str = if cfg!(target_os = "macos") {
    "Finder"
} else if cfg!(windows) {
    "File Explorer"
} else {
    "File Manager"
};

/// Opens `dir` in the file manager.
pub fn reveal(dir: &Path) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener).arg(dir).spawn().map(drop)
}
