//! The output control (the device's icon with its status dot and ▾, and the
//! volume slider) and the panel it opens: System output or a chosen device,
//! the device in use with its mode and volume, the other devices in their
//! groups, and the way to Settings.

use super::readout::{hover_text, paint_readout, readout_of, warning};
use super::volume::{db_text, device_volume_hover, slider_db, slider_pos};
use super::{ControlOut, Outputs, group_devices, kind_icon};
use crate::icons::{self, Icon};
use crate::settings::DbReadout;
use crate::theme::Tokens;
use audio::{BitPerfectStatus, DeviceInfo, DeviceVolume, EngineHandle, PlaybackMode};

const POPUP_W: f32 = 320.0;
/// The output control's height (the toolbar's rows are this tall).
const CONTROL_H: f32 = 28.0;
/// The device icon with its ▾.
const ICON_W: f32 = 40.0;
/// The card's text and controls, in from the panel's left: past the device's icon.
const CARD_INDENT: f32 = 36.0;
/// A listed device's icon, centred here, and its name, past the ▸ and the icon.
const LIST_ICON_X: f32 = 36.0;
const LIST_INDENT: f32 = 52.0;
/// The room a mode switch takes at the right of its row.
const SWITCH_W: f32 = 50.0;
/// Beside the panel's device slider: the dB field and the mute button.
const FIELD_MUTE_W: f32 = 112.0;
/// Beside the panel's Ond slider: "100%".
const PERCENT_W: f32 = 56.0;
/// Between the slider, the dB readout and the icon.
const GAP: f32 = 6.0;
/// A device in the panel's list.
const ROW_H: f32 = 32.0;
/// The list shows about three and a half devices before it scrolls.
const LIST_H: f32 = ROW_H * 3.5;
const BIT_PERFECT_HELP: &str = "Bit-perfect sends samples untouched and keeps other apps off the device while Ond plays. The volume then sets the device's own level, when it has one.";
/// The format pill's (and the list's bit-perfect dots') inset from the panel's right.
const PILL_INSET: f32 = 10.0;
/// Where the pill's dot sits, in from its right edge.
const PILL_DOT: f32 = 8.5;
/// The dB field's step: ↑↓, and a scroll notch.
const STEP_DB: f32 = 0.5;
/// With ⇧.
const FINE_STEP_DB: f32 = 0.1;
/// Scrolling this far over the dB field is one step.
const SCROLL_NOTCH: f32 = 24.0;
/// Room beside the slider for "−24.0 dB".
const DB_W: f32 = 52.0;

impl Outputs {
    /// The room the output control takes (to centre it in the toolbar). The
    /// same in every mode, so nothing moves when the mode does.
    pub fn control_width(&self, slider_w: Option<f32>) -> f32 {
        let db = self.prefs.get(|s| s.device_volume_db) == DbReadout::Always;
        ICON_W + slider_w.map_or(0.0, |w| w + GAP + if db { DB_W } else { 0.0 })
    }

