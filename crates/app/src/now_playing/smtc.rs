//! Windows: the System Media Transport Controls (the volume flyout's media
//! panel, media keys, the lock screen), attached to the app's window.
//!
//! The controls live on a thread of their own; updates reach it over a channel.
//! The name Windows shows comes from the executable's version info (`build.rs`).

use super::{Info, Remote, State};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Media::{
    MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs, SystemMediaTransportControls, SystemMediaTransportControlsButton,
    SystemMediaTransportControlsButtonPressedEventArgs, SystemMediaTransportControlsTimelineProperties,
};
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream, RandomAccessStreamReference};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;
use windows::core::{HSTRING, Result};

enum Msg {
    Publish(Option<Info>, State, f64),
    Artwork(String, Vec<u8>),
}

static TX: OnceLock<Sender<Msg>> = OnceLock::new();
static WINDOW: OnceLock<isize> = OnceLock::new();

/// The main window, which the controls attach to; set before [`install`].
/// Without one (tests, previews) there are no media controls.
pub fn set_window(hwnd: Option<isize>) {
    if let Some(h) = hwnd {
        let _ = WINDOW.set(h);
    }
}

/// Attaches the controls to the main window. `handler` runs on a system thread.
pub fn install(handler: Arc<dyn Fn(Remote) + Send + Sync>) {
    let Some(&hwnd) = WINDOW.get() else {
        return tracing::warn!("media controls off: no window handle");
    };
    let (tx, rx) = channel();
    if TX.set(tx).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new().name("media-controls".into()).spawn(move || {
        if let Err(e) = run(handler, hwnd, rx) {
            tracing::warn!("media controls off: {e}");
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("media controls off: {e}");
    }
}

/// Sets what's playing and where. `None` clears it.
pub fn publish(info: Option<Info>, state: State, elapsed: f64) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(Msg::Publish(info, state, elapsed));
    }
}

/// Artwork for `track_id` (encoded image bytes). The worker sends only the
/// cover of the track it last asked about, so a slow fetch never replaces a newer one.
pub fn set_artwork(track_id: String, bytes: Vec<u8>) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(Msg::Artwork(track_id, bytes));
    }
}

fn run(handler: Arc<dyn Fn(Remote) + Send + Sync>, hwnd: isize, rx: Receiver<Msg>) -> Result<()> {
    let interop = windows::core::factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
    // SAFETY: `hwnd` is the app's main window, which outlives this thread's use of it.
    let smtc: SystemMediaTransportControls = unsafe { interop.GetForWindow(HWND(hwnd as *mut _)) }?;
    smtc.SetIsPlayEnabled(true)?;
    smtc.SetIsPauseEnabled(true)?;
    smtc.SetIsNextEnabled(true)?;
    smtc.SetIsPreviousEnabled(true)?;

    let h = handler.clone();
    smtc.ButtonPressed(&TypedEventHandler::<SystemMediaTransportControls, SystemMediaTransportControlsButtonPressedEventArgs>::new(move |_, args| {
        if let Some(args) = args.as_ref() {
            let remote = match args.Button()? {
                SystemMediaTransportControlsButton::Play => Some(Remote::Play),
                SystemMediaTransportControlsButton::Pause | SystemMediaTransportControlsButton::Stop => Some(Remote::Pause),
                SystemMediaTransportControlsButton::Next => Some(Remote::Next),
                SystemMediaTransportControlsButton::Previous => Some(Remote::Previous),
                _ => None,
            };
            if let Some(r) = remote {
                h(r);
            }
        }
        Ok(())
    }))?;
    let h = handler;
    smtc.PlaybackPositionChangeRequested(&TypedEventHandler::<SystemMediaTransportControls, PlaybackPositionChangeRequestedEventArgs>::new(move |_, args| {
        if let Some(args) = args.as_ref() {
            h(Remote::Seek(secs(args.RequestedPlaybackPosition()?)));
        }
        Ok(())
    }))?;

    let mut art: Option<(String, Vec<u8>)> = None;
    let mut current: Option<Info> = None;
    for msg in rx {
        let done = match msg {
            Msg::Publish(info, state, elapsed) => {
                // The same track can come again with more known about it (a
                // late album name, a duration), so compare everything.
                let changed = current != info;
                let r = update(&smtc, info.as_ref(), state, elapsed, changed, &art);
                current = info;
                r
            }
            Msg::Artwork(track_id, bytes) => {
                art = Some((track_id, bytes));
                match &current {
                    Some(info) => show_track(&smtc, info, &art),
                    None => Ok(()),
                }
            }
        };
        if let Err(e) = done {
            tracing::debug!("media controls update failed: {e}");
        }
    }
    Ok(())
}

fn update(smtc: &SystemMediaTransportControls, info: Option<&Info>, state: State, elapsed: f64, changed: bool, art: &Option<(String, Vec<u8>)>) -> Result<()> {
    let Some(info) = info else {
        smtc.DisplayUpdater()?.ClearAll()?;
        smtc.SetPlaybackStatus(MediaPlaybackStatus::Stopped)?;
        return smtc.SetIsEnabled(false);
    };
    smtc.SetIsEnabled(true)?;
    if changed {
        show_track(smtc, info, art)?;
    }
    smtc.SetPlaybackStatus(match state {
        State::Playing | State::Buffering => MediaPlaybackStatus::Playing,
        State::Paused => MediaPlaybackStatus::Paused,
        State::Stopped => MediaPlaybackStatus::Stopped,
    })?;
    let end = info.duration_secs.unwrap_or(0.0);
    let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
    timeline.SetStartTime(span(0.0))?;
    timeline.SetEndTime(span(end))?;
    timeline.SetMinSeekTime(span(0.0))?;
    timeline.SetMaxSeekTime(span(end))?;
    timeline.SetPosition(span(elapsed))?;
    smtc.UpdateTimelineProperties(&timeline)
}

/// Title, artist, album and (when it's this track's) the cover.
fn show_track(smtc: &SystemMediaTransportControls, info: &Info, art: &Option<(String, Vec<u8>)>) -> Result<()> {
    let display = smtc.DisplayUpdater()?;
    display.SetType(MediaPlaybackType::Music)?;
    let music = display.MusicProperties()?;
    music.SetTitle(&HSTRING::from(&info.title))?;
    music.SetArtist(&HSTRING::from(info.artist.as_deref().unwrap_or_default()))?;
    music.SetAlbumTitle(&HSTRING::from(info.album.as_deref().unwrap_or_default()))?;
    match art {
        Some((id, bytes)) if *id == info.track_id => display.SetThumbnail(&thumbnail(bytes)?)?,
        _ => display.SetThumbnail(None)?,
    }
    display.Update()
}

fn thumbnail(bytes: &[u8]) -> Result<RandomAccessStreamReference> {
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(bytes)?;
    writer.StoreAsync()?.join()?;
    writer.DetachStream()?;
    stream.Seek(0)?;
    RandomAccessStreamReference::CreateFromStream(&stream)
}

/// Seconds as a WinRT `TimeSpan` (100 ns ticks).
fn span(secs: f64) -> TimeSpan {
    TimeSpan { Duration: (secs.max(0.0) * 1e7) as i64 }
}

fn secs(t: TimeSpan) -> f64 {
    t.Duration as f64 / 1e7
}
