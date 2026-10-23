//! macOS Now Playing (Control Center, the menu bar widget, AirPods and keyboard
//! media keys) through the MediaPlayer framework.
//!
//! Every call hops onto the main queue: MediaPlayer delivers remote commands
//! there, and keeping our updates on the same queue means they can't interleave.
//! The current artwork lives on the main thread too, so each publish can rebuild
//! the whole info dictionary instead of patching the previous one.

use super::{Info, Remote, State};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSImage;
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSData, NSMutableDictionary, NSNumber, NSString};
use objc2_media_player::{
    MPChangePlaybackPositionCommandEvent, MPMediaItemArtwork, MPMediaItemPropertyAlbumTitle, MPMediaItemPropertyArtist,
    MPMediaItemPropertyArtwork, MPMediaItemPropertyPlaybackDuration, MPMediaItemPropertyTitle, MPNowPlayingInfoCenter,
    MPNowPlayingInfoPropertyElapsedPlaybackTime, MPNowPlayingInfoPropertyPlaybackRate, MPNowPlayingPlaybackState,
    MPRemoteCommand, MPRemoteCommandCenter, MPRemoteCommandEvent, MPRemoteCommandHandlerStatus, MPSkipIntervalCommandEvent,
};
use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::Arc;

/// Skip ±N seconds (AirPods/Siri "skip forward"). macOS shows these in place of
/// previous/next in some Now Playing views when they're enabled, so they're
/// registered but left off; flip this to turn them on.
const ENABLE_SKIP: bool = false;
const SKIP_SECS: f64 = 15.0;

thread_local! {
    /// (track id, artwork) for the current track; main thread only.
    static ARTWORK: RefCell<Option<(String, Retained<MPMediaItemArtwork>)>> = const { RefCell::new(None) };
    /// The last published info, so artwork arriving later can be added to it.
    static LAST: RefCell<Option<(Option<Info>, State, f64)>> = const { RefCell::new(None) };
    /// Keeps the command handler blocks alive for the app's lifetime.
    static TARGETS: RefCell<Vec<Retained<AnyObject>>> = const { RefCell::new(Vec::new()) };
}

/// Registers the remote command handlers. `handler` runs on the main thread.
pub fn install(handler: Arc<dyn Fn(Remote) + Send + Sync>) {
    DispatchQueue::main().exec_async(move || {
        // SAFETY: MediaPlayer calls on the main queue, which is where we are.
        unsafe {
            let center = MPRemoteCommandCenter::sharedCommandCenter();
            let simple: [(Retained<MPRemoteCommand>, Remote); 5] = [
                (center.playCommand(), Remote::Play),
                (center.pauseCommand(), Remote::Pause),
                (center.togglePlayPauseCommand(), Remote::Toggle),
                (center.nextTrackCommand(), Remote::Next),
                (center.previousTrackCommand(), Remote::Previous),
            ];
            for (cmd, remote) in simple {
                let h = handler.clone();
                add(&cmd, true, move |_| {
                    h(remote);
                    MPRemoteCommandHandlerStatus::Success
                });
            }
            let h = handler.clone();
            add(&center.changePlaybackPositionCommand(), true, move |ev| {
                let any: &AnyObject = ev.as_ref();
                let Some(ev) = any.downcast_ref::<MPChangePlaybackPositionCommandEvent>() else {
                    return MPRemoteCommandHandlerStatus::CommandFailed;
                };
                h(Remote::Seek(ev.positionTime()));
                MPRemoteCommandHandlerStatus::Success
            });
            let intervals = objc2_foundation::NSArray::from_retained_slice(&[NSNumber::new_f64(SKIP_SECS)]);
            for (cmd, sign) in [(center.skipForwardCommand(), 1.0), (center.skipBackwardCommand(), -1.0)] {
                cmd.setPreferredIntervals(&intervals);
                let h = handler.clone();
                add(&cmd, ENABLE_SKIP, move |ev| {
                    let any: &AnyObject = ev.as_ref();
                    let Some(ev) = any.downcast_ref::<MPSkipIntervalCommandEvent>() else {
                        return MPRemoteCommandHandlerStatus::CommandFailed;
                    };
                    h(Remote::SkipBy(sign * ev.interval()));
                    MPRemoteCommandHandlerStatus::Success
                });
            }
        }
    });
}

