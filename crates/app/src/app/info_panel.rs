//! The sidebar's Info tab: everything known about the playing song, as quiet
//! label/value rows. Credits by role (names link to their pages), the album
//! it's from, the file and how it reaches the device, the loudness analysis
//! when there is one, and the user's plays. It follows playback.

use super::AnalysisAction;
use super::album_page::{Credit, credits};
use super::widgets::{KV_VALUE as VALUE, chip, elide, kv_row, kv_section, kv_text, link, link_line};
use super::view_ctx::ViewCtx;
use super::{Action, SongStats};
use crate::icons::Icon;
use crate::nav::{FilterKind, Route};
use crate::theme::{Tokens, semibold};
use egui::{FontId, Sense, Ui, vec2};
use library::Track;


/// The Info tab's text worked out from the song, its album and its analysis,
/// kept between frames (see [`InfoKey`]).
pub(super) struct InfoFacts {
    credits: Vec<Credit>,
    place: Option<String>,
    year: Option<String>,
    /// The file's facts (how it plays is added each frame: it can change).
    file: Vec<(&'static str, String)>,
    measurements: Vec<(&'static str, String)>,
}

/// Without the header (the Player, where the song shows right above), the
/// tab starts this much higher.
const NO_HEADER_PULL: f32 = 12.0;

/// The [`InfoFacts`] key: the song id's hash, the album and analysis versions.
pub(super) type InfoKey = (u64, u64, u64);

impl InfoFacts {
    fn new(track: &Track, album: Option<&library::Album>, analysis: Option<&analysis::TrackAnalysis>) -> Self {
        Self {
            credits: song_credits(track),
            place: track_place(track),
            year: album.and_then(|al| al.year).map(|y| y.to_string()),
            file: crate::fmt::track_facts(track, None),
            measurements: analysis.map(crate::fmt::measurements).unwrap_or_default(),
        }
    }
}

impl ViewCtx<'_> {
    /// `header`: lead with the song's artwork and title (not in the Player,
    /// where they're right above).
    pub fn sidebar_info(&mut self, ui: &mut Ui, header: bool) {
        // Borrowed from the player and library, not from `self`, so drawing can
        // take `&mut self`.
        let (player, data, analysis) = (self.player, self.data, self.analysis);
        let Some(track) = player.queue.current.as_ref().map(|c| &c.track) else {
            // Wrapped to the column: a long line must not widen the sidebar.
            let t = Tokens::of(ui);
            let width = ui.available_width();
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.set_max_width(width);
                let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                crate::icons::paint(ui.painter(), r, Icon::Note, t.muted);
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Nothing playing").font(FontId::new(14.0, semibold())).color(t.text));
                ui.add_space(4.0);
                ui.add(egui::Label::new(egui::RichText::new("Play a song to see its credits, file details and analysis.").size(12.0).color(t.muted)).wrap());
            });
            return;
        };
        if !self.song_stats.contains_key(&track.id) {
            self.actions.push(Action::NeedSongStats(track.id.clone()));
        }
        let album = data.album(&track.album_id);
        let key = (super::view_ctx::hash_str(&track.id), data.version, analysis.version);
        let facts = self.ui.memos.info.get(key, || InfoFacts::new(track, album, analysis.of(track)));
        // Vertical only, and the content held to the column: a long name or
        // value wraps instead of widening (and then clipping) the sidebar.
        egui::ScrollArea::vertical().id_salt("info").auto_shrink(false).show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            if header {
                self.info_header(ui, track, album);
            } else {
                // The first section's room above it is meant to follow the
                // header; without one, most of it goes.
                ui.add_space(-NO_HEADER_PULL);
            }

            // Live, so near the top where it's seen.
            if self.ui.prefs.experimental.spectrogram {
                kv_section(ui, "Spectrogram");
                self.spectrogram(ui);
            }

