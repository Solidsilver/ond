//! An album: header with links for artist / genre / year / label, then the
//! track list (composer and actions appear on hover), then "Album details":
//! credits gathered from the tracks' OpenSubsonic contributors and the
//! per-track measurements, folded away until asked for. The track list itself
//! is in `track_list`.

use super::track_list::ListOpts;
use super::view_ctx::ViewCtx;
use super::{Action, Keep};
use super::{AnalysisAction, OfflineAction};
use crate::favorites::StarKind;
use crate::icons::{self, Icon};
use crate::nav::{FilterKind, Route};
use crate::queue::Source;
use crate::theme::{Tokens, semibold};
use crate::widgets::{chip, elide, heart_button, link};
use analysis::TrackAnalysis;
use egui::{FontId, Sense, Ui, vec2};
use library::Track;

/// One line of credits: a role and the people in it, in first-seen order.
#[derive(Debug, Clone, PartialEq)]
pub struct Credit {
    pub role: String,
    /// (name, artist id)
    pub people: Vec<(String, Option<String>)>,
}

/// Groups the tracks' contributors by role. Performers come first, labelled by
/// instrument ("Piano"); then writers, then production. The main artists are
/// already in the header, so the "artist" roles are left out.
pub fn credits(tracks: &[Track]) -> Vec<Credit> {
    let mut out: Vec<Credit> = Vec::new();
    for t in tracks {
        for c in &t.contributors {
            let role = c.role.to_lowercase();
            if matches!(role.as_str(), "artist" | "albumartist") {
                continue;
            }
            let label = match (&role[..], c.sub_role.as_deref()) {
                ("performer", Some(sub)) if !sub.is_empty() => crate::fmt::capitalize(sub),
                _ => crate::fmt::capitalize(&role),
            };
            let entry = match out.iter_mut().position(|e| e.role == label) {
                Some(i) => &mut out[i],
                None => {
                    out.push(Credit { role: label, people: Vec::new() });
                    out.last_mut().expect("just pushed")
                }
            };
            if !entry.people.iter().any(|(n, _)| n == &c.name) {
                entry.people.push((c.name.clone(), c.artist_id.clone()));
            }
        }
    }
    let rank = |role: &str| match role.to_lowercase().as_str() {
        "composer" => 1,
        "lyricist" | "arranger" => 2,
        "conductor" => 3,
        "producer" => 4,
        "engineer" | "mixer" | "remixer" | "djmixer" => 5,
        _ => 0, // instruments and other performer roles
    };
    out.sort_by_key(|c| rank(&c.role));
    out
}

/// `FLAC 16/44.1` when every track shares it.
fn shared_format(tracks: &[Track]) -> Option<String> {
    let first = crate::fmt::quality(tracks.first()?);
    tracks.iter().all(|t| crate::fmt::quality(t) == first).then_some(first)
}

/// What the album page works out from the album and its tracks, kept between
/// frames: rebuilt when the album, its track list or their analysis change.
pub(super) struct AlbumFacts {
    summary: analysis::AlbumSummary,
    /// `FLAC 16/44.1` when every track shares it.
    format: Option<String>,
    /// "6 songs · 41 min"
    extent: String,
    credits: Vec<Credit>,
    source: Source,
    /// The tracks the key points at, held so that while this is cached no
    /// other list can take their address.
    _tracks: Option<crate::tracks::TrackSlice>,
}

/// The [`AlbumFacts`] key: the album id's hash, the album and analysis
/// versions, and the track list's address and length (a list fetched again is
/// a new one).
pub(super) type AlbumKey = (u64, u64, u64, usize, usize);

impl AlbumFacts {
    fn new(album: &library::Album, slice: Option<&crate::tracks::TrackSlice>, analysis: &super::Analysis) -> Self {
        let tracks: &[Track] = slice.map_or(&[], |s| s);
        let found: Vec<&TrackAnalysis> = tracks.iter().filter_map(|t| analysis.of(t)).collect();
        let secs = tracks.iter().filter_map(|t| t.duration_secs).sum::<u32>();
        let extent = format!("{} · {}", crate::fmt::plural(tracks.len(), "song"), crate::fmt::span(secs));
        Self {
            summary: analysis::summarize(&found, tracks.len()),
            format: shared_format(tracks),
            extent,
            credits: credits(tracks),
            source: Source::Album { id: album.id.clone(), name: album.name.clone() },
            _tracks: slice.cloned(),
        }
    }
}

