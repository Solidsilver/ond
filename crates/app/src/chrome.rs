//! The window's title bar, folded into our own toolbar (macOS). The window
//! keeps its native frame (rounded corners, shadow, resizing, full screen) but
//! its title bar is transparent and the content runs underneath it; the
//! traffic lights are moved to sit centred in our bar, or hidden in Mini.
//!
//! AppKit puts the buttons back whenever it lays the title bar out again
//! (resizes, key-window changes), so [`sync`] re-applies the layout
//! each frame; it's a handful of cheap property calls. It also ends the
//! presentation mode egui-wgpu keeps on after a resize the user didn't drag
//! (see [`mac::stop_transaction_presents`]).

/// How far the content must stay clear of the traffic lights, from the window's left edge.
pub const LIGHTS_W: f32 = 84.0;
/// The first button's left edge and the distance between buttons.
#[cfg(target_os = "macos")]
const LIGHTS_X: f64 = 18.0;
#[cfg(target_os = "macos")]
const LIGHTS_STEP: f64 = 20.0;

/// What the title bar should look like this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    /// Height of our bar the traffic lights centre in; `None` hides them (Mini).
    pub height: Option<f32>,
}

/// Whether the title bar is ours to draw in (macOS, not in full screen).
pub fn folded(ctx: &egui::Context) -> bool {
    cfg!(target_os = "macos") && !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))
}

/// How long the window's size must hold still before a resize counts as over.
#[cfg(target_os = "macos")]
const SETTLED: std::time::Duration = std::time::Duration::from_millis(120);

/// Re-applies `bar` to the main window. Called every frame: AppKit undoes it.
pub fn sync(ctx: &egui::Context, bar: Bar) {
    #[cfg(target_os = "macos")]
    settle_resize(ctx);
    if !folded(ctx) {
        return;
    }
    #[cfg(target_os = "macos")]
    mac::apply(bar);
    #[cfg(not(target_os = "macos"))]
    let _ = bar;
}

/// A double-click on our bar does what a double-click on a title bar does
/// (System Settings › Desktop & Dock: zoom, minimise, or nothing).
pub fn title_double_click(ctx: &egui::Context) {
    #[cfg(target_os = "macos")]
    if mac::title_double_click() {
        return;
    }
    let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
}

/// The window's size as last seen, and when it last changed (until that
/// resize is over).
#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct Resizing {
    size: egui::Vec2,
    changed: Option<std::time::Instant>,
}

