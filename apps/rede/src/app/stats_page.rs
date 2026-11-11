//! The Stats place: the listening, counted.
//!
//! Four figures first (time listened, the streak, books finished, this
//! week), then a few things worth a smile (what kind of listener the hours
//! make, the words that went by, the biggest day and the longest sitting),
//! a calendar of the last year shaded by how much each day held, when in the
//! week and the day the listening happens, and the books, authors and
//! narrators that took the most of it.
//!
//! What's counted is `crate::stats`'s, worked out once per arrival, library,
//! positions and day; a frame only draws. The page asks for the listening
//! when it's opened (`RedeApp::load_stats`), and says when what it shows is
//! this computer's alone.

use super::Action;
use super::library::{COVER_RADIUS, stand_in};
use super::nav::Route;
use super::view_ctx::ViewCtx;
use crate::stats::{Coverage, NOVEL_WORDS, Ranked, Stats, amount, grouped, hour_name};
use crate::widgets::elide;
use chrono::{Datelike, Days, NaiveDate};
use egui::{FontId, Rect, RichText, Sense, Ui, pos2, vec2};
use shell::icons::{self, Icon};
use shell::kit;
use shell::theme::{Tokens, semibold};

const GAP: f32 = 12.0;
const TILE_MIN: f32 = 150.0;
/// The calendar's cells, and how many weeks it shows at most.
const CELL: f32 = 11.0;
const CELL_GAP: f32 = 3.0;
const WEEKS: usize = 53;
const CHART_H: f32 = 96.0;
/// Two columns (the charts, the authors beside the narrators) once each
/// gets this much.
const HALF_MIN: f32 = 300.0;
const ROW_H: f32 = 52.0;
const ROW_COVER: f32 = 40.0;
const SECTION_GAP: f32 = 28.0;
/// The weekday names, Monday first, as the stats count them.
const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

