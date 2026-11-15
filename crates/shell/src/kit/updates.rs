//! What an app says about its own updates, drawn alike in both: About's
//! [`about_updates`] (the Updates group with its channel, whether updates
//! install themselves, how a download is going, and a newer version's release
//! notes under it) and the sidebar's [`update_notice`] above its foot, with
//! the [`notice_dot`] a rail's gear wears instead. The app holds the state
//! ([`Updates`]) and the preferences; these only draw them and say what was
//! asked for as [`UpdateAction`]s. Nothing here repaints on its own: a
//! download's progress is redrawn as its messages arrive.

use super::buttons::{primary, secondary};
use super::controls::{choice, switch};
use super::rows::{Head, group, managed, row_with};
use super::text::link;
use crate::icons::{self, Icon};
use crate::theme::{Tokens, semibold};
use crate::updates::install::{Fallback, State};
use crate::updates::{Block, Channel, UpdateAction, UpdateChannel, UpdateCheck, Updates, Version};
use egui::{FontId, Rect, Response, Sense, Ui, pos2, vec2};

/// The sidebar's update notice: as tall as the foot's buttons.
pub const UPDATE_NOTICE_H: f32 = 28.0;

/// What About's updates are drawn from.
pub struct AboutUpdates<'a> {
    /// The app's name, as the rows say it ("Ond 0.6.0 is available").
    pub app: &'a str,
    /// The version running.
    pub current: &'a Version,
    /// Where this copy gets its updates: only a GitHub copy looks itself.
    pub distribution: Channel,
    pub updates: &'a Updates,
    pub check: UpdateCheck,
    /// The channel the user picked; `None` follows the build.
    pub channel: Option<UpdateChannel>,
    /// The version the user skipped, as saved.
    pub skipped: Option<&'a str>,
    /// Under Check for updates: how often the app looks, and where.
    pub hint: &'a str,
    /// Why Check for updates can't be changed, when something outside the
    /// app sets it (Ond's managed.json): the row is locked and says so, and
    /// so is Install updates automatically, which is the same setting.
    pub check_locked: Option<&'a str>,
    /// The same for Channel.
    pub channel_locked: Option<&'a str>,
}

/// About's Updates group: for a GitHub copy, Check for updates, then (while
/// it's on) Install updates automatically, and the Channel (Releases or
/// Betas, the build's own until the user picks); then a newer version with
/// Download and Skip, or how its download is going, or how the last look went
/// with Check now; for any other, where its updates come from. A newer
/// version's release notes follow in a group of their own, asked for the
/// first time they're drawn. Returns what was asked for, in order.
pub fn about_updates(ui: &mut Ui, about: &AboutUpdates<'_>) -> Vec<UpdateAction> {
    let mut asked = Vec::new();
    let newer = about.updates.latest.clone().filter(|v| v > about.current);
    group(ui, "Updates", |ui| match about.distribution {
        Channel::GitHub => {
            if let Some(check) = check_rows(ui, about) {
                asked.push(UpdateAction::Check(check));
            }
            if let Some(channel) = channel_row(ui, about) {
                asked.push(UpdateAction::Channel(Some(channel)));
            }
            update_status(ui, about, newer.clone(), &mut asked);
        }
        Channel::Packaged(by) => row(ui, &format!("Updates come from {by}"), ""),
        Channel::Source => row(ui, "Built from source", "This copy doesn't look for updates."),
    });
    if let Some(v) = newer {
        release_notes(ui, about, v, &mut asked);
    }
    asked
}

