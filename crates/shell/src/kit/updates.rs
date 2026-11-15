//! What an app says about its own updates, drawn alike in both: About's
//! [`about_updates`] (the Updates group with its channel, and a newer version's release notes
//! under it) and the sidebar's [`update_notice`] above its foot, with the
//! [`notice_dot`] a rail's gear wears instead. The app holds the state
//! ([`Updates`]) and the preferences; these only draw them and say what was
//! asked for as [`UpdateAction`]s.

use super::buttons::{primary, secondary};
use super::controls::{choice, switch};
use super::rows::{Head, group, managed, row_with};
use super::text::link;
use crate::icons::{self, Icon};
use crate::theme::{Tokens, semibold};
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
    /// app sets it (Ond's managed.json): the row is locked and says so.
    pub check_locked: Option<&'a str>,
    /// The same for Channel.
    pub channel_locked: Option<&'a str>,
}

/// About's Updates group: for a GitHub copy, Check for updates and the
/// Channel (Releases or Betas, the build's own until the user picks), then a
/// newer version with Download and Skip, or how the last look went with Check now;
/// for any other, where its updates come from. A newer version's release
/// notes follow in a group of their own, asked for the first time they're
/// drawn. Returns what was asked for, in order.
pub fn about_updates(ui: &mut Ui, about: &AboutUpdates<'_>) -> Vec<UpdateAction> {
    let mut asked = Vec::new();
    let newer = about.updates.latest.clone().filter(|v| v > about.current);
    group(ui, "Updates", |ui| match about.distribution {
        Channel::GitHub => {
            let mut on = about.check != UpdateCheck::Off;
            let head = Head { label: "Check for updates", hint: about.hint, tag: about.check_locked, ..Default::default() };
            if managed(ui, about.check_locked.is_some(), |ui| row_with(ui, head, |ui| switch(ui, &mut on, "Check for updates")).changed()) {
                asked.push(UpdateAction::Check(if on { UpdateCheck::Notify } else { UpdateCheck::Off }));
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

/// Releases or Betas. Until the user picks, it shows the one the build
/// follows and says so; a pick is returned.
fn channel_row(ui: &mut Ui, about: &AboutUpdates<'_>) -> Option<UpdateChannel> {
    let mut pick = UpdateChannel::effective(about.channel, about.current);
    let hint = match about.channel {
        None if about.current.is_prerelease() => "Follows this build, which is a beta.",
        None => "Follows this build, which is a full release.",
        Some(UpdateChannel::Releases) => "Full releases only.",
        Some(UpdateChannel::Betas) => "Betas as well as full releases. A beta may have rough edges.",
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

/// The row under Check for updates: a newer version with Download, or how
/// the last look went, with Check now.
fn update_status(ui: &mut Ui, about: &AboutUpdates<'_>, newer: Option<Version>, asked: &mut Vec<UpdateAction>) {
    let (app, current, u) = (about.app, about.current, about.updates);
    if let Some(v) = newer {
        let skipped = about.skipped == Some(&v.to_string());
        let beta = if v.is_prerelease() { "A beta. " } else { "" };
        let hint = if skipped { format!("{beta}You have {current}. You skipped this version.") } else { format!("{beta}You have {current}.") };
        row_with(ui, Head { label: &format!("{app} {v} is available"), hint: &hint, ..Default::default() }, |ui| {
            if primary(ui, Some(Icon::Download), "Download").clicked() {
                asked.push(UpdateAction::Open(v.clone()));
            }
            if !skipped && secondary(ui, "Skip").clicked() {
                asked.push(UpdateAction::Skip(v.clone()));
            }
        });
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

/// A newer version is out, in a tinted strip the width of the sidebar above
/// its foot ("Ond 0.6.0 is available"). The app opens Settings › About on a
/// click.
pub fn update_notice(ui: &mut Ui, label: &str) -> Response {
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), UPDATE_NOTICE_H), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, 6.0, t.accent.gamma_multiply(if resp.hovered() { 0.2 } else { 0.12 }));
        icons::paint(p, Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(12.0, 12.0)), Icon::Download, t.accent);
        p.text(pos2(rect.left() + 30.0, rect.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(12.5), t.accent);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The dot a rail's icon button wears while a newer version is out, at its
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
    use egui_kittest::kittest::Queryable;

    struct Page {
        channel: Option<UpdateChannel>,
        locked: Option<&'static str>,
        updates: Updates,
        picked: Vec<Option<UpdateChannel>>,
    }

    fn harness(channel: Option<UpdateChannel>, locked: Option<&'static str>, updates: Updates) -> egui_kittest::Harness<'static, (bool, Page)> {
        let current = Version::parse("0.6.0").unwrap();
        let mut h = themed(Page { channel, locked, updates, picked: Vec::new() }, move |ui, page: &mut Page| {
            ui.set_width(560.0);
            let about = AboutUpdates {
                app: "Ond",
                current: &current,
                distribution: Channel::GitHub,
                updates: &page.updates,
                check: UpdateCheck::Notify,
                channel: page.channel,
                skipped: None,
                hint: "Once a day.",
                check_locked: None,
                channel_locked: page.locked,
            };
            for asked in about_updates(ui, &about) {
                if let UpdateAction::Channel(c) = asked {
                    page.picked.push(c);
                }
            }
        });
        h.set_size(vec2(600.0, 700.0));
        h.run();
        h
    }

    #[test]
    fn the_channel_follows_the_build_until_picked_and_hands_back_the_pick() {
        let mut h = harness(None, None, Updates::default());
        h.get_by_label_contains("Follows this build, which is a full release.");
        h.check_accessibility();
        h.get_by_label("Betas").click();
        h.run();
        assert_eq!(h.state().1.picked, vec![Some(UpdateChannel::Betas)]);
    }

    #[test]
    fn a_locked_channel_says_why_and_takes_no_pick() {
        let mut h = harness(Some(UpdateChannel::Releases), Some("Set in your Nix config"), Updates::default());
        h.get_by_label_contains("Set in your Nix config");
        h.get_by_label("Betas").click();
        h.run();
        assert!(h.state().1.picked.is_empty());
    }

    #[test]
    fn a_newer_beta_is_offered_as_one_with_its_notes_asked_for() {
        let beta = Version::parse("0.7.0-beta.1").unwrap();
        let updates = Updates { latest: Some(beta), ..Default::default() };
        let h = harness(Some(UpdateChannel::Betas), None, updates);
        h.get_by_label("Ond 0.7.0-beta.1 is available");
        h.get_by_label_contains("A beta. You have 0.6.0.");
        h.get_by_label("Loading the release notes…");
        h.check_accessibility();
    }
}