impl ViewCtx<'_> {
    pub(super) fn stats_page(&mut self, ui: &mut Ui) {
        if !self.stats.loaded {
            kit::page_heading(ui, "Stats", None);
            if self.stats.loading {
                let t = Tokens::of(ui);
                ui.label(RichText::new("Counting your listening…").size(13.0).color(t.muted));
            } else {
                kit::empty_state(ui, Icon::Chart, "No stats yet", "Your listening shows up here once Rede has your library.");
            }
            return;
        }
        let stats = self.stats.stats((self.books.version(), self.books.all()), (self.positions.version(), self.positions.all()), &now());
        let sub = match (stats.coverage, stats.first_day) {
            (Coverage::AllDevices, Some(first)) => Some(format!("Since {}", first.format("%B %Y"))),
            (Coverage::ThisComputer, _) => Some("On this computer".into()),
            _ => None,
        };
        kit::page_heading(ui, "Stats", sub);
        self.coverage_note(ui, &stats);
        if stats.total <= 0.0 {
            kit::empty_state(ui, Icon::Chart, "No listening yet", "Play a book, and how much, when and what you listen to shows up here.");
            return;
        }
        egui::ScrollArea::vertical().id_salt("stats").auto_shrink(false).show(ui, |ui| {
            let width = ui.available_width();
            tiles(ui, &stats, width);
            ui.add_space(SECTION_GAP);
            fun_facts(ui, &stats);
            ui.add_space(SECTION_GAP);
            calendar(ui, &stats, width);
            ui.add_space(SECTION_GAP);
            halves(ui, width, |ui, w| week_chart(ui, &stats, w), |ui, w| hour_chart(ui, &stats, w));
            ui.add_space(SECTION_GAP);
            self.top_books(ui, &stats.books, width.min(kit::LIST_W));
            ui.add_space(SECTION_GAP);
            halves(
                ui,
                width.min(kit::LIST_W * 1.4),
                |ui, w| ranking(ui, "Top authors", &stats.authors, w),
                |ui, w| ranking(ui, "Top narrators", &stats.narrators, w),
            );
            ui.add_space(24.0);
        });
    }

    /// Says when the figures are this computer's alone, and why.
    fn coverage_note(&self, ui: &mut Ui, stats: &Stats) {
        if stats.coverage == Coverage::AllDevices {
            return;
        }
        let t = Tokens::of(ui);
        let why = match &self.stats.missing {
            _ if self.sync.offline => "Can't reach the server, so only the listening on this computer is counted.".to_owned(),
            Some(e) if !e.is_network() => format!("Your listening history couldn't be loaded, so only this computer's is counted. {}", e.message()),
            _ => "Only the listening on this computer is counted.".to_owned(),
        };
        ui.horizontal_wrapped(|ui| {
            icons::inline(ui, if self.sync.offline { Icon::Offline } else { Icon::Chart }, t.muted);
            ui.label(RichText::new(why).size(12.5).color(t.muted));
        });
        ui.add_space(10.0);
    }

    /// The books that took the most listening, each opening its page when
    /// the library still has it.
    fn top_books(&mut self, ui: &mut Ui, books: &[Ranked], width: f32) {
        section_heading(ui, "Top books");
        let most = books.first().map_or(1.0, |b| b.secs).max(1.0);
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, ranked) in books.iter().enumerate() {
            let sense = if ranked.book.is_some() { Sense::click() } else { Sense::hover() };
            let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), sense);
            let book = ranked.book.as_deref().and_then(|id| self.books.by_id(id)).cloned();
            if ui.is_rect_visible(rect) {
                if ranked.book.is_some() {
                    ui.painter().rect_filled(rect, 6.0, kit::row_fill(&t, false, resp.hovered()));
                    kit::focus_ring(ui, &resp, rect, 6.0);
                }
                let rank = ui.painter().layout_no_wrap(format!("{}", i + 1), FontId::new(13.0, semibold()), t.muted);
                ui.painter().galley(pos2(rect.left() + 8.0, rect.center().y - rank.size().y / 2.0), rank, t.muted);
                let cover = Rect::from_min_size(pos2(rect.left() + 28.0, rect.center().y - ROW_COVER / 2.0), vec2(ROW_COVER, ROW_COVER));
                match &book {
                    Some(b) => self.art.paint(ui, cover, b.cover.as_deref(), COVER_RADIUS / 2.0, &stand_in(b)),
                    None => kit::generated_cover(ui, cover, COVER_RADIUS / 2.0, &kit::Generated { title: &ranked.name, byline: None, seed: &ranked.name }),
                }
                let x = cover.right() + 12.0;
                let figure = ui.painter().layout_no_wrap(amount(ranked.secs), FontId::proportional(12.5), t.muted);
                let right = rect.right() - 10.0;
                let room = (right - figure.size().x - 16.0 - x).max(20.0);
                let title = elide(ui, &ranked.name, FontId::proportional(13.0), t.text, room);
                let byline = elide(ui, ranked.detail.as_deref().unwrap_or("Unknown author"), FontId::proportional(12.0), t.muted, room);
                let top = rect.center().y - (title.size().y + byline.size().y + 9.0) / 2.0;
                let title_h = title.size().y;
                ui.painter().galley(pos2(x, top), title, t.text);
                ui.painter().galley(pos2(x, top + title_h + 2.0), byline, t.muted);
                let bar = Rect::from_min_size(pos2(x, rect.center().y + (title_h + 11.0) / 2.0 + 4.0), vec2(room, 3.0));
                kit::share_bar(ui, bar, (ranked.secs / most) as f32);
                ui.painter().galley(pos2(right - figure.size().x, rect.center().y - figure.size().y / 2.0), figure, t.muted);
            }
            let name = format!("{}. {}, {}", i + 1, ranked.name, amount(ranked.secs));
            let role = if ranked.book.is_some() { egui::Role::Button } else { egui::Role::Label };
            resp.widget_info(|| egui::WidgetInfo::labeled(role, true, &name));
            if resp.clicked()
                && let Some(id) = &ranked.book
            {
                self.actions.push(Action::Go(Route::Book(id.clone())));
            }
        }
    }
}

