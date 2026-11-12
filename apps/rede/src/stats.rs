//! Listening stats: how much, when, and what, worked out from the listening
//! the server remembers on every device and what this computer logged.
//!
//! The server's sessions are the whole history. What this computer heard and
//! hasn't told the server yet is added on top, so a book listened to offline
//! counts before it syncs; what it has told the server is already in the
//! sessions and isn't counted twice. Without the server's history (offline,
//! or a source that keeps none) the local log alone is used, and
//! [`Coverage`] says so for the page to tell the user.
//!
//! [`Stats::compute`] is a pure function of its inputs, the time zone and the
//! moment called "now", so the page memoises one [`Stats`] per change and a
//! test can fix the clock. Days and hours are the user's local ones. A stretch
//! of listening counts towards the day it started on; hours are split where
//! they fall, so a chapter that crosses midnight shows at 11 pm and at
//! midnight both.

use crate::failure::Failure;
use crate::memo::Memo;
use chrono::{DateTime, Datelike, Days, NaiveDate, TimeZone, Timelike};
use shelf::Logged;
use sources::{ItemSummary, Listen, Progress, Stamp};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Reports of the same book this close together are one sitting.
const SITTING_GAP_MS: i64 = 10 * 60 * 1000;
/// A day counts towards a streak once a minute of it was listening: a
/// play pressed by accident doesn't keep one alive.
const STREAK_DAY_SECS: f64 = 60.0;
/// Narrators read at about this many words a minute.
pub const WORDS_PER_MINUTE: f64 = 155.0;
/// A novel, in words: what "novels' worth" counts in.
pub const NOVEL_WORDS: f64 = 90_000.0;
/// How many of each ranking the page shows.
pub const TOP: usize = 5;
/// The page asks the server again when it's opened this long after the last
/// answer; sooner, it shows what it has.
const FRESH_FOR: Duration = Duration::from_secs(5 * 60);

/// Whose listening the stats count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// The server's history, with this computer's unsent listening on top.
    AllDevices,
    /// This computer's log only: the server's history couldn't be had.
    ThisComputer,
}

/// One stretch of listening: a server session or a local report.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub item: String,
    pub start_ms: i64,
    pub secs: f64,
}

/// The stretches the stats count, and whose they are. A local report is
/// stamped when it was made, at the end of what it counts, so it starts that
/// long before.
pub fn spans(history: Option<&[Listen]>, log: &[Logged]) -> (Vec<Span>, Coverage) {
    let local = |l: &Logged| Span {
        item: l.item.clone(),
        start_ms: Stamp::from(l.at).millis() - i64::try_from(l.listened.as_millis()).unwrap_or(0),
        secs: l.listened.as_secs_f64(),
    };
    match history {
        Some(history) => {
            let mut out: Vec<Span> =
                history.iter().map(|h| Span { item: h.item.clone(), start_ms: h.started.millis(), secs: h.listened.as_secs_f64() }).collect();
            out.extend(log.iter().filter(|l| !l.synced).map(local));
            (out, Coverage::AllDevices)
        }
        None => (log.iter().map(local).collect(), Coverage::ThisComputer),
    }
}

/// A book, an author or a narrator, with the time spent on them.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    pub name: String,
    /// The book's own id, when it's a book the library has.
    pub book: Option<String>,
    pub secs: f64,
    /// A book's author.
    pub detail: Option<String>,
}

/// The longest time spent on one book in one go.
#[derive(Debug, Clone, PartialEq)]
pub struct Sitting {
    pub title: String,
    pub secs: f64,
    pub day: NaiveDate,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    pub coverage: Coverage,
    pub total: f64,
    pub today: f64,
    /// The last seven days, today among them.
    pub week: f64,
    /// This calendar month so far.
    pub month: f64,
    pub year: f64,
    pub today_date: NaiveDate,
    /// Seconds listened per local day.
    pub days: BTreeMap<NaiveDate, f64>,
    pub first_day: Option<NaiveDate>,
    pub active_days: usize,
    /// Days in a row up to today, or up to yesterday while today has none
    /// yet: a streak isn't over until a day passes without listening.
    pub streak: u32,
    pub longest_streak: u32,
    pub best_day: Option<(NaiveDate, f64)>,
    /// Monday first.
    pub weekdays: [f64; 7],
    pub hours: [f64; 24],
    pub sittings: usize,
    pub longest_sitting: Option<Sitting>,
    pub books: Vec<Ranked>,
    pub authors: Vec<Ranked>,
    pub narrators: Vec<Ranked>,
    /// Books listened to at all.
    pub books_listened: usize,
    pub finished: usize,
    pub finished_this_year: usize,
    /// The longest book finished, with its length.
    pub longest_finished: Option<Ranked>,
}

