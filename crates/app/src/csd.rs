//! Window controls in our own bar (Linux), the way GNOME apps and Firefox do
//! it: the system title bar is off, the toolbar is the title bar, and the
//! close (and, where the desktop asks for them, minimize and maximize) buttons
//! sit at its end. Which buttons, and on which side, follows the desktop's own
//! setting (GNOME's `button-layout`, read through the settings portal).
//!
//! Without a system frame the compositor gives no resize borders either, so
//! [`overlay`] also puts thin resize bands along the window's edges.
//!
//! macOS folds its own title bar into ours (see `chrome`); Windows keeps the
//! system title bar for now. "Use the system title bar" in Settings turns all
//! of this off.

use egui::{Context, CursorIcon, Id, Order, Pos2, Rect, ResizeDirection, Sense, Stroke, Vec2, ViewportCommand, pos2, vec2};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};

/// Whether this platform draws its own window controls.
pub const SUPPORTED: bool = cfg!(all(unix, not(target_os = "macos")));

/// Diameter of a button (Adwaita's).
const BUTTON: f32 = 24.0;
const GAP: f32 = 8.0;
/// From the window edge to the outermost button.
const EDGE: f32 = 12.0;
/// Between our own widgets and the buttons.
const SEPARATION: f32 = 10.0;
/// How far into the window the resize bands reach, and how long the corners are.
const BAND: f32 = 5.0;
const CORNER: f32 = 16.0;

/// Set from the window plan: the controls are ours in this window mode.
static DRAWN: AtomicBool = AtomicBool::new(false);
/// Set from the window plan: the window has no system frame (so no resize borders).
static FRAMELESS: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Maximize,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// The buttons on each side, in order from left to right.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Layout {
    pub left: Vec<Button>,
    pub right: Vec<Button>,
}

impl Layout {
    /// GNOME's format: `"appmenu:minimize,maximize,close"`, left of the colon
    /// on the left. Names we don't draw (appmenu, icon, spacer) are skipped.
    pub fn parse(s: &str) -> Self {
        let side = |part: &str| {
            part.split(',')
                .filter_map(|name| match name.trim() {
                    "minimize" => Some(Button::Minimize),
                    "maximize" => Some(Button::Maximize),
                    "close" => Some(Button::Close),
                    _ => None,
                })
                .collect()
        };
        let (left, right) = s.split_once(':').unwrap_or(("", s));
        Self { left: side(left), right: side(right) }
    }

    fn side(&self, side: Side) -> &[Button] {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }
}

/// Called whenever the window plan changes: whether we draw the buttons, and
/// whether the window has a system frame at all.
pub fn set_window(controls: bool, frameless: bool) {
    DRAWN.store(controls && supported(), Ordering::Relaxed);
    FRAMELESS.store(frameless && supported(), Ordering::Relaxed);
}

#[cfg(test)]
static PREVIEW: AtomicBool = AtomicBool::new(false);

/// Lets the UI snapshots show the Linux controls on any platform.
#[cfg(test)]
pub fn preview(on: bool) {
    PREVIEW.store(on, Ordering::Relaxed);
    set_window(on, on);
}

fn supported() -> bool {
    #[cfg(test)]
    if PREVIEW.load(Ordering::Relaxed) {
        return true;
    }
    SUPPORTED
}

/// Whether we draw the controls now (not in full screen, where there are none).
pub fn active(ctx: &Context) -> bool {
    DRAWN.load(Ordering::Relaxed) && !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))
}

/// The room the buttons take on `side` of the bar, 0 when we draw none there.
pub fn reserved(ctx: &Context, side: Side) -> f32 {
    if !active(ctx) {
        return 0.0;
    }
    let n = layout().side(side).len() as f32;
    if n == 0.0 { 0.0 } else { EDGE + n * BUTTON + (n - 1.0) * GAP + SEPARATION }
}

/// The desktop's button layout. Read once, in the background; until it
/// arrives (or when there's no portal) a guess from the desktop's name.
pub fn layout() -> Layout {
    static READ: OnceLock<RwLock<Option<Layout>>> = OnceLock::new();
    let cell = READ.get_or_init(|| {
        #[cfg(all(unix, not(target_os = "macos")))]
        let _ = std::thread::Builder::new().name("button-layout".into()).spawn(|| {
            if let Some(s) = portal::button_layout()
                && let Some(cell) = READ.get()
            {
                *cell.write().unwrap_or_else(|e| e.into_inner()) = Some(Layout::parse(&s));
            }
        });
        RwLock::new(None)
    });
    cell.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(fallback)
}