            if !facts.credits.is_empty() {
                kv_section(ui, "Credits");
                for c in &facts.credits {
                    links_row(self.actions, ui, &c.role, c.people.iter().map(|(name, id)| (name.as_str(), || Route::Artist { name: name.clone(), id: id.clone() })));
                }
            }

            if let Some(al) = album {
                kv_section(ui, "Album");
                links_row(self.actions, ui, "Album", [(al.name.as_str(), || Route::Album(al.id.clone()))]);
                if let Some(artist) = &al.artist {
                    links_row(self.actions, ui, "Album artist", [(artist.as_str(), || Route::Artist { name: artist.clone(), id: al.artist_ref.clone() })]);
                }
                if let Some(place) = &facts.place {
                    kv_text(ui, "Track", place);
                }
                if let Some(y) = &facts.year {
                    links_row(self.actions, ui, "Year", [(y.as_str(), || Route::Filter { kind: FilterKind::Year, value: y.clone() })]);
                }
                let genres = al.all_genres();
                if !genres.is_empty() {
                    let label = if genres.len() == 1 { "Genre" } else { "Genres" };
                    links_row(self.actions, ui, label, genres.into_iter().map(|g| (g, move || Route::Filter { kind: FilterKind::Genre, value: g.to_owned() })));
                }
                if !al.labels.is_empty() {
                    links_row(self.actions, ui, "Label", al.labels.iter().map(|l| (l.as_str(), || Route::Filter { kind: FilterKind::Label, value: l.clone() })));
                }
            }

            let playback = self.output.signal().map(|s| s.summary());
            if !facts.file.is_empty() || playback.is_some() {
                kv_section(ui, "File");
                for (k, v) in &facts.file {
                    kv_text(ui, k, v);
                }
                if let Some(p) = &playback {
                    kv_text(ui, "Playback", p);
                }
            }

            kv_section(ui, "Analysis");
            self.analysis_rows(ui, track, &facts.measurements);

