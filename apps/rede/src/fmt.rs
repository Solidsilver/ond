//! Lengths as the interface says them: one place, so the same time reads the
//! same on a cover, a chapter and a book's page.

/// A length or a position: "3:07", "1:02:45".
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// How long a book is: "41 min", "11 hr 24 min". Minutes are rounded up, so a
/// book is never "0 min".
pub fn span(secs: f64) -> String {
    let mins = (secs.max(0.0) / 60.0).ceil() as u64;
    if mins >= 60 {
        let m = mins % 60;
        if m == 0 { format!("{} hr", mins / 60) } else { format!("{} hr {m} min", mins / 60) }
    } else {
        format!("{mins} min")
    }
}

/// How far into a book, and how long it is, in the tiles' short words:
/// "6h 25m of 11h 24m". The time in is rounded down (it isn't there yet),
/// the length up, as [`span`] rounds it.
pub fn of_span(at: f64, duration: f64) -> String {
    let hm = |mins: u64| if mins >= 60 { format!("{}h {}m", mins / 60, mins % 60) } else { format!("{mins}m") };
    format!("{} of {}", hm((at.max(0.0) / 60.0).floor() as u64), hm((duration.max(0.0) / 60.0).ceil() as u64))
}

/// How long ago, in Ond's words: "just now", "5 minutes ago", "3 hours ago",
/// "2 days ago".
pub fn ago(at: std::time::SystemTime) -> String {
    let s = at.elapsed().map_or(0, |d| d.as_secs());
    let (n, unit) = match s {
        0..60 => return "just now".into(),
        60..3600 => (s / 60, "minute"),
        3600..86400 => (s / 3600, "hour"),
        _ => (s / 86400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn how_far_reads_in_hours_and_minutes() {
        assert_eq!(of_span(23_100.0, 41_000.0), "6h 25m of 11h 24m");
        assert_eq!(of_span(59.0, 1_800.0), "0m of 30m");
    }

    #[test]
    fn lengths_read_like_clocks() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(67.9), "1:07");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(clock(-4.0), "0:00", "a position before the start is the start");
    }

    #[test]
    fn a_books_length_is_hours_and_minutes() {
        assert_eq!(span(30.0), "1 min");
        assert_eq!(span(2460.0), "41 min");
        assert_eq!(span(41_040.0), "11 hr 24 min");
        assert_eq!(span(36_000.0), "10 hr");
    }

    #[test]
    fn a_time_past_reads_in_the_largest_whole_unit() {
        let back = |s: u64| std::time::SystemTime::now() - std::time::Duration::from_secs(s);
        assert_eq!(ago(back(5)), "just now");
        assert_eq!(ago(back(60)), "1 minute ago");
        assert_eq!(ago(back(2 * 3600 + 59)), "2 hours ago");
        assert_eq!(ago(back(3 * 86400)), "3 days ago");
        assert_eq!(ago(std::time::SystemTime::now() + std::time::Duration::from_secs(30)), "just now", "a clock that moved back");
    }
}