/// Check for updates, and under it while it's on, Install updates
/// automatically: together the one setting, `Off`, `Notify` or `Install`.
/// Turned off and on again, checking comes back installing if it was before;
/// that's remembered for as long as the app runs, not saved.
fn check_rows(ui: &mut Ui, about: &AboutUpdates<'_>) -> Option<UpdateCheck> {
    let locked = about.check_locked.is_some();
    let installed = egui::Id::unique("kit-updates-installed-before-off");
    let mut on = about.check != UpdateCheck::Off;
    let head = Head { label: "Check for updates", hint: about.hint, tag: about.check_locked, ..Default::default() };
    if managed(ui, locked, |ui| row_with(ui, head, |ui| switch(ui, &mut on, "Check for updates")).changed()) {
        if !on {
            ui.data_mut(|d| d.insert_temp(installed, about.check == UpdateCheck::Install));
            return Some(UpdateCheck::Off);
        }
        let install = ui.data(|d| d.get_temp::<bool>(installed)).unwrap_or(false);
        return Some(if install { UpdateCheck::Install } else { UpdateCheck::Notify });
    }
    if about.check == UpdateCheck::Off {
        return None;
    }
    let mut install = about.check == UpdateCheck::Install;
    let hint = format!("{} downloads new versions in the background and installs them when you quit or restart.", about.app);
    let head = Head { label: "Install updates automatically", hint: &hint, tag: about.check_locked, ..Default::default() };
    let changed = managed(ui, locked, |ui| row_with(ui, head, |ui| switch(ui, &mut install, "Install updates automatically")).changed());
    changed.then_some(if install { UpdateCheck::Install } else { UpdateCheck::Notify })
}

/// Releases or Betas. Until the user picks, it shows the one the build
/// follows and says so; a pick is returned.
fn channel_row(ui: &mut Ui, about: &AboutUpdates<'_>) -> Option<UpdateChannel> {
    let mut pick = UpdateChannel::effective(about.channel, about.current);
    let hint = match about.channel {
        None if about.current.is_prerelease() => "Follows this build, which is a beta.",
        None => "Follows this build, which is a full release.",
        Some(UpdateChannel::Releases) => "Full releases only.",
        Some(UpdateChannel::Betas) => "The newest version, beta or full release. A beta may have rough edges.",
    };
    let head = Head { label: "Channel", hint, tag: about.channel_locked, ..Default::default() };
    let all = [UpdateChannel::Releases, UpdateChannel::Betas];
    let changed = managed(ui, about.channel_locked.is_some(), |ui| row_with(ui, head, |ui| choice(ui, &all, &mut pick, None, |c| c.label().into())));
    changed.then_some(pick)
}

/// A row with nothing to change.
fn row(ui: &mut Ui, label: &str, hint: &str) {
    row_with(ui, Head { label, hint, ..Default::default() }, |_| ());
}

/// The rows under the channel: why the last update didn't install, if it
/// didn't; then a newer version (downloading, ready to install, failed, or
/// to download by hand, with why it can't install itself when that's so), or
/// how the last look went, with Check now.
fn update_status(ui: &mut Ui, about: &AboutUpdates<'_>, newer: Option<Version>, asked: &mut Vec<UpdateAction>) {
    let (app, current, u) = (about.app, about.current, about.updates);
    if let Some(e) = &u.install_error {
        row(ui, "The last update didn't install", &sentence(&e.to_string()));
    }
    if let Some(v) = newer {
        let skipped = about.skipped == Some(&v.to_string());
        let beta = if v.is_prerelease() { "A beta. " } else { "" };
        let skip = |ui: &mut Ui, asked: &mut Vec<UpdateAction>| {
            if !skipped && secondary(ui, "Skip").clicked() {
                asked.push(UpdateAction::Skip(v.clone()));
            }
        };
        let install = Some(&u.install).filter(|s| s.version() == Some(&v));
        match install {
            Some(State::Downloading { received, total, .. }) => {
                let size = total.map(|t| format!("{} of {} MB. ", megabytes(*received), megabytes(t))).unwrap_or_default();
                let hint = format!("{beta}{size}You have {current}.");
                row_with(ui, Head { label: &format!("Downloading {app} {v}…"), hint: &hint, ..Default::default() }, |ui| skip(ui, asked));
            }
            Some(State::Ready { .. }) => {
                let hint = format!("{beta}It also installs when you quit {app}.");
                row_with(ui, Head { label: &format!("{app} {v} is ready"), hint: &hint, ..Default::default() }, |ui| {
                    if primary(ui, None, "Restart to Update").clicked() {
                        asked.push(UpdateAction::Restart);
                    }
                    skip(ui, asked);
                });
            }
            Some(State::Failed { error, .. }) => {
                let hint = format!("{beta}{}", sentence(&error.to_string()));
                row_with(ui, Head { label: &format!("{app} {v} didn't download"), hint: &hint, ..Default::default() }, |ui| {
                    if primary(ui, None, "Retry").clicked() {
                        asked.push(UpdateAction::Retry);
                    }
                    skip(ui, asked);
                });
            }
            _ => {
                let mut hint = if skipped { format!("{beta}You have {current}. You skipped this version.") } else { format!("{beta}You have {current}.") };
                if let Some(State::NotifyOnly { reason, .. }) = install {
                    hint.push('\n');
                    hint.push_str(&why_not(app, *reason));
                }
                row_with(ui, Head { label: &format!("{app} {v} is available"), hint: &hint, ..Default::default() }, |ui| {
                    if primary(ui, Some(Icon::Download), "Download").clicked() {
                        asked.push(UpdateAction::Open(v.clone()));
                    }
                    skip(ui, asked);
                });
            }
        }
        return;
    }
    let (label, hint) = if u.checking {
        ("Checking…".to_owned(), String::new())
    } else if let Some(e) = &u.error {
        ("Couldn't check for updates".to_owned(), e.clone())
    } else if let Some(at) = u.checked {
        (format!("{app} is up to date"), format!("Checked {}.", ago(at)))
    } else {
        ("Not checked yet".to_owned(), String::new())
    };
    row_with(ui, Head { label: &label, hint: &hint, ..Default::default() }, |ui| {
        if ui.add_enabled_ui(!u.checking, |ui| secondary(ui, "Check now")).inner.clicked() {
            asked.push(UpdateAction::CheckNow);
        }
    });
}

