//! The Settings window (⌘,): a list of sections on the left, one calm pane
//! on the right. Everything that used to hide in popovers (appearance,
//! ReplayGain, streaming, per-device modes, downloads, sync, analysis, the
//! account) lives here; the popovers keep only what's needed in the moment.

use super::seek::{WaveGrid, paint_wave};
use super::{Action, OndApp};
use super::{AnalysisAction, OfflineAction, OutputAction, PrefsAction, WindowAction};
use crate::icons::{self, Icon};
use crate::playback::{BITRATES, PlaybackPrefs, QualityMode, RgMode, TranscodeFormat};
use crate::queue::{AddAt, ClearWhat, NextAt, OnPlay, Preset};
use crate::theme::{Accent, ThemePref, Tokens, semibold};
use crate::visuals::visualizer::Pick;
use crate::visuals::waveform::{self, Bars};
use crate::widgets::{Head, ROW_PAD, choice, divider, group, managed, menu_label, row_with, secondary, secondary_danger, setting, switch, thin_slider};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use shell::kit::{self, readout, step_slider};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Section {
    #[default]
    General,
    Appearance,
    Playback,
    Queue,
    Output,
    Offline,
    Library,
    Account,
    Experimental,
    About,
}

impl Section {
    pub const ALL: [Section; 10] = [
        Section::General,
        Section::Appearance,
        Section::Playback,
        Section::Queue,
        Section::Output,
        Section::Offline,
        Section::Library,
        Section::Account,
        Section::Experimental,
        Section::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::Playback => "Playback",
            Section::Queue => "Queue",
            Section::Output => "Output",
            Section::Offline => "Offline",
            Section::Library => "Library",
            Section::Account => "Account",
            Section::Experimental => "Experimental",
            Section::About => "About",
        }
    }
}

const NAV_W: f32 = kit::SETTINGS_NAV_W;

