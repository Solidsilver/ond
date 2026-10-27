//! The Settings window (⌘,): a list of sections on the left, one calm pane
//! on the right. Everything that used to hide in popovers (appearance,
//! ReplayGain, streaming, per-device modes, downloads, sync, analysis, the
//! account) lives here; the popovers keep only what's needed in the moment.

use super::{AnalysisAction, OfflineAction, OutputAction, PrefsAction, WindowAction};
use super::widgets::{MODES, choice, menu_label, mode_label, secondary, secondary_danger, section, switch, thin_slider};
use super::seek::{WaveGrid, paint_wave};
use super::{Action, OndApp};
use crate::playback::{BITRATES, PlaybackPrefs, QualityMode, RgMode, TranscodeFormat};
use crate::theme::{Accent, ThemePref, Tokens, semibold};
use crate::visuals::visualizer::Pick;
use crate::visuals::waveform::{self, Bars};
use audio::PlaybackMode;
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Section {
    #[default]
    General,
    Appearance,
    Playback,
    Output,
    Offline,
    Library,
    Account,
    Experimental,
}

impl Section {
    pub const ALL: [Section; 8] = [Section::General, Section::Appearance, Section::Playback, Section::Output, Section::Offline, Section::Library, Section::Account, Section::Experimental];

    pub fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::Playback => "Playback",
            Section::Output => "Output",
            Section::Offline => "Offline",
            Section::Library => "Library",
            Section::Account => "Account",
            Section::Experimental => "Experimental",
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
                            Section::Appearance => self.appearance_settings(ui),
                            Section::Playback => self.playback_settings(ui),
                            Section::Output => self.output_settings(ui),
                            Section::Offline => self.offline_settings(ui),
                            Section::Library => self.library_settings(ui),
                            Section::Account => self.account_settings(ui),
                            Section::Experimental => self.experimental_settings(ui),
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
        group(ui, "Listening", |ui| {
            let mut scrobble = self.ui.prefs.scrobble;
            if switch_row(ui, "Scrobble plays", "Ond reports plays to your server, which can pass them on to Last.fm or ListenBrainz.", &mut scrobble) {
                self.actions.push(Action::Prefs(PrefsAction::Scrobble(scrobble)));
            }
        });
        group(ui, "Advanced", |ui| {
            let mut shown = self.ui.prefs.diagnostics_bar;
            if switch_row(ui, "Show diagnostics bar", "The signal path, buffer, downloads, sync and server timing, in a thin bar along the bottom (⌥⌘I).", &mut shown) {
                self.actions.push(Action::Prefs(PrefsAction::Diagnostics(shown)));
            }
            if crate::csd::SUPPORTED {
                let mut system = self.ui.prefs.system_title_bar;
                if switch_row(ui, "Use the system title bar", "Your desktop's own title bar and window frame, in place of Ond's buttons in the toolbar.", &mut system) {
                    self.actions.push(Action::Window(WindowAction::SetSystemTitleBar(system)));
                }
            }
        });
    }

    /// How Ond looks: its colors, the artwork behind the Player and Mini, and
    /// the seek bar.
    fn appearance_settings(&mut self, ui: &mut Ui) {
        group(ui, "Colors", |ui| {
            let mut theme = self.ui.prefs.theme;
            if setting(ui, "Theme", "", |ui| choice(ui, &ThemePref::ALL, &mut theme, None, |p| p.label().into())) {
                self.actions.push(Action::Prefs(PrefsAction::Theme(theme)));
            }
            let mut accent = self.ui.prefs.accent;
            if setting(ui, "Accent color", "", |ui| accent_swatches(ui, &mut accent)) {
                self.actions.push(Action::Prefs(PrefsAction::Accent(accent)));
            }
            let mut adapt = self.ui.prefs.adapt_colors;
            if switch_row(ui, "Adapt colors to the album art", "The accent, the Player and the now-playing display follow the playing album's cover, and an album's or artist's page follows its own art.", &mut adapt) {
                self.actions.push(Action::Prefs(PrefsAction::AdaptColors(adapt)));
            }
        });
        group(ui, "Artwork", |ui| {
            let mut on = self.ui.prefs.backdrop;
            if switch_row(ui, "Blurred artwork", "A soft, dimmed copy of the cover behind the Player and Mini.", &mut on) {
                self.actions.push(Action::Prefs(PrefsAction::Backdrop(on)));
            }
        });
        group(ui, "Seek bar", |ui| {
            let mut on = self.ui.prefs.waveform;
            if switch_row(ui, "Waveform", "Draws the track's shape in the seek bar. Ond reads each file once in the background.", &mut on) {
                self.actions.push(Action::Prefs(PrefsAction::Waveform(on)));
            }
            if on {
                let mut bars = self.ui.prefs.waveform_bars;
                waveform_bar_settings(ui, &mut bars);
                if bars != self.ui.prefs.waveform_bars {
                    self.actions.push(Action::Prefs(PrefsAction::WaveformBars(bars)));
                }
            }
        });
        group(ui, "Lists", |ui| {
            let mut on = self.ui.prefs.alpha_rail;
            if switch_row(ui, "Alphabet rail", "Letters beside Albums, Artists and Songs when they're sorted by name. Click or drag along them to jump.", &mut on) {
                self.actions.push(Action::Prefs(PrefsAction::AlphaRail(on)));
            }
            let mut on = self.ui.prefs.artist_photos;
            if switch_row(ui, "Artist photos", "Each artist's photo from the server beside their name in Artists, or their initial when the server has none.", &mut on) {
                self.actions.push(Action::Prefs(PrefsAction::ArtistPhotos(on)));
            }
        });
    }

    fn playback_settings(&mut self, ui: &mut Ui) {
        let mut p: PlaybackPrefs = self.output.prefs().clone();
        let before = p.clone();
        group(ui, "Volume", |ui| {
            let hint = if self.bit_perfect { format!("{}. Bypassed while bit-perfect is on.", p.replay_gain.hint()) } else { p.replay_gain.hint().to_owned() };
            setting(ui, "ReplayGain", &hint, |ui| choice(ui, &RgMode::ALL, &mut p.replay_gain, None, |m| m.label().into()));
            if p.replay_gain != RgMode::Off {
                db_row(ui, "Pre-amp", "", &mut p.preamp_db, -12.0..=12.0);
                db_row(ui, "Untagged tracks", "For tracks without ReplayGain tags. Ond keeps peaks below −1 dBFS.", &mut p.untagged_db, -12.0..=0.0);
            }
        });
        group(ui, "Streaming", |ui| {
            setting(ui, "Quality", p.quality.hint(), |ui| choice(ui, &QualityMode::ALL, &mut p.quality, None, |q| q.label().into()));
            if p.quality != QualityMode::Original {
                setting(ui, "Format", "", |ui| choice(ui, &TranscodeFormat::ALL, &mut p.format, None, |f| f.label().into()));
                let network = crate::fmt::capitalize(self.output.network().label());
                let now = match p.transcode(self.output.network()) {
                    Some(t) => format!("{network} · new tracks stream as {}", t.label()),
                    None => format!("{network} · new tracks stream as original files"),
                };
                setting(ui, "Bitrate", &now, |ui| choice(ui, &BITRATES, &mut p.kbps, None, |k| k.to_string()));
            }
        });
        if p != before {
            self.actions.push(Action::Output(OutputAction::Playback(p)));
        }
    }

    fn output_settings(&mut self, ui: &mut Ui) {
        let (devices, current) = self.output.devices();
        let rows: Vec<(String, String, String, PlaybackMode, bool)> = devices
            .iter()
            .map(|d| (d.uid.clone(), d.name.clone(), d.capability_line(), self.output.device_mode(d), current == Some(d.uid.as_str())))
            .collect();
        group(ui, "Devices", |ui| {
            if rows.is_empty() {
                setting(ui, "No output devices found", "", |_| ());
            }
            for (uid, name, caps, mode, in_use) in &rows {
                let mut m = *mode;
                let head = Head { label: name, tag: in_use.then_some("in use"), hint: caps, path: false };
                if row_with(ui, head, |ui| choice(ui, &MODES, &mut m, None, mode_label)) {
                    self.actions.push(Action::Output(OutputAction::DeviceMode(uid.clone(), m)));
                }
            }
            note(ui, "New devices start in Normal for speakers, Bluetooth and AirPlay, and in Bit-perfect for DACs and headphones. Bit-perfect sends samples untouched, so use the device's own volume.");
        });
    }

    fn offline_settings(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let snap = self.downloads.snap.clone();
        if snap.set_aside.is_some() {
            super::offline_page::set_aside_notice(ui, &snap, &mut self.actions, "settings");
            ui.add_space(28.0);
        }
        group(ui, "Storage", |ui| {
            setting(ui, "Storage limit", &super::offline_page::storage_used(&snap), |ui| {
                let label = match snap.limit {
                    Some(b) => crate::fmt::bytes(b),
                    None => "No limit".to_owned(),
                };
                let resp = menu_label(ui, &label, t.text);
                egui::Popup::menu(&resp).show(|ui| super::offline_page::limit_choices(ui, snap.limit, &mut self.actions));
            });
            let mut paused = snap.paused;
            if switch_row(ui, "Pause downloads", "", &mut paused) {
                self.actions.push(Action::Offline(OfflineAction::Pause(paused)));
            }
        });
        group(ui, "Location", |ui| {
            let dir = self.downloads.base.display().to_string();
            row_with(ui, Head { label: "Download folder", hint: &dir, path: true, ..Default::default() }, |ui| {
                // Right to left: the last button first.
                if secondary(ui, "Change…").clicked() {
                    self.actions.push(Action::Offline(OfflineAction::Move));
                }
                if secondary(ui, &format!("Show in {}", crate::folder::FILE_MANAGER)).clicked() {
                    self.actions.push(Action::Offline(OfflineAction::Reveal));
                }
            });
            note(ui, "Downloads move to a new location the next time Ond starts.");
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
                    if ui.add_enabled_ui(can, |ui| secondary(ui, "Sync now")).inner.clicked() {
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
        let mut playing = self.ui.prefs.analyze_playing;
        if switch_row(ui, "Analyze tracks as they play", "Each track is measured once it has downloaded for playback.", &mut playing) {
            self.actions.push(Action::Analysis(AnalysisAction::AnalyzePlaying(playing)));
        }
        note(ui, "Measures loudness and dynamic range, and checks for upsampled or lossy-sourced files. Runs in the background and pauses while playback needs the network.");
    }

    /// Visuals still being tried out, one switch each.
    fn experimental_settings(&mut self, ui: &mut Ui) {
        let mut x = self.ui.prefs.experimental;
        let before = x;
        paragraph(ui, "Visual features that are still being tried out. They may change or go away, and their settings may not carry over to the next version. None of them change the sound: bit-perfect playback stays bit-perfect.");
        ui.add_space(24.0);
        group(ui, "Live audio", |ui| {
            switch_row(ui, "Live spectrum", "In the now-playing display and the Player. Uses more battery while music plays.", &mut x.spectrum);
            switch_row(ui, "Spectrogram in Info", "Frequency over time, scrolling in the Info tab. Uses more battery while music plays.", &mut x.spectrogram);
            switch_row(ui, "Visualizer", "A full-window light show: ⌘T opens it, Esc leaves. Keeps the graphics card busy while open.", &mut x.visualizer);
            if x.visualizer {
                setting(ui, "Show", "Shuffle picks a different one each track. Inside the visualizer, ← and → switch.", |ui| {
                    choice(ui, &Pick::ALL, &mut x.visualizer_pick, None, |p| p.label().into());
                });
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
            let (text, color) = if self.session.online { ("Connected", t.ok) } else { ("Can't reach the server", t.warn) };
            setting(ui, "Status", "", |ui| value(ui, text, color));
        });
        group(ui, "Signed in", |ui| {
            setting(ui, &user, "Signing out forgets the saved password. Downloads stay on this computer.", |ui| {
                if secondary_danger(ui, "Sign out").clicked() {
                    self.actions.push(Action::SignOut);
                    self.ui.overlay = None;
                }
            });
        });
    }
}

/// A titled block of rows with hairlines between them: the title, 4, the
/// rows, 20 to the next.
fn group(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    section(ui, title);
    ui.add_space(4.0);
    ui.data_mut(|d| d.insert_temp(first_row(), true));
    body(ui);
    ui.add_space(20.0);
}

/// Set by a [`group`] and cleared by its first row, which has no hairline above.
fn first_row() -> egui::Id {
    egui::Id::unique("settings-first-row")
}

/// The hairline above a row, unless it's the first in its group.
fn divider(ui: &mut Ui) {
    let first = ui.data_mut(|d| std::mem::replace(d.get_temp_mut_or(first_row(), true), false));
    if !first {
        let t = Tokens::of(ui);
        let (x, y) = (ui.cursor().left(), ui.cursor().top());
        ui.painter().hline(x..=x + ui.available_width(), y, egui::Stroke::new(1.0, t.hairline));
    }
}

/// Every control fits this height (a button is the tallest).
const CONTROL_H: f32 = 30.0;
/// Space above and below a row's content.
const ROW_PAD: f32 = 6.0;
/// The least room between a row's text and its control.
const ROW_GAP: f32 = 24.0;
/// The most of a name a row keeps beside its control before putting the
/// control under it (longer names wrap).
const NAME_W: f32 = 160.0;
/// Sliders in rows (with their readout beside them).
const SLIDER_W: f32 = 160.0;

/// What a row says on its left.
#[derive(Default, Clone, Copy)]
struct Head<'a> {
    label: &'a str,
    /// A small accent note after the label ("in use").
    tag: Option<&'a str>,
    /// Muted text under the label, wrapped; empty for none.
    hint: &'a str,
    /// The hint is a path: one line, elided, never wrapped.
    path: bool,
}

/// One setting: its name and a short hint on the left, its control on the
/// right, level with the name. Every page is built from these, so controls
/// share one right edge and hints never run under them. The control is added
/// right to left (its last part first).
fn setting<R>(ui: &mut Ui, label: &str, hint: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    row_with(ui, Head { label, hint, ..Default::default() }, control)
}

/// [`setting`] with the whole [`Head`].
fn row_with<R>(ui: &mut Ui, head: Head<'_>, control: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::of(ui);
    divider(ui);
    let (left, top, width) = (ui.cursor().left(), ui.cursor().top(), ui.available_width());
    // The name's first line level with the control, and as much room below
    // the text as above it.
    let line = ui.ctx().fonts_mut(|f| f.row_height(&FontId::proportional(13.0)));
    let inset = (CONTROL_H - line) / 2.0;
    // The control goes beside the name when both fit (it did last frame), and
    // under it in a narrow window.
    let key = ui.scope_id().with(("setting", head.label));
    let name_w = ui.painter().layout_no_wrap(head.label.to_owned(), FontId::proportional(13.0), t.text).size().x.min(NAME_W);
    let fits = |w: f32| w + ROW_GAP + name_w <= width;
    let beside = fits(ui.data(|d| d.get_temp::<f32>(key)).unwrap_or(0.0));
    let (out, used, bottom) = if beside {
        let strip = Rect::from_min_size(pos2(left, top + ROW_PAD), vec2(width, CONTROL_H));
        let (out, used) = row_control(ui, strip, control);
        let room = (width - if used > 0.0 { used + ROW_GAP } else { 0.0 }).max(80.0);
        let text = row_text(ui, head, pos2(left, strip.top() + inset), room);
        (out, used, (text + inset).max(strip.bottom()) + ROW_PAD)
    } else {
        let text = row_text(ui, head, pos2(left, top + ROW_PAD + inset), width);
        let strip = Rect::from_min_size(pos2(left, text + 8.0), vec2(width, CONTROL_H));
        let (out, used) = row_control(ui, strip, control);
        (out, used, strip.bottom() + ROW_PAD)
    };
    ui.data_mut(|d| d.insert_temp(key, used));
    if fits(used) != beside {
        ui.ctx().request_discard("a setting's control moved beside or under its name");
    }
    ui.advance_cursor_after_rect(Rect::from_min_max(pos2(left, top), pos2(left + width, bottom)));
    out
}

/// A row's control, right-aligned in `strip`, and how wide it came out.
fn row_control<R>(ui: &mut Ui, strip: Rect, control: impl FnOnce(&mut Ui) -> R) -> (R, f32) {
    let mut right = ui.new_child(egui::UiBuilder::new().max_rect(strip).layout(egui::Layout::right_to_left(egui::Align::Center)));
    right.spacing_mut().item_spacing.x = 6.0;
    let out = control(&mut right);
    (out, right.min_rect().width())
}

/// A row's name (and tag) and hint from `at`, `room` wide. Returns where they end.
fn row_text(ui: &mut Ui, head: Head<'_>, at: egui::Pos2, room: f32) -> f32 {
    let t = Tokens::of(ui);
    let mut text = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(at, vec2(room, f32::INFINITY))).layout(egui::Layout::top_down(egui::Align::Min)));
    text.spacing_mut().item_spacing.y = 3.0;
    let mut job = egui::text::LayoutJob::default();
    job.append(head.label, 0.0, egui::TextFormat::simple(FontId::proportional(13.0), t.text));
    if let Some(tag) = head.tag {
        job.append(tag, 6.0, egui::TextFormat::simple(FontId::proportional(11.0), t.accent));
    }
    job.wrap.max_width = room;
    let label = text.painter().layout_job(job);
    text.add(egui::Label::new(label));
    if !head.hint.is_empty() {
        let hint = crate::keys::shown(head.hint);
        if head.path {
            let g = super::widgets::elide(&text, &hint, FontId::proportional(12.0), t.muted, room);
            text.add(egui::Label::new(g));
        } else {
            text.add(egui::Label::new(egui::RichText::new(hint).size(12.0).color(t.muted)).wrap());
        }
    }
    text.min_rect().bottom()
}