/// The four figures: time listened, the streak, books finished, this week.
fn tiles(ui: &mut Ui, s: &Stats, width: f32) {
    let days_nonstop = s.total / 86_400.0;
    let listened_note = if days_nonstop >= 1.0 {
        format!("{days_nonstop:.1} days nonstop")
    } else {
        format!("{} book{}", s.books_listened, if s.books_listened == 1 { "" } else { "s" })
    };
    let streak = days(s.streak);
    let longest = format!("Longest: {}", days(s.longest_streak));
    let finished = grouped(s.finished as u64);
    let this_year = format!("{} this year", s.finished_this_year);
    let week = amount(s.week);
    let today = format!("Today: {}", amount(s.today));
    let stats = [
        kit::Stat { label: "Listened", value: &amount(s.total), note: Some(&listened_note) },
        kit::Stat { label: "Streak", value: &streak, note: Some(&longest) },
        kit::Stat { label: "Finished", value: &finished, note: Some(&this_year) },
        kit::Stat { label: "Last 7 days", value: &week, note: Some(&today) },
    ];
    let (cols, w) = kit::tile_columns(width, TILE_MIN, GAP, stats.len());
    for row in stats.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for stat in row {
                kit::stat_tile(ui, w, stat);
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
}

/// A few lines worth a smile, each only when there's something to say.
fn fun_facts(ui: &mut Ui, s: &Stats) {
    section_heading(ui, "Fun facts");
    let t = Tokens::of(ui);
    let mut facts: Vec<(Option<&str>, String)> = Vec::new();
    if let Some((name, why)) = s.persona() {
        facts.push((Some(name), why));
    }
    let words = s.words();
    let novels = (words / NOVEL_WORDS).floor() as u64;
    let words_text = format!("about {} words", grouped(((words / 1_000.0).round() * 1_000.0) as u64));
    facts.push((
        None,
        match novels {
            0 => format!("That's {words_text} read to you."),
            1 => format!("That's {words_text} read to you: about a novel's worth."),
            n => format!("That's {words_text} read to you: some {} novels' worth.", grouped(n)),
        },
    ));
    if let Some((day, secs)) = s.best_day {
        facts.push((None, format!("Your biggest day was {}, with {}.", long_date(day, s.today_date), amount(secs))));
    }
    if let Some(sitting) = &s.longest_sitting {
        facts.push((None, format!("Your longest sitting: {} of {}, {}.", amount(sitting.secs), sitting.title, on_date(sitting.day, s.today_date))));
    }
    if let Some(book) = &s.longest_finished {
        facts.push((None, format!("The longest book you've finished is {}, at {}.", book.name, amount(book.secs))));
    }
    if s.active_days > 1 {
        let average = s.total / s.active_days as f64;
        facts.push((None, format!("{} days with listening, {} on an average one.", grouped(s.active_days as u64), amount(average))));
    }
    ui.scope(|ui| {
        ui.set_max_width(kit::LIST_W);
        ui.spacing_mut().item_spacing.y = 6.0;
        for (lead, text) in facts {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if let Some(lead) = lead {
                    ui.label(RichText::new(lead).font(FontId::new(13.5, semibold())).color(t.accent));
                }
                ui.label(RichText::new(text).size(13.5).color(t.text));
            });
        }
    });
}