impl OndApp {
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let w = (screen.width() - 64.0).clamp(480.0, 760.0);
        // Below the toolbar, so a narrow window's display keeps its times.
        let h = (screen.height() - 96.0).clamp(340.0, 540.0).min((screen.height() - super::toolbar::TOOLBAR_H - 16.0).max(200.0));
        let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin::ZERO).corner_radius(12);
        let modal = egui::Modal::new(egui::Id::unique("settings-window"))
            .frame(frame)
            .area(
                egui::Modal::default_area(egui::Id::unique("settings-window")).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, super::toolbar::TOOLBAR_H / 2.0)),
            )
            .show(ctx, |ui| {
                let t = Tokens::of(ui);
                let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
                let esc = ui.painter().layout_no_wrap("esc".into(), FontId::proportional(11.0), t.muted);
                ui.painter().galley(pos2(rect.right() - 14.0 - esc.size().x, rect.top() + 16.0), esc, t.muted);
                let side = Rect::from_min_size(rect.min, vec2(NAV_W, h));
                ui.painter().rect_filled(side, egui::CornerRadius { nw: 12, sw: 12, ..Default::default() }, t.sidebar);
                ui.painter().vline(side.right(), side.y_range(), egui::Stroke::new(1.0, t.hairline));

                // Section list.
                let labels = Section::ALL.map(Section::label);
                let at = Section::ALL.iter().position(|s| *s == self.ui.settings_section).unwrap_or(0);
                if let Some(i) = kit::settings_nav(ui, side, &labels, at) {
                    self.ui.settings_section = Section::ALL[i];
                }

                // The pane.
                let pane = Rect::from_min_max(pos2(side.right() + 1.0, rect.top()), rect.max);
                let section = self.ui.settings_section;
                kit::settings_pane(ui, pane, ("settings-pane", section), section.label(), |ui| match section {
                    Section::General => self.general_settings(ui),
                    Section::Appearance => self.appearance_settings(ui),
                    Section::Playback => self.playback_settings(ui),
                    Section::Queue => self.queue_settings(ui),
                    Section::Output => self.output_settings(ui),
                    Section::Offline => self.offline_settings(ui),
                    Section::Library => self.library_settings(ui),
                    Section::Account => self.account_settings(ui),
                    Section::Experimental => self.experimental_settings(ui),
                    Section::About => self.about(ui),
                });
            });
        if modal.should_close() {
            self.ui.overlay = None;
        }
    }

    fn general_settings(&mut self, ui: &mut Ui) {
        group(ui, "Listening", |ui| {
            let hint = "Ond reports plays to your server, which can pass them on to Last.fm or ListenBrainz.";
            self.pref_switch(ui, "scrobble", "Scrobble plays", hint, self.ui.prefs.scrobble, PrefsAction::Scrobble);
        });
        group(ui, "Advanced", |ui| {
            let hint = "The signal path, buffer, downloads, sync and server timing, in a thin bar along the bottom (⌥⌘I).";
            self.pref_switch(ui, "diagnostics_bar", "Show diagnostics bar", hint, self.ui.prefs.diagnostics_bar, PrefsAction::Diagnostics);
            if crate::csd::SUPPORTED {
                let hint = "Your desktop's own title bar and window frame, in place of Ond's buttons in the toolbar.";
                let on = self.ui.prefs.system_title_bar;
                self.pref_switch(ui, "window.system_title_bar", "Use the system title bar", hint, on, WindowAction::SetSystemTitleBar);
            }
        });
    }

    /// The app and its version, whether Ond looks for newer ones (copies
    /// from the GitHub releases; a package manager updates its own), and a
    /// newer release's notes.
    fn about(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let current = crate::updates::Version::current();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            ui.add(egui::Image::new((app_icon(ui.ctx()).id(), vec2(56.0, 56.0))));
            ui.vertical(|ui| {
                ui.add_space(8.0);
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.label(egui::RichText::new("Ond").font(FontId::new(17.0, semibold())).color(t.text));
                ui.label(egui::RichText::new(format!("Version {current}")).size(12.5).color(t.muted));
            });
        });
        ui.add_space(20.0);
        let newer = self.updates.latest.filter(|v| *v > current);
        group(ui, "Updates", |ui| match crate::updates::channel() {
            crate::updates::Channel::GitHub => {
                let on = self.prefs.get(|s| s.updates) != crate::settings::UpdateCheck::Off;
                let hint = "Once a day, Ond looks on GitHub for a newer version and tells you when there is one.";
                self.pref_switch(ui, "updates", "Check for updates", hint, on, |on| {
                    let check = if on { crate::settings::UpdateCheck::Notify } else { crate::settings::UpdateCheck::Off };
                    Action::Update(super::UpdateAction::Check(check))
                });
                self.update_status(ui, current, newer);
            }
            crate::updates::Channel::Packaged(by) => setting(ui, &format!("Updates come from {by}"), "", |_| ()),
            crate::updates::Channel::Source => setting(ui, "Built from source", "This copy doesn't look for updates.", |_| ()),
        });
        if let Some(v) = newer {
            self.release_notes(ui, v);
        }
        group(ui, "Links", |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                for (text, url) in [("Source code", crate::updates::REPO.to_owned()), ("Report a problem", format!("{}/issues", crate::updates::REPO))] {
                    if crate::widgets::link(ui, text, FontId::proportional(13.0), t.accent).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    }
                }
            });
        });
    }

    /// The row under Check for updates: a newer version with Download, or
    /// how the last look went, with Check Now.
    fn update_status(&mut self, ui: &mut Ui, current: crate::updates::Version, newer: Option<crate::updates::Version>) {
        let u = &self.updates;
        if let Some(v) = newer {
            let skipped = self.prefs.get(|s| s.skipped_update.as_deref() == Some(&v.to_string()));
            let hint = if skipped { format!("You have {current}. You skipped this version.") } else { format!("You have {current}.") };
            setting(ui, &format!("Ond {v} is available"), &hint, |ui| {
                if crate::widgets::primary(ui, Some(Icon::Download), "Download").clicked() {
                    self.actions.push(Action::Update(super::UpdateAction::Open(v)));
                }
                if !skipped && secondary(ui, "Skip").clicked() {
                    self.actions.push(Action::Update(super::UpdateAction::Skip(v)));
                }
            });
            return;
        }
        let (label, hint) = if u.checking {
            ("Checking…".to_owned(), String::new())
        } else if let Some(e) = &u.error {
            ("Couldn't check for updates".to_owned(), e.clone())
        } else if let Some(at) = u.checked {
            ("Ond is up to date".to_owned(), format!("Checked {}.", ago(at)))
        } else {
            ("Not checked yet".to_owned(), String::new())
        };
        let checking = u.checking;
        setting(ui, &label, &hint, |ui| {
            if ui.add_enabled_ui(!checking, |ui| secondary(ui, "Check now")).inner.clicked() {
                self.actions.push(Action::Update(super::UpdateAction::CheckNow));
            }
        });
    }

    /// What's new in `v`, from its release notes (fetched the first time
    /// they're shown).
    fn release_notes(&mut self, ui: &mut Ui, v: crate::updates::Version) {
        let t = Tokens::of(ui);
        if self.updates.notes.as_ref().is_none_or(|(n, _)| *n != v) {
            self.actions.push(Action::Update(super::UpdateAction::Notes(v)));
        }
        group(ui, &format!("What's new in {v}"), |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            match self.updates.notes.as_ref().and_then(|(_, n)| n.as_ref()) {
                None => paragraph(ui, "Loading the release notes…"),
                Some(Err(_)) => paragraph(ui, "Ond couldn't load the release notes. They're on the release page."),
                Some(Ok(blocks)) => {
                    for (i, b) in blocks.iter().enumerate() {
                        match b {
                            crate::updates::Block::Heading(h) => {
                                if i > 0 {
                                    ui.add_space(8.0);
                                }
                                ui.label(egui::RichText::new(h).font(FontId::new(13.0, semibold())).color(t.text));
                            }
                            crate::updates::Block::Paragraph(p) => {
                                ui.add(egui::Label::new(egui::RichText::new(p).size(13.0).color(t.text)).wrap());
                            }
                            crate::updates::Block::Bullet(b) => {
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
            if crate::widgets::link(ui, "Release page", FontId::proportional(13.0), t.accent).clicked() {
                self.actions.push(Action::Update(super::UpdateAction::Open(v)));
            }
        });
    }

    /// How Ond looks: its colors, the artwork behind the Player and Mini, and
    /// the seek bar.
    fn appearance_settings(&mut self, ui: &mut Ui) {
        group(ui, "Colors", |ui| {
            let mut theme = self.ui.prefs.theme;
            if choice_row(ui, self.prefs.managed("theme"), "Theme", "", &ThemePref::ALL, &mut theme, |p| p.label().into()) {
                self.actions.push(Action::Prefs(PrefsAction::Theme(theme)));
            }
            let mut accent = self.ui.prefs.accent;
            if managed(ui, self.prefs.managed("accent"), |ui| setting(ui, "Accent color", "", |ui| accent_swatches(ui, &mut accent))) {
                self.actions.push(Action::Prefs(PrefsAction::Accent(accent)));
            }
            let hint =
                "The accent, the Player and the now-playing display follow the playing album's cover, and an album's or artist's page follows its own art.";
            self.pref_switch(ui, "adapt_colors", "Adapt colors to the album art", hint, self.ui.prefs.adapt_colors, PrefsAction::AdaptColors);
        });
        group(ui, "Artwork", |ui| {
            let hint = "A soft, dimmed copy of the cover behind the Player and Mini.";
            self.pref_switch(ui, "backdrop", "Blurred artwork", hint, self.ui.prefs.backdrop, PrefsAction::Backdrop);
        });
        group(ui, "Seek bar", |ui| {
            let hint = "Draws the track's shape in the seek bar. Ond reads each file once in the background.";
            if self.pref_switch(ui, "waveform", "Waveform", hint, self.ui.prefs.waveform, PrefsAction::Waveform) {
                let mut bars = self.ui.prefs.waveform_bars;
                let locks = [self.prefs.managed("waveform_bars.rounded"), self.prefs.managed("waveform_bars.width"), self.prefs.managed("waveform_bars.gap")];
                waveform_bar_settings(ui, &mut bars, locks);
                if bars != self.ui.prefs.waveform_bars {
                    self.actions.push(Action::Prefs(PrefsAction::WaveformBars(bars)));
                }
            }
        });
        group(ui, "Lists", |ui| {
            let hint = "Letters beside Albums, Artists and Songs when they're sorted by name. Click or drag along them to jump.";
            self.pref_switch(ui, "alpha_rail", "Alphabet rail", hint, self.ui.prefs.alpha_rail, PrefsAction::AlphaRail);
            let hint = "Each artist's photo from the server beside their name in Artists, or their initial when the server has none.";
            self.pref_switch(ui, "artist_photos", "Artist photos", hint, self.ui.prefs.artist_photos, PrefsAction::ArtistPhotos);
        });
    }

    fn playback_settings(&mut self, ui: &mut Ui) {
        let mut p: PlaybackPrefs = self.output.prefs().clone();
        let before = p.clone();
        group(ui, "Volume", |ui| {
            let hint = if self.bit_perfect { format!("{}. Bypassed while bit-perfect is on.", p.replay_gain.hint()) } else { p.replay_gain.hint().to_owned() };
            choice_row(ui, self.prefs.managed("playback.replay_gain"), "ReplayGain", &hint, &RgMode::ALL, &mut p.replay_gain, |m| m.label().into());
            if p.replay_gain != RgMode::Off {
                managed(ui, self.prefs.managed("playback.preamp_db"), |ui| db_row(ui, "Pre-amp", "", &mut p.preamp_db, -12.0..=12.0));
                managed(ui, self.prefs.managed("playback.untagged_db"), |ui| {
                    db_row(ui, "Untagged tracks", "For tracks without ReplayGain tags. Ond keeps peaks below −1 dBFS.", &mut p.untagged_db, -12.0..=0.0)
                });
            }
        });
        group(ui, "Streaming", |ui| {
            let hint = p.quality.hint();
            choice_row(ui, self.prefs.managed("playback.quality"), "Quality", hint, &QualityMode::ALL, &mut p.quality, |q| q.label().into());
            if p.quality != QualityMode::Original {
                let hint = if p.format == TranscodeFormat::Opus {
                    "Opus sounds best for its size. Your server needs an Opus transcoding; Navidrome has one unless it was removed."
                } else {
                    ""
                };
                choice_row(ui, self.prefs.managed("playback.format"), "Format", hint, &TranscodeFormat::ALL, &mut p.format, |f| f.label().into());
                let network = crate::fmt::capitalize(self.output.network().label());
                let now = match p.transcode(self.output.network()) {
                    Some(t) => format!("{network} · new tracks stream as {}", t.label()),
                    None => format!("{network} · new tracks stream as original files"),
                };
                choice_row(ui, self.prefs.managed("playback.kbps"), "Bitrate", &now, &BITRATES, &mut p.kbps, |k| k.to_string());
            }
        });
        if p != before {
            self.actions.push(Action::Output(OutputAction::Playback(p)));
        }
    }

    fn queue_settings(&mut self, ui: &mut Ui) {
        let mut q = self.prefs.get(|s| s.queue);
        let before = q;
        group(ui, "How the queue works", |ui| {
            let mut preset = Preset::of(q);
            let picks = [Preset::Ond, Preset::AppleMusic, Preset::Spotify];
            let shown: &[Preset] = if preset == Preset::Custom { &Preset::ALL } else { &picks };
            let hint = match preset {
                Preset::Custom => "Your own mix of the settings below",
                _ => "Works like the player you're used to; change anything below to make it your own",
            };
            if setting(ui, "Style", hint, |ui| choice(ui, shown, &mut preset, None, |p| p.label().into())) {
                q = preset.prefs().unwrap_or(q);
            }
            let hint = q.play_next.hint();
            choice_row(ui, self.prefs.managed("queue.play_next"), "Play next", hint, &NextAt::ALL, &mut q.play_next, |n| n.label().into());
            let hint = q.add.hint();
            choice_row(ui, self.prefs.managed("queue.add"), "Add to queue", hint, &AddAt::ALL, &mut q.add, |a| a.label().into());
            let hint = q.on_play.hint();
            choice_row(ui, self.prefs.managed("queue.on_play"), "Playing something new", hint, &OnPlay::ALL, &mut q.on_play, |o| o.label().into());
            let hint = q.clear.hint();
            choice_row(ui, self.prefs.managed("queue.clear"), "Clear removes", hint, &ClearWhat::ALL, &mut q.clear, |c| c.label().into());
            note(ui, "Tracks you add show under “You added” in the queue. When they end up in more than one group, right-click one to bring them together.");
        });
        if q != before {
            self.actions.push(Action::Prefs(PrefsAction::Queue(q)));
        }
    }

    fn output_settings(&mut self, ui: &mut Ui) {
        let (devices, current) = self.output.devices();
        let (devices, current) = (devices.to_vec(), current.map(str::to_owned));
        let (hidden, named) = self.prefs.get(|s| (s.device_hidden.clone(), s.device_groups.clone()));
        // The groups as the output menu has them, then the hidden devices.
        let mut sections = crate::output::group_devices(devices.iter().filter(|d| !hidden.contains(&d.uid)), &named);
        let hidden_devices: Vec<audio::DeviceInfo> = devices.iter().filter(|d| hidden.contains(&d.uid)).cloned().collect();
        // The two built in, then the user's.
        let mut groups: Vec<String> =
            named.values().map(|g| g.trim().to_owned()).filter(|g| !g.is_empty() && g != crate::output::DEVICES && g != crate::output::OTHER).collect();
        groups.sort();
        groups.dedup();
        groups.splice(0..0, [crate::output::DEVICES.to_owned(), crate::output::OTHER.to_owned()]);
        group(ui, "Devices", |ui| {
            if devices.is_empty() {
                setting(ui, "No output devices found", "", |_| ());
            }
            let t = Tokens::of(ui);
            for (k, (name, list)) in sections.drain(..).enumerate() {
                if let Some(name) = name {
                    ui.add_space(if k == 0 { 2.0 } else { 12.0 });
                    ui.label(egui::RichText::new(name).size(11.5).color(t.muted));
                    ui.add_space(4.0);
                }
                for d in &list {
                    self.device_card(ui, d, current.as_deref() == Some(d.uid.as_str()), false, &groups);
                }
            }
            if !hidden_devices.is_empty() {
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Hidden from the output menu").size(11.5).color(t.muted));
                ui.add_space(4.0);
                for d in &hidden_devices {
                    self.device_card(ui, d, current.as_deref() == Some(d.uid.as_str()), true, &groups);
                }
            }
            ui.add_space(6.0);
            note(
                ui,
                "Bit-perfect keeps a device for Ond alone while it plays and sends it the samples untouched. Speakers, Bluetooth, AirPlay and software devices can't play bit-perfect.",
            );
        });
        group(ui, "Volume and notices", |ui| {
            let (mut green, mut db, mut toast) = self.prefs.get(|s| (s.device_volume_green, s.device_volume_db, s.device_change_toast));
            let before = (green, db, toast);
            let hint = "A green dot in the volume slider's knob, in bit-perfect, when the slider sets the device's own level.";
            managed_switch(ui, self.prefs.managed("device_volume_green"), "Show when the device sets the volume", hint, &mut green);
            let hint = "The device's own level beside the slider, in bit-perfect.";
            choice_row(ui, self.prefs.managed("device_volume_db"), "Show the level in dB", hint, &crate::settings::DbReadout::ALL, &mut db, |d| {
                d.label().into()
            });
            let hint = "A notice names the device whenever playback moves to another.";
            managed_switch(ui, self.prefs.managed("device_change_toast"), "Notify when the output changes", hint, &mut toast);
            if (green, db, toast) != before {
                self.actions.push(Action::Output(OutputAction::Show { green, db, toast }));
            }
        });
        if cfg!(target_os = "macos") {
            group(ui, "Normal mode", |ui| {
                let hint = "Switches the device to the song's rate, as bit-perfect does, while other apps keep sharing it. Off, the device stays as macOS has it and Ond converts the rate.";
                self.pref_switch(ui, "output_match_rate", "Match each song's sample rate", hint, self.output.match_rate(), OutputAction::MatchRate);
                note(ui, "A few seconds after playback stops, Ond puts the device back the way it found it.");
            });
        }
    }

    /// A device in Settings › Output, as in the output menu: its header opens
    /// it (the chevron on the left), and open it sits on a card with its mode
    /// (when it can play bit-perfect), whether the menu shows it, and its group.
    fn device_card(&mut self, ui: &mut Ui, d: &audio::DeviceInfo, in_use: bool, hidden: bool, groups: &[String]) {
        let t = Tokens::of(ui);
        let open = self.ui.output_open.contains(&d.uid);
        let (rect, head) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
        let dim = |c: egui::Color32| if hidden { c.gamma_multiply(0.5) } else { c };
        if open {
            // The card's top, behind the header; its body follows the settings.
            ui.painter().rect_filled(rect, egui::CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, t.field);
        } else if head.hovered() {
            ui.painter().rect_filled(rect, 8.0, t.hover);
        }
        let chevron = Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(9.0, 9.0));
        icons::paint(ui.painter(), chevron, if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.muted);
        let glyph = Rect::from_center_size(pos2(rect.left() + 36.0, rect.center().y), vec2(15.0, 15.0));
        icons::paint(ui.painter(), glyph, crate::output::kind_icon(Some(d.kind)), dim(t.text));
        let x = rect.left() + 54.0;
        let name = ui.painter().layout_no_wrap(d.name.clone(), FontId::proportional(13.0), dim(t.text));
        let name_w = name.size().x;
        ui.painter().galley(pos2(x, rect.top() + 6.0), name, dim(t.text));
        if in_use {
            ui.painter().text(pos2(x + name_w + 8.0, rect.top() + 6.0), egui::Align2::LEFT_TOP, "in use", FontId::proportional(11.0), t.accent);
        }
        ui.painter().text(pos2(x, rect.top() + 23.0), egui::Align2::LEFT_TOP, d.capability_line(), FontId::proportional(11.0), dim(t.muted));
        // Shown or hidden in the output menu: the eye at the right, without opening the device.
        let eye_rect = Rect::from_center_size(pos2(rect.right() - 22.0, rect.center().y), vec2(28.0, 28.0));
        let eye = ui.interact(eye_rect, egui::Id::unique(("device-eye", &d.uid)), Sense::click());
        let eye_locked = self.prefs.managed("device_hidden");
        if eye.hovered() && !eye_locked {
            ui.painter().rect_filled(eye_rect, 6.0, t.hover);
        }
        let glyph = Rect::from_center_size(eye_rect.center(), vec2(14.0, 14.0));
        icons::paint(ui.painter(), glyph, if hidden { Icon::EyeClosed } else { Icon::Eye }, if hidden { t.muted.gamma_multiply(0.7) } else { t.muted });
        let eye_tip = if eye_locked {
            crate::managed::LOCKED.to_owned()
        } else if hidden {
            "Show in the output menu".into()
        } else {
            "Hide from the output menu".into()
        };
        let eye_label = eye_tip.clone();
        eye.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, !eye_locked, !hidden, &eye_label));
        if eye.on_hover_text(eye_tip).clicked() && !eye_locked {
            self.actions.push(Action::Output(OutputAction::DeviceHidden(d.uid.clone(), !hidden)));
        }
        let label = d.name.clone();
        head.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, &label));
        if head.clicked() {
            if open {
                self.ui.output_open.remove(&d.uid);
            } else {
                self.ui.output_open.insert(d.uid.clone());
            }
        }
        if !open {
            ui.add_space(2.0);
            return;
        }
        let top = ui.cursor().top();
        // The card behind the settings: its place in the paint order now, its size once they're laid out.
        let bg = ui.painter().add(egui::Shape::Noop);
        let inner = ui
            .scope_builder(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(rect.left() + 54.0, top), pos2(rect.right() - 12.0, top + 400.0))), |ui| {
                if d.can_bit_perfect() {
                    let mut mode = self.output.device_mode(d);
                    let changed = managed(ui, self.output.mode_locked(Some(&d.uid)), |ui| {
                        setting(ui, "Bit-perfect", "", |ui| crate::widgets::bit_perfect_switch(ui, &d.name, &mut mode)).changed()
                    });
                    if changed {
                        self.actions.push(Action::Output(OutputAction::DeviceMode(d.uid.clone(), mode)));
                    }
                }
                let named = self.prefs.get(|s| s.device_groups.clone());
                let place = crate::output::group_of(d, &named);
                let auto = crate::output::auto_group(d);
                let locked = self.prefs.managed(&format!("device_groups.{}", d.uid));
                managed(ui, locked, |ui| {
                    setting(ui, "Group", "", |ui| {
                        let resp = menu_label(ui, &place, t.text);
                        egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                            ui.set_min_width(180.0);
                            for g in groups {
                                if ui.selectable_label(&place == g, g).clicked() {
                                    // Its own group is no choice to remember: a blank puts it back.
                                    let keep = if g == auto { String::new() } else { g.clone() };
                                    self.actions.push(Action::Output(OutputAction::DeviceGroup(d.uid.clone(), keep)));
                                    ui.close();
                                }
                            }
                            ui.separator();
                            let draft = self.ui.group_drafts.entry(d.uid.clone()).or_default();
                            let edit = ui.add(egui::TextEdit::singleline(draft).hint_text("New group…").desired_width(170.0));
                            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !draft.trim().is_empty() {
                                self.actions.push(Action::Output(OutputAction::DeviceGroup(d.uid.clone(), draft.trim().to_owned())));
                                draft.clear();
                                ui.close();
                            }
                        });
                    })
                });
                ui.min_rect().bottom()
            })
            .inner;
        // The card under the settings, joining the header's.
        let body = Rect::from_min_max(pos2(rect.left(), top), pos2(rect.right(), inner + 8.0));
        ui.painter().set(bg, egui::Shape::rect_filled(body, egui::CornerRadius { nw: 0, ne: 0, sw: 8, se: 8 }, t.field));
        ui.advance_cursor_after_rect(body);
        ui.add_space(6.0);
    }

    fn offline_settings(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let snap = self.downloads.snap.clone();
        if snap.set_aside.is_some() {
            super::offline_page::set_aside_notice(ui, &snap, &mut self.actions, "settings");
            ui.add_space(28.0);
        }
        group(ui, "Without the server", |ui| {
            let hint = "Don't connect to the server. Show only music that plays without it: downloads, and songs streamed recently enough to still be cached.";
            self.pref_switch(ui, "work_offline", "Offline mode", hint, self.session.work_offline, OfflineAction::WorkOffline);
            let mut unplayable = self.prefs.get(|s| s.offline_unplayable);
            let hint = "What playback does with them while the server can't be reached.";
            let locked = self.prefs.managed("offline_unplayable");
            if choice_row(ui, locked, "Songs that aren't downloaded", hint, &crate::settings::Unplayable::ALL, &mut unplayable, |u| u.label().into()) {
                self.actions.push(Action::Offline(OfflineAction::Unplayable(unplayable)));
            }
        });
        group(ui, "Storage", |ui| {
            managed(ui, self.prefs.managed("offline_limit_gb"), |ui| {
                setting(ui, "Storage limit", &super::offline_page::storage_used(&snap), |ui| {
                    let label = match snap.limit {
                        Some(b) => crate::fmt::bytes(b),
                        None => "No limit".to_owned(),
                    };
                    let resp = menu_label(ui, &label, t.text);
                    egui::Popup::menu(&resp).show(|ui| super::offline_page::limit_choices(ui, snap.limit, &mut self.actions));
                })
            });
            let mut paused = snap.paused;
            if switch_row(ui, "Pause downloads", "", &mut paused) {
                self.actions.push(Action::Offline(OfflineAction::Pause(paused)));
            }
        });
        group(ui, "Location", |ui| {
            let dir = self.downloads.base.display().to_string();
            let folder_locked = self.prefs.managed("offline_dir");
            row_with(ui, Head { label: "Download folder", tag: folder_locked.then_some(crate::managed::LOCKED), hint: &dir, path: true }, |ui| {
                // Right to left: the last button first.
                if ui.add_enabled_ui(!folder_locked, |ui| secondary(ui, "Change…")).inner.clicked() {
                    self.actions.push(Action::Offline(OfflineAction::Move));
                }
                if secondary(ui, &format!("Show in {}", crate::folder::FILE_MANAGER)).clicked() {
                    self.actions.push(Action::Offline(OfflineAction::Reveal));
                }
            });
            if let Some(to) = self.prefs.get(|s| s.offline_move_from.is_some().then(|| s.offline_dir.clone()).flatten()) {
                note(ui, &format!("Downloads move to {} the next time Ond starts.", to.display()));
            }
        });
    }

    fn library_settings(&mut self, ui: &mut Ui) {
        group(ui, "Sync", |ui| {
            let counts = format!("{} albums · {} tracks", self.data.albums.len(), self.data.tracks.all.len());
            let when = if self.sync.tracks {
                self.sync.label.clone()
            } else {
                match self.sync.last {
                    Some(at) => format!("Last synced at {}", at.format("%-I:%M %p")),
                    None if self.session.client.is_some() => "Synced at launch".to_owned(),
                    None => "Not connected".to_owned(),
                }
            };
            setting(ui, &counts, &when, |ui| {
                if self.sync.tracks {
                    if secondary(ui, "Stop").clicked() {
                        self.actions.push(Action::StopSync);
                    }
                } else {
                    let can = self.session.client.is_some() && self.session.online;
                    let why = if self.session.work_offline { "Offline mode" } else { "Not connected" };
                    if ui.add_enabled_ui(can, |ui| secondary(ui, "Sync now")).inner.on_disabled_hover_text(why).clicked() {
                        self.actions.push(Action::SyncNow);
                    }
                }
            });
        });
        group(ui, "Analysis", |ui| self.analysis_settings(ui));
        group(ui, "Cache", |ui| {
            let art = self.art.disk_size();
            let hint = match art {
                Some(0) => "Nothing kept on this computer.".to_owned(),
                Some(n) => format!("{} on this computer. Covers download again as they're shown.", crate::fmt::bytes(n)),
                None => "Covers download again as they're shown.".to_owned(),
            };
            setting(ui, "Cover art", &hint, |ui| {
                if ui.add_enabled_ui(art != Some(0), |ui| secondary(ui, "Clear")).inner.clicked() {
                    self.actions.push(Action::ClearArtCache);
                }
            });
            let audio = self.cache.size();
            let hint = match audio {
                0 => "Nothing kept on this computer.".to_owned(),
                n => format!("{} of tracks played lately. They stream again when played.", crate::fmt::bytes(n)),
            };
            setting(ui, "Streamed audio", &hint, |ui| {
                if ui.add_enabled_ui(audio > 0, |ui| secondary(ui, "Clear")).inner.clicked() {
                    self.actions.push(Action::ClearStreamCache);
                }
            });
        });
    }

    /// Library analysis: start it, follow it, pause or stop it; and whether
    /// tracks are analyzed as they play.
    fn analysis_settings(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let p = self.analysis.runner.progress();
        let failed = if p.failed > 0 { format!("{} couldn't be read", p.failed) } else { String::new() };
        if p.active() {
            let status = if p.paused {
                format!("Paused at {} of {}", p.done.min(p.total), p.total)
            } else if p.waiting {
                format!("{} of {} · waiting for playback", p.done.min(p.total), p.total)
            } else {
                format!("Analyzing {} of {} · {} left", p.done.min(p.total), p.total, crate::fmt::about(p.eta_secs))
            };
            setting(ui, &status, &failed, |ui| {
                // Right to left: the last button first.
                if secondary(ui, "Stop").clicked() {
                    self.actions.push(Action::Analysis(AnalysisAction::Stop));
                }
                let (label, paused) = if p.paused { ("Resume", false) } else { ("Pause", true) };
                if secondary(ui, label).clicked() {
                    self.actions.push(Action::Analysis(AnalysisAction::Pause(paused)));
                }
            });
            let frac = if p.total > 0 { p.done.min(p.total) as f32 / p.total as f32 } else { 0.0 };
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, t.hairline);
            ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(r.width() * frac, r.height())), 2.0, t.accent);
            ui.add_space(ROW_PAD + 4.0);
        } else if self.session.client.is_some() && !self.data.tracks.all.is_empty() {
            let (n, secs) = self.unanalysed();
            if n == 0 {
                setting(ui, "Every track is analyzed", &failed, |_| ());
            } else {
                let label = format!("{} not analyzed yet", crate::fmt::plural(n, "track"));
                let eta = crate::fmt::about(crate::analyze::assumed_eta(secs));
                let hint = if failed.is_empty() { eta } else { format!("{eta} · {failed}") };
                setting(ui, &label, &hint, |ui| {
                    if secondary(ui, "Analyze library").clicked() {
                        self.actions.push(Action::Analysis(AnalysisAction::Library));
                    }
                });
            }
        } else {
            setting(ui, "Library analysis", "Available once the library has synced.", |_| ());
        }
        let hint = "Each track is measured once it has downloaded for playback.";
        self.pref_switch(ui, "analyze_playing", "Analyze tracks as they play", hint, self.ui.prefs.analyze_playing, AnalysisAction::AnalyzePlaying);
        note(
            ui,
            "Measures loudness and dynamic range, and checks for upsampled or lossy-sourced files. Runs in the background and pauses while playback needs the network.",
        );
    }

    /// Visuals still being tried out, one switch each.
    fn experimental_settings(&mut self, ui: &mut Ui) {
        let mut x = self.ui.prefs.experimental;
        let before = x;
        paragraph(
            ui,
            "Visual features that are still being tried out. They may change or go away, and their settings may not carry over to the next version. None of them change the sound: bit-perfect playback stays bit-perfect.",
        );
        ui.add_space(24.0);
        group(ui, "Live audio", |ui| {
            let hint = "In the now-playing display and the Player. Uses more battery while music plays.";
            managed_switch(ui, self.prefs.managed("experimental.spectrum"), "Live spectrum", hint, &mut x.spectrum);
            let hint = "Frequency over time, scrolling in the Info tab. Uses more battery while music plays.";
            managed_switch(ui, self.prefs.managed("experimental.spectrogram"), "Spectrogram in Info", hint, &mut x.spectrogram);
            let hint = "A full-window light show: ⌘T opens it, Esc leaves. Keeps the graphics card busy while open.";
            managed_switch(ui, self.prefs.managed("experimental.visualizer"), "Visualizer", hint, &mut x.visualizer);
            if x.visualizer {
                let hint = "Shuffle picks a different one each track. Inside the visualizer, ← and → switch.";
                choice_row(ui, self.prefs.managed("experimental.visualizer_pick"), "Show", hint, &Pick::ALL, &mut x.visualizer_pick, |p| p.label().into());
            }
        });
        if x != before {
            self.actions.push(Action::Prefs(PrefsAction::Experimental(x)));
        }
    }

    fn account_settings(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let (server, user) = match &self.session.settings {
            Some(s) => (s.server.clone(), s.username.clone()),
            None => (String::new(), String::new()),
        };
        if server.is_empty() {
            group(ui, "Server", |ui| setting(ui, "Not connected", "", |_| ()));
            return;
        }
        group(ui, "Server", |ui| {
            setting(ui, "Address", "", |ui| value(ui, &server, t.muted));
            if !self.session.server_info.is_empty() {
                setting(ui, "Version", "", |ui| value(ui, &self.session.server_info, t.muted));
            }
            let (text, color) = if self.session.work_offline {
                ("Offline mode", t.muted)
            } else if self.session.online {
                ("Connected", t.ok)
            } else {
                ("Can't reach the server", t.warn)
            };
            setting(ui, "Status", "", |ui| value(ui, text, color));
        });
        group(ui, "Signed in", |ui| {
            setting(ui, &user, "Signing out forgets the saved password. Downloads stay on this computer.", |ui| {
                if secondary_danger(ui, "Sign out…").clicked() {
                    self.actions.push(Action::AskSignOut);
                }
            });
        });
    }
}