/// Per-track loudness, peak and dynamic range, with any finding the album as a
/// whole doesn't already show. The title column takes what's left.
fn analysis_table(ui: &mut Ui, tracks: &[Track], found: &[Option<&TrackAnalysis>], album_verdict: Option<&analysis::Verdict>) {
    let t = Tokens::of(ui);
    let small = |s: String, c| egui::RichText::new(s).size(12.0).color(c);
    let title_w = (ui.available_width() - 360.0).clamp(90.0, 320.0);
    egui::Grid::new("album-analysis").num_columns(6).spacing(vec2(14.0, 6.0)).show(ui, |ui| {
        for h in ["", "", "Loudness", "Peak", "DR", ""] {
            ui.label(small(h.to_owned(), t.muted));
        }
        ui.end_row();
        for (i, (track, a)) in tracks.iter().zip(found).enumerate() {
            ui.label(small(track.track.unwrap_or(i as u32 + 1).to_string(), t.muted));
            let g = elide(ui, &track.title, FontId::proportional(12.0), t.text, title_w);
            let (r, _) = ui.allocate_exact_size(vec2(title_w, g.size().y), Sense::hover());
            ui.painter().galley(r.min, g, t.text);
            match a {
                Some(a) => {
                    let dash = || "–".to_owned();
                    ui.label(small(a.integrated_lufs.map_or_else(dash, |l| crate::fmt::level(l, "LUFS")), t.text));
                    ui.label(small(a.true_peak_dbtp.map_or_else(dash, |p| crate::fmt::level(p, "dBTP")), t.text));
                    ui.label(small(a.dr_rounded().map_or_else(dash, |d| format!("DR{d}")), t.text));
                    match a.verdicts.iter().find(|v| album_verdict.is_none_or(|av| !av.same_kind(v))) {
                        Some(v) => {
                            chip(ui, &v.label(), t.warn).on_hover_text(v.evidence());
                        }
                        None => {
                            ui.label("");
                        }
                    }
                }
                None => {
                    ui.label(small("not analyzed".into(), t.muted));
                    for _ in 0..3 {
                        ui.label("");
                    }
                }
            }
            ui.end_row();
        }
    });
}

/// The album page's header stacks the artwork above the text below this width.
const HEADER_STACK_BELOW: f32 = 460.0;
/// The artwork's side beside the text: 30% of the width, within these.
const HEADER_ART_MIN: f32 = 150.0;
const HEADER_ART_MAX: f32 = 220.0;
/// The technical line's text, a step under the catalogue line's 12.
const MEASURES_SIZE: f32 = 11.0;
/// The header text's height before it's first measured.
const HEADER_TEXT_H_GUESS: f32 = 150.0;

/// What the album page's header draws from.
struct AlbumHeader<'p> {
    album: &'p library::Album,
    tracks: &'p [Track],
    facts: &'p AlbumFacts,
    /// How many of its tracks wait to be (or are being) analysed.
    analysing: usize,
}

