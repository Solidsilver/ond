//! The first-run sheet, [`onboarding`]: a welcome line, then the kinds of
//! source the app can add as tiles. Picking one shows that kind's form,
//! which the app draws with the [`form`](super::form) kit; once a source is
//! added it's listed above the tiles with **Add another** and **Done**. The
//! same sheet opens from the switcher's Add source…, on the tiles.
//!
//! The sheet keeps no state of its own. The app holds where it is (an
//! [`OnboardingStep`]), describes the kinds ([`SourceKind`]) and the
//! sources added so far ([`SourceInfo`]), and gets back what the user did as
//! an [`OnboardingEvent`], which it turns into its own actions. Nothing here
//! knows what any kind of source is: the names, icons and blurbs are the
//! app's.
//!
//! Done is the only way out, and only once there's a source: until then the
//! sheet offers nothing else, and the app shows nothing beside it.

use super::form::form_button;
use super::sources::{SourceInfo, source_dot};
use super::text::elide;
use super::{focus_ring, quiet_button};
use crate::icons::{self, Icon};
use crate::theme::{Tokens, semibold};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};

/// A kind of source the app can add, as the app describes it.
#[derive(Debug, Clone, Copy)]
pub struct SourceKind<'a> {
    /// "Navidrome", "Local folder".
    pub name: &'a str,
    pub icon: Icon,
    /// A line on what it is, in the app's voice: "Your own server, at home
    /// or away."
    pub blurb: &'a str,
    /// False shows it as coming later, and it can't be picked.
    pub available: bool,
}

/// Where the sheet is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnboardingStep {
    /// The kinds as tiles, under whatever's been added.
    #[default]
    Kinds,
    /// The form for the kind at this index in [`Onboarding::kinds`].
    Form(usize),
    /// A source was just added: the list, with Add another and Done.
    Added,
}

/// What the sheet shows.
#[derive(Debug, Clone, Copy)]
pub struct Onboarding<'a> {
    /// The heading over the tiles: "Where's your music?"
    pub welcome: &'a str,
    pub kinds: &'a [SourceKind<'a>],
    /// The sources there are, in the order they were added. With none,
    /// there's no Done.
    pub added: &'a [SourceInfo<'a>],
    pub step: OnboardingStep,
    /// Offer Add another once a source is added.
    pub add_another: bool,
    /// Offer the way back from a form to the tiles. Off while the form waits
    /// on an answer, or when it was opened on its own (signing in again).
    pub back: bool,
    /// Roughly how tall the app's form is, so the column sits a little above
    /// the middle of the window whatever the form holds.
    pub form_height: f32,
}

/// What the user did on the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingEvent {
    /// Picked the kind at this index: show its form.
    Picked(usize),
    /// Back from a form to the tiles.
    Back,
    /// Add another: back to the tiles.
    AddAnother,
    /// Done: the sheet closes.
    Done,
}

/// The column's width, the sign-in form's.
pub const ONBOARDING_W: f32 = 320.0;
const TILE_H: f32 = 92.0;
const TILE_GAP: f32 = 8.0;
const ADDED_ROW_H: f32 = 42.0;
/// The heading, and the space under it.
const HEADING_H: f32 = 56.0;
const BACK_H: f32 = 34.0;