/// GNOME shows only close; KDE and most others all three.
fn fallback() -> Layout {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_lowercase();
    Layout::parse(if desktop.contains("gnome") { "appmenu:close" } else { ":minimize,maximize,close" })
}

/// Where this frame's title bar is: the toolbar or the Player's top bar
/// records itself, and [`overlay`] centres the buttons in it.
pub fn set_bar(ctx: &Context, bar: Rect) {
    ctx.data_mut(|d| d.insert_temp(bar_id(), Some(bar)));
}

fn bar_id() -> Id {
    Id::unique("csd-title-bar")
}

/// Draws the buttons, centred in this frame's title bar (see [`set_bar`]),
/// or in a bar `fallback_h` tall along the top when no bar recorded itself;
/// `None` draws no buttons (Mini). Then the resize bands. Call after the
/// window's content, so both take the pointer from whatever is underneath.
pub fn overlay(ctx: &Context, fallback_h: Option<f32>) {
    if !supported() {
        return;
    }
    let screen = ctx.content_rect();
    let (maximized, fullscreen) = ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
    let bar = ctx.data_mut(|d| d.remove_temp::<Option<Rect>>(bar_id())).flatten();
    if let Some(h) = fallback_h
        && active(ctx)
    {
        let layout = layout();
        let y = bar.map_or(screen.top() + h / 2.0, |b| b.center().y);
        // Right: the last button nearest the edge. Left: the first.
        for (i, &b) in layout.right.iter().rev().enumerate() {
            let x = screen.right() - EDGE - BUTTON / 2.0 - i as f32 * (BUTTON + GAP);
            button(ctx, b, pos2(x, y), maximized);
        }
        for (i, &b) in layout.left.iter().enumerate() {
            let x = screen.left() + EDGE + BUTTON / 2.0 + i as f32 * (BUTTON + GAP);
            button(ctx, b, pos2(x, y), maximized);
        }
    }
    if FRAMELESS.load(Ordering::Relaxed) && !maximized && !fullscreen {
        resize_bands(ctx, screen);
    }
}

fn button(ctx: &Context, b: Button, center: Pos2, maximized: bool) {
    let rect = Rect::from_center_size(center, Vec2::splat(BUTTON));
    egui::Area::new(Id::unique(("window-button", b as u8))).order(Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
        let t = crate::theme::Tokens::of(ui);
        let (rect, resp) = ui.allocate_exact_size(rect.size(), Sense::click());
        let fill = if resp.is_pointer_button_down_on() {
            t.hairline
        } else if resp.hovered() {
            t.selected
        } else {
            t.hover
        };
        let p = ui.painter();
        p.circle_filled(rect.center(), BUTTON / 2.0, fill);
        let stroke = Stroke::new(1.4, t.text);
        let c = rect.center();
        let s = 4.0;
        match b {
            Button::Close => {
                p.line_segment([c + vec2(-s, -s), c + vec2(s, s)], stroke);
                p.line_segment([c + vec2(-s, s), c + vec2(s, -s)], stroke);
            }
            Button::Minimize => {
                p.line_segment([c + vec2(-s, s), c + vec2(s, s)], stroke);
            }
            Button::Maximize if maximized => {
                // Restore: two overlapping squares.
                let back = Rect::from_min_size(c + vec2(-s + 2.0, -s - 1.0), Vec2::splat(2.0 * s - 1.0));
                p.line_segment([back.left_top(), back.right_top()], stroke);
                p.line_segment([back.right_top(), back.right_bottom()], stroke);
                p.rect_stroke(Rect::from_min_size(c + vec2(-s - 1.0, -s + 2.0), Vec2::splat(2.0 * s - 1.0)), 1.0, stroke, egui::StrokeKind::Middle);
            }
            Button::Maximize => {
                p.rect_stroke(Rect::from_center_size(c, Vec2::splat(2.0 * s)), 1.0, stroke, egui::StrokeKind::Middle);
            }
        }
        let label = match b {
            Button::Close => "Close",
            Button::Minimize => "Minimize",
            Button::Maximize if maximized => "Restore",
            Button::Maximize => "Maximize",
        };
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
        if resp.clicked() {
            ui.ctx().send_viewport_cmd(match b {
                Button::Close => ViewportCommand::Close,
                Button::Minimize => ViewportCommand::Minimized(true),
                Button::Maximize => ViewportCommand::Maximized(!maximized),
            });
        }
    });
}