impl Stats {
    /// The stats of `spans` as of `now`, in `now`'s time zone. `history` names
    /// books the library no longer lists; `positions` say which are finished.
    pub fn compute<'a, Tz: TimeZone>(
        spans: &[Span],
        coverage: Coverage,
        history: Option<&[Listen]>,
        books: &[ItemSummary],
        positions: impl IntoIterator<Item = &'a Progress>,
        now: &DateTime<Tz>,
    ) -> Self {
        let tz = now.timezone();
        let today = now.date_naive();
        let local = |ms: i64| tz.timestamp_millis_opt(ms).earliest();
        let by_id: HashMap<&str, &ItemSummary> = books.iter().map(|b| (b.id.as_str(), b)).collect();
        let mut said: HashMap<&str, (Option<&str>, Option<&str>)> = HashMap::new();
        for h in history.unwrap_or_default() {
            let entry = said.entry(h.item.as_str()).or_default();
            entry.0 = entry.0.or(h.title.as_deref());
            entry.1 = entry.1.or(h.author.as_deref());
        }
        let title = |id: &str| -> String {
            by_id.get(id).map(|b| b.title.clone()).or_else(|| said.get(id).and_then(|s| s.0).map(str::to_owned)).unwrap_or_else(|| "Unknown book".into())
        };

        let mut days: BTreeMap<NaiveDate, f64> = BTreeMap::new();
        let mut weekdays = [0.0; 7];
        let mut hours = [0.0; 24];
        let mut per_book: HashMap<&str, f64> = HashMap::new();
        let mut total = 0.0;
        for span in spans {
            let Some(start) = local(span.start_ms) else { continue };
            let secs = span.secs.max(0.0);
            total += secs;
            *days.entry(start.date_naive()).or_default() += secs;
            weekdays[start.weekday().num_days_from_monday() as usize] += secs;
            *per_book.entry(span.item.as_str()).or_default() += secs;
            spread_over_hours(&mut hours, start, secs);
        }

        let since = |from: NaiveDate| days.range(from..=today).map(|(_, s)| s).sum::<f64>();
        let week = since(today.checked_sub_days(Days::new(6)).unwrap_or(today));
        let month = since(today.with_day(1).unwrap_or(today));
        let year = since(NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap_or(today));
        let active: HashSet<NaiveDate> = days.iter().filter(|(_, s)| **s >= STREAK_DAY_SECS).map(|(d, _)| *d).collect();
        let (streak, longest_streak) = streaks(&active, today);
        let best_day = days.iter().max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0))).map(|(d, s)| (*d, *s));

        let (sittings, longest) = sittings(spans);
        let longest_sitting = longest.and_then(|(item, start, secs)| Some(Sitting { title: title(item), secs, day: local(start)?.date_naive() }));

        let mut ranked_books: Vec<Ranked> = per_book
            .iter()
            .map(|(id, secs)| Ranked {
                name: title(id),
                book: by_id.contains_key(id).then(|| (*id).to_owned()),
                secs: *secs,
                detail: by_id.get(id).and_then(|b| b.authors.first().cloned()).or_else(|| said.get(id).and_then(|s| s.1).map(str::to_owned)),
            })
            .collect();
        let mut authors: HashMap<String, f64> = HashMap::new();
        let mut narrators: HashMap<String, f64> = HashMap::new();
        for (id, secs) in &per_book {
            match by_id.get(id) {
                Some(book) => {
                    for a in &book.authors {
                        *authors.entry(a.clone()).or_default() += secs;
                    }
                    for n in &book.narrators {
                        *narrators.entry(n.clone()).or_default() += secs;
                    }
                }
                None => {
                    if let Some(a) = said.get(id).and_then(|s| s.1).filter(|a| !a.is_empty()) {
                        *authors.entry(a.to_owned()).or_default() += secs;
                    }
                }
            }
        }

        let mut finished = 0;
        let mut finished_this_year = 0;
        let mut longest_finished: Option<Ranked> = None;
        for p in positions.into_iter().filter(|p| p.finished) {
            finished += 1;
            if p.at.and_then(|at| local(Stamp::from(at).millis())).is_some_and(|at| at.year() == today.year()) {
                finished_this_year += 1;
            }
            if let Some(book) = by_id.get(p.item.item.as_str())
                && let Some(d) = book.duration.filter(|d| *d > 0.0)
                && longest_finished.as_ref().is_none_or(|l| d > l.secs)
            {
                longest_finished = Some(Ranked { name: book.title.clone(), book: Some(book.id.clone()), secs: d, detail: book.authors.first().cloned() });
            }
        }

        Self {
            coverage,
            total,
            today: days.get(&today).copied().unwrap_or(0.0),
            week,
            month,
            year,
            today_date: today,
            first_day: days.keys().next().copied(),
            active_days: active.len(),
            streak,
            longest_streak,
            best_day,
            weekdays,
            hours,
            sittings,
            longest_sitting,
            books_listened: per_book.len(),
            books: top(ranked_books.drain(..)),
            authors: top(authors.into_iter().map(|(name, secs)| Ranked { name, book: None, secs, detail: None })),
            narrators: top(narrators.into_iter().map(|(name, secs)| Ranked { name, book: None, secs, detail: None })),
            days,
            finished,
            finished_this_year,
            longest_finished,
        }
    }

    /// About how many words went by, at a narrator's pace.
    pub fn words(&self) -> f64 {
        self.total / 60.0 * WORDS_PER_MINUTE
    }

    /// The listener the hours and days make: a name and why.
    pub fn persona(&self) -> Option<(&'static str, String)> {
        if self.total < 3_600.0 {
            return None;
        }
        let share = |hours: &[usize]| hours.iter().map(|&h| self.hours[h]).sum::<f64>() / self.total;
        let percent = |f: f64| (f * 100.0).round() as u32;
        let night = share(&[22, 23, 0, 1, 2, 3]);
        let morning = share(&[5, 6, 7, 8]);
        let weekend = (self.weekdays[5] + self.weekdays[6]) / self.total;
        Some(if night >= 0.35 {
            ("Night owl", format!("{}% of your listening is after 10 pm.", percent(night)))
        } else if morning >= 0.3 {
            ("Early bird", format!("{}% of your listening is before 9 am.", percent(morning)))
        } else if weekend >= 0.5 {
            ("Weekend binger", format!("{}% of your listening is on Saturdays and Sundays.", percent(weekend)))
        } else {
            let peak = (0..24).max_by(|a, b| self.hours[*a].total_cmp(&self.hours[*b]).then(b.cmp(a))).unwrap_or(0);
            ("Steady listener", format!("You listen most around {}.", hour_name(peak)))
        })
    }
}