/// The last year's days, a column a week (Monday at the top), shaded by how
/// much each held: as many weeks as the width has room for.
fn calendar(ui: &mut Ui, s: &Stats, width: f32) {
    section_heading(ui, "Listening by day");
    let mut weeks = WEEKS;
    while weeks > 4 && kit::level_grid_width(weeks, CELL, CELL_GAP) > width {
        weeks -= 1;
    }
    let today = s.today_date;
    let monday = today.checked_sub_days(Days::new(u64::from(today.weekday().num_days_from_monday()))).unwrap_or(today);
    let start = monday.checked_sub_days(Days::new(7 * (weeks as u64 - 1))).unwrap_or(monday);
    let day = |c: usize, r: usize| start.checked_add_days(Days::new((c * 7 + r) as u64));
    let level = |c: usize, r: usize| {
        let d = day(c, r).filter(|d| *d <= today)?;
        let secs = s.days.get(&d).copied().unwrap_or(0.0);
        Some(match secs {
            x if x <= 0.0 => 0,
            x if x < 15.0 * 60.0 => 1,
            x if x < 45.0 * 60.0 => 2,
            x if x < 90.0 * 60.0 => 3,
            _ => 4,
        })
    };
    let tip = |c: usize, r: usize| match day(c, r) {
        Some(d) => format!("{}: {}", long_date(d, today), amount(s.days.get(&d).copied().unwrap_or(0.0))),
        None => String::new(),
    };
    // A month is named over the week its first day falls in.
    let column_label = |c: usize| (0..7).filter_map(|r| day(c, r)).find(|d| d.day() == 1).map(|d| d.format("%b").to_string());
    let row_label = |r: usize| [0, 2, 4].contains(&r).then(|| WEEKDAYS[r][..3].to_owned());
    let active = s.days.range(start..=today).filter(|(_, v)| **v > 0.0).count();
    let name = format!("Listening by day over the last {weeks} weeks: {active} days with listening");
    kit::level_grid(
        ui,
        CELL,
        CELL_GAP,
        &kit::Grid { columns: weeks, rows: 7, level: &level, tip: &tip, column_label: &column_label, row_label: &row_label, name: &name },
    );
    ui.add_space(6.0);
    kit::level_legend(ui, CELL - 1.0);
}

fn week_chart(ui: &mut Ui, s: &Stats, width: f32) {
    section_heading(ui, "By day of the week");
    let label = |i: usize| Some(WEEKDAYS[i][..3].to_owned());
    let tip = |i: usize| format!("{}s: {}", WEEKDAYS[i], amount(s.weekdays[i]));
    let busiest = (0..7).max_by(|a, b| s.weekdays[*a].total_cmp(&s.weekdays[*b]).then(b.cmp(a))).unwrap_or(0);
    let name = format!("Listening by day of the week; most on {}s", WEEKDAYS[busiest]);
    kit::bar_chart(ui, width, CHART_H, &kit::Bars { values: &s.weekdays, label: &label, tip: &tip, name: &name });
}

fn hour_chart(ui: &mut Ui, s: &Stats, width: f32) {
    section_heading(ui, "By hour of the day");
    let label = |i: usize| i.is_multiple_of(6).then(|| short_hour(i));
    let tip = |i: usize| format!("{} to {}: {}", hour_name(i), hour_name((i + 1) % 24), amount(s.hours[i]));
    let peak = (0..24).max_by(|a, b| s.hours[*a].total_cmp(&s.hours[*b]).then(b.cmp(a))).unwrap_or(0);
    let name = format!("Listening by hour of the day; most around {}", hour_name(peak));
    kit::bar_chart(ui, width, CHART_H, &kit::Bars { values: &s.hours, label: &label, tip: &tip, name: &name });
}

