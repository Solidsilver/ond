//! The Settings window (⌘,): a list of sections on the left, one calm pane
//! on the right. Everything that used to hide in popovers (appearance,
//! ReplayGain, streaming, per-device modes, downloads, sync, analysis, the
//! account) lives here; the popovers keep only what's needed in the moment.

use super::widgets::{MODES, choice, danger, menu_label, mode_label, secondary, section, thin_slider, toggle};
use super::{Action, OndApp};
use crate::playback::{BITRATES, PlaybackPrefs, QualityMode, RgMode, TranscodeFormat};
use crate::theme::{ThemePref, Tokens, semibold};
use audio::PlaybackMode;
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};


#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Section {
    #[default]
    General,
    Playback,
    Output,
    Offline,
    Library,
    Account,
}

impl Section {
    pub const ALL: [Section; 6] = [Section::General, Section::Playback, Section::Output, Section::Offline, Section::Library, Section::Account];

    pub fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Playback => "Playback",
            Section::Output => "Output",
            Section::Offline => "Offline",
            Section::Library => "Library",
            Section::Account => "Account",
        }
    }
}

const NAV_W: f32 = 176.0;
/// The right pane's content width (it scrolls when taller than the window).
const PANE_PAD: f32 = 28.0;

impl OndApp {
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let w = (screen.width() - 64.0).clamp(480.0, 760.0);
        let h = (screen.height() - 96.0).clamp(340.0, 540.0);
        let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin::ZERO).corner_radius(12);
        let modal = egui::Modal::new(egui::Id::unique("settings-window")).frame(frame).show(ctx, |ui| {
            let t = Tokens::of(ui);
            let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
            let side = Rect::from_min_size(rect.min, vec2(NAV_W, h));
            ui.painter().rect_filled(side, egui::CornerRadius { nw: 12, sw: 12, ..Default::default() }, t.sidebar);
            ui.painter().vline(side.right(), side.y_range(), egui::Stroke::new(1.0, t.hairline));

            // Section list.
            let mut nav = ui.new_child(egui::UiBuilder::new().max_rect(side.shrink2(vec2(10.0, 16.0))).layout(egui::Layout::top_down(egui::Align::Min)));
            nav.spacing_mut().item_spacing.y = 2.0;
            nav.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Settings").font(FontId::new(15.0, semibold())).color(t.text));
            });
            nav.add_space(12.0);
            for s in Section::ALL {
                if self.place(&mut nav, s.label(), self.ui.settings_section == s).clicked() {
                    self.ui.settings_section = s;
                }
            }

            // The pane.
            let pane = Rect::from_min_max(pos2(side.right() + 1.0, rect.top()), rect.max);
            let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(pane).layout(egui::Layout::top_down(egui::Align::Min)));
            egui::ScrollArea::vertical().id_salt(("settings-pane", self.ui.settings_section)).auto_shrink(false).show(&mut ui, |ui| {
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    ui.add_space(PANE_PAD);
                    ui.vertical(|ui| {
                        let width = pane.width() - PANE_PAD * 2.0;
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(egui::RichText::new(self.ui.settings_section.label()).font(FontId::new(20.0, semibold())).color(t.text));
                        ui.add_space(16.0);
                        match self.ui.settings_section {
                            Section::General => self.general_settings(ui),
                            Section::Playback => self.playback_settings(ui, width),
                            Section::Output => self.output_settings(ui),
                            Section::Offline => self.offline_settings(ui),
                            Section::Library => self.library_settings(ui),
                            Section::Account => self.account_settings(ui),
                        }
                        ui.add_space(24.0);
                    });
                });
            });
        });
        if modal.should_close() {
            self.ui.overlay = None;
        }
    }

    fn general_settings(&mut self, ui: &mut Ui) {
        group(ui, "Appearance", |ui| {
            let mut theme = self.theme();
            if choice(ui, &ThemePref::ALL, &mut theme, Some(240.0), |p| p.label().into()) {
                self.actions.push(Action::SetTheme(theme));
            }
        });
        group(ui, "Listening", |ui| {
            let mut scrobble = self.scrobble();
            if narrow_toggle(ui, &mut scrobble, "Scrobble plays").changed() {
                self.actions.push(Action::SetScrobble(scrobble));
            }
            hint(ui, "Ond reports plays to your server, which can pass them on to Last.fm or ListenBrainz.");
            ui.add_space(10.0);
            let mut playing = self.analyze_playing();
            if narrow_toggle(ui, &mut playing, "Analyze tracks as they play").changed() {
                self.actions.push(Action::SetAnalyzePlaying(playing));
            }
            hint(ui, "Measures loudness and dynamic range once a track has downloaded for playback.");
        });
        group(ui, "Advanced", |ui| {
            let mut shown = self.diagnostics_shown();
            if narrow_toggle(ui, &mut shown, "Show diagnostics bar").changed() {
                self.actions.push(Action::SetDiagnostics(shown));
            }
            hint(ui, "Shows the signal path, buffer, downloads, sync and server timing in a thin bar along the bottom (⌥⌘I).");
            if crate::csd::SUPPORTED {
                ui.add_space(10.0);
                let mut system = self.system_title_bar();
                if narrow_toggle(ui, &mut system, "Use the system title bar").changed() {
                    self.actions.push(Action::SetSystemTitleBar(system));
                }
                hint(ui, "Your desktop's own title bar and window frame, in place of Ond's buttons in the toolbar.");
            }
        });
    }

    fn playback_settings(&mut self, ui: &mut Ui, width: f32) {
        let mut p: PlaybackPrefs = self.output.prefs().clone();
        let before = p.clone();
        group(ui, "ReplayGain", |ui| {
            choice(ui, &RgMode::ALL, &mut p.replay_gain, Some(width.min(360.0)), |m| m.label().into());
            hint(ui, p.replay_gain.hint());
            if p.replay_gain != RgMode::Off {
                ui.add_space(12.0);
                db_row(ui, "Pre-amp", &mut p.preamp_db, -12.0..=12.0, width);
                db_row(ui, "Untagged tracks", &mut p.untagged_db, -12.0..=0.0, width);
                hint(ui, "Gain for tracks without ReplayGain tags. Ond keeps peaks below −1 dBFS.");
            }
            if self.bit_perfect {
                ui.add_space(6.0);
                hint(ui, "Bypassed while bit-perfect is on.");
            }
        });
        group(ui, "Streaming", |ui| {
            choice(ui, &QualityMode::ALL, &mut p.quality, Some(width.min(360.0)), |q| q.label().into());
            hint(ui, p.quality.hint());
            if p.quality != QualityMode::Original {
                ui.add_space(12.0);
                row(ui, "Format", |ui| {
                    choice(ui, &TranscodeFormat::ALL, &mut p.format, None, |f| f.label().into());
                });
                row(ui, "Bitrate", |ui| {
                    choice(ui, &BITRATES, &mut p.kbps, None, |k| k.to_string());
                });
                let network = crate::fmt::capitalize(self.output.network().label());
                let now = match p.transcode(self.output.network()) {
                    Some(t) => format!("{network} · new tracks stream as {}", t.label()),
                    None => format!("{network} · new tracks stream as original files"),
                };
                hint(ui, &now);
            }
        });
        if p != before {
            self.actions.push(Action::SetPlaybackPrefs(p));
        }
    }

    fn output_settings(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let (devices, current) = self.output.devices();
        let rows: Vec<(String, String, String, PlaybackMode, bool)> = devices
            .iter()
            .map(|d| (d.uid.clone(), d.name.clone(), d.capability_line(), self.output.device_mode(d), current == Some(d.uid.as_str())))
            .collect();
        group(ui, "Devices", |ui| {
            if rows.is_empty() {
                hint(ui, "No output devices found.");
            }
            for (k, (uid, name, caps, mode, in_use)) in rows.iter().enumerate() {
                if k > 0 {
                    ui.add_space(6.0);
                    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), egui::Stroke::new(1.0, t.hairline));
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
                    ui.set_min_height(36.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(name).size(13.0).color(t.text));
                            if *in_use {
                                ui.label(egui::RichText::new("in use").size(11.0).color(t.accent));
                            }
                        });
                        if !caps.is_empty() {
                            ui.label(egui::RichText::new(caps).size(11.0).color(t.muted));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut m = *mode;
                        if choice(ui, &MODES, &mut m, None, mode_label) {
                            self.actions.push(Action::SetDeviceMode(uid.clone(), m));
                        }
                    });
                });
            }
            ui.add_space(10.0);
            hint(ui, "New devices start in Normal for speakers, Bluetooth and AirPlay, and in Bit-perfect for DACs and headphones. Bit-perfect sends samples untouched, so use the device's own volume.");
        });
    }

    fn offline_settings(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let snap = self.downloads.snap.clone();
        group(ui, "Storage", |ui| {
            ui.label(egui::RichText::new(super::offline_page::storage_used(&snap)).size(13.0).color(t.text));
            ui.add_space(10.0);
            row(ui, "Limit", |ui| {
                let label = match snap.limit {
                    Some(b) => crate::fmt::bytes(b),
                    None => "No limit".to_owned(),
                };
                let resp = menu_label(ui, &label, t.text);
                egui::Popup::menu(&resp).show(|ui| super::offline_page::limit_choices(ui, snap.limit, &mut self.actions));
            });
            let mut paused = snap.paused;
            if narrow_toggle(ui, &mut paused, "Pause downloads").changed() {
                self.actions.push(Action::PauseDownloads(paused));
            }
        });
        group(ui, "Location", |ui| {
            let dir = self.downloads.base.display().to_string();
            let g = super::widgets::elide(ui, &dir, FontId::proportional(12.0), t.muted, ui.available_width());
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
            ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, t.muted);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if secondary(ui, &format!("Show in {}", crate::folder::FILE_MANAGER)).clicked() {
                    self.actions.push(Action::RevealOffline);
                }
                ui.add_space(4.0);
                if secondary(ui, "Change…").clicked() {
                    self.actions.push(Action::MoveOffline);
                }
            });
            ui.add_space(6.0);
            hint(ui, "Downloads move to a new location the next time Ond starts.");
        });
    }

    fn library_settings(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        group(ui, "Sync", |ui| {
            let albums = self.data.albums.len();
            let tracks = self.data.tracks.all.len();
            ui.label(egui::RichText::new(format!("{albums} albums · {tracks} tracks")).size(13.0).color(t.text));
            let when = if self.sync.tracks {
                self.sync.label.clone()
            } else {
                match self.sync.last {
                    Some(at) => format!("Last synced at {}", at.format("%-I:%M %p")),
                    None if self.session.client.is_some() => "Synced at launch".to_owned(),
                    None => "Not connected".to_owned(),
                }
            };
            hint(ui, &when);
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if self.sync.tracks {
                    if secondary(ui, "Stop").clicked() {
                        self.bridge.cancel_sync();
                    }
                } else {
                    let can = self.session.client.is_some() && self.session.online;
                    if ui.add_enabled_ui(can, |ui| secondary(ui, "Sync now")).inner.clicked() {
                        self.actions.push(Action::SyncNow);
                    }
                }
            });
        });
        group(ui, "Analysis", |ui| self.analysis_settings(ui));
    }

    /// Library analysis: start it, follow it, pause or stop it.
    fn analysis_settings(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let p = self.analysis.runner.progress();
        if p.active() {
            let status = if p.paused {
                format!("Paused at {} of {}", p.done.min(p.total), p.total)
            } else if p.waiting {
                format!("{} of {} · waiting for playback", p.done.min(p.total), p.total)
            } else {
                format!("Analyzing {} of {} · {} left", p.done.min(p.total), p.total, crate::fmt::about(p.eta_secs))
            };
            ui.label(egui::RichText::new(status).size(13.0).color(t.text));
            ui.add_space(6.0);
            let frac = if p.total > 0 { p.done.min(p.total) as f32 / p.total as f32 } else { 0.0 };
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().min(360.0), 4.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, t.hairline);
            ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(r.width() * frac, r.height())), 2.0, t.accent);
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let (label, paused) = if p.paused { ("Resume", false) } else { ("Pause", true) };
                if secondary(ui, label).clicked() {
                    self.actions.push(Action::PauseAnalysis(paused));
                }
                ui.add_space(4.0);
                if secondary(ui, "Stop").clicked() {
                    self.actions.push(Action::StopAnalysis);
                }
            });
        } else if self.session.client.is_some() && !self.data.tracks.all.is_empty() {
            let (n, secs) = self.unanalysed();
            if n == 0 {
                ui.label(egui::RichText::new("Every track is analyzed.").size(13.0).color(t.text));
            } else {
                let text = format!("{} not analyzed yet · {}", crate::fmt::plural(n, "track"), crate::fmt::about(crate::analyze::assumed_eta(secs)));
                ui.label(egui::RichText::new(text).size(13.0).color(t.text));
                ui.add_space(10.0);
                if secondary(ui, "Analyze library").clicked() {
                    self.actions.push(Action::AnalyzeLibrary);
                }
            }
        } else {
            hint(ui, "Available once the library has synced.");
        }
        if p.failed > 0 {
            ui.add_space(6.0);
            hint(ui, &format!("{} couldn't be read", p.failed));
        }
        ui.add_space(8.0);
        hint(ui, "Measures loudness and dynamic range, and checks for upsampled or lossy-sourced files. Runs in the background and pauses while playback needs the network.");
    }

    fn account_settings(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let (server, user) = match &self.session.settings {
            Some(s) => (s.server.clone(), s.username.clone()),
            None => (String::new(), String::new()),
        };
        group(ui, "Server", |ui| {
            if server.is_empty() {
                hint(ui, "Not connected.");
                return;
            }
            row(ui, "Address", |ui| {
                ui.label(egui::RichText::new(&server).size(13.0).color(t.text));
            });
            row(ui, "User", |ui| {
                ui.label(egui::RichText::new(&user).size(13.0).color(t.text));
            });
            if !self.session.server_info.is_empty() {
                row(ui, "Server", |ui| {
                    ui.label(egui::RichText::new(&self.session.server_info).size(13.0).color(t.text));
                });
            }
            row(ui, "Status", |ui| {
                let (text, color) = if self.session.online { ("Connected", t.ok) } else { ("Can't reach the server", t.warn) };
                ui.label(egui::RichText::new(text).size(13.0).color(color));
            });
            ui.add_space(14.0);
            if danger(ui, "Sign out").clicked() {
                self.actions.push(Action::SignOut);
                self.ui.overlay = None;
            }
            ui.add_space(6.0);
            hint(ui, "Forgets the saved password. Downloads stay on this computer.");
        });
    }
}

