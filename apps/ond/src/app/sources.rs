//! Sources as the user manages them: a folder of music added from the
//! first-run sheet (its form, drawn in the sheet's column), a source's
//! folders changed in Settings › Sources, the folder picker that both use,
//! and removing a source.
//!
//! Ond holds one source live at a time: the first in `settings.json`'s list.
//! Adding a folder makes it that one; a server configured before stays in
//! the list with its library kept on disk, only not in use (signing in to
//! it again makes it the live one). Running several at once is for later
//! (docs/sources.md, milestone 4).
//!
//! The picker is a seam ([`FolderPicker`]): the app's is the system's
//! dialog, and tests and snapshots hand in one that answers at once. Its
//! answer comes back as a [`Msg::FolderChosen`](crate::bridge::Msg), as
//! background work's results do.

use super::session::LoginForm;
use super::{Action, OndApp};
use crate::bridge::Msg;
use crate::settings::{FolderEntry, SourceKind};
use accounts::AccountId;
use shell::kit;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Asks for a folder (its message, where to start) and hands the choice, or
/// `None` when cancelled, to the callback, from any thread.
pub type FolderPicker = Arc<dyn Fn(&str, &Path, Box<dyn FnOnce(Option<PathBuf>) + Send>) + Send + Sync>;

/// The system's folder picker.
pub fn system_picker() -> FolderPicker {
    Arc::new(crate::folder::pick)
}

/// Where a chosen folder goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderTarget {
    /// The first-run sheet's folder form.
    Onboarding,
    /// A configured source's folders, from Settings.
    Source(AccountId),
}

/// The first-run sheet's folder form while it's up.
#[derive(Debug, Default)]
pub(super) struct FolderForm {
    pub(super) folders: Vec<PathBuf>,
    /// What the source is to be called; empty for its first folder's name.
    pub(super) name: String,
    /// Added, and the first scan is running.
    pub(super) busy: bool,
    pub(super) error: Option<String>,
    /// The source once added, so a second try changes it rather than adding
    /// another.
    pub(super) account: Option<AccountId>,
}

/// A change to the configured sources.
pub enum SourceAction {
    /// Choose folder…: open the picker for `target`.
    Choose(FolderTarget),
    /// The folder form's Add: the folders become a source, which connects
    /// and is scanned.
    AddFolders,
    /// Close the sheet while the first scan carries on.
    KeepScanning,
    /// Take the folder at this index off a source's list (and rescan).
    RemoveFolder(AccountId, usize),
    /// Remove…: ask first.
    AskRemove(AccountId),
    /// The answer to [`AskRemove`](SourceAction::AskRemove).
    Remove(AccountId),
}

impl From<SourceAction> for Action {
    fn from(a: SourceAction) -> Self {
        Action::Sources(a)
    }
}

/// The folder form's rough height, for the sheet's layout.
pub(super) const FOLDER_FORM_H: f32 = 300.0;

impl OndApp {
    pub(super) fn apply_source(&mut self, action: SourceAction) {
        match action {
            SourceAction::Choose(target) => self.choose_folder(target),
            SourceAction::AddFolders => self.add_folders(),
            SourceAction::KeepScanning => {
                self.onboarding = None;
                self.login = None;
            }
            SourceAction::RemoveFolder(id, at) => {
                let Some(mut folders) = self.prefs.get(|s| s.source(id).map(|e| e.folders.clone())) else { return };
                if folders.len() <= 1 {
                    return self.show_notice("A music folder source needs at least one folder. Remove the source instead.");
                }
                if at < folders.len() {
                    folders.remove(at);
                    self.set_folders(id, folders);
                }
            }
            SourceAction::AskRemove(id) => {
                let Some(name) = self.prefs.get(|s| s.source(id).map(|e| e.name.clone().unwrap_or_else(|| super::sidebar::default_name(e)))) else {
                    return;
                };
                self.ui.overlay = Some(super::Overlay::RemoveSource { id, name });
            }
            SourceAction::Remove(id) => self.remove_source(id),
        }
    }

    /// The picker's answer: into the sheet's form, or onto a source's list.
    pub(super) fn folder_chosen(&mut self, target: FolderTarget, path: PathBuf) {
        match target {
            FolderTarget::Onboarding => {
                if let Some(form) = self.onboarding.as_mut().map(|o| &mut o.folders)
                    && !form.busy
                {
                    form.error = None;
                    if !form.folders.contains(&path) {
                        form.folders.push(path);
                    }
                }
            }
            FolderTarget::Source(id) => {
                let Some(mut folders) = self.prefs.get(|s| s.source(id).map(|e| e.folders.clone())) else { return };
                if !folders.iter().any(|f| f.path == path) {
                    folders.push(FolderEntry::new(path));
                    self.set_folders(id, folders);
                }
            }
        }
    }

