//! Book time is independent of both file boundaries and playback speed. The
//! timeline owns only ranges; URLs, the engine and listening time stay outside.

use sources::Chapter;

#[derive(Clone, Debug)]
pub struct Timeline {
    files: Vec<(f64, f64)>,
    pub chapters: Vec<Chapter>,
    pub duration: f64,
}

fn seconds(value: f64) -> f64 {
    if value.is_finite() { value.max(0.0) } else { 0.0 }
}

impl Timeline {
    pub fn new(files: &[(f64, Option<f64>)], chapters: Vec<Chapter>, duration: Option<f64>) -> Self {
        let mut end = seconds(duration.unwrap_or(0.0));
        for &(start, length) in files {
            end = end.max(seconds(start) + seconds(length.unwrap_or(0.0)));
        }
        for chapter in &chapters {
            end = end.max(seconds(chapter.end));
        }
        let files = files
            .iter()
            .enumerate()
            .map(|(i, &(start, length))| {
                let start = seconds(start);
                let stop = length.map(|d| start + seconds(d)).unwrap_or_else(|| files.get(i + 1).map_or(end, |f| seconds(f.0)));
                (start, stop.max(start))
            })
            .collect();
        Self { files, chapters, duration: end }
    }

    pub fn clamp(&self, at: f64) -> f64 {
        if at == f64::INFINITY { self.duration } else { seconds(at).min(self.duration) }
    }

    /// At a boundary the following file owns the position. At the book's end
    /// the last file owns its end offset, so callers can seek there safely.
    pub fn locate(&self, at: f64) -> Option<(usize, f64)> {
        let at = self.clamp(at);
        let i = self.files.iter().rposition(|&(start, _)| start <= at).or_else(|| (!self.files.is_empty()).then_some(0))?;
        let (start, end) = self.files[i];
        Some((i, (at - start).clamp(0.0, end - start)))
    }

    pub fn book_position(&self, file: usize, offset: f64) -> f64 {
        self.files.get(file).map_or(0.0, |&(start, end)| self.clamp(start + seconds(offset).min(end - start)))
    }

    pub fn chapter(&self, at: f64) -> Option<usize> {
        let at = self.clamp(at);
        self.chapters.iter().rposition(|c| c.start <= at && (at < c.end || (at == self.duration && c.end >= at)))
    }

    pub fn chapter_range(&self, at: f64) -> (f64, f64) {
        self.chapter(at).map_or((0.0, self.duration), |i| (self.chapters[i].start, self.chapters[i].end.min(self.duration)))
    }

    pub fn previous_chapter(&self, at: f64) -> f64 {
        let Some(i) = self.chapter(at) else { return 0.0 };
        if self.clamp(at) - self.chapters[i].start > 3.0 { self.chapters[i].start } else { self.chapters[i.saturating_sub(1)].start }
    }

    pub fn next_chapter(&self, at: f64) -> f64 {
        self.chapters.iter().find(|c| c.start > self.clamp(at)).map_or(self.duration, |c| c.start)
    }

    pub fn remaining(&self, at: f64, speed: f32) -> (f64, f64) {
        let at = self.clamp(at);
        let speed = f64::from(normalize_speed(speed));
        ((self.chapter_range(at).1 - at).max(0.0) / speed, (self.duration - at) / speed)
    }
}

pub fn normalize_speed(speed: f32) -> f32 {
    if speed.is_finite() { (speed.clamp(0.5, 3.0) * 20.0).round() / 20.0 } else { 1.0 }
}

pub fn left(seconds: f64, speed: f32) -> String {
    format!("{} at {}", span_left(seconds), speed_label(speed))
}

/// "4h 10m left", in whole minutes rounded up, for a span already at speed.
pub fn span_left(seconds: f64) -> String {
    let minutes = (seconds.max(0.0) / 60.0).ceil() as u64;
    let span = if minutes >= 60 { format!("{}h {}m", minutes / 60, minutes % 60) } else { format!("{minutes}m") };
    format!("{span} left")
}

