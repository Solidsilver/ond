//! Launch Services delivers both cold-launch and running-app URLs as GURL
//! Apple Events. Install before eframe starts its event loop; buffer early
//! events until the app's inbox and repaint callback exist. winit 0.30.13 and
//! eframe 0.37 expose no URL-open event to the application.

use objc2::rc::Retained;
use objc2::{MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{MainThreadMarker, NSAppleEventDescriptor, NSAppleEventManager, NSObject, NSObjectProtocol};
use std::cell::RefCell;
use std::sync::Arc;

type Receive = Arc<dyn Fn(String) + Send + Sync>;

thread_local! {
    static TARGET: RefCell<Option<Retained<UrlHandler>>> = const { RefCell::new(None) };
    static RECEIVE: RefCell<Option<Receive>> = const { RefCell::new(None) };
    static EARLY: RefCell<Option<String>> = const { RefCell::new(None) };
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. Delivery and storage
    // stay on the main thread; the target lives as long as the event loop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    struct UrlHandler;

    // SAFETY: NSObjectProtocol adds no requirements.
    unsafe impl NSObjectProtocol for UrlHandler {}

    impl UrlHandler {
        #[unsafe(method(handleURL:withReplyEvent:))]
        fn handle_url(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            // keyDirectObject ('----') is the URL string in a kAEGetURL event.
            if let Some(url) = event.paramDescriptorForKeyword(u32::from_be_bytes(*b"----")).and_then(|d| d.stringValue()) {
                deliver(url.to_string());
            }
        }
    }
);

fn deliver(url: String) {
    let receive = RECEIVE.with_borrow(Clone::clone);
    if let Some(receive) = receive {
        receive(url);
    } else {
        // Only one transaction can be pending. Bound untrusted early input.
        EARLY.with_borrow_mut(|early| *early = Some(url));
    }
}

/// Called from main before run_native, so an initial GURL cannot be lost
/// while the app and its keyring reader are being constructed.
pub fn install() {
    let mtm = MainThreadMarker::new().expect("URL handling starts on the main thread");
    // SAFETY: NSObject init and the selector's two descriptor arguments match
    // the registered Objective-C method. The thread-local retains the target.
    unsafe {
        let handler: Retained<UrlHandler> = msg_send![UrlHandler::alloc(mtm), init];
        NSAppleEventManager::sharedAppleEventManager().setEventHandler_andSelector_forEventClass_andEventID(
            &handler,
            sel!(handleURL:withReplyEvent:),
            u32::from_be_bytes(*b"GURL"),
            u32::from_be_bytes(*b"GURL"),
        );
        TARGET.with_borrow_mut(|target| *target = Some(handler));
    }
}

/// Connects the Apple Event receiver to the UI inbox, including an event that
/// launched us before CreationContext existed.
pub fn attach(receive: Receive) {
    MainThreadMarker::new().expect("the app attaches URL handling on the main thread");
    RECEIVE.with_borrow_mut(|slot| *slot = Some(receive.clone()));
    if let Some(url) = EARLY.with_borrow_mut(Option::take) {
        receive(url);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_url_is_buffered_and_running_urls_are_forwarded() {
        RECEIVE.with_borrow_mut(|slot| *slot = None);
        EARLY.with_borrow_mut(|slot| *slot = None);
        deliver("rede://oauth?code=cold".into());
        let (tx, rx) = std::sync::mpsc::channel();
        // The delivery boundary can be exercised without a macOS event loop.
        RECEIVE.with_borrow_mut(|slot| {
            *slot = Some(Arc::new(move |url| {
                let _ = tx.send(url);
            }))
        });
        if let Some(url) = EARLY.with_borrow_mut(Option::take) {
            deliver(url);
        }
        deliver("rede://oauth?code=running".into());
        assert_eq!(rx.recv().unwrap(), "rede://oauth?code=cold");
        assert_eq!(rx.recv().unwrap(), "rede://oauth?code=running");
        RECEIVE.with_borrow_mut(|slot| *slot = None);
    }
}