/// A [`setting`] that's a switch. True when it changed.
fn switch_row(ui: &mut Ui, label: &str, hint: &str, on: &mut bool) -> bool {
    setting(ui, label, hint, |ui| switch(ui, on, label)).changed()
}

/// A value read out on the right of a row.
fn value(ui: &mut Ui, text: &str, color: egui::Color32) {
    ui.label(egui::RichText::new(text).size(13.0).color(color));
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

/// A slider's value, right-aligned in a fixed width so the slider beside it
/// doesn't shift as the value changes.
fn readout(ui: &mut Ui, text: &str) {
    let t = Tokens::of(ui);
    let (r, _) = ui.allocate_exact_size(vec2(52.0, 18.0), Sense::hover());
    ui.painter().text(r.right_center(), egui::Align2::RIGHT_CENTER, text, FontId::proportional(12.0), t.muted);
}

/// The waveform's bars (their ends, thickness and spacing) under a preview
/// of them across the pane.
fn waveform_bar_settings(ui: &mut Ui, bars: &mut Bars) {
    divider(ui);
    ui.add_space(ROW_PAD + 3.0);
    waveform_preview(ui, *bars, ui.available_width());
    ui.add_space(ROW_PAD + 3.0);
    setting(ui, "Bars", "", |ui| choice(ui, &[false, true], &mut bars.rounded, None, |r| if r { "Rounded" } else { "Square" }.into()));
    step_row(ui, "Thickness", &mut bars.width, Bars::WIDTHS, half_points);
    step_row(ui, "Spacing", &mut bars.gap, Bars::GAPS, half_points);
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

/// A whole-number setting in `range` on a thin slider, with its value read
/// out. The slider's own position is kept between frames, so the wheel and
/// the arrow keys, which move it a little at a time, add up to a step.
fn step_row(ui: &mut Ui, label: &str, value: &mut u8, range: std::ops::RangeInclusive<u8>, text: fn(u8) -> String) {
    let (lo, hi) = (*range.start(), *range.end());
    let span = f32::from(hi - lo);
    let at = (*value).clamp(lo, hi);
    let snapped = |v: f32| lo + (v * span).round() as u8;
    setting(ui, label, "", |ui| {
        readout(ui, &text(at));
        let id = ui.scope_id().with(("step", label));
        let mut v = ui.data(|d| d.get_temp::<f32>(id)).filter(|&v| snapped(v) == at).unwrap_or(f32::from(at - lo) / span);
        if thin_slider(ui, &mut v, SLIDER_W, label).changed() {
            ui.data_mut(|d| d.insert_temp(id, v));
            *value = snapped(v);
        }
    });
}

/// Half points as points: "1 pt", "1.5 pt".
fn half_points(v: u8) -> String {
    if v.is_multiple_of(2) { format!("{} pt", v / 2) } else { format!("{}.5 pt", v / 2) }
}