/// Draws the sheet in `ui`, which it fills (scrolling when the window is
/// short). `form` draws the picked kind's form in the column, with the kind
/// as the app described it. Returns what the user did this frame.
pub fn onboarding(ui: &mut Ui, sheet: &Onboarding<'_>, form: impl FnOnce(&mut Ui, &SourceKind<'_>)) -> Option<OnboardingEvent> {
    let step = match sheet.step {
        OnboardingStep::Form(i) if i < sheet.kinds.len() => OnboardingStep::Form(i),
        OnboardingStep::Added if !sheet.added.is_empty() => OnboardingStep::Added,
        _ => OnboardingStep::Kinds,
    };
    let height = content_height(sheet, step);
    let room = ui.available_height();
    // A short window keeps the buttons in view with a smaller heading.
    let compact = room < height + 160.0;
    let mut event = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(((room - height) * 0.42).max(if compact { 12.0 } else { 24.0 }));
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - ONBOARDING_W) / 2.0).max(0.0));
            ui.vertical(|ui| {
                ui.set_width(ONBOARDING_W);
                ui.spacing_mut().item_spacing.y = 4.0;
                match step {
                    OnboardingStep::Form(i) => {
                        let kind = &sheet.kinds[i];
                        if sheet.back {
                            if quiet_button(ui, Some(Icon::ChevronLeft), "Back").clicked() {
                                event = Some(OnboardingEvent::Back);
                            }
                            ui.add_space(4.0);
                        }
                        heading(ui, Some(kind.icon), kind.name, compact);
                        form(ui, kind);
                    }
                    OnboardingStep::Kinds => {
                        heading(ui, None, sheet.welcome, compact);
                        if !sheet.added.is_empty() {
                            added(ui, sheet.added);
                            ui.add_space(16.0);
                        }
                        if let Some(i) = tiles(ui, sheet.kinds) {
                            event = Some(OnboardingEvent::Picked(i));
                        }
                        if !sheet.added.is_empty() {
                            ui.add_space(16.0);
                            if form_button(ui, "Done", true, None).clicked() {
                                event = Some(OnboardingEvent::Done);
                            }
                        }
                    }
                    OnboardingStep::Added => {
                        heading(ui, None, sheet.welcome, compact);
                        added(ui, sheet.added);
                        ui.add_space(18.0);
                        if sheet.add_another {
                            if form_button(ui, "Add another", false, None).clicked() {
                                event = Some(OnboardingEvent::AddAnother);
                            }
                            ui.add_space(4.0);
                        }
                        let done = form_button(ui, "Done", true, None);
                        // Nothing on this step takes typing, so Enter is Done.
                        if done.clicked() || (ui.memory(|m| m.focused().is_none()) && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                            event = Some(OnboardingEvent::Done);
                        }
                    }
                }
            });
        });
        ui.add_space(24.0);
    });
    event
}

/// Roughly how tall the column is on `step`.
fn content_height(sheet: &Onboarding<'_>, step: OnboardingStep) -> f32 {
    let list = |n: usize| n as f32 * ADDED_ROW_H + 2.0;
    let button = 38.0;
    HEADING_H
        + match step {
            OnboardingStep::Form(_) => sheet.form_height + if sheet.back { BACK_H } else { 0.0 },
            OnboardingStep::Kinds => {
                let tiles = sheet.kinds.len().div_ceil(2) as f32 * (TILE_H + TILE_GAP);
                if sheet.added.is_empty() { tiles } else { list(sheet.added.len()) + 16.0 + tiles + 16.0 + button }
            }
            OnboardingStep::Added => list(sheet.added.len()) + 18.0 + if sheet.add_another { 2.0 * button } else { button },
        }
}