            let rating = self.favorites.rating(&track.id);
            let plays = play_rows(self.song_stats.get(&track.id).and_then(Option::as_ref), rating);
            if !plays.is_empty() {
                kv_section(ui, "Plays");
                for (k, v) in &plays {
                    kv_text(ui, k, v);
                }
            }
            ui.add_space(24.0);
        });
    }

    /// The last few seconds as frequency over time (experimental): low notes
    /// at the bottom on a log scale, louder in a stronger accent.
    fn spectrogram(&mut self, ui: &mut Ui) {
        use crate::visuals::spectrogram::height_of;
        let t = Tokens::of(ui);
        ui.add_space(4.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 104.0), Sense::hover());
        let gutter = 30.0;
        let plot = egui::Rect::from_min_max(egui::pos2(rect.left() + gutter, rect.top() + 4.0), egui::pos2(rect.right(), rect.bottom() - 4.0));
        let painter = ui.painter();
        painter.rect_filled(plot.expand(1.0), 4.0, t.field);
        if let Some(f) = self.listen(ui)
            && f.fresh
        {
            self.ui.spectrogram.push(ui.ctx(), &f.raw, ui.ctx().cumulative_pass_nr());
        }
        self.ui.spectrogram.paint(painter, plot, t.accent);
        for (hz, label) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
            let y = plot.bottom() - height_of(hz) * plot.height();
            painter.hline(plot.x_range(), y, egui::Stroke::new(1.0, t.hairline.gamma_multiply(0.6)));
            painter.text(egui::pos2(plot.left() - 6.0, y), egui::Align2::RIGHT_CENTER, label, FontId::proportional(9.5), t.muted);
        }
        ui.allocate_response(vec2(0.0, 0.0), Sense::hover()).widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Spectrogram of the last few seconds"));
    }

    /// Small artwork, the title, and artist / album as links.
    fn info_header(&mut self, ui: &mut Ui, track: &Track, album: Option<&library::Album>) {
        let t = Tokens::of(ui);
        let width = ui.available_width();
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            let (art, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
            let data = self.data;
            self.art.paint(ui, art, data.cover_for(track), 4.0);
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let text_w = width - 66.0 - 8.0;
                ui.add_space(4.0);
                let title = elide(ui, &track.title, FontId::new(14.0, semibold()), t.text, text_w);
                let (r, _) = ui.allocate_exact_size(title.size(), Sense::hover());
                ui.painter().galley(r.min, title, t.text);
                if let Some(artist) = &track.artist
                    && link_line(ui, artist, FontId::proportional(VALUE), t.accent, text_w).clicked()
                    && let Some(route) = Route::artist_of(track)
                {
                    self.actions.push(Action::Go(route));
                }
                if let Some(al) = album
                    && link_line(ui, &al.name, FontId::proportional(VALUE), t.muted, text_w).clicked()
                {
                    self.ui.reveal = Some(track.id.clone());
                    self.actions.push(Action::Go(Route::Album(al.id.clone())));
                }
            });
        });
    }

    /// Loudness, peak and dynamic range (`measurements`, as worked out from
    /// it), a chip per finding (evidence on hover); or a way to measure it.
    fn analysis_rows(&mut self, ui: &mut Ui, track: &Track, measurements: &[(&'static str, String)]) {
        let t = Tokens::of(ui);
        let analysis = self.analysis;
        let Some(a) = analysis.of(track) else {
            let busy = self.analysis.runner.pending([track.id.as_str()]) > 0;
            kv_row(ui, "Loudness", |ui| {
                if busy {
                    ui.label(egui::RichText::new("Analyzing…").size(VALUE).color(t.muted));
                } else if link(ui, "Analyze this song", FontId::proportional(VALUE), t.accent)
                    .on_hover_text("Measure loudness and dynamic range, and check for upsampled or lossy-sourced files")
                    .clicked()
                {
                    self.actions.push(Action::Analysis(AnalysisAction::Tracks(vec![track.clone()])));
                }
            });
            return;
        };
        for (k, v) in measurements {
            kv_text(ui, k, v);
        }
        if !a.verdicts.is_empty() {
            kv_row(ui, "Finding", |ui| {
                ui.spacing_mut().interact_size.y = 0.0;
                ui.horizontal_wrapped(|ui| {
                    for v in &a.verdicts {
                        chip(ui, &v.label(), t.warn).on_hover_text(v.evidence());
                    }
                });
            });
        }
    }
}

/// A label and one or more links ("Matthew West, Mark Hall"), wrapping. Each
/// item's place is made only when it's clicked.
pub(super) fn links_row<'s, F: FnOnce() -> Route>(actions: &mut Vec<Action>, ui: &mut Ui, label: &str, items: impl IntoIterator<Item = (&'s str, F)>) {
    let t = Tokens::of(ui);
    kv_row(ui, label, |ui| {
        // Rows as tall as the text, so links sit level with their label
        // (a wrapped row takes its height from this when it's created).
        ui.spacing_mut().interact_size.y = 0.0;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 3.0);
            for (i, (text, route)) in items.into_iter().enumerate() {
                if i > 0 {
                    ui.label(egui::RichText::new(", ").size(VALUE).color(t.muted));
                }
                if link(ui, text, FontId::proportional(VALUE), t.accent).clicked() {
                    actions.push(Action::Go(route()));
                }
            }
        });
    });
}

/// The song's own credits; a composer tag stands in when there are no
/// structured contributors.
fn song_credits(track: &Track) -> Vec<Credit> {
    let mut people = credits(std::slice::from_ref(track));
    if people.is_empty()
        && let Some(c) = track.composer.as_deref().filter(|c| !c.trim().is_empty())
    {
        let names = c.split([';', '/']).flat_map(|p| p.split(", ")).map(str::trim).filter(|n| !n.is_empty());
        people.push(Credit { role: "Composer".into(), people: names.map(|n| (n.to_owned(), None)).collect() });
    }
    people
}

