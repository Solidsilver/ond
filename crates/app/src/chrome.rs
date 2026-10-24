//! The window's title bar, folded into our own toolbar (macOS). The window
//! keeps its native frame (rounded corners, shadow, resizing, full screen) but
//! its title bar is transparent and the content runs underneath it; the
//! traffic lights are moved to sit centred in our bar, or hidden in Mini.
//!
//! AppKit puts the buttons back whenever it lays the title bar out again
//! (resizes, key-window changes), so [`sync`] re-applies the layout
//! each frame; it's a handful of cheap property calls.

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

/// Re-applies `bar` to the main window. Called every frame: AppKit undoes it.
pub fn sync(ctx: &egui::Context, bar: Bar) {
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

/// Ends egui-wgpu's "live resize" presentation mode after a resize we made
/// ourselves. See [`mac::stop_transaction_presents`].
pub fn end_programmatic_resize() {
    #[cfg(target_os = "macos")]
    mac::stop_transaction_presents();
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
        match action.as_deref() {
            Some("Minimize") => window.miniaturize(None),
            Some("None") => {}
            _ => window.zoom(None),
        }
        true
    }

    /// egui-wgpu turns on `presentsWithTransaction` on the window's
    /// CAMetalLayer when a resize starts (it stops live-resize jitter) and
    /// turns it off at the next non-resize window event. A resize we make
    /// ourselves (switching window modes) has no such event after it, so the
    /// layer stays in that mode: frames drawn later from a timer (every
    /// 250 ms while music plays) then wait on drawables that are only released
    /// with a Core Animation commit, stalling up to a second each. Turning it
    /// off once our resize has landed puts presentation back to normal; the
    /// next live resize by the user turns it on again.
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