impl ViewCtx<'_> {
    pub(super) fn album_page(&mut self, ui: &mut Ui, id: &str) {
        let t = Tokens::of(ui);
        // Borrowed from the library, not from `self`, so drawing can take `&mut self`.
        let (data, analysis) = (self.data, self.analysis);
        let Some(album) = data.album(id) else {
            crate::widgets::empty_state(ui, Icon::Disc, "Loading…", "");
            return;
        };
        let slice = data.tracks.album(id);
        let tracks: &[Track] = slice.map_or(&[], |s| s);
        if self.download_key(ui) {
            let kept = self.downloads.snap.pin(&offline::Pin::Album(id.to_owned())).is_some();
            self.actions.push(Action::Offline(OfflineAction::Keep(Keep::Album(id.to_owned()), !kept)));
        }
        let key = (super::view_ctx::hash_str(id), data.version, analysis.version, tracks.as_ptr() as usize, tracks.len());
        let facts = self.ui.memos.album.get(key, || AlbumFacts::new(album, slice, analysis));
        let analysing = analysis.runner.pending(tracks.iter().map(|t| t.id.as_str()));
        let page = AlbumHeader { album, tracks, facts: &facts, analysing };
        super::layout::into_gutter(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(("album", id)).auto_shrink(false).content_margin(super::layout::gutter()).show(ui, |ui| {
                self.album_header(ui, &t, &page);
                ui.add_space(24.0);
                if tracks.is_empty() {
                    crate::widgets::empty_state(ui, Icon::Note, "Loading tracks…", "");
                    return;
                }

                self.track_list(ui, tracks, &facts.source, ListOpts { numbers: true, artist: false, playlist: None });
                ui.add_space(16.0);
                self.album_details(ui, tracks, &facts, analysing > 0);
                ui.add_space(24.0);
            })
        });
    }

    /// The artwork beside the album's name, artist, facts and buttons, or
    /// above them when there's no room.
    fn album_header(&mut self, ui: &mut Ui, t: &Tokens, page: &AlbumHeader) {
        let avail = ui.available_width();
        let stacked = avail < HEADER_STACK_BELOW;
        let art_side = if stacked { avail.min(HEADER_ART_MAX) } else { (avail * 0.3).clamp(HEADER_ART_MIN, HEADER_ART_MAX) };
        let layout = if stacked { egui::Layout::top_down(egui::Align::Min) } else { egui::Layout::left_to_right(egui::Align::Min) };
        ui.with_layout(layout, |ui| {
            let (art, _) = ui.allocate_exact_size(vec2(art_side, art_side), Sense::hover());
            self.art.paint_as(ui, art, page.album.cover_art.as_deref(), 6.0, &crate::art::album_cover(page.album));
            ui.add_space(if stacked { 16.0 } else { 20.0 });
            // The text block sits on the artwork's bottom edge. Its height
            // (it wraps) is known after drawing, so use last frame's and
            // redo the frame when it changed.
            let measured = ui.scope_id().with(("album-header-h", &page.album.id));
            let last_h = ui.data(|d| d.get_temp::<f32>(measured)).unwrap_or(HEADER_TEXT_H_GUESS);
            let block = ui.vertical(|ui| {
                if !stacked {
                    ui.add_space((art_side - last_h).max(0.0));
                }
                let top = ui.cursor().top();
                self.album_header_text(ui, t, page);
                ui.min_rect().bottom() - top
            });
            let h = block.inner.round();
            if (h - last_h).abs() > 0.5 {
                ui.data_mut(|d| d.insert_temp(measured, h));
                ui.ctx().request_discard("album header height changed");
            }
        });
    }

    /// The name, the artist, the catalogue facts, the measurements under
    /// them, and the buttons.
    fn album_header_text(&mut self, ui: &mut Ui, t: &Tokens, page: &AlbumHeader) {
        let album = page.album;
        ui.add(egui::Label::new(egui::RichText::new(&album.name).font(FontId::new(26.0, semibold())).color(t.text)).wrap());
        if let Some(artist) = &album.artist
            && link(ui, artist, FontId::proportional(18.0), t.accent).clicked()
        {
            self.actions.push(Action::Go(Route::Artist { name: artist.clone(), id: album.artist_ref.clone() }));
        }
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| self.album_facts_line(ui, t, page));
        if Self::has_measures(page) {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| self.album_measures_line(ui, t, page));
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| self.album_buttons(ui, t, page));
    }

    /// The catalogue line: genres, year and labels as links, then how many
    /// songs and how long.
    fn album_facts_line(&mut self, ui: &mut Ui, t: &Tokens, page: &AlbumHeader) {
        let album = page.album;
        ui.spacing_mut().item_spacing.x = 0.0;
        let small = FontId::proportional(12.0);
        let mut sep = separator(t, 12.0);
        for g in album.all_genres() {
            sep(ui);
            if link(ui, g, small.clone(), t.muted).clicked() {
                self.actions.push(Action::Go(Route::Filter { kind: FilterKind::Genre, value: g.to_owned() }));
            }
        }
        if let Some(y) = album.year {
            sep(ui);
            if link(ui, &y.to_string(), small.clone(), t.muted).clicked() {
                self.actions.push(Action::Go(Route::Filter { kind: FilterKind::Year, value: y.to_string() }));
            }
        }
        for l in &album.labels {
            sep(ui);
            if link(ui, l, small.clone(), t.muted).clicked() {
                self.actions.push(Action::Go(Route::Filter { kind: FilterKind::Label, value: l.clone() }));
            }
        }
        if !page.tracks.is_empty() {
            sep(ui);
            ui.label(egui::RichText::new(&page.facts.extent).size(12.0).color(t.muted));
        }
    }

    /// The technical line, set smaller: the shared format, then the
    /// measurements (or how far analysing has got) and any finding.
    fn album_measures_line(&mut self, ui: &mut Ui, t: &Tokens, page: &AlbumHeader) {
        let summary = &page.facts.summary;
        ui.spacing_mut().item_spacing.x = 0.0;
        let size = MEASURES_SIZE;
        let mut sep = separator(t, size);
        if let Some(f) = &page.facts.format {
            sep(ui);
            ui.label(egui::RichText::new(f).size(size).color(t.muted));
        }
        // Measurements once the whole album is known; a quiet
        // count while it's being worked on.
        if let (true, Some(dr), Some(lufs)) = (summary.complete(), summary.dr, summary.lufs) {
            sep(ui);
            ui.label(egui::RichText::new(format!("DR{dr}")).size(size).color(t.muted)).on_hover_text("Dynamic range (DR meter), averaged across the album");
            sep(ui);
            ui.label(egui::RichText::new(crate::fmt::level(lufs, "LUFS")).size(size).color(t.muted))
                .on_hover_text("Integrated loudness (EBU R128), across the album");
        } else if page.analysing > 0 {
            sep(ui);
            ui.label(egui::RichText::new(format!("analyzing {}/{}", summary.analyzed, page.tracks.len())).size(size).color(t.muted));
        }
        if let Some((v, n)) = &summary.verdict {
            if page.facts.format.is_some() || summary.complete() || page.analysing > 0 {
                ui.add_space(10.0);
            }
            let tracks_note = if *n == summary.analyzed { "every analyzed track".to_owned() } else { format!("{n} of {} analyzed tracks", summary.analyzed) };
            chip(ui, &v.label(), t.warn).on_hover_text(format!("{}\n\nSeen in {tracks_note}.", v.evidence()));
        }
    }

    /// Whether [`album_measures_line`](Self::album_measures_line) has anything to show.
    fn has_measures(page: &AlbumHeader) -> bool {
        let s = &page.facts.summary;
        page.facts.format.is_some() || (s.complete() && s.dr.is_some() && s.lufs.is_some()) || page.analysing > 0 || s.verdict.is_some()
    }

    /// Play and Shuffle, the heart, ⋯ (the album's menu), and how its
    /// download is going.
    fn album_buttons(&mut self, ui: &mut Ui, t: &Tokens, page: &AlbumHeader) {
        let (album, tracks) = (page.album, page.tracks);
        if let Some(a) = super::browse::play_shuffle(ui) {
            self.actions.push(Action::Album(album.id.clone(), a));
        }
        let loved = self.favorites.is_on(StarKind::Album, &album.id);
        if heart_button(ui, loved, 30.0, 14.0).clicked() {
            self.actions.push(Action::ToggleStar { kind: StarKind::Album, id: album.id.clone(), track: None, name: None });
        }
        let more = icons::sized_button(ui, Icon::More, "More", vec2(30.0, 30.0), 13.0, Some(t.accent));
        let pin = offline::Pin::Album(album.id.clone());
        let kept = self.downloads.snap.pin(&pin).is_some();
        egui::Popup::menu(&more).show(|ui| {
            let c = super::browse::Collection::Album(album.id.clone());
            super::browse::collection_menu(ui, &c, false, kept, Some("⇧⌘D"), self.actions);
            if !page.facts.summary.complete() && page.analysing == 0 && !tracks.is_empty() {
                ui.separator();
                if ui.button("Analyze album").on_hover_text("Measure loudness and dynamic range, and check for upsampled or lossy-sourced files").clicked() {
                    self.actions.push(Action::Analysis(AnalysisAction::Tracks(tracks.to_vec())));
                }
            }
            if !tracks.is_empty() {
                ui.separator();
                self.add_to_playlist_menu(ui, tracks);
            }
        });
        ui.add_space(6.0);
        self.pin_status(ui, &pin);
    }

    /// "Album details ›": the album's credits and its per-track measurements,
    /// folded away until asked for (open stays open for the session).
    pub(super) fn album_details(&mut self, ui: &mut Ui, tracks: &[Track], facts: &AlbumFacts, analysing: bool) {
        let t = Tokens::of(ui);
        let open = self.ui.album_details_open;
        let toggle = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let l = link(ui, "Album details", FontId::proportional(12.0), t.accent);
            let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            icons::paint(ui.painter(), rect, if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.accent);
            l
        });
        if toggle.inner.clicked() {
            self.ui.album_details_open = !open;
        }
        if !open {
            return;
        }
        let width = ui.available_width().min(shell::kit::LIST_W);
        ui.allocate_ui_with_layout(vec2(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_max_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            if !facts.credits.is_empty() {
                crate::widgets::kv_section(ui, "Credits");
                for c in &facts.credits {
                    super::info_panel::links_row(
                        self.actions,
                        ui,
                        &c.role,
                        c.people.iter().map(|(name, id)| (name.as_str(), || Route::Artist { name: name.clone(), id: id.clone() })),
                    );
                }
            }
            crate::widgets::kv_section(ui, "Analysis");
            let analysis = self.analysis;
            let found: Vec<Option<&TrackAnalysis>> = tracks.iter().map(|t| analysis.of(t)).collect();
            if found.iter().all(Option::is_none) {
                crate::widgets::kv_row(ui, "Loudness", |ui| {
                    if analysing {
                        ui.label(egui::RichText::new("Analyzing…").size(12.5).color(t.muted));
                    } else if link(ui, "Analyze album", FontId::proportional(12.5), t.accent)
                        .on_hover_text("Measure loudness and dynamic range, and check for upsampled or lossy-sourced files")
                        .clicked()
                    {
                        self.actions.push(Action::Analysis(AnalysisAction::Tracks(tracks.to_vec())));
                    }
                });
            } else {
                ui.spacing_mut().item_spacing.y = 6.0;
                analysis_table(ui, tracks, &found, facts.summary.verdict.as_ref().map(|(v, _)| v));
            }
        });
    }
}

