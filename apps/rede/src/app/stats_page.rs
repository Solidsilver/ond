//! The Stats place: the listening, counted.
//!
//! Four figures first (time listened, the streak, books finished, this
//! week), then a card of things worth a smile (what kind of listener the
//! hours make, the words that went by, the biggest day and the longest
//! sitting), a calendar of the last year shaded by how much each day held,
//! when in the week and the day the listening happens, and the books,
//! authors and narrators that took the most of it.
//!
//! Each part is a card, and the page lays them out by its width: up to
//! [`MAX_W`] across (wider, and lines and bars stretch past reading), the
//! tiles four across or two by two, the calendar's cells grown to fill its
//! card or its weeks cut to fit, the charts and the rankings side by side
//! when each gets [`HALF_MIN`], and the books beside the authors and
//! narrators once there's room for all three ([`THREE_UP`]). Cards side by
//! side come out level.
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
use egui::text::{LayoutJob, TextFormat};
use egui::{Align, FontId, Layout, Rect, RichText, Sense, Ui, pos2, vec2};
use shell::icons::{self, Icon};
use shell::kit;
use shell::theme::{Tokens, semibold};

/// The widest the page's cards grow.
const MAX_W: f32 = 1320.0;
/// Between cards, and between tiles.
const GAP: f32 = 16.0;
const TILE_MIN: f32 = 170.0;
/// The calendar's cells: grown up to the largest to fill the card, and at
/// the smallest, weeks are dropped instead (down to [`MIN_WEEKS`]).
const CELL_MIN: f32 = 9.0;
const CELL_MAX: f32 = 17.0;
const CELL_GAP: f32 = 3.0;
const WEEKS: usize = 53;
const MIN_WEEKS: usize = 4;
const CHART_H: f32 = 120.0;
/// Two cards side by side once each gets this much.
const HALF_MIN: f32 = 320.0;
/// The top books beside the top authors and narrators from this width.
const THREE_UP: f32 = 1060.0;
/// Facts in two columns once the fun-facts card is this wide inside.
const FACT_COLUMNS_MIN: f32 = 640.0;
const FACT_ICON: f32 = 28.0;
/// A book's row, and an author's or narrator's when it stands alone (beside
/// the books, theirs take the books' height so the rows line up).
const ROW_H: f32 = 52.0;
const RANK_H: f32 = 38.0;
const ROW_COVER: f32 = 40.0;
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
            let width = ui.available_width().min(MAX_W);
            tiles(ui, &stats, width);
            gap(ui);
            fun_facts(ui, &stats, width);
            gap(ui);
            calendar(ui, &stats, width);
            gap(ui);
            pair(ui, width, &mut |ui, w| week_chart(ui, &stats, w), &mut |ui, w| hour_chart(ui, &stats, w));
            gap(ui);
            self.rankings(ui, &stats, width);
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

    /// The books, authors and narrators listened to most: all three across
    /// when there's room, else the books over the other two.
    fn rankings(&mut self, ui: &mut Ui, s: &Stats, width: f32) {
        let tallest = |row_h: f32| {
            let rows = s.books.len().max(s.authors.len()).max(s.narrators.len()).max(1);
            kit::CARD_CHROME + kit::CARD_HEADING_H + rows as f32 * row_h
        };
        if width >= THREE_UP {
            let h = tallest(ROW_H);
            columns(
                ui,
                width,
                &mut [
                    (2.0, &mut |ui: &mut Ui, w| kit::card(ui, w, h, |ui, w| self.top_books(ui, &s.books, w))),
                    (1.0, &mut |ui: &mut Ui, w| ranking(ui, "Top authors", &s.authors, w, ROW_H, h)),
                    (1.0, &mut |ui: &mut Ui, w| ranking(ui, "Top narrators", &s.narrators, w, ROW_H, h)),
                ],
            );
        } else {
            kit::card(ui, width, 0.0, |ui, w| self.top_books(ui, &s.books, w));
            gap(ui);
            let h = tallest(RANK_H);
            pair(ui, width, &mut |ui, w| ranking(ui, "Top authors", &s.authors, w, RANK_H, h), &mut |ui, w| {
                ranking(ui, "Top narrators", &s.narrators, w, RANK_H, h)
            });
        }
    }

    /// The books that took the most listening, each opening its page when
    /// the library still has it.
    fn top_books(&mut self, ui: &mut Ui, books: &[Ranked], width: f32) {
        kit::card_heading(ui, "Top books", None);
        let most = books.first().map_or(1.0, |b| b.secs).max(1.0);
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, ranked) in books.iter().enumerate() {
            let sense = if ranked.book.is_some() { Sense::click() } else { Sense::hover() };
            let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), sense);
            let book = ranked.book.as_deref().and_then(|id| self.books.by_id(id)).cloned();
            if ui.is_rect_visible(rect) {
                if ranked.book.is_some() {
                    ui.painter().rect_filled(rect.expand2(vec2(8.0, 0.0)), 8.0, kit::row_fill(&t, false, resp.hovered()));
                    kit::focus_ring(ui, &resp, rect.expand2(vec2(8.0, 0.0)), 8.0);
                }
                rank_number(ui, i, rect.left(), rect.center().y, &t);
                let cover = Rect::from_min_size(pos2(rect.left() + 24.0, rect.center().y - ROW_COVER / 2.0), vec2(ROW_COVER, ROW_COVER));
                match &book {
                    Some(b) => self.art.paint(ui, cover, b.cover.as_deref(), COVER_RADIUS / 2.0, &stand_in(b)),
                    None => kit::generated_cover(ui, cover, COVER_RADIUS / 2.0, &kit::Generated { title: &ranked.name, byline: None, seed: &ranked.name }),
                }
                let x = cover.right() + 12.0;
                let figure = ui.painter().layout_no_wrap(amount(ranked.secs), FontId::new(12.5, semibold()), t.text);
                let right = rect.right();
                let room = (right - figure.size().x - 16.0 - x).max(20.0);
                let title = elide(ui, &ranked.name, FontId::proportional(13.0), t.text, room);
                let byline = elide(ui, ranked.detail.as_deref().unwrap_or("Unknown author"), FontId::proportional(12.0), t.muted, room);
                let top = rect.center().y - (title.size().y + byline.size().y + 9.0) / 2.0;
                let title_h = title.size().y;
                ui.painter().galley(pos2(x, top), title, t.text);
                ui.painter().galley(pos2(x, top + title_h + 2.0), byline, t.muted);
                let bar = Rect::from_min_size(pos2(x, rect.center().y + (title_h + 11.0) / 2.0 + 4.0), vec2(right - x, 3.0));
                kit::share_bar(ui, bar, (ranked.secs / most) as f32);
                ui.painter().galley(pos2(right - figure.size().x, top + 1.0), figure, t.text);
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
    let best = if s.streak > 0 && s.streak >= s.longest_streak { "Your best yet".to_owned() } else { format!("Best: {}", days(s.longest_streak)) };
    let finished = grouped(s.finished as u64);
    let this_year = format!("{} this year", s.finished_this_year);
    let week = amount(s.week);
    let today = if s.today > 0.0 { format!("{} today", amount(s.today)) } else { "None yet today".to_owned() };
    let stats = [
        kit::Stat { label: "Listened", value: &amount(s.total), note: Some(&listened_note), icon: Some(Icon::Headphones) },
        kit::Stat { label: "Streak", value: &streak, note: Some(&best), icon: Some(Icon::Flame) },
        kit::Stat { label: "Finished", value: &finished, note: Some(&this_year), icon: Some(Icon::Check) },
        kit::Stat { label: "Last 7 days", value: &week, note: Some(&today), icon: Some(Icon::Clock) },
    ];
    let (cols, w) = kit::tile_columns(width, TILE_MIN, GAP, stats.len());
    for (i, row) in stats.chunks(cols).enumerate() {
        if i > 0 {
            gap(ui);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for stat in row {
                kit::stat_tile(ui, w, stat);
            }
        });
    }
}