/// # Safety
/// Main thread only.
unsafe fn add(cmd: &MPRemoteCommand, enabled: bool, f: impl Fn(&MPRemoteCommandEvent) -> MPRemoteCommandHandlerStatus + 'static) {
    let block = RcBlock::new(move |ev: NonNull<MPRemoteCommandEvent>| f(unsafe { ev.as_ref() }));
    unsafe {
        cmd.setEnabled(enabled);
        let target = cmd.addTargetWithHandler(&block);
        TARGETS.with_borrow_mut(|t| t.push(target));
    }
}

/// Sets what's playing and where. `None` clears Now Playing.
pub fn publish(info: Option<Info>, state: State, elapsed: f64) {
    DispatchQueue::main().exec_async(move || {
        LAST.set(Some((info.clone(), state, elapsed)));
        apply(info.as_ref(), state, elapsed);
    });
}

/// Artwork for `track_id` (encoded image bytes); ignored if the track changed meanwhile.
pub fn set_artwork(track_id: String, bytes: Vec<u8>) {
    DispatchQueue::main().exec_async(move || {
        let data = NSData::from_vec(bytes);
        let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else { return };
        let size = image.size();
        let size = CGSize { width: size.width, height: size.height };
        let handler = RcBlock::new(move |_: CGSize| -> NonNull<NSImage> { NonNull::from(&*image) });
        // SAFETY: the block hands back an image it keeps alive itself.
        let artwork = unsafe { MPMediaItemArtwork::initWithBoundsSize_requestHandler(MPMediaItemArtwork::alloc(), size, &handler) };
        ARTWORK.set(Some((track_id.clone(), artwork)));
        // Republish with the artwork if this is still the current track.
        if let Some((Some(info), state, elapsed)) = LAST.with_borrow(Clone::clone)
            && info.track_id == track_id
        {
            apply(Some(&info), state, elapsed);
        }
    });
}

fn apply(info: Option<&Info>, state: State, elapsed: f64) {
    // SAFETY: plain property access on the shared center, on the main queue.
    unsafe {
        let center = MPNowPlayingInfoCenter::defaultCenter();
        let Some(info) = info else {
            center.setNowPlayingInfo(None);
            center.setPlaybackState(MPNowPlayingPlaybackState::Stopped);
            return;
        };
        let dict: Retained<NSMutableDictionary<NSString, AnyObject>> = NSMutableDictionary::new();
        let put = |key: &NSString, value: &AnyObject| dict.insert(key, value);
        put(MPMediaItemPropertyTitle, &NSString::from_str(&info.title));
        if let Some(a) = &info.artist {
            put(MPMediaItemPropertyArtist, &NSString::from_str(a));
        }
        if let Some(a) = &info.album {
            put(MPMediaItemPropertyAlbumTitle, &NSString::from_str(a));
        }
        if let Some(d) = info.duration_secs {
            put(MPMediaItemPropertyPlaybackDuration, &NSNumber::new_f64(d));
        }
        put(MPNowPlayingInfoPropertyElapsedPlaybackTime, &NSNumber::new_f64(elapsed));
        // With the rate set, macOS advances the elapsed time by itself between updates.
        let rate = if state == State::Playing { 1.0 } else { 0.0 };
        put(MPNowPlayingInfoPropertyPlaybackRate, &NSNumber::new_f64(rate));
        ARTWORK.with_borrow(|art| {
            if let Some((id, artwork)) = art
                && *id == info.track_id
            {
                put(MPMediaItemPropertyArtwork, artwork);
            }
        });
        center.setNowPlayingInfo(Some(&dict));
        center.setPlaybackState(match state {
            State::Playing | State::Buffering => MPNowPlayingPlaybackState::Playing,
            State::Paused => MPNowPlayingPlaybackState::Paused,
            State::Stopped => MPNowPlayingPlaybackState::Stopped,
        });
    }
}