/// Sliders in rows (with their readout beside them).
const SLIDER_W: f32 = 160.0;

/// A [`setting`] that's a switch. True when it changed.
fn switch_row(ui: &mut Ui, label: &str, hint: &str, on: &mut bool) -> bool {
    setting(ui, label, hint, |ui| switch(ui, on, label)).changed()
}

/// A [`switch_row`] for a setting that managed.json may lock. True when it changed.
fn managed_switch(ui: &mut Ui, locked: bool, label: &str, hint: &str, on: &mut bool) -> bool {
    managed(ui, locked, |ui| switch_row(ui, label, hint, on))
}

/// A [`setting`] whose control is a segmented choice among `all`, for a
/// setting that managed.json may lock. True when it changed.
fn choice_row<T: Copy + PartialEq>(ui: &mut Ui, locked: bool, label: &str, hint: &str, all: &[T], value: &mut T, name: impl Fn(T) -> String) -> bool {
    managed(ui, locked, |ui| setting(ui, label, hint, |ui| choice(ui, all, value, None, name)))
}

impl OndApp {
    /// A switch for one setting, `on` as it stands and `key` its name in
    /// managed.json. Flipping it pushes `act` with the new value. Returns
    /// the switch's value as shown, changed this frame or not.
    fn pref_switch<A: Into<Action>>(&mut self, ui: &mut Ui, key: &str, label: &str, hint: &str, on: bool, act: impl FnOnce(bool) -> A) -> bool {
        let mut on = on;
        if managed_switch(ui, self.prefs.managed(key), label, hint, &mut on) {
            self.actions.push(act(on).into());
        }
        on
    }
}