/// A fact: its mark, and its words, the figures in them set apart.
struct Fact {
    icon: Icon,
    parts: Vec<(String, bool)>,
}

fn fact(icon: Icon, parts: &[(&str, bool)]) -> Fact {
    Fact { icon, parts: parts.iter().map(|(p, strong)| ((*p).to_owned(), *strong)).collect() }
}

/// A few lines worth a smile, each only when there's something to say: what
/// kind of listener the hours make on top, the rest in one or two columns.
fn fun_facts(ui: &mut Ui, s: &Stats, width: f32) {
    let t = Tokens::of(ui);
    let mut facts = Vec::new();
    let words = s.words();
    let novels = (words / NOVEL_WORDS).floor() as u64;
    let words_text = format!("{} words", grouped(((words / 1_000.0).round() * 1_000.0) as u64));
    let novels_text = format!("{} novels", grouped(novels));
    facts.push(match novels {
        0 => fact(Icon::Lyrics, &[("About ", false), (&words_text, true), (" read to you.", false)]),
        1 => fact(Icon::Lyrics, &[("About ", false), (&words_text, true), (" read to you: about a novel's worth.", false)]),
        _ => fact(Icon::Lyrics, &[("About ", false), (&words_text, true), (" read to you: some ", false), (&novels_text, true), ("' worth.", false)]),
    });
    if let Some((day, secs)) = s.best_day {
        facts.push(fact(
            Icon::Star,
            &[("Your biggest day was ", false), (&long_date(day, s.today_date), true), (", with ", false), (&amount(secs), true), (".", false)],
        ));
    }
    if let Some(sitting) = &s.longest_sitting {
        let when = on_date(sitting.day, s.today_date);
        facts.push(fact(
            Icon::Headphones,
            &[("Your longest sitting: ", false), (&amount(sitting.secs), true), (" of ", false), (&sitting.title, true), (&format!(", {when}."), false)],
        ));
    }
    if let Some(book) = &s.longest_finished {
        facts.push(fact(
            Icon::Check,
            &[("The longest book you've finished is ", false), (&book.name, true), (", at ", false), (&amount(book.secs), true), (".", false)],
        ));
    }
    if s.active_days > 1 {
        let average = amount(s.total / s.active_days as f64);
        let active = format!("{} days", grouped(s.active_days as u64));
        facts.push(fact(Icon::Clock, &[(&active, true), (" with listening, ", false), (&average, true), (" on an average one.", false)]));
    }
    kit::card(ui, width, 0.0, |ui, w| {
        kit::card_heading(ui, "Fun facts", None);
        if let Some((name, why)) = s.persona() {
            persona(ui, name, &why, &t);
            ui.add_space(16.0);
        }
        let cols = if w >= FACT_COLUMNS_MIN { 2 } else { 1 };
        let col_gap = 28.0;
        let col_w = (w - col_gap * (cols - 1) as f32) / cols as f32;
        for (i, row) in facts.chunks(cols).enumerate() {
            if i > 0 {
                ui.add_space(12.0 - ui.spacing().item_spacing.y);
            }
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = col_gap;
                for f in row {
                    ui.allocate_ui_with_layout(vec2(col_w, 0.0), Layout::left_to_right(Align::Min), |ui| {
                        ui.set_width(col_w);
                        fact_line(ui, f, col_w, &t);
                    });
                }
            });
        }
    });
}

