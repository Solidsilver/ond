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

#[cfg(test)]
mod tests {
    use super::*;

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
}
