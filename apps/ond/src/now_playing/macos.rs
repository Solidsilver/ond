//! Ond's music policy over the shared macOS playback integration.

use super::{Info, Remote, State};
use shell::now_playing as shared;
use std::sync::Arc;

pub fn install(handler: Arc<dyn Fn(Remote) + Send + Sync>) {
    shared::install(
        Arc::new(move |command| {
            handler(match command {
                shared::Remote::Play => Remote::Play,
                shared::Remote::Pause => Remote::Pause,
                shared::Remote::Toggle => Remote::Toggle,
                shared::Remote::Next => Remote::Next,
                shared::Remote::Previous => Remote::Previous,
                shared::Remote::Seek(at) => Remote::Seek(at),
                shared::Remote::SkipBy(by) => Remote::SkipBy(by),
            })
        }),
        false,
    );
    shared::intervals(15.0, 15.0);
}

pub fn publish(info: Option<Info>, state: State, elapsed: f64) {
    let info = info.map(|info| shared::Info {
        track_id: info.track_id,
        title: info.title,
        artist: info.artist,
        album: info.album,
        duration_secs: info.duration_secs,
        rate: 1.0,
        chapter: None,
    });
    let state = match state {
        State::Playing => shared::State::Playing,
        State::Buffering => shared::State::Buffering,
        State::Paused => shared::State::Paused,
        State::Stopped => shared::State::Stopped,
    };
    shared::publish(info, state, elapsed);
}

pub use shared::set_artwork;