/// A value read out on the right of a row.
fn value(ui: &mut Ui, text: &str, color: egui::Color32) {
    ui.label(egui::RichText::new(text).size(13.0).color(color));
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

/// Ond's icon, for the About page.
fn app_icon(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::unique("about-app-icon");
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return tex;
    }
    let icon =
        eframe::icon_data::from_png_bytes(include_bytes!("../../../../packaging/linux/hicolor/256x256/apps/dev.solidsilver.ond.png")).unwrap_or_default();
    let image = egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
    let tex = ctx.load_texture("about-app-icon", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// Muted text under a group's rows, about all of them.
fn note(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    paragraph(ui, text);
}

/// Muted, wrapped text.
fn paragraph(ui: &mut Ui, text: &str) {
    let t = Tokens::of(ui);
    ui.add(egui::Label::new(egui::RichText::new(crate::keys::shown(text)).size(12.0).color(t.muted)).wrap());
}

/// The accent presets as round swatches, each in the color this theme
/// draws it, then the user's own color in a picker. True when it changed.
fn accent_swatches(ui: &mut Ui, accent: &mut Accent) -> bool {
    // In order, left to right, though the row adds its control right to left.
    // Packed closer when the pane is narrow.
    let n = Accent::PRESETS.len() as f32 + 1.0;
    let (cell, gap) = if ui.available_width() >= n * 30.0 - 4.0 { (26.0, 4.0) } else { (24.0, 2.0) };
    let size = vec2(n * cell + (n - 1.0) * gap, cell);
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| swatches(ui, accent, cell, gap)).inner
}

fn swatches(ui: &mut Ui, accent: &mut Accent, cell: f32, gap: f32) -> bool {
    let dark = ui.visuals().dark_mode;
    let mut changed = false;
    ui.spacing_mut().item_spacing.x = gap;
    for preset in Accent::PRESETS {
        if swatch(ui, cell, Some(preset.colors(dark).0), *accent == preset, preset.label()).clicked() && *accent != preset {
            *accent = preset;
            changed = true;
        }
    }
    let own = matches!(accent, Accent::Custom(_));
    let resp = swatch(ui, cell, own.then(|| accent.colors(dark).0), own, "Custom color…");
    let picker = egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).layout(egui::Layout::top_down(egui::Align::Min));
    picker.show(|ui| {
        ui.spacing_mut().slider_width = 240.0;
        // Starts from the color as picked, or from the accent in use.
        let mut c = match *accent {
            Accent::Custom([r, g, b]) => egui::Color32::from_rgb(r, g, b),
            other => other.colors(dark).0,
        };
        if egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque) {
            *accent = Accent::Custom([c.r(), c.g(), c.b()]);
            changed = true;
        }
        ui.add_space(4.0);
        ui.set_max_width(240.0);
        note(ui, "Made lighter or darker where needed, so text and controls stay readable in both themes.");
    });
    changed
}