    /// The output control: (given `slider_w`) the volume slider, then the
    /// device's icon with its status dot and a ▾, which opens the panel. The
    /// slider is Ond's volume, or in bit-perfect the device's own when it has
    /// one (unusable when it doesn't).
    pub fn control_ui(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, bit_perfect: &mut bool, volume: f32, slider_w: Option<f32>) -> ControlOut {
        let mut out = ControlOut::default();
        let hw = self.device_volume.filter(|_| *bit_perfect);
        let readout = self.prefs.get(|s| s.device_volume_db);
        let width = self.control_width(slider_w);
        ui.allocate_ui_with_layout(egui::vec2(width, CONTROL_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            if let Some(w) = slider_w {
                let name = self.in_use_name().unwrap_or("the device").to_owned();
                let (resp, label) = match hw {
                    Some(v) => {
                        let mut pos = slider_pos(&v);
                        let resp = self.device_slider(ui, &mut pos, w, &name, &v);
                        if resp.changed() {
                            self.set_device_volume(slider_db(&v, pos), engine);
                        }
                        (resp, self.device_volume.map(|v| db_text(&v)))
                    }
                    None => {
                        let mut level = volume;
                        let resp = ui
                            .add_enabled_ui(!*bit_perfect, |ui| crate::app::thin_slider(ui, &mut level, w, "Volume"))
                            .inner
                            .on_disabled_hover_text(format!("Bit-perfect: Ond doesn't change the level. Use {name}'s own volume."));
                        if resp.changed() {
                            out.volume = Some(level);
                        }
                        (resp, (!*bit_perfect).then(|| format!("{:.0}%", level * 100.0)))
                    }
                };
                let adjusting = resp.hovered() || resp.dragged();
                let t = Tokens::of(ui);
                match (readout, label) {
                    // Its own room beside the slider, in every mode.
                    (DbReadout::Always, label) => {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(DB_W - GAP, 18.0), egui::Sense::hover());
                        if let Some(l) = label.filter(|_| hw.is_some()) {
                            ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, l, egui::FontId::proportional(11.0), t.muted);
                        }
                    }
                    // Floating under the knob while it's adjusted: takes no room.
                    (DbReadout::Adjusting, Some(l)) if adjusting && hw.is_some() => {
                        let layer = egui::LayerId::new(egui::Order::Tooltip, ui.scope_id().with("db-label"));
                        let painter = ui.ctx().layer_painter(layer);
                        let knob = resp.rect.left() + 6.0 + (resp.rect.width() - 12.0) * slider_pos(&hw.expect("matched"));
                        let at = egui::pos2(knob, resp.rect.bottom() + 3.0);
                        let g = painter.layout_no_wrap(l, egui::FontId::proportional(10.5), t.text);
                        let r = egui::Rect::from_center_size(at + egui::vec2(0.0, g.size().y / 2.0 + 2.0), g.size() + egui::vec2(10.0, 4.0));
                        painter.rect(r, 4.0, t.panel, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
                        painter.galley(r.center() - g.size() / 2.0, g, t.text);
                    }
                    _ => {}
                }
                self.adjusting = adjusting;
            }
            let icon = self.icon_button(ui);
            egui::Popup::menu(&icon)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .width(POPUP_W)
                .show(|ui| self.popup_ui(ui, engine, bit_perfect, volume, slider_w.is_none(), &mut out));
        });
        out.settings = std::mem::take(&mut self.wants_settings);
        out
    }

    /// The device's icon with the status dot at its corner (bit-perfect's
    /// verdict; none in Normal mode), and a ▾. Its id is fixed, so the panel
    /// stays open whatever moves beside it.
    fn icon_button(&self, ui: &mut egui::Ui) -> egui::Response {
        let t = Tokens::of(ui);
        let dot = self.dot_color(ui);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ICON_W, CONTROL_H), egui::Sense::hover());
        let resp = ui.interact(rect, egui::Id::unique("output-control-icon"), egui::Sense::click());
        if ui.is_rect_visible(rect) {
            if resp.hovered() {
                ui.painter().rect_filled(rect, 6.0, t.hover);
            }
            let glyph = egui::Rect::from_center_size(egui::pos2(rect.left() + 14.0, rect.center().y), egui::vec2(15.0, 15.0));
            icons::paint(ui.painter(), glyph, kind_icon(self.in_use().map(|d| d.kind)), t.text);
            if let Some(dot) = dot {
                ui.painter().circle(egui::pos2(glyph.right() + 1.0, glyph.top() + 1.0), 3.5, dot, egui::Stroke::new(1.5, t.bg));
            }
            let chevron = egui::Rect::from_center_size(egui::pos2(rect.right() - 8.0, rect.center().y + 1.0), egui::vec2(7.0, 7.0));
            icons::paint(ui.painter(), chevron, Icon::ChevronDown, t.muted);
        }
        let name = self.in_use_name().unwrap_or("System output").to_owned();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, true, format!("Output: {name}")));
        let hover = match &self.signal {
            Some(p) => format!("{name}\n{}", hover_text(p)),
            None => name,
        };
        resp.on_hover_text(hover)
    }

    /// The device volume's slider: dimmed while the device is muted (moving it unmutes).
    fn device_slider(&self, ui: &mut egui::Ui, pos: &mut f32, w: f32, name: &str, v: &DeviceVolume) -> egui::Response {
        let mark = self.mark(ui);
        ui.scope(|ui| {
            if v.muted == Some(true) {
                ui.multiply_opacity(0.4);
            }
            crate::app::thin_slider_marked(ui, pos, w, "Device volume", mark)
        })
        .inner
        .on_hover_text(device_volume_hover(name, v))
    }

    /// The slider knob's mark in bit-perfect mode: a green dot, unless turned off.
    fn mark(&self, ui: &egui::Ui) -> Option<egui::Color32> {
        self.prefs.get(|s| s.device_volume_green).then(|| Tokens::of(ui).ok)
    }

    /// The panel: System output or a chosen device, the device in use (with
    /// its mode and volume), the other devices (grouped, scrolling), and the
    /// way to Settings.
    /// `volume_here`: the toolbar has no slider (narrow), so Ond's volume is here.
    fn popup_ui(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, bit_perfect: &mut bool, volume: f32, volume_here: bool, out: &mut ControlOut) {
        let t = Tokens::of(ui);
        ui.set_width(POPUP_W);
        ui.spacing_mut().item_spacing.y = 0.0;
        let locked = self.device_locked();
        let following = self.following_system();
        ui.add_space(8.0);
        if locked {
            locked_note(ui);
        }
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.add_enabled_ui(!locked, |ui| {
                let mut system = following;
                let labels = |b: bool| if b { "System output".to_owned() } else { "Choose a device".to_owned() };
                if crate::app::choice(ui, &[true, false], &mut system, Some(POPUP_W - 20.0), labels) {
                    if system {
                        self.choose(None, engine);
                    } else {
                        self.choose_a_device(engine);
                    }
                }
            });
        });
        ui.add_space(10.0);
        if following {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Currently using").size(11.0).color(t.muted));
            });
            ui.add_space(4.0);
        }
        self.in_use_card(ui, engine, bit_perfect, volume, volume_here, out);
        ui.add_space(8.0);
        ui.separator();
        // Following the system, the system picks: no list (the tab above leaves it).
        if !following {
            let groups = self.menu_groups();
            egui::ScrollArea::vertical().max_height(LIST_H).auto_shrink([false, true]).show(ui, |ui| {
                for (name, devices) in groups {
                    if let Some(name) = name {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            ui.label(egui::RichText::new(name).size(11.0).color(t.muted));
                        });
                        ui.add_space(2.0);
                    }
                    for d in devices {
                        self.device_entry(ui, engine, &d, locked, bit_perfect);
                    }
                }
            });
            ui.add_space(4.0);
            ui.separator();
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            if crate::app::link_text(ui, "Playback settings…", egui::FontId::proportional(12.0), t.accent).clicked() {
                self.wants_settings = true;
                ui.close();
            }
        });
        ui.add_space(4.0);
    }

    /// The device in use, at a glance: its icon and name with its mode switch,
    /// what's playing through it, and the volume when it's the device's own
    /// (or the toolbar has none). Details are in the hovers.
    fn in_use_card(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, bit_perfect: &mut bool, volume: f32, volume_here: bool, out: &mut ControlOut) {
        let t = Tokens::of(ui);
        let following = self.following_system();
        if let Some((name, instead)) = self.waiting_for() {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                let text = match instead {
                    Some(i) => format!("Waiting for {name}: playing through {i}"),
                    None => format!("Waiting for {name}"),
                };
                ui.add(egui::Label::new(egui::RichText::new(text).size(11.0).color(t.muted)).wrap());
            });
            ui.add_space(6.0);
        }
        let Some(d) = self.in_use().cloned() else {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(egui::RichText::new("No output device").size(13.0).color(t.muted));
            });
            return;
        };
        // Its name, and what's playing through it at the right: the format
        // pill, its dot green when bit-perfect is verified.
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
        let g = egui::Rect::from_center_size(egui::pos2(rect.left() + 20.0, rect.center().y), egui::vec2(16.0, 16.0));
        icons::paint(ui.painter(), g, kind_icon(Some(d.kind)), t.text);
        let path = self.signal.clone();
        let pill_w = match &path {
            Some(p) => {
                let r = readout_of(p, String::new());
                let pill = paint_readout(ui, egui::pos2(rect.right() - PILL_INSET, rect.center().y), &r, 11.0, true);
                ui.interact(pill, egui::Id::unique("card-pill"), egui::Sense::hover()).on_hover_text(hover_text(p));
                pill.width()
            }
            None => 0.0,
        };
        let name_w = rect.width() - CARD_INDENT - pill_w - PILL_INSET - 8.0;
        let name = crate::app::elide_text(ui, &d.name, egui::FontId::new(13.0, crate::theme::semibold()), t.text, name_w);
        ui.painter().galley(egui::pos2(rect.left() + CARD_INDENT, rect.center().y - name.size().y / 2.0), name, t.text);
        // Only when it isn't what was asked for.
        if let Some((text, color)) = path.as_ref().and_then(|p| warning(p, &t)) {
            ui.horizontal(|ui| {
                ui.add_space(CARD_INDENT);
                ui.add(egui::Label::new(egui::RichText::new(text).size(11.0).color(color)).wrap());
            });
        }
        if d.can_bit_perfect() && !following {
            ui.add_space(2.0);
            let (row, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), egui::Sense::hover());
            self.mode_switch(ui, engine, &d, bit_perfect, row, CARD_INDENT);
        }
        if (*bit_perfect && !following) || volume_here {
            ui.add_space(6.0);
            self.volume_row(ui, engine, *bit_perfect, volume, out);
        }
    }

    /// The bit-perfect row for device `d`: "Bit-perfect" at `indent`, its
    /// switch (green when on) at the right; the label switches too.
    fn mode_switch(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, d: &DeviceInfo, bit_perfect: &mut bool, row: egui::Rect, indent: f32) {
        let t = Tokens::of(ui);
        let was = self.device_mode(d) == PlaybackMode::BitPerfect;
        let mut on = was;
        let locked = self.mode_locked(Some(&d.uid));
        let help = if locked { crate::managed::LOCKED.to_owned() } else { BIT_PERFECT_HELP.to_owned() };
        let label_rect = egui::Rect::from_min_max(egui::pos2(row.left() + indent, row.top()), egui::pos2(row.right() - SWITCH_W, row.bottom()));
        let label = ui.interact(label_rect, egui::Id::unique(("mode-label", &d.uid)), if locked { egui::Sense::hover() } else { egui::Sense::click() });
        ui.painter().text(egui::pos2(label_rect.left(), row.center().y), egui::Align2::LEFT_CENTER, "Bit-perfect", egui::FontId::proportional(12.0), if on { t.text } else { t.muted });
        let area = egui::Rect::from_min_max(egui::pos2(row.right() - SWITCH_W, row.top()), egui::pos2(row.right() - PILL_INSET, row.bottom()));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::right_to_left(egui::Align::Center)));
        let sw = child.add_enabled_ui(!locked, |ui| crate::app::switch_tinted(ui, &mut on, &format!("Bit-perfect on {}", d.name), t.ok)).inner;
        if label.on_hover_text(&help).clicked() {
            on = !on;
        }
        sw.on_hover_text(help);
        if on != was {
            let mode = if on { PlaybackMode::BitPerfect } else { PlaybackMode::Normal };
            if self.set_device_mode(&d.uid, mode, engine) {
                *bit_perfect = on;
            }
        }
    }

    /// One device in the list: its icon and name (what it can do on hover);
    /// a green dot when it's set to bit-perfect; a click plays through it. One
    /// that can play bit-perfect has a ▸ that opens its mode here, without switching.
    fn device_entry(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, d: &DeviceInfo, locked: bool, bit_perfect: &mut bool) {
        let t = Tokens::of(ui);
        let opens = d.can_bit_perfect();
        let open = opens && self.expanded.contains(&d.uid);
        let (rect, row) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), if locked { egui::Sense::hover() } else { egui::Sense::click() });
        let chevron = egui::Rect::from_min_size(rect.min, egui::vec2(26.0, ROW_H));
        let toggle = opens.then(|| ui.interact(chevron, egui::Id::unique(("device-open", &d.uid)), egui::Sense::click()));
        if ui.is_rect_visible(rect) {
            if row.hovered() || toggle.as_ref().is_some_and(egui::Response::hovered) {
                ui.painter().rect_filled(rect.shrink2(egui::vec2(4.0, 0.0)), 6.0, t.hover);
            }
            if opens {
                let c = egui::Rect::from_center_size(chevron.center(), egui::vec2(8.0, 8.0));
                icons::paint(ui.painter(), c, if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.muted);
            }
            let g = egui::Rect::from_center_size(egui::pos2(rect.left() + LIST_ICON_X, rect.center().y), egui::vec2(14.0, 14.0));
            icons::paint(ui.painter(), g, kind_icon(Some(d.kind)), t.muted);
            let x = rect.left() + LIST_INDENT;
            let name = crate::app::elide_text(ui, &d.name, egui::FontId::proportional(13.0), t.text, rect.right() - CARD_INDENT - x);
            ui.painter().galley(egui::pos2(x, rect.center().y - name.size().y / 2.0), name, t.text);
            // Set to bit-perfect: a green dot where the card's pill has its own.
            if opens && self.device_mode(d) == PlaybackMode::BitPerfect {
                ui.painter().circle_filled(egui::pos2(rect.right() - PILL_INSET - PILL_DOT, rect.center().y), 2.6, t.ok);
            }
        }
        let caps = d.capability_line();
        let caps = if opens && self.device_mode(d) == PlaybackMode::BitPerfect { format!("{caps} · set to bit-perfect") } else { caps };
        let hover = format!("Play through {}\n{caps}", d.name);
        let label = d.name.clone();
        let row = row.on_hover_text(hover);
        row.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, !locked, &label));
        if toggle.is_some_and(|r| r.on_hover_text(if open { "Hide its mode" } else { "Its mode" }).clicked()) {
            if open {
                self.expanded.remove(&d.uid);
            } else {
                self.expanded.insert(d.uid.clone());
            }
        } else if row.clicked() {
            self.expanded.remove(&d.uid);
            self.choose(Some(d.uid.clone()), engine);
        }
        if open {
            let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
            // The same switch as the device in use has; it doesn't switch devices.
            self.mode_switch(ui, engine, d, bit_perfect, r, LIST_INDENT);
            ui.add_space(4.0);
        }
    }

    /// The devices for the menu, grouped: hidden ones and the one in use
    /// left out; the user's groups between the unnamed hardware group and
    /// "Other" (software devices); each alphabetical.
    fn menu_groups(&self) -> Vec<(Option<String>, Vec<DeviceInfo>)> {
        let (hidden, named) = self.prefs.get(|s| (s.device_hidden.clone(), s.device_groups.clone()));
        let current = self.current.clone();
        group_devices(self.devices.iter().filter(|d| Some(&d.uid) != current.as_ref() && !hidden.contains(&d.uid)), &named)
    }

    /// The volume line of the card: in bit-perfect the device's own level over
    /// its whole range, with its exact dB (↑↓ step 0.5 dB, ⇧ 0.1; scrolling
    /// too) and mute; Ond's volume when the toolbar has no slider; or a word
    /// on using the device's own control when it has none Ond can set.
    fn volume_row(&mut self, ui: &mut egui::Ui, engine: &EngineHandle, bit_perfect: bool, volume: f32, out: &mut ControlOut) {
        let t = Tokens::of(ui);
        let name = self.in_use_name().unwrap_or("the device").to_owned();
        ui.horizontal(|ui| {
            ui.add_space(CARD_INDENT);
            ui.spacing_mut().item_spacing.x = 8.0;
            match self.device_volume.filter(|_| bit_perfect) {
                Some(v) => {
                    let mut pos = (v.db - v.min_db) / (v.max_db - v.min_db).max(0.01);
                    let w = (ui.available_width() - FIELD_MUTE_W).max(100.0);
                    let resp = self.device_slider(ui, &mut pos, w, &name, &v);
                    if resp.changed() {
                        self.set_device_volume(v.min_db + pos * (v.max_db - v.min_db), engine);
                    }
                    // The field's own arrows step by its drag speed: ours step like the old buttons.
                    let focused = self.db_field.is_some_and(|id| ui.memory(|m| m.has_focus(id)));
                    let step = if focused {
                        ui.input_mut(|i| {
                            let fine = i.count_and_consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp) as f32
                                - i.count_and_consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown) as f32;
                            let coarse = i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) as f32
                                - i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) as f32;
                            fine * FINE_STEP_DB + coarse * STEP_DB
                        })
                    } else {
                        0.0
                    };
                    // The level now: the slider above may just have moved it.
                    let now = self.device_volume.map_or(v.db, |v| v.db);
                    let mut db = now;
                    let field = ui.add(egui::DragValue::new(&mut db).range(v.min_db..=v.max_db).speed(0.1).fixed_decimals(1).suffix(" dB"));
                    self.db_field = Some(field.id);
                    let mut target = field.changed().then_some(db);
                    if step != 0.0 {
                        target = Some(now + step);
                    }
                    if field.hovered() {
                        self.scroll += ui.input(|i| i.smooth_scroll_delta.y);
                        let notches = (self.scroll / SCROLL_NOTCH).trunc();
                        if notches != 0.0 {
                            self.scroll -= notches * SCROLL_NOTCH;
                            target = Some(now + notches * STEP_DB);
                        }
                    }
                    if let Some(db) = target {
                        self.set_device_volume(db, engine);
                    }
                    field.on_hover_text("↑↓ 0.5 dB · ⇧↑↓ 0.1 dB · or scroll");
                    if let Some(muted) = self.device_volume.and_then(|v| v.muted) {
                        let (r, b) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                        let g = egui::Rect::from_center_size(r.center(), egui::vec2(13.0, 13.0));
                        icons::paint(ui.painter(), g, if muted { Icon::Muted } else { Icon::Volume }, if muted { t.accent } else { t.muted });
                        if b.on_hover_text(if muted { "Unmute" } else { "Mute" }).clicked() {
                            self.set_device_mute(!muted, engine);
                        }
                    }
                }
                None if bit_perfect => {
                    let why = format!("Ond doesn't change the level in bit-perfect mode. Use {name}'s own volume.");
                    ui.add(egui::Label::new(egui::RichText::new(why).size(11.0).color(t.muted)).wrap());
                }
                None => {
                    let mut level = volume;
                    let w = (ui.available_width() - PERCENT_W).max(100.0);
                    if crate::app::thin_slider(ui, &mut level, w, "Volume").changed() {
                        out.volume = Some(level);
                    }
                    ui.label(egui::RichText::new(format!("{:.0}%", level * 100.0)).size(11.0).color(t.muted));
                }
            }
        });
    }

    /// The status dot's color: bit-perfect verified, unavailable or not yet
    /// verified. `None` in Normal mode, which has nothing to report.
    fn dot_color(&self, ui: &egui::Ui) -> Option<egui::Color32> {
        let v = ui.visuals();
        match self.signal.as_ref().map(|p| &p.bit_perfect)? {
            BitPerfectStatus::Verified => Some(crate::theme::Tokens::of(ui).ok),
            BitPerfectStatus::Unavailable { .. } => Some(v.warn_fg_color),
            BitPerfectStatus::Requested { .. } => Some(v.text_color()),
            BitPerfectStatus::Off => None,
        }
    }
}

/// Above a part of the popup set in managed.json, which can't be changed here.
fn locked_note(ui: &mut egui::Ui) {
    let t = Tokens::of(ui);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(crate::managed::LOCKED).size(11.0).color(t.muted));
    });
    ui.add_space(4.0);
}