/// A " · " before every item but the first, at `size`.
fn separator(t: &Tokens, size: f32) -> impl FnMut(&mut Ui) {
    let (mut first, muted) = (true, t.muted);
    move |ui: &mut Ui| {
        if !first {
            ui.label(egui::RichText::new("  ·  ").size(size).color(muted));
        }
        first = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::Contributor;

    fn track(contributors: &[(&str, Option<&str>, &str)]) -> Track {
        Track {
            suffix: Some("flac".into()),
            bit_depth: Some(16),
            sample_rate: Some(44_100),
            contributors: contributors
                .iter()
                .map(|(role, sub, name)| Contributor { role: (*role).into(), sub_role: sub.map(Into::into), name: (*name).into(), artist_id: None })
                .collect(),
            ..crate::test_tracks::track("t", "a", "t")
        }
    }

    #[test]
    fn credits_group_by_role_with_instruments_first() {
        let tracks = [
            track(&[("producer", None, "Orrin Keepnews"), ("composer", None, "Scott LaFaro"), ("performer", Some("piano"), "Bill Evans")]),
            track(&[("composer", None, "George Gershwin"), ("performer", Some("bass"), "Scott LaFaro"), ("albumartist", None, "Bill Evans Trio")]),
            track(&[("composer", None, "Scott LaFaro"), ("performer", Some("piano"), "Bill Evans")]),
        ];
        let c = credits(&tracks);
        let roles: Vec<&str> = c.iter().map(|c| c.role.as_str()).collect();
        assert_eq!(roles, ["Piano", "Bass", "Composer", "Producer"]);
        let composers: Vec<&str> = c[2].people.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(composers, ["Scott LaFaro", "George Gershwin"]);
        assert!(credits(&[track(&[])]).is_empty());
    }

    #[test]
    fn shared_format_only_when_uniform() {
        let a = track(&[]);
        let mut b = track(&[]);
        assert_eq!(shared_format(&[a.clone(), b.clone()]).as_deref(), Some("FLAC 16/44.1"));
        b.bit_depth = Some(24);
        assert_eq!(shared_format(&[a, b]), None);
        assert_eq!(shared_format(&[]), None);
    }
}