/// A titled block with an 8pt rhythm: label, 8, content, 28 to the next.
fn group(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    section(ui, title);
    ui.add_space(8.0);
    body(ui);
    ui.add_space(28.0);
}

/// Width of the label column in [`row`]s, so controls line up down a section.
const LABEL_W: f32 = 136.0;

/// A label in a fixed column, then its control, 32 high.
fn row(ui: &mut Ui, label: &str, control: impl FnOnce(&mut Ui)) {
    let t = Tokens::of(ui);
    ui.horizontal(|ui| {
        ui.set_min_height(32.0);
        let (r, _) = ui.allocate_exact_size(vec2(LABEL_W, 20.0), Sense::hover());
        ui.painter().text(r.left_center(), egui::Align2::LEFT_CENTER, label, FontId::proportional(13.0), t.text);
        control(ui);
    });
}

/// Muted explanation text under a control.
fn hint(ui: &mut Ui, text: &str) {
    let t = Tokens::of(ui);
    ui.add_space(4.0);
    ui.add(egui::Label::new(egui::RichText::new(crate::keys::shown(text)).size(12.0).color(t.muted)).wrap());
}

/// A dB value on a thin slider with a readout, snapped to 0.5 dB.
fn db_row(ui: &mut Ui, label: &str, db: &mut f32, range: std::ops::RangeInclusive<f32>, width: f32) {
    let t = Tokens::of(ui);
    let (lo, hi) = (*range.start(), *range.end());
    row(ui, label, |ui| {
        let mut v = (*db - lo) / (hi - lo);
        if thin_slider(ui, &mut v, (width - LABEL_W - 80.0).clamp(140.0, 240.0), label).changed() {
            *db = ((lo + v * (hi - lo)) * 2.0).round() / 2.0;
        }
        let readout = crate::fmt::gain(*db);
        let (r, _) = ui.allocate_exact_size(vec2(56.0, 18.0), Sense::hover());
        ui.painter().text(r.right_center(), egui::Align2::RIGHT_CENTER, readout, FontId::proportional(12.0), t.muted);
    });
}

/// A switch row kept to a readable width, so the switch sits near its label.
fn narrow_toggle(ui: &mut Ui, on: &mut bool, label: &str) -> egui::Response {
    ui.scope(|ui| {
        ui.set_max_width(ui.available_width().min(420.0));
        toggle(ui, on, label)
    })
    .inner
}