/// One round swatch, ringed when chosen: `color`, or a wheel of hues for a
/// color still to pick.
fn swatch(ui: &mut Ui, cell: f32, color: Option<egui::Color32>, chosen: bool, label: &str) -> egui::Response {
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(cell, cell), Sense::click());
    if ui.is_rect_visible(rect) {
        let c = rect.center();
        let p = ui.painter();
        match color {
            Some(color) => {
                p.circle_filled(c, 8.5, color);
            }
            None => {
                // Twelve hues around the centre.
                let mut mesh = egui::Mesh::default();
                mesh.colored_vertex(c, egui::Color32::from_gray(if ui.visuals().dark_mode { 200 } else { 245 }));
                for k in 0..=24 {
                    let a = std::f32::consts::TAU * k as f32 / 24.0;
                    let hue = egui::ecolor::Hsva::new(k as f32 / 24.0, 0.65, 0.85, 1.0);
                    mesh.colored_vertex(c + 8.5 * vec2(a.cos(), a.sin()), hue.into());
                    if k > 0 {
                        mesh.add_triangle(0, k, k + 1);
                    }
                }
                p.add(egui::Shape::mesh(mesh));
            }
        }
        if chosen {
            p.circle_stroke(c, 11.5, egui::Stroke::new(1.5, t.text));
        } else if resp.hovered() {
            p.circle_stroke(c, 11.5, egui::Stroke::new(1.0, t.muted));
        }
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, chosen, label));
    resp.on_hover_text(label)
}