/// Why `app` can't install an update itself, so the user downloads it.
fn why_not(app: &str, reason: Fallback) -> String {
    match reason {
        Fallback::UnsupportedPlatform => format!("{app} can't install updates itself on this system."),
        Fallback::UnknownLayout => format!("{app} isn't installed in a way it knows how to update."),
        Fallback::Translocated => format!("Move {app} to Applications to install updates automatically."),
        Fallback::DiskImage => format!("{app} is running from the disk image. Copy it to Applications to install updates automatically."),
        Fallback::ReadOnly => format!("{app} can't write to the folder it's in, so it can't update itself."),
    }
}

/// An error's message as a sentence: capitalised, with a full stop.
fn sentence(message: &str) -> String {
    let mut chars = message.trim().chars();
    let Some(first) = chars.next() else { return String::new() };
    let mut out: String = first.to_uppercase().chain(chars).collect();
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// Bytes as the nearest whole megabyte (decimal, as the Finder counts
/// them), and at least one once anything has arrived.
fn megabytes(bytes: u64) -> u64 {
    if bytes == 0 { 0 } else { (bytes.saturating_add(500_000) / 1_000_000).max(1) }
}

/// What's new in `v`, from its release notes (asked for the first time
/// they're shown), and a link to its release page.
fn release_notes(ui: &mut Ui, about: &AboutUpdates<'_>, v: Version, asked: &mut Vec<UpdateAction>) {
    let t = Tokens::of(ui);
    let notes = about.updates.notes.as_ref().filter(|(n, _)| *n == v);
    if notes.is_none() {
        asked.push(UpdateAction::Notes(v.clone()));
    }
    group(ui, &format!("What's new in {v}"), |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        match notes.and_then(|(_, n)| n.as_ref()) {
            None => paragraph(ui, "Loading the release notes…"),
            Some(Err(_)) => paragraph(ui, &format!("{} couldn't load the release notes. They're on the release page.", about.app)),
            Some(Ok(blocks)) => {
                for (i, b) in blocks.iter().enumerate() {
                    match b {
                        Block::Heading(h) => {
                            if i > 0 {
                                ui.add_space(8.0);
                            }
                            ui.label(egui::RichText::new(h).font(FontId::new(13.0, semibold())).color(t.text));
                        }
                        Block::Paragraph(p) => {
                            ui.add(egui::Label::new(egui::RichText::new(p).size(13.0).color(t.text)).wrap());
                        }
                        Block::Bullet(b) => {
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                let (dot, _) = ui.allocate_exact_size(vec2(6.0, 18.0), Sense::hover());
                                ui.painter().circle_filled(pos2(dot.center().x, dot.top() + 9.0), 2.0, t.muted);
                                ui.add(egui::Label::new(egui::RichText::new(b).size(13.0).color(t.text)).wrap());
                            });
                        }
                    }
                }
            }
        }
        ui.add_space(4.0);
        if link(ui, "Release page", FontId::proportional(13.0), t.accent).clicked() {
            asked.push(UpdateAction::Open(v.clone()));
        }
    });
}

