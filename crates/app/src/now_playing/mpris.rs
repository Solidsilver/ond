//! MPRIS on Linux and the BSDs: GNOME's and KDE Plasma's media controls, the
//! desktop's media keys, and phones through KDE Connect / GSConnect.
//!
//! mpris-server's player isn't `Send`, so it lives on a thread of its own with
//! a single-threaded runtime, and updates reach it over a channel.

use super::{Info, Remote, State};
use mpris_server::{Metadata, PlaybackStatus, Player, Time, TrackId};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

enum Msg {
    Publish(Option<Info>, State, f64),
    Artwork(String, Vec<u8>),
}

static TX: OnceLock<UnboundedSender<Msg>> = OnceLock::new();

/// Claims `org.mpris.MediaPlayer2.ond` on the session bus. `handler` runs on
/// the MPRIS thread.
pub fn install(handler: Arc<dyn Fn(Remote) + Send + Sync>) {
    let (tx, rx) = unbounded_channel();
    if TX.set(tx).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new().name("mpris".into()).spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => return tracing::warn!("MPRIS off: no runtime ({e})"),
        };
        tokio::task::LocalSet::new().block_on(&rt, async move {
            if let Err(e) = serve(handler, rx).await {
                tracing::warn!("MPRIS off: {e}");
            }
        });
    });
    if let Err(e) = spawned {
        tracing::warn!("MPRIS off: {e}");
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

async fn serve(handler: Arc<dyn Fn(Remote) + Send + Sync>, mut rx: UnboundedReceiver<Msg>) -> mpris_server::zbus::Result<()> {
    let player = Player::builder("ond")
        .identity("Ond")
        // `dev.solidsilver.ond.desktop`, for the app's name and icon.
        .desktop_entry("dev.solidsilver.ond")
        .can_raise(true)
        .can_play(true)
        .can_pause(true)
        .can_go_next(true)
        .can_go_previous(true)
        .can_seek(true)
        .can_control(true)
        .build()
        .await?;
    let on = |r: Remote| {
        let h = handler.clone();
        move |_: &Player| h(r)
    };
    player.connect_play(on(Remote::Play));
    player.connect_pause(on(Remote::Pause));
    player.connect_stop(on(Remote::Pause));
    player.connect_play_pause(on(Remote::Toggle));
    player.connect_next(on(Remote::Next));
    player.connect_previous(on(Remote::Previous));
    player.connect_raise(on(Remote::Raise));
    let h = handler.clone();
    player.connect_seek(move |_, by| h(Remote::SkipBy(secs(by))));
    let h = handler.clone();
    player.connect_set_position(move |p, track, at| {
        // The spec: ignore a position meant for a track that's no longer playing.
        if p.metadata().trackid().as_ref() == Some(track) && !at.is_negative() {
            h(Remote::Seek(secs(at)));
        }
    });
    tokio::task::spawn_local(player.run());

    let art_dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from).join("ond");
    let mut art: Option<(String, PathBuf)> = None;
    let mut last: Option<(Option<Info>, State)> = None;
    while let Some(msg) = rx.recv().await {
        let done = match msg {
            Msg::Publish(info, state, elapsed) => {
                // The same track can come again with more known about it (a
                // late album name, a duration), so compare everything.
                let unchanged = last.as_ref().is_some_and(|(i, _)| *i == info);
                let r = update(&player, info.as_ref(), state, elapsed, unchanged, &art).await;
                last = Some((info, state));
                r
            }
            Msg::Artwork(track_id, bytes) => {
                let Some(path) = save_art(&art_dir, &track_id, &bytes, art.take()) else { continue };
                art = Some((track_id, path));
                match &last {
                    Some((Some(info), _)) if Some(&info.track_id) == art.as_ref().map(|(id, _)| id) => player.set_metadata(metadata(info, &art)).await,
                    _ => Ok(()),
                }
            }
        };
        if let Err(e) = done {
            tracing::debug!("MPRIS update failed: {e}");
        }
    }
    Ok(())
}

async fn update(
    player: &Player,
    info: Option<&Info>,
    state: State,
    elapsed: f64,
    unchanged: bool,
    art: &Option<(String, PathBuf)>,
) -> mpris_server::zbus::Result<()> {
    let Some(info) = info else {
        player.set_metadata(Metadata::new()).await?;
        return player.set_playback_status(PlaybackStatus::Stopped).await;
    };
    if !unchanged {
        player.set_metadata(metadata(info, art)).await?;
    }
    // MPRIS has no "buffering": clients extrapolate from the rate, and a drift
    // republish corrects them once audio flows.
    player
        .set_playback_status(match state {
            State::Playing | State::Buffering => PlaybackStatus::Playing,
            State::Paused => PlaybackStatus::Paused,
            State::Stopped => PlaybackStatus::Stopped,
        })
        .await?;
    let at = Time::from_micros((elapsed.max(0.0) * 1e6) as i64);
    player.set_position(at);
    // Position changes don't signal by themselves; `Seeked` makes clients re-read it.
    player.seeked(at).await
}

fn metadata(info: &Info, art: &Option<(String, PathBuf)>) -> Metadata {
    let mut m = Metadata::new();
    m.set_trackid(Some(track_id(&info.track_id)));
    m.set_title(Some(info.title.clone()));
    m.set_artist(info.artist.clone().map(|a| [a]));
    m.set_album(info.album.clone());
    m.set_length(info.duration_secs.map(|d| Time::from_micros((d * 1e6) as i64)));
    let art = art.as_ref().filter(|(id, _)| *id == info.track_id);
    m.set_art_url(art.and_then(|(_, path)| url::Url::from_file_path(path).ok()).map(String::from));
    m
}

/// A D-Bus object path for the track: `/dev/solidsilver/ond/track/<id>`, with
/// anything outside `[A-Za-z0-9_]` replaced.
fn track_id(id: &str) -> TrackId {
    let clean: String = id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if clean.is_empty() {
        return TrackId::NO_TRACK;
    }
    TrackId::try_from(format!("/dev/solidsilver/ond/track/{clean}")).unwrap_or(TrackId::NO_TRACK)
}

/// Writes the cover where the shell can read it (the per-user runtime dir),
/// replacing the previous one.
fn save_art(dir: &Path, track_id: &str, bytes: &[u8], previous: Option<(String, PathBuf)>) -> Option<PathBuf> {
    if let Some((_, old)) = previous {
        let _ = std::fs::remove_file(old);
    }
    std::fs::create_dir_all(dir).ok()?;
    let name: String = track_id.chars().filter(char::is_ascii_alphanumeric).collect();
    let path = dir.join(format!("cover-{name}"));
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

fn secs(t: Time) -> f64 {
    t.as_micros() as f64 / 1e6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_ids_become_valid_object_paths() {
        assert_eq!(track_id("a1-b2.c3").to_string(), "/dev/solidsilver/ond/track/a1_b2_c3");
        assert_eq!(track_id(""), TrackId::NO_TRACK);
    }
}