/// A dB value on a thin slider with a readout, snapped to 0.5 dB.
fn db_row(ui: &mut Ui, label: &str, hint: &str, db: &mut f32, range: std::ops::RangeInclusive<f32>) {
    let (lo, hi) = (*range.start(), *range.end());
    setting(ui, label, hint, |ui| {
        readout(ui, &crate::fmt::gain(*db));
        let mut v = (*db - lo) / (hi - lo);
        if thin_slider(ui, &mut v, SLIDER_W, label).changed() {
            *db = ((lo + v * (hi - lo)) * 2.0).round() / 2.0;
        }
    });
}

/// The waveform's bars (their ends, thickness and spacing) under a preview
/// of them across the pane.
/// `locks`: whether the ends, thickness and spacing are set in managed.json.
fn waveform_bar_settings(ui: &mut Ui, bars: &mut Bars, locks: [bool; 3]) {
    divider(ui);
    ui.add_space(ROW_PAD + 3.0);
    waveform_preview(ui, *bars, ui.available_width());
    ui.add_space(ROW_PAD + 3.0);
    choice_row(ui, locks[0], "Bars", "", &[false, true], &mut bars.rounded, |r| if r { "Rounded" } else { "Square" }.into());
    managed(ui, locks[1], |ui| step_row(ui, "Thickness", &mut bars.width, Bars::WIDTHS, half_points));
    managed(ui, locks[2], |ui| step_row(ui, "Spacing", &mut bars.gap, Bars::GAPS, half_points));
}