/// Once the window's size has held still for [`SETTLED`] and the user isn't
/// dragging an edge, ends egui-wgpu's live-resize presentation and draws once
/// more, so the last frame of a zoom or a mode switch shows at its size.
#[cfg(target_os = "macos")]
fn settle_resize(ctx: &egui::Context) {
    let Some(size) = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size())) else { return };
    let id = egui::Id::unique("chrome-resizing");
    let mut seen: Resizing = ctx.data(|d| d.get_temp(id)).unwrap_or(Resizing { size, changed: None });
    if seen.size != size {
        seen = Resizing { size, changed: Some(std::time::Instant::now()) };
        ctx.request_repaint_after(SETTLED);
    } else if let Some(at) = seen.changed {
        let still = at.elapsed();
        if still < SETTLED {
            ctx.request_repaint_after(SETTLED - still);
        } else {
            seen.changed = None;
            // A drag that pauses isn't over; letting go ends it (egui-wgpu
            // sees the mouse-up).
            if !mac::live_resizing() {
                mac::stop_transaction_presents();
                ctx.request_repaint();
            }
        }
    }
    ctx.data_mut(|d| d.insert_temp(id, seen));
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{Bar, LIGHTS_STEP, LIGHTS_X};
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::msg_send;
    use objc2_app_kit::{NSApplication, NSWindow, NSWindowButton, NSWindowStyleMask, NSWindowTitleVisibility};
    use objc2_core_foundation::{CGPoint, CGRect};

    fn window() -> Option<Retained<NSWindow>> {
        let mtm = MainThreadMarker::new()?;
        let app = NSApplication::sharedApplication(mtm);
        app.mainWindow().or_else(|| app.keyWindow()).or_else(|| app.windows().firstObject())
    }

    pub fn apply(bar: Bar) {
        let Some(window) = window() else { return };
        // Transparent title bar over full-size content: set once at startup by
        // the viewport builder, again here in case anything reset it.
        let mask = window.styleMask();
        if !mask.contains(NSWindowStyleMask::FullSizeContentView) {
            window.setStyleMask(mask | NSWindowStyleMask::FullSizeContentView);
        }
        if !window.titlebarAppearsTransparent() {
            window.setTitlebarAppearsTransparent(true);
        }
        if window.titleVisibility() != NSWindowTitleVisibility::Hidden {
            window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        }
        let buttons: Vec<Retained<objc2_app_kit::NSButton>> =
            [NSWindowButton::CloseButton, NSWindowButton::MiniaturizeButton, NSWindowButton::ZoomButton].into_iter().filter_map(|k| window.standardWindowButton(k)).collect();
        let hide = bar.height.is_none();
        for b in &buttons {
            if b.isHidden() != hide {
                b.setHidden(hide);
            }
        }
        let (Some(height), Some(close)) = (bar.height, buttons.first()) else { return };
        // The buttons live in the title bar view, inside its container; grow
        // the container to our bar's height so the buttons can sit centred in it.
        // SAFETY: walking up the view tree on the main thread.
        let Some(container) = (unsafe { close.superview().and_then(|v| v.superview()) }) else { return };
        let button_h = close.frame().size.height;
        let bar_h = f64::from(height);
        let win_h = window.frame().size.height;
        let mut frame: CGRect = container.frame();
        if (frame.size.height - bar_h).abs() > 0.5 || (frame.origin.y - (win_h - bar_h)).abs() > 0.5 {
            frame.size.height = bar_h;
            frame.origin.y = win_h - bar_h;
            container.setFrame(frame);
        }
        // The title bar view fills the container; buttons are placed from its
        // bottom edge (AppKit's views aren't flipped).
        let y = ((bar_h - button_h) / 2.0).round();
        for (i, b) in buttons.iter().enumerate() {
            let want = CGPoint { x: LIGHTS_X + LIGHTS_STEP * i as f64, y };
            let at = b.frame().origin;
            if (at.x - want.x).abs() > 0.5 || (at.y - want.y).abs() > 0.5 {
                b.setFrameOrigin(want);
            }
        }
    }

    /// Zoom, minimise or nothing, as the user set it for title bars.
    pub fn title_double_click() -> bool {
        let Some(window) = window() else { return false };
        // SAFETY: plain Foundation calls on the main thread; the key may be unset.
        let action: Option<String> = unsafe {
            let defaults: *mut AnyObject = msg_send![objc2::class!(NSUserDefaults), standardUserDefaults];
            let key = objc2_foundation::NSString::from_str("AppleActionOnDoubleClick");
            let value: Option<Retained<objc2_foundation::NSString>> = msg_send![defaults, stringForKey: &*key];
            value.map(|v| v.to_string())
        };
        let action = match action.as_deref() {
            Some("Minimize") => objc2::sel!(miniaturize:),
            Some("None") => return true,
            _ => objc2::sel!(zoom:),
        };
        // Not now: we're in the middle of drawing a frame, and the zoom
        // animates inside the call. winit would hold its resizes back until
        // it ended, and the rest of this frame would land, at the old size,
        // in the zoomed window (stretched). On the next turn of the run loop
        // it resizes as from a title bar, a frame for each step.
        // SAFETY: NSObject's delayed perform, on the main thread; both actions take a sender.
        unsafe {
            let _: () = msg_send![&*window, performSelector: action, withObject: std::ptr::null::<AnyObject>(), afterDelay: 0.0f64];
        }
        true
    }

    /// The user is dragging an edge of the window.
    pub fn live_resizing() -> bool {
        window().is_some_and(|w| w.inLiveResize())
    }

    /// egui-wgpu turns on `presentsWithTransaction` on the window's
    /// CAMetalLayer when a resize starts (it stops live-resize jitter) and
    /// turns it off at the next non-resize window event. A resize the user
    /// doesn't drag (a zoom, switching window modes) has no such event after
    /// it, so the layer stays in that mode: frames drawn later wait on
    /// drawables that are only released with a Core Animation commit,
    /// stalling up to a second each, and the window can show a stale frame
    /// stretched to its new size. Turning it off once the resize is over puts
    /// presentation back to normal; the next resize turns it on again.
    pub fn stop_transaction_presents() {
        let Some(window) = window() else { return };
        let Some(view) = window.contentView() else { return };
        let Some(metal) = AnyClass::get(c"CAMetalLayer") else { return };
        // SAFETY: CALayer accessors on the main thread; the layer tree is the
        // content view's, which wgpu set up.
        unsafe {
            let root: *mut AnyObject = msg_send![&*view, layer];
            visit(root, metal, 0);
        }
    }

    /// # Safety
    /// `layer` is a CALayer (or null).
    unsafe fn visit(layer: *mut AnyObject, metal: &AnyClass, depth: u32) {
        if layer.is_null() || depth > 4 {
            return;
        }
        unsafe {
            let is_metal: bool = msg_send![layer, isKindOfClass: metal];
            if is_metal {
                let on: bool = msg_send![layer, presentsWithTransaction];
                if on {
                    let _: () = msg_send![layer, setPresentsWithTransaction: false];
                }
            }
            let subs: *mut AnyObject = msg_send![layer, sublayers];
            if subs.is_null() {
                return;
            }
            let n: usize = msg_send![subs, count];
            for i in 0..n {
                let sub: *mut AnyObject = msg_send![subs, objectAtIndex: i];
                visit(sub, metal, depth + 1);
            }
        }
    }
}