/// The current streak (ending today, or yesterday while today has none) and
/// the longest run of active days.
fn streaks(active: &HashSet<NaiveDate>, today: NaiveDate) -> (u32, u32) {
    let run_back = |mut day: NaiveDate| {
        let mut n = 0;
        while active.contains(&day) {
            n += 1;
            let Some(before) = day.pred_opt() else { break };
            day = before;
        }
        n
    };
    let current = if active.contains(&today) { run_back(today) } else { today.pred_opt().map_or(0, run_back) };
    // A run's length is counted from its first day only.
    let longest = active.iter().filter(|d| d.pred_opt().is_none_or(|p| !active.contains(&p))).map(|d| {
        let mut n = 0;
        let mut day = *d;
        while active.contains(&day) {
            n += 1;
            let Some(next) = day.succ_opt() else { break };
            day = next;
        }
        n
    });
    (current, longest.max().unwrap_or(0))
}

/// How many sittings there were, and the longest: (book, start, seconds).
/// Spans of one book with less than [`SITTING_GAP_MS`] between them are one.
fn sittings(spans: &[Span]) -> (usize, Option<(&str, i64, f64)>) {
    let mut sorted: Vec<&Span> = spans.iter().collect();
    sorted.sort_by(|a, b| (&a.item, a.start_ms).cmp(&(&b.item, b.start_ms)));
    type Open<'s> = Option<(&'s str, i64, i64, f64)>;
    fn close<'s>(c: Open<'s>, count: &mut usize, longest: &mut Option<(&'s str, i64, f64)>) {
        if let Some((item, start, _, secs)) = c {
            *count += 1;
            if longest.is_none_or(|l| secs > l.2) {
                *longest = Some((item, start, secs));
            }
        }
    }
    let mut count = 0;
    let mut longest = None;
    let mut current: Open = None;
    for s in sorted {
        let end = s.start_ms + (s.secs * 1000.0) as i64;
        match current.as_mut() {
            Some((item, _, last_end, secs)) if *item == s.item.as_str() && s.start_ms <= *last_end + SITTING_GAP_MS => {
                *last_end = (*last_end).max(end);
                *secs += s.secs;
            }
            _ => {
                close(current.take(), &mut count, &mut longest);
                current = Some((s.item.as_str(), s.start_ms, end, s.secs));
            }
        }
    }
    close(current, &mut count, &mut longest);
    (count, longest)
}