/// A made-up waveform's bars as `look` draws them, at the Player's size and
/// the now-playing display's, part played, `width` wide.
fn waveform_preview(ui: &mut Ui, look: Bars, width: f32) {
    let t = Tokens::of(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 96.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    painter.rect(rect, 8.0, t.bg, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
    let inner = rect.shrink2(vec2(14.0, 0.0));
    for (y, wave_h, fill) in [(rect.top() + 36.0, 13.0, t.text), (rect.bottom() - 22.0, 4.0, t.muted)] {
        let line = Rect::from_center_size(pos2(inner.center().x, y), vec2(inner.width(), 2.0));
        let grid = WaveGrid::along(line, painter.pixels_per_point(), look);
        paint_wave(painter, line, &grid, &waveform::sample(grid.n), line.left() + line.width() * 0.4, wave_h, fill);
    }
}

/// A whole-number setting in `range` on the kit's [`step_slider`].
fn step_row(ui: &mut Ui, label: &str, value: &mut u8, range: std::ops::RangeInclusive<u8>, text: fn(u8) -> String) {
    setting(ui, label, "", |ui| step_slider(ui, label, value, range, SLIDER_W, text));
}

/// Half points as points: "1 pt", "1.5 pt".
fn half_points(v: u8) -> String {
    if v.is_multiple_of(2) { format!("{} pt", v / 2) } else { format!("{}.5 pt", v / 2) }
}