/// Authors or narrators, the most listened first, each with its share of the
/// first's time.
fn ranking(ui: &mut Ui, title: &str, list: &[Ranked], width: f32) {
    section_heading(ui, title);
    let t = Tokens::of(ui);
    if list.is_empty() {
        ui.label(RichText::new("Nothing to rank yet.").size(12.5).color(t.muted));
        return;
    }
    let most = list[0].secs.max(1.0);
    for (i, ranked) in list.iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
        if ui.is_rect_visible(rect) {
            let figure = ui.painter().layout_no_wrap(amount(ranked.secs), FontId::proportional(12.0), t.muted);
            let room = (rect.width() - figure.size().x - 30.0).max(20.0);
            let name = elide(ui, &ranked.name, FontId::proportional(13.0), t.text, room);
            ui.painter().text(pos2(rect.left(), rect.top() + 2.0), egui::Align2::LEFT_TOP, format!("{}", i + 1), FontId::new(12.0, semibold()), t.muted);
            ui.painter().galley(pos2(rect.left() + 18.0, rect.top() + 1.0), name, t.text);
            ui.painter().galley(pos2(rect.right() - figure.size().x, rect.top() + 2.0), figure, t.muted);
            let bar = Rect::from_min_size(pos2(rect.left() + 18.0, rect.top() + 22.0), vec2(rect.width() - 18.0, 3.0));
            kit::share_bar(ui, bar, (ranked.secs / most) as f32);
        }
        let name = format!("{}. {}, {}", i + 1, ranked.name, amount(ranked.secs));
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &name));
    }
}

#[cfg(not(test))]
fn now() -> chrono::DateTime<chrono::Local> {
    chrono::Local::now()
}
/// The fixture's clock, in UTC, so tests and snapshots count the same days on
/// any machine.
#[cfg(test)]
fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::TimeZone::from_utc_datetime(&chrono::Utc, &super::fixture::wall_clock())
}

/// Two things side by side when each gets [`HALF_MIN`], else one over the
/// other.
fn halves(ui: &mut Ui, width: f32, left: impl FnOnce(&mut Ui, f32), right: impl FnOnce(&mut Ui, f32)) {
    let gap = 32.0;
    if width >= 2.0 * HALF_MIN + gap {
        let w = (width - gap) / 2.0;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.allocate_ui_with_layout(vec2(w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| left(ui, w));
            ui.allocate_ui_with_layout(vec2(w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| right(ui, w));
        });
    } else {
        left(ui, width);
        ui.add_space(SECTION_GAP);
        right(ui, width);
    }
}

fn section_heading(ui: &mut Ui, title: &str) {
    let t = Tokens::of(ui);
    ui.label(RichText::new(title).font(FontId::new(17.0, semibold())).color(t.text));
    ui.add_space(10.0);
}

/// "1 day", "12 days".
fn days(n: u32) -> String {
    format!("{n} day{}", if n == 1 { "" } else { "s" })
}

/// "12a", "6a", "12p", "6p": an hour under a narrow bar.
fn short_hour(h: usize) -> String {
    match h {
        0 => "12a".into(),
        12 => "12p".into(),
        1..12 => format!("{h}a"),
        _ => format!("{}p", h - 12),
    }
}

/// "Saturday 7 March", with the year only when it isn't this one.
fn long_date(day: NaiveDate, today: NaiveDate) -> String {
    if day.year() == today.year() { day.format("%A %-d %B").to_string() } else { day.format("%A %-d %B %Y").to_string() }
}

/// "today", "yesterday", or "on Saturday 7 March".
fn on_date(day: NaiveDate, today: NaiveDate) -> String {
    if day == today {
        "today".into()
    } else if today.pred_opt() == Some(day) {
        "yesterday".into()
    } else {
        format!("on {}", long_date(day, today))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_drop_the_year_they_share_with_today_and_say_today_and_yesterday() {
        let today = NaiveDate::from_ymd_opt(2026, 3, 9).unwrap();
        assert_eq!(long_date(NaiveDate::from_ymd_opt(2026, 3, 7).unwrap(), today), "Saturday 7 March");
        assert_eq!(long_date(NaiveDate::from_ymd_opt(2025, 3, 7).unwrap(), today), "Friday 7 March 2025");
        assert_eq!(on_date(today, today), "today");
        assert_eq!(on_date(today.pred_opt().unwrap(), today), "yesterday");
        assert_eq!(on_date(NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(), today), "on Sunday 1 March");
        assert_eq!((short_hour(0), short_hour(6), short_hour(12), short_hour(18)), ("12a".into(), "6a".into(), "12p".into(), "6p".into()));
    }
}