/// The kind of listener the hours make, in a tinted pill, and why.
fn persona(ui: &mut Ui, name: &str, why: &str, t: &Tokens) {
    let icon = match name {
        "Night owl" => Icon::Moon,
        "Early bird" => Icon::Star,
        "Weekend binger" => Icon::Headphones,
        _ => Icon::Clock,
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let label = ui.painter().layout_no_wrap(name.to_owned(), FontId::new(14.0, semibold()), t.accent);
        let (icon_w, pad) = (14.0, vec2(12.0, 6.0));
        let size = vec2(pad.x * 2.0 + icon_w + 6.0 + label.size().x, label.size().y + pad.y * 2.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, rect.height() / 2.0, t.accent.gamma_multiply(0.16));
        let icon_rect = Rect::from_center_size(pos2(rect.left() + pad.x + icon_w / 2.0, rect.center().y), vec2(icon_w, icon_w));
        icons::paint(p, icon_rect, icon, t.accent);
        p.galley(pos2(icon_rect.right() + 6.0, rect.center().y - label.size().y / 2.0), label, t.accent);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, name));
        ui.label(RichText::new(why).size(13.5).color(t.text));
    });
}

/// A fact's mark in a tinted disc, its words wrapped beside it.
fn fact_line(ui: &mut Ui, f: &Fact, width: f32, t: &Tokens) {
    let (disc, _) = ui.allocate_exact_size(vec2(FACT_ICON, FACT_ICON), Sense::hover());
    ui.painter().circle_filled(disc.center(), FACT_ICON / 2.0, t.accent.gamma_multiply(0.12));
    icons::paint(ui.painter(), Rect::from_center_size(disc.center(), vec2(13.0, 13.0)), f.icon, t.accent);
    ui.add_space(12.0 - ui.spacing().item_spacing.x);
    let mut job = LayoutJob::default();
    for (text, strong) in &f.parts {
        let format = if *strong {
            TextFormat { font_id: FontId::new(13.5, semibold()), color: t.text, ..Default::default() }
        } else {
            TextFormat { font_id: FontId::proportional(13.5), color: t.muted, ..Default::default() }
        };
        job.append(text, 0.0, format);
    }
    job.wrap.max_width = width - FACT_ICON - 12.0;
    ui.vertical(|ui| {
        ui.add_space(5.0);
        ui.add(egui::Label::new(job).wrap());
    });
}

