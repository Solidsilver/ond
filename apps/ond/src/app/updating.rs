//! Update actions and the app's projection of the shared checker's answers.
//! Checks and notes describe the app release, independent of any sign-in.

use super::{OndApp, PrefsAction};
use crate::updates::{self, UpdateChannel, UpdateCheck, UpdateMsg};

pub use shell::updates::{UpdateAction, Updates};

impl OndApp {
    pub(super) fn update_action(&mut self, action: UpdateAction) {
        match action {
            UpdateAction::Open(v) => self.ctx.open_url(egui::OpenUrl::new_tab(v.page())),
            UpdateAction::Skip(v) => {
                self.prefs.edit(|s| s.skipped_update = Some(v.to_string()));
                self.select_update();
            }
            UpdateAction::Check(check) => self.apply_prefs(PrefsAction::Updates(check)),
            UpdateAction::Channel(channel) => self.apply_prefs(PrefsAction::UpdateChannel(channel)),
            UpdateAction::CheckNow => {
                if updates::channel() == updates::Channel::GitHub {
                    self.updates.checking = true;
                    self.update_now.notify_one();
                }
            }
            UpdateAction::Restart => {
                if self.updates.request_restart() {
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            UpdateAction::Retry => {
                self.updates.cancel_install();
                self.select_update();
            }
            UpdateAction::Notes(v) => {
                if self.updates.notes.as_ref().is_none_or(|(n, _)| *n != v) {
                    self.updates.notes = Some((v.clone(), None));
                    self.bridge.update_notes(v);
                }
            }
        }
    }

    pub(super) fn set_update_check(&mut self, check: UpdateCheck) {
        self.prefs.edit(|s| s.updates = check);
        self.update_check.store(self.prefs.get(|s| s.updates) != UpdateCheck::Off, std::sync::atomic::Ordering::Relaxed);
        self.select_update();
    }

    pub(super) fn set_update_channel(&mut self, channel: Option<UpdateChannel>) {
        self.prefs.edit(|s| s.update_channel = channel);
        self.select_update();
    }

    /// The channel in effect, including the installed build's default.
    pub fn effective_update_channel(&self) -> UpdateChannel {
        UpdateChannel::effective(self.prefs.get(|s| s.update_channel), &updates::current())
    }

    fn select_update(&mut self) {
        let channel = self.effective_update_channel();
        self.update = self.prefs.get(|s| self.updates.select(&updates::current(), Some(channel), s.updates, s.skipped_update.as_deref()));
        if updates::channel() == updates::Channel::GitHub {
            let check = self.prefs.get(|s| s.updates);
            self.bridge.stage_update(&mut self.updates, self.update.as_ref(), check, self.update_cache.clone());
        }
    }

    pub(super) fn update_message(&mut self, msg: UpdateMsg) {
        match msg {
            UpdateMsg::Install(event) => self.updates.install_event(event),
            UpdateMsg::InstallError(error) => self.updates.install_error = Some(error),
            UpdateMsg::Checked(result) => {
                self.updates.receive(result);
                self.select_update();
            }
            UpdateMsg::ReleaseNotes(v, notes) => {
                if let Some((n, slot)) = &mut self.updates.notes
                    && *n == v
                {
                    *slot = Some(notes);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, Msg, PrefsAction};
    use crate::updates::{Candidates, Version};

    fn candidates() -> Candidates {
        Candidates { release: Some(Version::parse("99.0.0").unwrap()), beta: Some(Version::parse("100.0.0-beta.2").unwrap()) }
    }

    #[test]
    fn install_messages_expose_progress_failure_and_the_notify_only_reason() {
        use crate::updates::install::{Error, Event, EventKind, Fallback, State};
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = super::super::fixture::populated(&ctx, dir.path());
        app.updates.install = State::Downloading { version: candidates().release.unwrap(), received: 0, total: None };
        app.handle(Msg::Update(UpdateMsg::Install(Event { generation: 0, kind: EventKind::Progress { received: 12, total: Some(24) } })));
        assert!(matches!(app.updates.install, State::Downloading { received: 12, total: Some(24), .. }));
        app.handle(Msg::Update(UpdateMsg::Install(Event { generation: 0, kind: EventKind::Finished(Err(Error::Signature)) })));
        assert!(matches!(app.updates.install, State::Failed { error: Error::Signature, .. }));
        app.handle(Msg::Update(UpdateMsg::Install(Event { generation: 0, kind: EventKind::NotifyOnly(Fallback::ReadOnly) })));
        assert!(matches!(app.updates.install, State::NotifyOnly { reason: Fallback::ReadOnly, .. }));
        app.handle(Msg::Update(UpdateMsg::InstallError(Error::Command("helper"))));
        assert!(matches!(app.updates.install_error, Some(Error::Command("helper"))));
    }

    #[test]
    fn check_results_select_each_channel_and_switch_without_another_check() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = super::super::fixture::populated(&ctx, dir.path());
        for channel in [UpdateChannel::Releases, UpdateChannel::Betas] {
            app.apply(Action::Prefs(PrefsAction::UpdateChannel(Some(channel))));
            app.handle(Msg::Update(UpdateMsg::Checked(Ok(candidates()))));
            let expected = candidates().latest(channel).cloned();
            assert_eq!(app.update, expected);
            assert_eq!(app.updates.latest, expected);
            assert_eq!(app.effective_update_channel(), channel);
        }
        let checked = app.updates.checked;
        app.apply(Action::Update(UpdateAction::Channel(Some(UpdateChannel::Releases))));
        assert_eq!(app.update, candidates().release);
        assert_eq!(app.updates.checked, checked);
        assert!(!app.updates.checking);
        app.apply(Action::Update(UpdateAction::Channel(None)));
        let follows = UpdateChannel::effective(None, &updates::current());
        assert_eq!(app.effective_update_channel(), follows);
        assert_eq!(app.update, candidates().latest(follows).cloned());
        assert_eq!(app.prefs.get(|s| s.update_channel), None);
    }

    #[test]
    fn skipping_a_beta_survives_channel_switches_and_the_next_beta_shows() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = super::super::fixture::populated(&ctx, dir.path());
        app.apply(Action::Update(UpdateAction::Channel(Some(UpdateChannel::Betas))));
        app.handle(Msg::Update(UpdateMsg::Checked(Ok(candidates()))));
        let beta = candidates().beta.unwrap();
        app.apply(Action::Update(UpdateAction::Skip(beta.clone())));
        assert!(app.update.is_none());
        assert_eq!(app.updates.latest, Some(beta.clone()));
        assert_eq!(app.prefs.get(|s| s.skipped_update.clone()).as_deref(), Some("100.0.0-beta.2"));
        app.apply(Action::Update(UpdateAction::Channel(Some(UpdateChannel::Releases))));
        assert_eq!(app.update, candidates().release);
        app.apply(Action::Update(UpdateAction::Channel(Some(UpdateChannel::Betas))));
        assert!(app.update.is_none());
        app.handle(Msg::Update(UpdateMsg::Checked(Ok(candidates()))));
        assert!(app.update.is_none());
        let next = Version::parse("100.0.0-beta.3").unwrap();
        app.handle(Msg::Update(UpdateMsg::Checked(Ok(Candidates { beta: Some(next.clone()), ..candidates() }))));
        assert_eq!(app.update, Some(next.clone()));
        app.apply(Action::Update(UpdateAction::Check(UpdateCheck::Off)));
        assert!(app.update.is_none());
        app.handle(Msg::Update(UpdateMsg::Checked(Ok(Candidates { beta: Some(next.clone()), ..candidates() }))));
        assert!(app.update.is_none(), "a check already in flight cannot restore the notice");
        app.apply(Action::Update(UpdateAction::Check(UpdateCheck::Notify)));
        assert_eq!(app.update, Some(next));
        let view = crate::app::PrefsView::read(&app.prefs);
        assert_eq!(view.updates, UpdateCheck::Notify);
        assert_eq!(view.update_channel, Some(UpdateChannel::Betas));
        assert_eq!(view.skipped_update, Some(beta.to_string()));
    }

    #[test]
    fn a_check_with_only_betas_leaves_the_releases_channel_empty() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut app = super::super::fixture::populated(&ctx, dir.path());
        app.apply(Action::Update(UpdateAction::Channel(Some(UpdateChannel::Releases))));
        let beta = candidates().beta;
        app.handle(Msg::Update(UpdateMsg::Checked(Ok(Candidates { release: None, beta: beta.clone() }))));
        assert!(app.updates.checked.is_some());
        assert!(app.updates.error.is_none());
        assert!(app.updates.latest.is_none() && app.update.is_none());
        app.apply(Action::Update(UpdateAction::Channel(Some(UpdateChannel::Betas))));
        assert_eq!(app.update, beta);
    }
    #[test]
    fn a_managed_install_choice_is_accepted_and_locked() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("managed.json"), r#"{"updates":"install"}"#).unwrap();
        let ctx = egui::Context::default();
        let mut app = super::super::fixture::populated(&ctx, dir.path());
        assert_eq!(app.prefs.get(|s| s.updates), UpdateCheck::Install);
        app.apply(Action::Update(UpdateAction::Check(UpdateCheck::Notify)));
        assert_eq!(app.prefs.get(|s| s.updates), UpdateCheck::Install);
    }

    #[test]
    fn managed_update_preferences_lock_both_action_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("managed.json"), r#"{"update_channel":"releases","updates":"off"}"#).unwrap();
        let ctx = egui::Context::default();
        let mut app = super::super::fixture::populated(&ctx, dir.path());
        app.apply(Action::Update(UpdateAction::Channel(Some(UpdateChannel::Betas))));
        app.apply(Action::Prefs(PrefsAction::UpdateChannel(Some(UpdateChannel::Betas))));
        app.apply(Action::Update(UpdateAction::Check(UpdateCheck::Notify)));
        app.apply(Action::Prefs(PrefsAction::Updates(UpdateCheck::Notify)));
        assert_eq!(app.prefs.get(|s| s.update_channel), Some(UpdateChannel::Releases));
        assert_eq!(app.prefs.get(|s| s.updates), UpdateCheck::Off);
        assert_eq!(app.effective_update_channel(), UpdateChannel::Releases);
    }
}