    fn choose_folder(&self, target: FolderTarget) {
        let start = match target {
            FolderTarget::Source(id) => self.prefs.get(|s| s.source(id).and_then(|e| e.folders.first().and_then(|f| f.path.parent()).map(Path::to_path_buf))),
            FolderTarget::Onboarding => None,
        };
        let start = start.or_else(|| directories::UserDirs::new().map(|u| u.audio_dir().unwrap_or(u.home_dir()).to_path_buf())).unwrap_or_default();
        let bridge = self.bridge.clone();
        (self.pick_folder)(
            "Choose a folder of music",
            &start,
            Box::new(move |chosen| {
                if let Some(path) = chosen {
                    bridge.post(Msg::FolderChosen { target, path });
                }
            }),
        );
    }

    /// The folder form's Add: the folders become the source Ond plays from,
    /// connected and scanned while the sheet shows how far it's got.
    fn add_folders(&mut self) {
        let Some(form) = self.onboarding.as_mut().map(|o| &mut o.folders).filter(|f| !f.busy) else { return };
        if form.folders.is_empty() {
            form.error = Some("Choose a folder first.".into());
            return;
        }
        let name = Some(form.name.trim().to_owned()).filter(|n| !n.is_empty());
        let folders = form.folders.clone();
        let mut id = form.account;
        self.prefs.edit(|s| {
            let account = match id.filter(|a| s.source(*a).is_some()) {
                Some(a) => {
                    let existing = s.source(a).map(|e| e.folders.clone()).unwrap_or_default();
                    let entries =
                        folders.iter().map(|p| existing.iter().find(|f| f.path == *p).cloned().unwrap_or_else(|| FolderEntry::new(p.clone()))).collect();
                    s.set_folders(a, entries);
                    s.set_source_name(a, name.clone());
                    a
                }
                None => {
                    let entries = folders.iter().cloned().map(FolderEntry::new).collect();
                    s.add_local(name.clone(), entries)
                }
            };
            s.make_primary(account);
            s.signed_out = false;
            id = Some(account);
        });
        let form = self.onboarding.as_mut().map(|o| &mut o.folders).expect("checked above");
        form.account = id;
        form.busy = true;
        form.error = None;
        // Whatever was connecting (a server signed out of) delivers nothing now.
        self.bridge.end_session();
        self.forget_in_flight();
        self.forget_waiting();
        let settings = self.prefs.snapshot();
        self.session.settings = Some(settings.clone());
        self.session.connecting = true;
        self.bridge.connect(settings, None);
    }

    /// The first scan of a source added from the sheet ended: on success the
    /// sheet closes on the library; a failure goes back to the form. True
    /// when the sheet took it (so it needn't be said again in a toast).
    pub(super) fn first_scan_done(&mut self, error: Option<&str>) -> bool {
        let Some(form) = self.onboarding.as_mut().map(|o| &mut o.folders).filter(|f| f.busy) else { return false };
        match error {
            None => {
                self.onboarding = None;
                self.login = None;
            }
            Some(e) => {
                form.busy = false;
                form.error = Some(e.to_owned());
            }
        }
        true
    }

    /// A source's folders changed: saved, and when it's the one in use it
    /// connects again with them and is scanned.
    fn set_folders(&mut self, id: AccountId, folders: Vec<FolderEntry>) {
        self.prefs.edit(|s| s.set_folders(id, folders.clone()));
        if let Some(s) = self.session.settings.as_mut() {
            s.set_folders(id, folders);
        }
        if self.session.account() == Some(id) {
            let settings = self.prefs.snapshot();
            self.session.settings = Some(settings.clone());
            self.bridge.connect(settings, None);
        }
    }