/// "Disc 2 · Track 5", or just "5" on a single disc.
fn track_place(t: &Track) -> Option<String> {
    match (t.disc.filter(|d| *d > 1), t.track) {
        (Some(d), Some(n)) => Some(format!("Disc {d} · Track {n}")),
        (Some(d), None) => Some(format!("Disc {d}")),
        (None, Some(n)) => Some(n.to_string()),
        (None, None) => None,
    }
}

/// Plays, when last played, and the rating, as far as they're known.
fn play_rows(stats: Option<&SongStats>, rating: u8) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    if let Some(s) = stats {
        if let Some(n) = s.plays {
            rows.push(("Plays", if n == 1 { "Once".to_owned() } else { n.to_string() }));
        }
        if let Some(when) = s.last_played.as_deref().and_then(|w| chrono::DateTime::parse_from_rfc3339(w).ok()) {
            rows.push(("Last played", when.with_timezone(&chrono::Local).format("%b %-d, %Y").to_string()));
        }
    }
    if rating > 0 {
        rows.push(("Rating", crate::fmt::stars(rating)));
    }
    rows
}


#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Track {
        Track {
            id: "t".into(),
            album_id: "a".into(),
            title: "x".into(),
            artist: None,
            artist_ref: None,
            track: None,
            disc: None,
            duration_secs: None,
            suffix: None,
            content_type: None,
            size: None,
            bit_rate_kbps: None,
            bit_depth: None,
            sample_rate: None,
            channels: None,
            track_gain: None,
            album_gain: None,
            track_peak: None,
            album_peak: None,
            composer: None,
            contributors: Vec::new(),
            cover_art: None,
            artists: Vec::new(),
        }
    }

    #[test]
    fn rows_only_say_what_is_known() {
        let mut t = track();
        assert!(crate::fmt::track_facts(&t, None).is_empty());
        assert_eq!(track_place(&t), None);
        assert!(play_rows(None, 0).is_empty());
        t.suffix = Some("flac".into());
        t.bit_depth = Some(24);
        t.sample_rate = Some(96_000);
        t.channels = Some(2);
        t.track_gain = Some(-6.3);
        t.track = Some(3);
        let rows = crate::fmt::track_facts(&t, Some("24/96 · bit-perfect".into()));
        assert_eq!(rows[0], ("Format", "FLAC 24/96".into()));
        assert_eq!(rows[1], ("Channels", "Stereo".into()));
        assert_eq!(rows[2], ("ReplayGain", "track \u{2212}6.3 dB".into()));
        assert_eq!(rows[3], ("Playback", "24/96 · bit-perfect".into()));
        assert_eq!(track_place(&t).as_deref(), Some("3"));
        t.disc = Some(2);
        assert_eq!(track_place(&t).as_deref(), Some("Disc 2 · Track 3"));
    }

    #[test]
    fn plays_and_rating() {
        let stats = SongStats { plays: Some(1), last_played: Some("not a date".into()) };
        assert_eq!(play_rows(Some(&stats), 0), [("Plays", "Once".to_owned())]);
        let stats = SongStats { plays: Some(12), last_played: Some("2026-10-21T20:15:00Z".into()) };
        let rows = play_rows(Some(&stats), 3);
        assert_eq!(rows[0], ("Plays", "12".into()));
        assert_eq!(rows[1].0, "Last played");
        assert!(rows[1].1.contains("2026"), "{}", rows[1].1);
        assert_eq!(rows[2], ("Rating", "3 stars".into()));
    }

    #[test]
    fn a_composer_tag_stands_in_for_missing_credits() {
        let mut t = track();
        assert!(song_credits(&t).is_empty());
        t.composer = Some("Matthew West, Mark Hall; Bernie Herms".into());
        let c = song_credits(&t);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].role, "Composer");
        assert_eq!(c[0].people.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["Matthew West", "Mark Hall", "Bernie Herms"]);
    }
}