/// Adds `secs` from `start` to the hours of the day they fall in. A stretch
/// longer than a day is someone who left it playing; a day of it is plenty.
fn spread_over_hours<Tz: TimeZone>(hours: &mut [f64; 24], start: DateTime<Tz>, secs: f64) {
    let mut at = start;
    let mut left = secs.min(86_400.0);
    while left > 0.0 {
        let into = f64::from(at.minute() * 60 + at.second()) + f64::from(at.nanosecond() / 1_000_000) / 1000.0;
        let take = (3_600.0 - into).clamp(0.001, left);
        hours[at.hour() as usize] += take;
        left -= take;
        at += chrono::Duration::milliseconds((take * 1000.0).ceil() as i64);
    }
}

/// The first [`TOP`], most time first; ties by name, so the order is stable.
fn top(items: impl Iterator<Item = Ranked>) -> Vec<Ranked> {
    let mut all: Vec<Ranked> = items.collect();
    all.sort_by(|a, b| b.secs.total_cmp(&a.secs).then_with(|| a.name.cmp(&b.name)));
    all.truncate(TOP);
    all
}

/// "midnight", "9 am", "noon", "10 pm".
pub fn hour_name(hour: usize) -> String {
    match hour {
        0 => "midnight".into(),
        12 => "noon".into(),
        1..12 => format!("{hour} am"),
        _ => format!("{} pm", hour - 12),
    }
}

/// An amount of listening: "42 min", "3 hr 5 min", "1,204 hr".
pub fn amount(secs: f64) -> String {
    if secs < 60.0 {
        return if secs > 0.0 { "under a minute".into() } else { "none".into() };
    }
    if secs < 10.0 * 3_600.0 {
        return crate::fmt::span(secs - secs % 60.0);
    }
    format!("{} hr", grouped((secs / 3_600.0).round() as u64))
}

/// A count with thousands set apart: "1,204".
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What the page knows of the listening, and the stats made from it.
#[derive(Default)]
pub struct State {
    /// The server's sessions; `None` until they arrive, or when they can't.
    pub history: Option<Arc<Vec<Listen>>>,
    pub log: Arc<Vec<Logged>>,
    /// Why the server's history isn't here, when it was asked for and isn't.
    pub missing: Option<Failure>,
    /// Something has arrived since the page was first opened.
    pub loaded: bool,
    pub loading: bool,
    asked: Option<Instant>,
    /// Bumped by every arrival, so the stats are memoised on it.
    version: u64,
    memo: Memo<(u64, u64, u64, NaiveDate), Stats>,
}

impl State {
    /// Whether the page should ask again: never asked, or not lately.
    pub fn due(&self) -> bool {
        !self.loading && self.asked.is_none_or(|at| at.elapsed() >= FRESH_FOR)
    }

    pub fn asking(&mut self) {
        self.loading = true;
        self.asked = Some(Instant::now());
    }

    /// What the bridge brought: the local log, and the server's history or
    /// why there's none (`None`: the source wasn't asked).
    pub fn arrived(&mut self, log: Vec<Logged>, history: Option<Result<Vec<Listen>, Failure>>) {
        self.loading = false;
        self.loaded = true;
        self.version += 1;
        self.log = Arc::new(log);
        match history {
            Some(Ok(history)) => {
                self.history = Some(Arc::new(history));
                self.missing = None;
            }
            // A history from an earlier answer is still the better one.
            Some(Err(e)) => self.missing = Some(e),
            None => {}
        }
    }