/// The last year's days, a column a week (Monday at the top), shaded by how
/// much each held: the cells grown to fill the card, or as many weeks as fit.
fn calendar(ui: &mut Ui, s: &Stats, width: f32) {
    kit::card(ui, width, 0.0, |ui, w| {
        let spare = w - kit::level_grid_width(WEEKS, 0.0, CELL_GAP);
        let cell = (spare / WEEKS as f32).floor().clamp(CELL_MIN, CELL_MAX);
        let mut weeks = WEEKS;
        while weeks > MIN_WEEKS && kit::level_grid_width(weeks, cell, CELL_GAP) > w {
            weeks -= 1;
        }
        let today = s.today_date;
        let monday = today.checked_sub_days(Days::new(u64::from(today.weekday().num_days_from_monday()))).unwrap_or(today);
        let start = monday.checked_sub_days(Days::new(7 * (weeks as u64 - 1))).unwrap_or(monday);
        let active = s.days.range(start..=today).filter(|(_, v)| **v > 0.0).count();
        let span = if weeks == WEEKS { "the last year".to_owned() } else { format!("the last {weeks} weeks") };
        let note = format!("{} {} in {span}", grouped(active as u64), if active == 1 { "day" } else { "days" });
        kit::card_heading(ui, "Listening by day", Some(&note));
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
        let name = format!("Listening by day over {span}: {note}");
        let grid_w = kit::level_grid_width(weeks, cell, CELL_GAP);
        // Centred in the card when the cells have grown as far as they go.
        let indent = ((w - grid_w) / 2.0).floor().max(0.0);
        ui.horizontal(|ui| {
            ui.add_space(indent);
            kit::level_grid(
                ui,
                cell,
                CELL_GAP,
                &kit::Grid { columns: weeks, rows: 7, level: &level, tip: &tip, column_label: &column_label, row_label: &row_label, name: &name },
            );
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(indent);
            kit::level_legend(ui, (cell - 1.0).min(11.0), grid_w);
        });
    });
}

fn week_chart(ui: &mut Ui, s: &Stats, width: f32) {
    let busiest = (0..7).max_by(|a, b| s.weekdays[*a].total_cmp(&s.weekdays[*b]).then(b.cmp(a))).unwrap_or(0);
    let note = format!("Most on {}s", WEEKDAYS[busiest]);
    kit::card(ui, width, 0.0, |ui, w| {
        kit::card_heading(ui, "By day of the week", Some(&note));
        let label = |i: usize| Some(WEEKDAYS[i][..3].to_owned());
        let tip = |i: usize| format!("{}s: {}", WEEKDAYS[i], amount(s.weekdays[i]));
        let name = format!("Listening by day of the week; most on {}s", WEEKDAYS[busiest]);
        kit::bar_chart(ui, w, CHART_H, &kit::Bars { values: &s.weekdays, label: &label, tip: &tip, name: &name });
    });
}