/// Muted, wrapped text.
fn paragraph(ui: &mut Ui, text: &str) {
    let t = Tokens::of(ui);
    ui.add(egui::Label::new(egui::RichText::new(crate::keys::shown(text)).size(12.0).color(t.muted)).wrap());
}

/// "just now", "5 minutes ago", "3 hours ago", "2 days ago".
fn ago(at: std::time::SystemTime) -> String {
    let s = at.elapsed().map_or(0, |d| d.as_secs());
    let (n, unit) = match s {
        0..60 => return "just now".into(),
        60..3600 => (s / 60, "minute"),
        3600..86400 => (s / 3600, "hour"),
        _ => (s / 86400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// What the sidebar's notice (and the rail's gear) says of `v`: that it's
/// out, downloading, or ready to install.
pub fn notice_label(app: &str, v: &Version, install: &State) -> String {
    match install {
        State::Downloading { version, .. } if version == v => format!("Downloading {app} {v}…"),
        State::Ready { version, .. } if version == v => format!("{app} {v} is ready"),
        _ => format!("{app} {v} is available"),
    }
}

/// A newer version is out, in a tinted strip the width of the sidebar above
/// its foot ("Ond 0.6.0 is available", or downloading, or ready). The app
/// opens Settings › About on a click.
pub fn update_notice(ui: &mut Ui, app: &str, v: &Version, install: &State) -> Response {
    let label = &notice_label(app, v, install);
    let icon = if matches!(install, State::Ready { version, .. } if version == v) { Icon::Check } else { Icon::Download };
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), UPDATE_NOTICE_H), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, 6.0, t.accent.gamma_multiply(if resp.hovered() { 0.2 } else { 0.12 }));
        icons::paint(p, Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(12.0, 12.0)), icon, t.accent);
        p.text(pos2(rect.left() + 30.0, rect.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(12.5), t.accent);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The dot a rail's icon button wears while a newer version is out (or
/// downloading, or ready), at its
/// top right, ringed in the sidebar's colour.
pub fn notice_dot(ui: &Ui, button: Rect) {
    let t = Tokens::of(ui);
    let dot = pos2(button.center().x + 6.0, button.center().y - 6.0);
    ui.painter().circle_filled(dot, 4.0, t.sidebar);
    ui.painter().circle_filled(dot, 3.0, t.accent);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::themed;
    use crate::updates::install::Error;
    use egui_kittest::kittest::Queryable;

    struct Page {
        check: UpdateCheck,
        channel: Option<UpdateChannel>,
        check_locked: Option<&'static str>,
        channel_locked: Option<&'static str>,
        updates: Updates,
        asked: Vec<UpdateAction>,
    }

    impl Page {
        fn picked(&self) -> Vec<Option<UpdateChannel>> {
            self.asked.iter().filter_map(|a| if let UpdateAction::Channel(c) = a { Some(*c) } else { None }).collect()
        }

        fn checks(&self) -> Vec<UpdateCheck> {
            self.asked.iter().filter_map(|a| if let UpdateAction::Check(c) = a { Some(*c) } else { None }).collect()
        }
    }

    /// Checking, with the channel locked by `locked`.
    fn harness(channel: Option<UpdateChannel>, locked: Option<&'static str>, updates: Updates) -> egui_kittest::Harness<'static, (bool, Page)> {
        show(Page { check: UpdateCheck::Notify, channel, check_locked: None, channel_locked: locked, updates, asked: Vec::new() })
    }

    /// `check`, locked by `locked`, on the build's channel.
    fn page(check: UpdateCheck, locked: Option<&'static str>, updates: Updates) -> egui_kittest::Harness<'static, (bool, Page)> {
        show(Page { check, channel: None, check_locked: locked, channel_locked: None, updates, asked: Vec::new() })
    }

    fn show(page: Page) -> egui_kittest::Harness<'static, (bool, Page)> {
        let current = Version::parse("0.6.0").unwrap();
        let mut h = themed(page, move |ui, page: &mut Page| {
            ui.set_width(560.0);
            let about = AboutUpdates {
                app: "Ond",
                current: &current,
                distribution: Channel::GitHub,
                updates: &page.updates,
                check: page.check,
                channel: page.channel,
                skipped: None,
                hint: "Once a day.",
                check_locked: page.check_locked,
                channel_locked: page.channel_locked,
            };
            let asked = about_updates(ui, &about);
            page.asked.extend(asked.into_iter().filter(|a| !matches!(a, UpdateAction::Notes(_))));
        });
        h.set_size(vec2(600.0, 900.0));
        h.run();
        h
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    /// 0.7.0 is out, and `install` is how its download is going.
    fn newer(install: State) -> Updates {
        Updates { latest: Some(v("0.7.0")), install, ..Default::default() }
    }

    fn switch_named(h: &egui_kittest::Harness<'static, (bool, Page)>, name: &str) -> bool {
        h.query_by_role_and_label(egui::accesskit::Role::Switch, name).is_some()
    }

    #[test]
    fn the_channel_follows_the_build_until_picked_and_hands_back_the_pick() {
        let mut h = harness(None, None, Updates::default());
        h.get_by_label_contains("Follows this build, which is a full release.");
        h.check_accessibility();
        h.get_by_label("Betas").click();
        h.run();
        assert_eq!(h.state().1.picked(), vec![Some(UpdateChannel::Betas)]);
    }

    #[test]
    fn betas_says_it_means_the_newest_version_of_either_kind() {
        let h = harness(Some(UpdateChannel::Betas), None, Updates::default());
        h.get_by_label("The newest version, beta or full release. A beta may have rough edges.");
    }

    #[test]
    fn a_locked_channel_says_why_and_takes_no_pick() {
        let mut h = harness(Some(UpdateChannel::Releases), Some("Set in your Nix config"), Updates::default());
        h.get_by_label_contains("Set in your Nix config");
        h.get_by_label("Betas").click();
        h.run();
        assert!(h.state().1.picked().is_empty());
    }

    #[test]
    fn a_newer_beta_is_offered_as_one_with_its_notes_asked_for() {
        let beta = v("0.7.0-beta.1");
        let updates = Updates { latest: Some(beta), ..Default::default() };
        let h = harness(Some(UpdateChannel::Betas), None, updates);
        h.get_by_label("Ond 0.7.0-beta.1 is available");
        h.get_by_label_contains("A beta. You have 0.6.0.");
        h.get_by_label("Loading the release notes…");
        h.check_accessibility();
    }

    #[test]
    fn installing_is_offered_only_while_checking_and_hands_back_the_choice() {
        let mut h = page(UpdateCheck::Off, None, Updates::default());
        assert!(!switch_named(&h, "Install updates automatically"), "no install switch while checking is off");
        h.state_mut().1.check = UpdateCheck::Notify;
        h.run();
        h.get_by_label_contains("downloads new versions in the background and installs them when you quit or restart");
        h.check_accessibility();
        h.get_by_role_and_label(egui::accesskit::Role::Switch, "Install updates automatically").click();
        h.run();
        assert_eq!(h.state().1.checks(), vec![UpdateCheck::Install]);
    }

    #[test]
    fn checking_turned_off_and_on_again_installs_as_it_did_before() {
        let mut h = page(UpdateCheck::Install, None, Updates::default());
        h.get_by_role_and_label(egui::accesskit::Role::Switch, "Check for updates").click();
        h.run();
        h.state_mut().1.check = UpdateCheck::Off;
        h.run();
        h.get_by_role_and_label(egui::accesskit::Role::Switch, "Check for updates").click();
        h.run();
        assert_eq!(h.state().1.checks(), vec![UpdateCheck::Off, UpdateCheck::Install]);
    }

    #[test]
    fn a_locked_check_locks_installing_too() {
        let mut h = page(UpdateCheck::Install, Some("Set in your Nix config"), Updates::default());
        h.get_by_role_and_label(egui::accesskit::Role::Switch, "Install updates automatically").click();
        h.run();
        assert!(h.state().1.checks().is_empty());
    }

    #[test]
    fn a_download_shows_how_far_along_it_is_when_the_size_is_known() {
        let h = page(UpdateCheck::Install, None, newer(State::Downloading { version: v("0.7.0"), received: 12_400_000, total: Some(24_000_000) }));
        h.get_by_label("Downloading Ond 0.7.0…");
        h.get_by_label("12 of 24 MB. You have 0.6.0.");
        h.check_accessibility();
        let h = page(UpdateCheck::Install, None, newer(State::Downloading { version: v("0.7.0"), received: 12_400_000, total: None }));
        h.get_by_label("You have 0.6.0.");
    }

    #[test]
    fn a_ready_update_restarts_to_install_and_says_quitting_installs_it_too() {
        let mut h = page(UpdateCheck::Install, None, newer(State::preview_ready(v("0.7.0"))));
        h.get_by_label("Ond 0.7.0 is ready");
        h.get_by_label("It also installs when you quit Ond.");
        h.check_accessibility();
        h.get_by_label("Restart to Update").click();
        h.run();
        assert!(h.state().1.asked.iter().any(|a| matches!(a, UpdateAction::Restart)));
    }

    #[test]
    fn a_failed_download_says_why_and_can_be_retried() {
        let mut h = page(UpdateCheck::Install, None, newer(State::Failed { version: v("0.7.0"), error: Error::Signature }));
        h.get_by_label("Ond 0.7.0 didn't download");
        h.get_by_label("The update signature did not verify.");
        h.check_accessibility();
        h.get_by_label("Retry").click();
        h.run();
        assert!(h.state().1.asked.iter().any(|a| matches!(a, UpdateAction::Retry)));
    }

    #[test]
    fn an_update_that_cant_install_itself_offers_the_download_and_says_why() {
        let h = page(UpdateCheck::Install, None, newer(State::NotifyOnly { version: v("0.7.0"), reason: Fallback::Translocated }));
        h.get_by_label("Ond 0.7.0 is available");
        h.get_by_label("You have 0.6.0.\nMove Ond to Applications to install updates automatically.");
        h.get_by_label("Download");
        h.check_accessibility();
    }

    #[test]
    fn an_update_that_didnt_install_last_time_says_why() {
        let updates = Updates { install_error: Some(Error::Worker("the old copy is at /tmp/x".into())), ..Default::default() };
        let h = page(UpdateCheck::Install, None, updates);
        h.get_by_label("The last update didn't install");
        h.get_by_label("The installer could not finish: the old copy is at /tmp/x.");
    }

    #[test]
    fn the_notice_says_whether_the_update_is_out_downloading_or_ready() {
        let ready = State::preview_ready(v("0.7.0"));
        assert_eq!(notice_label("Ond", &v("0.7.0"), &State::Idle), "Ond 0.7.0 is available");
        assert_eq!(notice_label("Ond", &v("0.7.0"), &State::Downloading { version: v("0.7.0"), received: 0, total: None }), "Downloading Ond 0.7.0…");
        assert_eq!(notice_label("Rede", &v("0.7.0"), &ready), "Rede 0.7.0 is ready");
        assert_eq!(notice_label("Ond", &v("0.8.0"), &ready), "Ond 0.8.0 is available", "another version's download says nothing of this one");
    }

    #[test]
    fn sizes_round_to_whole_megabytes_and_show_a_start() {
        assert_eq!(megabytes(0), 0);
        assert_eq!(megabytes(1), 1);
        assert_eq!(megabytes(12_400_000), 12);
        assert_eq!(megabytes(12_600_000), 13);
    }
}
