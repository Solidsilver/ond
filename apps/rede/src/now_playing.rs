//! Book time is the system's time base: the scrubber stays useful over chapter
//! and file boundaries, and remote seeks map directly onto the book timeline.

use crate::player::{Control, Player};
use audio::PlayState;
use shell::now_playing::{Info, Remote, State};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub info: Info,
    pub state: State,
    pub elapsed: f64,
}

pub fn snapshot(player: &Player, output: PlayState) -> Option<Snapshot> {
    let book = player.book.as_ref().filter(|_| player.active)?;
    let chapter = book.timeline.chapter(player.position);
    let count = book.timeline.chapters.len();
    let subtitle = chapter.map(|i| format!("Chapter {} of {count} · {}", i + 1, book.timeline.chapters[i].title));
    let state = if player.paused {
        State::Paused
    } else {
        match output {
            PlayState::Playing => State::Playing,
            PlayState::Paused => State::Paused,
            PlayState::Loading | PlayState::Buffering => State::Buffering,
            PlayState::Stopped => State::Stopped,
        }
    };
    Some(Snapshot {
        info: Info {
            track_id: book.key.to_string(),
            title: book.item.summary.title.clone(),
            artist: subtitle,
            album: Some(book.item.summary.authors.join(", ")),
            duration_secs: Some(book.timeline.duration),
            rate: f64::from(player.speed),
            chapter: chapter.map(|i| ((i + 1) as u64, count as u64)),
        },
        state,
        elapsed: player.position,
    })
}

pub fn control(remote: Remote, player: &Player, back: u32, forward: u32) -> Option<Control> {
    match remote {
        Remote::Play if player.paused || !player.active => Some(Control::Toggle),
        Remote::Pause if player.active && !player.paused => Some(Control::Toggle),
        Remote::Play | Remote::Pause => None,
        Remote::Toggle => Some(Control::Toggle),
        Remote::Next => Some(Control::Skip(f64::from(forward))),
        Remote::Previous => Some(Control::Skip(-f64::from(back))),
        Remote::SkipBy(by) => Some(Control::Skip(by)),
        Remote::Seek(at) => Some(Control::Seek(at)),
    }
}

#[derive(Default)]
pub struct Publisher {
    last: Option<(Option<Snapshot>, Instant)>,
}
impl Publisher {
    /// Returns whether the book changed, so its artwork can be fetched once.
    pub fn update(&mut self, player: &Player, output: PlayState, now: Instant) -> bool {
        let next = snapshot(player, output);
        let previous = self.last.as_ref().and_then(|(last, _)| last.as_ref());
        let changed = previous.map(|s| &s.info.track_id) != next.as_ref().map(|s| &s.info.track_id);
        let due = self.last.as_ref().is_none_or(|(last, at)| match (last, &next) {
            (Some(last), Some(next)) => {
                let since = now.saturating_duration_since(*at);
                let predicted = last.elapsed + if last.state == State::Playing { since.as_secs_f64() * last.info.rate } else { 0.0 };
                last.info != next.info
                    || last.state != next.state
                    || (next.elapsed - predicted).abs() > 0.5
                    || (next.state == State::Playing && since >= Duration::from_secs(5))
            }
            (None, None) => false,
            _ => true,
        });
        if due {
            #[cfg(all(target_os = "macos", not(test)))]
            shell::now_playing::publish(
                next.as_ref().map(|s| s.info.clone()),
                next.as_ref().map_or(State::Stopped, |s| s.state),
                next.as_ref().map_or(0.0, |s| s.elapsed),
            );
            self.last = Some((next, now));
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn book_time_and_chapter_metadata_follow_speed_and_pause() {
        let mut player = Player::default();
        assert!(snapshot(&player, PlayState::Playing).is_none());
        // The player tests construct real sessions through its transport seam.
        crate::player::tests::load_for_metadata(&mut player);
        let info = snapshot(&player, PlayState::Playing).unwrap();
        assert_eq!(info.info.title, "Book");
        assert_eq!(info.info.artist.as_deref(), Some("Chapter 2 of 2 · Second"));
        assert_eq!(info.info.chapter, Some((2, 2)));
        assert_eq!((info.elapsed, info.info.duration_secs, info.info.rate), (75.0, Some(100.0), 1.5));
        player.paused = true;
        assert_eq!(snapshot(&player, PlayState::Playing).unwrap().state, State::Paused);
        player.paused = false;
        assert_eq!(snapshot(&player, PlayState::Buffering).unwrap().state, State::Buffering);
        assert_eq!(control(Remote::Seek(80.0), &player, 15, 30), Some(Control::Seek(80.0)));
        assert_eq!(control(Remote::Next, &player, 15, 45), Some(Control::Skip(45.0)));
        assert_eq!(control(Remote::Previous, &player, 20, 30), Some(Control::Skip(-20.0)));
        let now = Instant::now();
        let mut publisher = Publisher::default();
        assert!(publisher.update(&player, PlayState::Playing, now));
        assert!(!publisher.update(&player, PlayState::Playing, now));
    }
}