/// "1.5×", as the speed is actually played.
pub fn speed_label(speed: f32) -> String {
    format!("{}×", normalize_speed(speed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timeline(files: &[(f64, Option<f64>)]) -> Timeline {
        Timeline::new(
            files,
            vec![Chapter { start: 0.0, end: 40.0, title: "First".into() }, Chapter { start: 40.0, end: 100.0, title: "Second".into() }],
            Some(100.0),
        )
    }

    #[test]
    fn a_single_m4b_maps_every_chapter_to_the_same_file() {
        let t = timeline(&[(0.0, Some(100.0))]);
        for at in [0.0, 39.0, 40.0, 99.0, 100.0] {
            assert_eq!(t.locate(at), Some((0, at)));
            assert_eq!(t.book_position(0, at), at);
        }
        assert_eq!(t.chapter(39.9), Some(0));
        assert_eq!(t.chapter(40.0), Some(1));
        assert_eq!(t.chapter(100.0), Some(1));
    }

    #[test]
    fn file_boundaries_and_chapter_boundaries_are_independent() {
        let t = timeline(&[(0.0, Some(60.0)), (60.0, Some(40.0))]);
        assert_eq!(t.locate(40.0), Some((0, 40.0)));
        assert_eq!(t.locate(60.0), Some((1, 0.0)));
        assert_eq!(t.locate(83.0), Some((1, 23.0)));
        assert_eq!(t.book_position(1, 23.0), 83.0);
        assert_eq!(t.locate(100.0), Some((1, 40.0)));
        assert_eq!(t.locate(500.0), Some((1, 40.0)));
        assert_eq!(t.book_position(1, 500.0), 100.0);
    }

    #[test]
    fn a_previous_chapter_restarts_after_three_seconds() {
        let t = timeline(&[(0.0, Some(100.0))]);
        assert_eq!(t.previous_chapter(43.0), 0.0);
        assert_eq!(t.previous_chapter(43.1), 40.0);
        assert_eq!(t.previous_chapter(2.0), 0.0);
        assert_eq!(t.next_chapter(0.0), 40.0);
        assert_eq!(t.next_chapter(40.0), 100.0);
    }

    #[test]
    fn remaining_time_uses_source_time_and_the_current_speed() {
        let t = timeline(&[(0.0, Some(100.0))]);
        assert_eq!(t.remaining(25.0, 1.5), (10.0, 50.0));
        assert_eq!(t.remaining(500.0, 3.0), (0.0, 0.0));
        assert_eq!(left(15_000.0, 1.5), "4h 10m left at 1.5×");
        assert_eq!(span_left(59.0), "1m left");
        assert_eq!(speed_label(1.0), "1×");
        assert_eq!(normalize_speed(1.526), 1.55);
        assert_eq!(normalize_speed(0.0), 0.5);
        assert_eq!(normalize_speed(9.0), 3.0);
        assert_eq!(normalize_speed(f32::NAN), 1.0);
    }

    #[test]
    fn missing_lengths_use_the_next_offset_or_the_book_duration() {
        let t = timeline(&[(0.0, None), (60.0, None)]);
        assert_eq!(t.locate(60.0), Some((1, 0.0)));
        assert_eq!(t.locate(100.0), Some((1, 40.0)));
    }

    #[test]
    fn empty_books_and_invalid_positions_are_safe() {
        let t = Timeline::new(&[], vec![], None);
        assert_eq!(t.locate(20.0), None);
        assert_eq!(t.chapter(0.0), None);
        assert_eq!(t.remaining(0.0, 1.0), (0.0, 0.0));
        let t = timeline(&[(0.0, Some(100.0))]);
        assert_eq!(t.locate(-5.0), Some((0, 0.0)));
        assert_eq!(t.locate(f64::NAN), Some((0, 0.0)));
        assert_eq!(t.locate(f64::INFINITY), Some((0, 100.0)));
    }
}
