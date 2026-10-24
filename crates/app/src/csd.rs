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

/// What the window plan said, kept with the egui context.
#[derive(Debug, Clone, Copy, Default)]
struct State {
    /// The controls are ours in this window mode.
    drawn: bool,
    /// The window has no system frame (so no resize borders either).
    frameless: bool,
    /// The UI snapshots show the Linux controls on any platform.
    preview: bool,
}

fn state(ctx: &Context) -> State {
    ctx.data(|d| d.get_temp::<State>(Id::unique("csd-state"))).unwrap_or_default()
}

fn store(ctx: &Context, s: State) {
    ctx.data_mut(|d| d.insert_temp(Id::unique("csd-state"), s));
}

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
pub fn set_window(ctx: &Context, controls: bool, frameless: bool) {
    let s = state(ctx);
    let on = SUPPORTED || s.preview;
    store(ctx, State { drawn: controls && on, frameless: frameless && on, ..s });
}

/// Lets the UI snapshots and tests show the Linux controls on any platform.
#[cfg(test)]
pub fn preview(ctx: &Context, on: bool) {
    store(ctx, State { preview: on, ..state(ctx) });
}

/// Whether we draw the controls now (not in full screen, where there are none).
pub fn active(ctx: &Context) -> bool {
    state(ctx).drawn && !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))
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
    let s = state(ctx);
    if !SUPPORTED && !s.preview {
        return;
    }
    let screen = ctx.content_rect();
    let (maximized, fullscreen) = ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
    let bar = ctx.data_mut(|d| d.remove_temp::<Option<Rect>>(bar_id())).flatten();
    // One foreground layer over the window. Its widgets take the pointer only
    // at their own exact rects (an `Area` remembers its size, and could grow
    // over the content).
    let layer = egui::LayerId::new(Order::Foreground, Id::unique("window-frame"));
    let mut ui = egui::Ui::new(ctx.clone(), Id::unique("window-frame"), egui::UiBuilder::new().layer_id(layer).max_rect(screen));
    if let Some(h) = fallback_h
        && active(ctx)
    {
        let layout = layout();
        let y = bar.map_or(screen.top() + h / 2.0, |b| b.center().y);
        let ui = &mut ui;
        // Right: the last button nearest the edge. Left: the first.
        for (i, &b) in layout.right.iter().rev().enumerate() {
            let x = screen.right() - EDGE - BUTTON / 2.0 - i as f32 * (BUTTON + GAP);
            button(ui, b, pos2(x, y), maximized);
        }
        for (i, &b) in layout.left.iter().enumerate() {
            let x = screen.left() + EDGE + BUTTON / 2.0 + i as f32 * (BUTTON + GAP);
            button(ui, b, pos2(x, y), maximized);
        }
    }
    if s.frameless && !maximized && !fullscreen {
        resize_bands(&mut ui, screen);
    }
}

fn button(ui: &mut egui::Ui, b: Button, center: Pos2, maximized: bool) {
    let rect = Rect::from_center_size(center, Vec2::splat(BUTTON));
    {
        let resp = ui.interact(rect, Id::unique(("window-button", b as u8)), Sense::click());
        let t = crate::theme::Tokens::of(ui);
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
    }
}

/// Thin bands just inside the window's edges that start a resize: without a
/// system frame there's nothing else to grab.
fn resize_bands(ui: &mut egui::Ui, screen: Rect) {
    use ResizeDirection as D;
    let (l, r, t, b) = (screen.left(), screen.right(), screen.top(), screen.bottom());
    // Each corner is an L: a short piece along each of its two edges.
    let h = |x0: f32, x1: f32, y0: f32, y1: f32| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
    let bands = [
        (D::North, h(l + CORNER, r - CORNER, t, t + BAND)),
        (D::South, h(l + CORNER, r - CORNER, b - BAND, b)),
        (D::West, h(l, l + BAND, t + CORNER, b - CORNER)),
        (D::East, h(r - BAND, r, t + CORNER, b - CORNER)),
        (D::NorthWest, h(l, l + CORNER, t, t + BAND)),
        (D::NorthWest, h(l, l + BAND, t, t + CORNER)),
        (D::NorthEast, h(r - CORNER, r, t, t + BAND)),
        (D::NorthEast, h(r - BAND, r, t, t + CORNER)),
        (D::SouthWest, h(l, l + CORNER, b - BAND, b)),
        (D::SouthWest, h(l, l + BAND, b - CORNER, b)),
        (D::SouthEast, h(r - CORNER, r, b - BAND, b)),
        (D::SouthEast, h(r - BAND, r, b - CORNER, b)),
    ];
    for (i, (dir, rect)) in bands.into_iter().enumerate() {
        let resp = ui.interact(rect, Id::unique(("resize-band", i)), Sense::drag());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(cursor(dir));
        }
        if resp.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
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