fn hour_chart(ui: &mut Ui, s: &Stats, width: f32) {
    let peak = (0..24).max_by(|a, b| s.hours[*a].total_cmp(&s.hours[*b]).then(b.cmp(a))).unwrap_or(0);
    let note = format!("Peak around {}", hour_name(peak));
    kit::card(ui, width, 0.0, |ui, w| {
        kit::card_heading(ui, "By hour of the day", Some(&note));
        let label = |i: usize| i.is_multiple_of(6).then(|| short_hour(i));
        let tip = |i: usize| format!("{} to {}: {}", hour_name(i), hour_name((i + 1) % 24), amount(s.hours[i]));
        let name = format!("Listening by hour of the day; most around {}", hour_name(peak));
        kit::bar_chart(ui, w, CHART_H, &kit::Bars { values: &s.hours, label: &label, tip: &tip, name: &name });
    });
}

/// Authors or narrators on a card at least `min_height` tall, the most
/// listened first, each with its share of the first's time, `row_h` a row.
fn ranking(ui: &mut Ui, title: &str, list: &[Ranked], width: f32, row_h: f32, min_height: f32) {
    let t = Tokens::of(ui);
    kit::card(ui, width, min_height, |ui, w| {
        kit::card_heading(ui, title, None);
        if list.is_empty() {
            ui.label(RichText::new("Nothing to rank yet.").size(12.5).color(t.muted));
            return;
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        let most = list[0].secs.max(1.0);
        for (i, ranked) in list.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(w, row_h), Sense::hover());
            if ui.is_rect_visible(rect) {
                let top = rect.center().y - 13.0;
                let figure = ui.painter().layout_no_wrap(amount(ranked.secs), FontId::new(12.0, semibold()), t.text);
                let x = rect.left() + 24.0;
                let room = (rect.right() - figure.size().x - 12.0 - x).max(20.0);
                let name = elide(ui, &ranked.name, FontId::proportional(13.0), t.text, room);
                rank_number(ui, i, rect.left(), top + name.size().y / 2.0, &t);
                ui.painter().galley(pos2(x, top), name, t.text);
                ui.painter().galley(pos2(rect.right() - figure.size().x, top + 1.0), figure, t.text);
                let bar = Rect::from_min_size(pos2(x, top + 22.0), vec2(rect.right() - x, 3.0));
                kit::share_bar(ui, bar, (ranked.secs / most) as f32);
            }
            let name = format!("{}. {}, {}", i + 1, ranked.name, amount(ranked.secs));
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &name));
        }
    });
}

/// A list's place, at `x`, centred on `y`: the first in the accent.
fn rank_number(ui: &Ui, i: usize, x: f32, y: f32, t: &Tokens) {
    let color = if i == 0 { t.accent } else { t.muted };
    let rank = ui.painter().layout_no_wrap(format!("{}", i + 1), FontId::new(13.0, semibold()), color);
    ui.painter().galley(pos2(x + 2.0, y - rank.size().y / 2.0), rank, color);
}

/// The room between cards.
fn gap(ui: &mut Ui) {
    ui.add_space((GAP - ui.spacing().item_spacing.y).max(0.0));
}

/// Two cards side by side when each gets [`HALF_MIN`], else one over the
/// other.
fn pair(ui: &mut Ui, width: f32, left: &mut dyn FnMut(&mut Ui, f32), right: &mut dyn FnMut(&mut Ui, f32)) {
    if width >= 2.0 * HALF_MIN + GAP {
        columns(ui, width, &mut [(1.0, left), (1.0, right)]);
    } else {
        left(ui, width);
        gap(ui);
        right(ui, width);
    }
}

/// A card drawn at the width it's given.
type Part<'a> = dyn FnMut(&mut Ui, f32) + 'a;

/// Cards across `width`, [`GAP`] apart, each as wide as its share of the
/// weights.
fn columns(ui: &mut Ui, width: f32, parts: &mut [(f32, &mut Part)]) {
    let total: f32 = parts.iter().map(|(weight, _)| weight).sum();
    let room = width - GAP * (parts.len().saturating_sub(1)) as f32;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for (weight, part) in parts.iter_mut() {
            let w = (room * *weight / total).floor();
            ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(w);
                part(ui, w);
            });
        }
    });
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