    /// Takes a source off the list and deletes Ond's copy of it (its library
    /// mirror, scan index, analyses and cached art; never the music files).
    /// When it was the one in use, the next one configured takes over, or the
    /// first-run sheet opens.
    fn remove_source(&mut self, id: AccountId) {
        let Some(entry) = self.prefs.get(|s| s.source(id).cloned()) else { return };
        let name = entry.name.clone().unwrap_or_else(|| super::sidebar::default_name(&entry));
        let live = self.session.account() == Some(id);
        self.prefs.edit(|s| {
            s.remove_source(id);
        });
        if let Some(s) = self.session.settings.as_mut() {
            s.remove_source(id);
        }
        if !live {
            self.bridge.forget_account(id);
            self.ui.overlay = Some(super::Overlay::Settings);
            return self.show_notice(removed_notice(&name, entry.kind));
        }
        self.with_player(|p, cx| p.reset(cx));
        self.ui.overlay = None;
        self.ui.palette = None;
        self.favorites = Default::default();
        self.lyrics.clear();
        self.song_stats.clear();
        self.forget_waiting();
        self.bridge.end_session();
        self.analysis.runner.set_server(None);
        self.analysis.runner.clear();
        self.analysis.library_run = false;
        self.set_offline();
        self.session.unreachable = false;
        self.session.connecting = false;
        self.session.sign_in = None;
        self.session.folder_missing.clear();
        self.session.skipped = 0;
        self.session.server_label.clear();
        self.now_playing.clear_source();
        self.art.clear_source();
        if let Some(store) = self.bridge.forget_account(id) {
            self.analysis.records.clear();
            self.analysis.version += 1;
            self.analysis.runner.set_store(store);
        }
        self.data.clear();
        self.search.stale = true;
        self.ui.nav = Default::default();
        let next = self.prefs.snapshot();
        match next.primary().map(|e| (e.kind, next.signed_out)) {
            None => {
                self.session.settings = None;
                self.login = Some(LoginForm { server: "http://localhost:4533".into(), ..Default::default() });
                self.onboarding = Some(super::onboarding::Onboard::default());
            }
            Some((SourceKind::Subsonic, true)) => {
                self.session.settings = Some(next.clone());
                self.login = Some(LoginForm { server: next.server().to_owned(), username: next.username().to_owned(), ..Default::default() });
                self.onboarding = Some(super::onboarding::Onboard::default());
            }
            Some(_) => {
                self.session.settings = Some(next.clone());
                self.session.connecting = true;
                self.bridge.connect(next, None);
            }
        }
        self.show_notice(removed_notice(&name, entry.kind));
    }

    /// The first-run sheet's form for a folder of music: the folders, with
    /// Choose folder…, an optional name, and Add. While the first scan runs,
    /// how many tracks it's found, and a way to carry on without waiting.
    pub(super) fn folder_form(&mut self, ui: &mut egui::Ui) {
        let t = crate::theme::Tokens::of(ui);
        let progress = self.sync.progress.map(|p| (p.done, p.total));
        let Some(form) = self.onboarding.as_mut().map(|o| &mut o.folders) else { return };
        kit::field_label(ui, "Folders");
        ui.add_space(2.0);
        match kit::folder_list(ui, &form.folders, "No folder chosen yet", form.busy) {
            Some(kit::FolderListEvent::Remove(i)) if i < form.folders.len() => {
                form.folders.remove(i);
            }
            Some(kit::FolderListEvent::Choose) => self.actions.push(SourceAction::Choose(FolderTarget::Onboarding).into()),
            _ => {}
        }
        ui.add_space(12.0);
        let default = form.folders.first().and_then(|f| f.file_name()).map_or_else(|| "Music".to_owned(), |n| n.to_string_lossy().into_owned());
        let mut enter = false;
        ui.add_enabled_ui(!form.busy, |ui| {
            enter = kit::TextField::new("Name").hint(&default).help(Some("Optional. How Ond shows this source.")).show(ui, &mut form.name);
        });
        ui.add_space(18.0);
        let label = if form.busy { "Scanning…" } else { "Add" };
        let add = kit::form_button(ui, label, true, form.busy.then_some(true));
        if form.busy {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
            ui.add_space(10.0);
            let found = match progress {
                Some((done, total)) if total > 0 => format!("{} of {} tracks", crate::fmt::thousands(done), crate::fmt::thousands(total)),
                _ => "Looking for music…".to_owned(),
            };
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(found).size(12.0).color(t.muted));
                ui.add_space(6.0);
                if crate::widgets::link(ui, "Continue while it scans", egui::FontId::proportional(12.0), t.accent).clicked() {
                    self.actions.push(SourceAction::KeepScanning.into());
                }
            });
        } else if add.clicked() || enter {
            self.actions.push(SourceAction::AddFolders.into());
        }
        if let Some(form) = self.onboarding.as_ref().map(|o| &o.folders)
            && let Some(e) = &form.error
        {
            ui.add_space(12.0);
            kit::field_error(ui, "Folders", e);
        }
    }
}

/// What's said once a source is gone, which for a folder includes that its
/// music is still there.
fn removed_notice(name: &str, kind: SourceKind) -> String {
    match kind {
        SourceKind::Local => format!("Removed “{name}”. Your music files are still in their folders."),
        SourceKind::Subsonic => format!("Removed “{name}”."),
    }
}