/// The sheet's heading, centred: the welcome, or a kind's icon and name
/// over its form. A short window leaves the icon out, so the form's buttons
/// stay in view.
fn heading(ui: &mut Ui, icon: Option<Icon>, text: &str, compact: bool) {
    let t = Tokens::of(ui);
    ui.vertical_centered(|ui| {
        if let Some(icon) = icon.filter(|_| !compact) {
            let (rect, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
            icons::paint(ui.painter(), rect, icon, t.accent);
            ui.add_space(6.0);
        }
        ui.label(egui::RichText::new(text).font(FontId::new(if compact { 18.0 } else { 22.0 }, semibold())).color(t.text));
        ui.add_space(if compact { 14.0 } else { 22.0 });
    });
}

/// The kinds, two to a row (one alone spans the row). The index of the one
/// picked, if any; a kind that isn't available can't be.
fn tiles(ui: &mut Ui, kinds: &[SourceKind<'_>]) -> Option<usize> {
    let mut picked = None;
    let half = (ONBOARDING_W - TILE_GAP) / 2.0;
    for (row, pair) in kinds.chunks(2).enumerate() {
        if row > 0 {
            ui.add_space(TILE_GAP - ui.spacing().item_spacing.y);
        }
        let (rect, _) = ui.allocate_exact_size(vec2(ONBOARDING_W, TILE_H), Sense::hover());
        for (col, kind) in pair.iter().enumerate() {
            let width = if pair.len() == 1 { ONBOARDING_W } else { half };
            let tile_rect = Rect::from_min_size(pos2(rect.left() + col as f32 * (half + TILE_GAP), rect.top()), vec2(width, TILE_H));
            if tile(ui, tile_rect, kind) {
                picked = Some(row * 2 + col);
            }
        }
    }
    picked
}

/// One kind: its icon, its name and its blurb, or "Coming later" dimmed.
/// True when picked.
fn tile(ui: &mut Ui, rect: Rect, kind: &SourceKind<'_>) -> bool {
    let t = Tokens::of(ui);
    let id = ui.scope_id().with(("onboarding-kind", kind.name));
    let resp = ui.interact(rect, id, if kind.available { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let live = kind.available && resp.hovered();
        let p = ui.painter();
        // One that can't be picked yet is only outlined, so the ones that
        // can stand out.
        if kind.available {
            let fill = if resp.is_pointer_button_down_on() {
                t.selected
            } else if live {
                t.hover
            } else {
                t.field
            };
            p.rect_filled(rect, 10.0, fill);
        } else {
            p.rect_stroke(rect, 10.0, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
        }
        focus_ring(ui, &resp, rect, 10.0);
        let (icon_ink, name_ink) = if kind.available { (t.accent, t.text) } else { (t.muted, t.muted) };
        let x = rect.left() + 14.0;
        icons::paint(p, Rect::from_min_size(pos2(x, rect.top() + 14.0), vec2(18.0, 18.0)), kind.icon, icon_ink);
        let room = rect.width() - 28.0;
        let name = elide(ui, kind.name, FontId::new(13.0, semibold()), name_ink, room);
        let name_h = name.size().y;
        p.galley(pos2(x, rect.top() + 42.0), name, name_ink);
        let below = if kind.available { kind.blurb } else { "Coming later" };
        let mut job = egui::text::LayoutJob::simple(below.to_owned(), FontId::proportional(11.0), t.muted, room);
        job.wrap.max_rows = 2;
        let blurb = ui.fonts_mut(|f| f.layout_job(job));
        p.galley(pos2(x, rect.top() + 44.0 + name_h), blurb, t.muted);
    }
    let label = if kind.available { format!("{}, {}", kind.name, kind.blurb) } else { format!("{}, coming later", kind.name) };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, kind.available, &label));
    let resp = if kind.available { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
    kind.available && resp.clicked()
}

/// The sources added so far, in an outlined group: each one's dot, its
/// name over its kind, and a check.
fn added(ui: &mut Ui, sources: &[SourceInfo<'_>]) {
    let t = Tokens::of(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ONBOARDING_W, sources.len() as f32 * ADDED_ROW_H + 2.0), Sense::hover());
    let p = ui.painter();
    p.rect_stroke(rect, 10.0, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
    for (i, s) in sources.iter().enumerate() {
        let row = Rect::from_min_size(pos2(rect.left(), rect.top() + 1.0 + i as f32 * ADDED_ROW_H), vec2(rect.width(), ADDED_ROW_H));
        if i > 0 {
            p.hline(row.left() + 14.0..=row.right() - 14.0, row.top(), egui::Stroke::new(1.0, t.hairline));
        }
        source_dot(ui, pos2(row.left() + 18.0, row.center().y), s.color);
        let x = row.left() + 32.0;
        let room = row.width() - 32.0 - 40.0;
        let name = elide(ui, s.name, FontId::proportional(13.0), t.text, room);
        let kind = elide(ui, s.kind, FontId::proportional(11.0), t.muted, room);
        let top = row.center().y - (name.size().y + kind.size().y) / 2.0;
        let kind_y = top + name.size().y;
        p.galley(pos2(x, top), name, t.text);
        p.galley(pos2(x, kind_y), kind, t.muted);
        icons::paint(p, Rect::from_center_size(pos2(row.right() - 20.0, row.center().y), vec2(12.0, 12.0)), Icon::Check, t.ok);
        let label = format!("{}, {}, added", s.name, s.kind);
        let resp = ui.interact(row, ui.scope_id().with(("onboarding-added", s.id)), Sense::hover());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &label));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;
    use accounts::{AccountId, SourceColor, SourceState};
    use egui_kittest::kittest::{NodeT, Queryable};

    const ONLINE: SourceState = SourceState::Online;

    const KINDS: [SourceKind<'static>; 4] = [
        SourceKind { name: "Server", icon: Icon::Server, blurb: "Your own server, at home or away.", available: true },
        SourceKind { name: "Local folder", icon: Icon::Folder, blurb: "Music on this computer.", available: false },
        SourceKind { name: "Cloud", icon: Icon::Server, blurb: "", available: false },
        SourceKind { name: "Other", icon: Icon::Server, blurb: "", available: false },
    ];

    fn home() -> SourceInfo<'static> {
        SourceInfo {
            id: "00000000-0000-4000-8000-000000000001".parse::<AccountId>().unwrap(),
            name: "Home server",
            kind: "Server",
            icon: Icon::Server,
            color: SourceColor::new(0),
            state: &ONLINE,
            detail: None,
        }
    }

    /// What a test's app holds: where the sheet is, whether a source has
    /// been added, and every event it got.
    #[derive(Default)]
    struct App {
        step: OnboardingStep,
        added: bool,
        add_another: bool,
        events: Vec<OnboardingEvent>,
    }

    /// The sheet as an app would drive it: events move the step, as an
    /// app's actions would, and the form is one button that adds a source.
    fn harness(app: App) -> egui_kittest::Harness<'static, (bool, App)> {
        super::super::themed(app, |ui, app: &mut App| {
            let added = [home()];
            let sheet = Onboarding {
                welcome: "Where's your music?",
                kinds: &KINDS,
                added: if app.added { &added } else { &[] },
                step: app.step,
                add_another: app.add_another,
                back: true,
                form_height: 120.0,
            };
            let mut signed_in = false;
            let event = onboarding(ui, &sheet, |ui, kind| {
                signed_in = form_button(ui, &format!("Connect to {}", kind.name), true, None).clicked();
            });
            if signed_in {
                app.added = true;
                app.step = OnboardingStep::Added;
            }
            if let Some(event) = event {
                app.events.push(event);
                app.step = match event {
                    OnboardingEvent::Picked(i) => OnboardingStep::Form(i),
                    OnboardingEvent::Back | OnboardingEvent::AddAnother | OnboardingEvent::Done => OnboardingStep::Kinds,
                };
            }
        })
    }

    #[test]
    fn a_kind_that_is_not_available_cannot_be_picked() {
        let mut h = harness(App::default());
        h.run_steps(2);
        let later = h.get_by_label("Local folder, coming later");
        assert!(later.accesskit_node().is_disabled(), "it says it can't be used");
        later.click();
        h.run_steps(2);
        assert!(h.state().1.events.is_empty() && h.state().1.step == OnboardingStep::Kinds);
        h.get_by_label("Server, Your own server, at home or away.").click();
        h.run_steps(2);
        assert_eq!(h.state().1.events, [OnboardingEvent::Picked(0)]);
        h.get_by_label("Connect to Server");
    }

    #[test]
    fn done_is_absent_until_a_source_is_added() {
        let mut h = harness(App { add_another: true, ..App::default() });
        h.run_steps(2);
        assert!(h.query_by_label("Done").is_none() && h.query_by_label("Add another").is_none());
        h.get_by_label_contains("Server, ").click();
        h.run_steps(2);
        assert!(h.query_by_label("Done").is_none(), "nor on the form");
        h.get_by_label("Connect to Server").click();
        h.run_steps(2);
        h.get_by_label("Home server, Server, added");
        h.get_by_label("Add another").click();
        h.run_steps(2);
        // Back on the tiles, with the source above them and Done under them.
        h.get_by_label("Home server, Server, added");
        h.get_by_label_contains("Local folder");
        h.get_by_label("Done").click();
        h.run_steps(2);
        assert_eq!(h.state().1.events.last(), Some(&OnboardingEvent::Done));
    }

    #[test]
    fn add_another_is_offered_only_when_the_app_says() {
        let mut h = harness(App { step: OnboardingStep::Added, added: true, ..App::default() });
        h.run_steps(2);
        assert!(h.query_by_label("Add another").is_none());
        h.get_by_label("Done");
    }

    #[test]
    fn back_returns_to_the_tiles() {
        let mut h = harness(App { step: OnboardingStep::Form(0), ..App::default() });
        h.run_steps(2);
        assert!(h.query_by_label_contains("Local folder").is_none(), "the form stands alone");
        h.get_by_label("Back").click();
        h.run_steps(2);
        assert_eq!(h.state().1.events, [OnboardingEvent::Back]);
        h.get_by_label_contains("Local folder");
    }

    #[test]
    fn a_form_opened_on_its_own_has_no_way_back() {
        let mut h = super::super::themed((), |ui, _| {
            let sheet = Onboarding { welcome: "", kinds: &KINDS, added: &[], step: OnboardingStep::Form(0), add_another: false, back: false, form_height: 0.0 };
            onboarding(ui, &sheet, |_, _| {});
        });
        h.run_steps(2);
        assert!(h.query_by_label("Back").is_none());
    }

    /// Renders the sheet with no sources, one added, the form open, one
    /// kind alone, and the pointer on a kind that isn't available, to look
    /// at: `cargo test -p shell --release snapshots -- --ignored` writes
    /// `target/snapshots/kit-onboarding-*.png`.
    #[test]
    #[ignore = "writes pictures to look at"]
    fn snapshots() {
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
        std::fs::create_dir_all(&out).unwrap();
        type Scene = (&'static str, OnboardingStep, bool, &'static [SourceKind<'static>]);
        let scenes: [Scene; 5] = [
            ("kinds", OnboardingStep::Kinds, false, &KINDS),
            ("added", OnboardingStep::Added, true, &KINDS),
            ("added-tiles", OnboardingStep::Kinds, true, &KINDS),
            ("form", OnboardingStep::Form(0), false, &KINDS),
            ("one-kind", OnboardingStep::Kinds, false, &KINDS[..1]),
        ];
        for (theme, pref) in [("light", theme::ThemePref::Light), ("dark", theme::ThemePref::Dark)] {
            for &(name, step, added, kinds) in &scenes {
                let mut h = canvas(pref, move |ui| {
                    let sources = [home()];
                    let sheet = Onboarding {
                        welcome: "Where's your music?",
                        kinds,
                        added: if added { &sources } else { &[] },
                        step,
                        add_another: true,
                        back: true,
                        form_height: 150.0,
                    };
                    onboarding(ui, &sheet, |ui, _| {
                        let mut address = String::new();
                        super::super::TextField::new("Address").hint("music.example.com").show(ui, &mut address);
                        ui.add_space(18.0);
                        form_button(ui, "Connect", true, None);
                    });
                });
                h.run();
                h.render().unwrap().save(out.join(format!("kit-onboarding-{name}-{theme}.png"))).unwrap();
            }
            // The pointer on a kind that can't be picked yet: it doesn't
            // light up.
            let mut h = canvas(pref, |ui| {
                let sheet = Onboarding {
                    welcome: "Where's your music?",
                    kinds: &KINDS,
                    added: &[],
                    step: OnboardingStep::Kinds,
                    add_another: false,
                    back: true,
                    form_height: 0.0,
                };
                onboarding(ui, &sheet, |_, _| {});
            });
            h.run();
            let at = h.get_by_label("Local folder, coming later").rect().center();
            h.hover_at(at);
            h.run();
            h.render().unwrap().save(out.join(format!("kit-onboarding-unavailable-{theme}.png"))).unwrap();
        }
    }

    /// A window-sized harness on the window's ground, in `pref`'s theme,
    /// that draws `body` once the theme's fonts are in.
    fn canvas(pref: theme::ThemePref, mut body: impl FnMut(&mut Ui) + 'static) -> egui_kittest::Harness<'static, bool> {
        let dark = matches!(pref, theme::ThemePref::Dark);
        egui_kittest::Harness::builder()
            .with_size(vec2(520.0, 560.0))
            .with_pixels_per_point(2.0)
            .with_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light })
            .build_ui_state(
                move |ui, installed: &mut bool| {
                    if !std::mem::replace(installed, true) {
                        theme::install(ui.ctx(), pref);
                        ui.ctx().request_repaint();
                        return;
                    }
                    let ground = Tokens::of(ui).bg;
                    ui.ctx().layer_painter(egui::LayerId::background()).rect_filled(Rect::EVERYTHING, 0.0, ground);
                    body(ui);
                },
                false,
            )
    }
}