    /// The stats as of `now`, worked out once per arrival, library,
    /// positions and day.
    pub fn stats<'a, Tz: TimeZone>(
        &mut self,
        books: (u64, &[ItemSummary]),
        positions: (u64, impl IntoIterator<Item = &'a Progress>),
        now: &DateTime<Tz>,
    ) -> Arc<Stats> {
        let (history, log) = (self.history.clone(), self.log.clone());
        self.memo.get((self.version, books.0, positions.0, now.date_naive()), || {
            let (spans, coverage) = spans(history.as_deref().map(Vec::as_slice), &log);
            Stats::compute(&spans, coverage, history.as_deref().map(Vec::as_slice), books.1, positions.1, now)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, Utc};
    use sources::{AccountId, ItemKey, Position};

    const HOUR: f64 = 3_600.0;

    /// Noon UTC on `day` of March 2026, `h` hours on.
    fn at(day: u32, h: f64) -> i64 {
        Utc.with_ymd_and_hms(2026, 3, day, 12, 0, 0).unwrap().timestamp_millis() + (h * HOUR * 1000.0) as i64
    }

    fn span(item: &str, start_ms: i64, secs: f64) -> Span {
        Span { item: item.into(), start_ms, secs }
    }

    fn now(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, day, 20, 0, 0).unwrap()
    }

    fn book(id: &str, title: &str, author: &str, narrator: &str, duration: f64) -> ItemSummary {
        ItemSummary {
            id: id.into(),
            title: title.into(),
            authors: vec![author.into()],
            narrators: vec![narrator.into()],
            duration: Some(duration),
            ..ItemSummary::default()
        }
    }

    fn finished(id: &str, ms: i64) -> Progress {
        Progress {
            item: ItemKey::new(AccountId::from_uuid(uuid::Uuid::nil()), id),
            position: Position::from_secs(1.0),
            finished: true,
            at: Some(Stamp::from_millis(ms).system_time()),
        }
    }

    fn stats(spans: &[Span], books: &[ItemSummary], positions: &[Progress], day: u32) -> Stats {
        Stats::compute(spans, Coverage::AllDevices, None, books, positions, &now(day))
    }

    #[test]
    fn the_servers_history_counts_with_only_the_local_listening_it_hasnt_heard() {
        let history = [Listen { item: "a".into(), title: None, author: None, started: Stamp::from_millis(1_000), listened: Duration::from_secs(60) }];
        let logged = |synced, at_ms| Logged { item: "a".into(), at: Stamp::from_millis(at_ms).system_time(), listened: Duration::from_secs(30), synced };
        let log = [logged(true, 61_000), logged(false, 200_000)];
        let (all, coverage) = spans(Some(&history), &log);
        assert_eq!(coverage, Coverage::AllDevices);
        assert_eq!(all, [span("a", 1_000, 60.0), span("a", 170_000, 30.0)], "a report starts as long before its stamp as it counts");
        let (local, coverage) = spans(None, &log);
        assert_eq!(coverage, Coverage::ThisComputer);
        assert_eq!(local.len(), 2, "without the server, everything this computer heard");
    }

    #[test]
    fn totals_count_today_the_last_seven_days_the_month_and_the_year() {
        let spans = [span("a", at(20, 0.0), HOUR), span("a", at(14, 0.0), 2.0 * HOUR), span("a", at(13, 0.0), 4.0 * HOUR), span("a", at(1, 0.0), 8.0 * HOUR)];
        let s = stats(&spans, &[], &[], 20);
        assert_eq!(s.total, 15.0 * HOUR);
        assert_eq!(s.today, HOUR);
        assert_eq!(s.week, 3.0 * HOUR, "the 14th is six days back, the 13th seven");
        assert_eq!(s.month, 15.0 * HOUR);
        assert_eq!(s.best_day, Some((NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(), 8.0 * HOUR)));
        assert_eq!(s.first_day, NaiveDate::from_ymd_opt(2026, 3, 1));
    }

    #[test]
    fn a_streak_lives_through_today_until_a_day_passes_without_listening() {
        let days = |list: &[u32]| list.iter().map(|d| span("a", at(*d, 0.0), 600.0)).collect::<Vec<_>>();
        let s = stats(&days(&[1, 2, 3, 4, 10, 11, 12]), &[], &[], 13);
        assert_eq!((s.streak, s.longest_streak), (3, 4), "nothing yet today, and yesterday's run still counts");
        assert_eq!(stats(&days(&[10, 11, 12, 13]), &[], &[], 13).streak, 4);
        assert_eq!(stats(&days(&[10, 11]), &[], &[], 13).streak, 0, "a whole day missed ends it");
        let brief = [span("a", at(12, 0.0), 20.0)];
        assert_eq!(stats(&brief, &[], &[], 12).streak, 0, "a few seconds isn't a day of listening");
    }

    #[test]
    fn days_and_hours_are_the_listeners_own() {
        // 23:30 UTC on the 2nd is 01:30 on the 3rd two hours east.
        let late = Utc.with_ymd_and_hms(2026, 3, 2, 23, 30, 0).unwrap().timestamp_millis();
        let spans = [span("a", late, HOUR)];
        let east = FixedOffset::east_opt(2 * 3_600).unwrap();
        let s = Stats::compute(&spans, Coverage::AllDevices, None, &[], [], &now(5).with_timezone(&east));
        assert_eq!(s.days.keys().next(), NaiveDate::from_ymd_opt(2026, 3, 3).as_ref());
        assert_eq!((s.hours[1], s.hours[2]), (1_800.0, 1_800.0), "split where the hour turns");
        assert_eq!(s.weekdays[1], HOUR, "the 3rd of March 2026 is a Tuesday");
    }

    #[test]
    fn reports_close_together_make_one_sitting() {
        let mut spans: Vec<Span> = (0..6).map(|i| span("a", at(5, 0.0) + i * 300_000, 300.0)).collect();
        spans.push(span("a", at(5, 3.0), 600.0));
        spans.push(span("b", at(5, 0.1), 900.0));
        let s = stats(&spans, &[book("a", "Dune", "Herbert", "Brick", 10.0 * HOUR)], &[], 5);
        assert_eq!(s.sittings, 3);
        let longest = s.longest_sitting.unwrap();
        assert_eq!((longest.title.as_str(), longest.secs), ("Dune", 1_800.0));
    }

    #[test]
    fn rankings_credit_every_author_and_narrator_and_name_books_the_library_lost() {
        let books =
            [book("a", "Dune", "Frank Herbert", "Scott Brick", 20.0 * HOUR), book("b", "Children of Dune", "Frank Herbert", "Simon Vance", 30.0 * HOUR)];
        let history = [Listen {
            item: "gone".into(),
            title: Some("Earthsea".into()),
            author: Some("Ursula K. Le Guin".into()),
            started: Stamp::from_millis(at(3, 0.0)),
            listened: Duration::from_secs(10_800),
        }];
        let spans = [span("a", at(1, 0.0), 2.0 * HOUR), span("b", at(2, 0.0), HOUR), span("gone", at(3, 0.0), 3.0 * HOUR)];
        let positions = [finished("a", at(4, 0.0)), finished("b", Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap().timestamp_millis())];
        let s = Stats::compute(&spans, Coverage::AllDevices, Some(&history), &books, &positions, &now(10));
        let names = |r: &[Ranked]| r.iter().map(|r| (r.name.clone(), r.secs / HOUR)).collect::<Vec<_>>();
        assert_eq!(names(&s.books), [("Earthsea".into(), 3.0), ("Dune".into(), 2.0), ("Children of Dune".into(), 1.0)]);
        assert_eq!(s.books[0].book, None, "not in the library, so nothing to open");
        assert_eq!(s.books[1].book.as_deref(), Some("a"));
        assert_eq!(names(&s.authors), [("Frank Herbert".into(), 3.0), ("Ursula K. Le Guin".into(), 3.0)]);
        assert_eq!(names(&s.narrators), [("Scott Brick".into(), 2.0), ("Simon Vance".into(), 1.0)]);
        assert_eq!((s.finished, s.finished_this_year, s.books_listened), (2, 1, 3));
        assert_eq!(s.longest_finished.unwrap().name, "Children of Dune");
    }

    #[test]
    fn the_hours_say_what_kind_of_listener_this_is() {
        let night = [span("a", Utc.with_ymd_and_hms(2026, 3, 4, 23, 0, 0).unwrap().timestamp_millis(), 2.0 * HOUR), span("a", at(4, 0.0), HOUR)];
        assert_eq!(stats(&night, &[], &[], 5).persona().unwrap().0, "Night owl");
        let steady = [span("a", at(4, 2.0), 2.0 * HOUR)];
        assert_eq!(stats(&steady, &[], &[], 5).persona().unwrap().1, "You listen most around 2 pm.");
        assert_eq!(stats(&[span("a", at(4, 0.0), 600.0)], &[], &[], 5).persona(), None, "too little to tell");
    }

    #[test]
    fn amounts_read_in_minutes_then_hours() {
        assert_eq!(amount(0.0), "none");
        assert_eq!(amount(42.0 * 60.0 + 30.0), "42 min");
        assert_eq!(amount(3.0 * HOUR + 300.0), "3 hr 5 min");
        assert_eq!(amount(1_204.0 * HOUR), "1,204 hr");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!((hour_name(0), hour_name(9), hour_name(12), hour_name(22)), ("midnight".into(), "9 am".into(), "noon".into(), "10 pm".into()));
    }
}