/// Thin bands just inside the window's edges that start a resize: without a
/// system frame there's nothing else to grab.
fn resize_bands(ctx: &Context, screen: Rect) {
    use ResizeDirection as D;
    let (l, r, t, b) = (screen.left(), screen.right(), screen.top(), screen.bottom());
    let bands = [
        (D::North, Rect::from_min_max(pos2(l + CORNER, t), pos2(r - CORNER, t + BAND))),
        (D::South, Rect::from_min_max(pos2(l + CORNER, b - BAND), pos2(r - CORNER, b))),
        (D::West, Rect::from_min_max(pos2(l, t + CORNER), pos2(l + BAND, b - CORNER))),
        (D::East, Rect::from_min_max(pos2(r - BAND, t + CORNER), pos2(r, b - CORNER))),
        (D::NorthWest, Rect::from_min_max(pos2(l, t), pos2(l + CORNER, t + BAND)).union(Rect::from_min_max(pos2(l, t), pos2(l + BAND, t + CORNER)))),
        (D::NorthEast, Rect::from_min_max(pos2(r - CORNER, t), pos2(r, t + BAND)).union(Rect::from_min_max(pos2(r - BAND, t), pos2(r, t + CORNER)))),
        (D::SouthWest, Rect::from_min_max(pos2(l, b - BAND), pos2(l + CORNER, b)).union(Rect::from_min_max(pos2(l, b - CORNER), pos2(l + BAND, b)))),
        (D::SouthEast, Rect::from_min_max(pos2(r - CORNER, b - BAND), pos2(r, b)).union(Rect::from_min_max(pos2(r - BAND, b - CORNER), pos2(r, b)))),
    ];
    for (dir, rect) in bands {
        let corner = matches!(dir, D::NorthWest | D::NorthEast | D::SouthWest | D::SouthEast);
        egui::Area::new(Id::unique(("resize-band", dir as u8))).order(Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
            // A corner is an L; its bounding square only takes the pointer along the edges.
            let resp = ui.allocate_rect(rect, Sense::drag());
            let pos = ui.ctx().pointer_hover_pos();
            let on_band = !corner || pos.is_some_and(|p| (p.x - l).min(r - p.x) <= BAND || (p.y - t).min(b - p.y) <= BAND);
            if resp.hovered() && on_band {
                ui.ctx().set_cursor_icon(cursor(dir));
            }
            if resp.drag_started() && on_band {
                ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(dir));
            }
        });
    }
}

fn cursor(dir: ResizeDirection) -> CursorIcon {
    match dir {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}

/// The XDG settings portal, which GNOME, KDE and the other desktops answer.
#[cfg(all(unix, not(target_os = "macos")))]
mod portal {
    pub fn button_layout() -> Option<String> {
        let conn = zbus::blocking::Connection::session().ok()?;
        let reply = conn
            .call_method(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                Some("org.freedesktop.portal.Settings"),
                "ReadOne",
                &("org.gnome.desktop.wm.preferences", "button-layout"),
            )
            .ok()?;
        let value: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
        String::try_from(value).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gnome_button_layouts() {
        assert_eq!(Layout::parse("appmenu:close"), Layout { left: vec![], right: vec![Button::Close] });
        assert_eq!(
            Layout::parse("appmenu:minimize,maximize,close"),
            Layout { left: vec![], right: vec![Button::Minimize, Button::Maximize, Button::Close] }
        );
        // macOS-style, on the left.
        assert_eq!(Layout::parse("close,minimize,maximize:"), Layout { left: vec![Button::Close, Button::Minimize, Button::Maximize], right: vec![] });
        assert_eq!(Layout::parse("icon:spacer,close"), Layout { left: vec![], right: vec![Button::Close] });
        assert_eq!(Layout::parse("close"), Layout { left: vec![], right: vec![Button::Close] });
    }
}
