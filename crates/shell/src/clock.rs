//! Times of day as the apps say them: "until 4:22 PM" over Ond's queue, "ends
//! Wed 1:10 AM" over Rede's chapters. The hour follows the system's 12- or
//! 24-hour convention where an app asks for it ([`uses_24_hour`]); the
//! formatting itself is pure, so tests pin both.

use chrono::{NaiveDateTime, NaiveTime, TimeDelta};

/// `at` as "4:22 PM", or "16:22" with `h24`.
pub fn time_of_day(at: NaiveTime, h24: bool) -> String {
    at.format(if h24 { "%H:%M" } else { "%-I:%M %p" }).to_string()
}

/// The time `offset` seconds after `now`: "11:42 PM" while it's still
/// today, else with the weekday, "Wed 1:10 AM". A week or more away it
/// still names only the weekday; no listening session is that long.
pub fn after(now: NaiveDateTime, offset: f64, h24: bool) -> String {
    let at = now + TimeDelta::seconds(offset.max(0.0).round() as i64);
    let time = time_of_day(at.time(), h24);
    if at.date() == now.date() { time } else { format!("{} {time}", at.format("%a")) }
}

/// Whether the system shows times of day on a 24-hour clock. Read once:
/// a change in System Settings shows after a restart.
pub fn uses_24_hour() -> bool {
    static H24: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *H24.get_or_init(system_24_hour)
}

/// macOS: the hour in the user's locale's own pattern for "an hour", which
/// honours the 24-hour switch in System Settings.
#[cfg(target_os = "macos")]
fn system_24_hour() -> bool {
    use objc2_foundation::{NSDateFormatter, NSLocale, NSString};
    let locale = NSLocale::currentLocale();
    NSDateFormatter::dateFormatFromTemplate_options_locale(&NSString::from_str("j"), 0, Some(&locale))
        .is_some_and(|pattern| pattern.to_string().contains(['H', 'k']))
}

/// Elsewhere: the time locale's region, by the few that keep a 12-hour clock.
#[cfg(not(target_os = "macos"))]
fn system_24_hour() -> bool {
    let locale = ["LC_ALL", "LC_TIME", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty())).unwrap_or_default();
    !twelve_hour_locale(&locale)
}

/// Whether a POSIX locale name ("en_US.UTF-8") is one whose people read a
/// 12-hour clock.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn twelve_hour_locale(locale: &str) -> bool {
    let name = locale.split(['.', '@']).next().unwrap_or("");
    matches!(name, "en_US" | "en_CA" | "en_AU" | "en_NZ" | "en_PH" | "en_IN" | "es_US" | "es_MX" | "hi_IN" | "ko_KR" | "ar_SA" | "ar_EG")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(day: u32, h: u32, m: u32) -> NaiveDateTime {
        // 4 November 2026 is a Wednesday.
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap().and_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn a_time_of_day_reads_on_either_clock() {
        let t = NaiveTime::from_hms_opt(16, 22, 40).unwrap();
        assert_eq!(time_of_day(t, false), "4:22 PM");
        assert_eq!(time_of_day(t, true), "16:22");
        let early = NaiveTime::from_hms_opt(0, 5, 0).unwrap();
        assert_eq!(time_of_day(early, false), "12:05 AM");
        assert_eq!(time_of_day(early, true), "00:05");
    }

    #[test]
    fn a_time_later_today_has_no_day_and_one_past_midnight_names_it() {
        assert_eq!(after(at(3, 22, 30), 72.0 * 60.0, false), "11:42 PM");
        assert_eq!(after(at(3, 22, 30), 72.0 * 60.0, true), "23:42");
        assert_eq!(after(at(3, 22, 30), 160.0 * 60.0, false), "Wed 1:10 AM");
        assert_eq!(after(at(3, 22, 30), 160.0 * 60.0, true), "Wed 01:10");
        assert_eq!(after(at(3, 22, 30), -5.0, false), "10:30 PM", "nothing left ends now");
    }

    #[test]
    fn a_few_regions_keep_a_twelve_hour_clock() {
        assert!(twelve_hour_locale("en_US.UTF-8"));
        assert!(twelve_hour_locale("en_AU"));
        assert!(!twelve_hour_locale("en_GB.UTF-8"));
        assert!(!twelve_hour_locale("de_DE.UTF-8"));
        assert!(!twelve_hour_locale("C"));
        // The system's own answer, whatever it is, comes without a fault.
        let _ = uses_24_hour();
    }
}
