//! Covers made up for what has no art: a ground in a colour picked from the
//! item's name, with the title set on it, so a shelf of books or albums
//! without art still tells them apart at a glance instead of turning into a
//! wall of identical grey. [`generated_cover`] is the square (a book, an
//! album); [`generated_disc`] the circle with initials (a person);
//! [`generated_ground`] the colour alone.
//!
//! The colour comes from a hash of a seed the caller picks (an id, a name),
//! written out here rather than std's hasher, whose keys may change between
//! builds: the same seed gives the same colour on every run and machine. The
//! grounds are a few muted hues, pale under deep ink in the light theme and
//! deep under pale ink in the dark, so a made-up cover never outshines a real
//! one beside it, and every one keeps the text on it at AA.
//!
//! Also here, what's laid over a cover (made up or real) or under a card to
//! say how far something has got: [`cover_progress`], a thin line along the
//! cover's foot; [`cover_badge`], a round badge in its corner; and
//! [`progress_bar`], the same line on its own. Painted only; the tile or card
//! they sit on says the same in its name for a screen reader.
//!
//! These are drawn every frame for every visible tile: nothing is kept, no
//! texture is made, and the only allocation is the text layout, which egui
//! caches.

use crate::icons::{self, Icon};
use crate::theme::{Tokens, semibold};
use egui::text::{LayoutJob, TextWrapping};
use egui::{Align2, Color32, CornerRadius, FontId, Painter, Rect, TextFormat, Ui, pos2, vec2};

/// What a generated cover says, and what picks its colour.
#[derive(Debug, Clone, Copy)]
pub struct Generated<'a> {
    /// Set large at the top: a book's or an album's title.
    pub title: &'a str,
    /// Set quietly at the foot: the author or the artist.
    pub byline: Option<&'a str>,
    /// What the colour is picked from; the same seed, the same colour.
    pub seed: &'a str,
}

/// The colours of one generated cover.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ground {
    pub fill: Color32,
    /// The title and initials.
    pub ink: Color32,
    /// The byline: the ink, quieter.
    pub quiet: Color32,
}

/// The hues, each a deep and a pale tone of it: slate, teal, moss, ochre,
/// rust, wine, plum and indigo, spread round the wheel so neighbours on a
/// shelf differ (seven times in eight). The light theme sets deep ink on the
/// pale tone, the dark theme pale ink on the deep one, so a made-up cover
/// sits back in either and a real cover beside it stays the one that shows.
const HUES: [(u32, u32); 8] = [
    (0x2F435A, 0xD3DDE8),
    (0x22504C, 0xCFE3E0),
    (0x3A4D30, 0xDCE5D2),
    (0x5A4519, 0xEEE2C8),
    (0x6A3424, 0xF0D9CF),
    (0x632F40, 0xEFD6DE),
    (0x4E3552, 0xE5D8E8),
    (0x3B3A6A, 0xDAD9EE),
];
/// How far each theme's ink goes past its tone (toward black on the pale
/// ground, toward white on the deep one), and how far the byline's ink then
/// goes back toward the ground.
const INK_DEPTH: [f32; 2] = [0.3, 0.4];
/// How far the dark theme's grounds go toward its own field grey: enough to
/// quiet them on near-black, not so much that they all turn the old grey.
const DARK_CALM: f32 = 0.3;
const QUIET: [f32; 2] = [0.22, 0.28];
/// Below this side a cover is only its ground; below [`TITLE_FROM`] it
/// carries initials, which still read where a title would be a smudge.
const INITIALS_FROM: f32 = 24.0;
const TITLE_FROM: f32 = 72.0;

/// FNV-1a, then a final mix so seeds that differ only at the end still
/// spread across the grounds.
pub fn seed_hash(seed: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in seed.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^ (h >> 33)
}

/// The ground `seed` gets in the light or `dark` theme.
pub fn ground(seed: &str, dark: bool) -> Ground {
    let (deep, pale) = HUES[(seed_hash(seed) % HUES.len() as u64) as usize];
    let color = |rgb: u32| Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);
    let (deep, pale) = (color(deep), color(pale));
    let (fill, ink) = if dark {
        (deep.lerp_to_gamma(crate::theme::Tokens::DARK.field, DARK_CALM), pale.lerp_to_gamma(Color32::WHITE, INK_DEPTH[1]))
    } else {
        (pale, deep.lerp_to_gamma(Color32::BLACK, INK_DEPTH[0]))
    };
    Ground { fill, ink, quiet: ink.lerp_to_gamma(fill, QUIET[usize::from(dark)]) }
}

/// Up to two letters for `name`: the first and last words' initials ("BE"
/// for Bill Evans), a leading "The", "A" or "An" skipped; a single word's
/// first two letters, the second in lower case ("Bj" for Björk). `#` for a
/// name with no letter or digit at all.
pub fn initials(name: &str) -> String {
    let mut words = name.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric));
    let Some(mut first) = words.next() else { return "#".into() };
    let article = ["the", "a", "an"].iter().any(|a| first.eq_ignore_ascii_case(a));
    if article && let Some(next) = words.next() {
        first = next;
    }
    let mut letters = first.chars().filter(|c| c.is_alphanumeric());
    let mut out = String::new();
    if let Some(c) = letters.next() {
        out.extend(c.to_uppercase());
    }
    match words.next_back().and_then(|l| l.chars().find(|c| c.is_alphanumeric())) {
        Some(c) => out.extend(c.to_uppercase()),
        None => out.extend(letters.next().into_iter().flat_map(char::to_lowercase)),
    }
    out
}

/// A made-up cover filling `rect`: its ground, the title set semibold from
/// the top left, wrapped to the lines there's room for and cut with an
/// ellipsis, and the byline quietly at the foot. Small, it shows the
/// title's initials instead; a tiny one, only its ground.
pub fn generated_cover(ui: &Ui, rect: Rect, radius: impl Into<CornerRadius>, cover: &Generated) {
    let g = ground(cover.seed, ui.visuals().dark_mode);
    let painter = ui.painter();
    painter.rect_filled(rect, radius, g.fill);
    let side = rect.width().min(rect.height());
    if side < INITIALS_FROM {
        return;
    }
    if side < TITLE_FROM {
        painter.text(rect.center(), Align2::CENTER_CENTER, initials(cover.title), FontId::new((side * 0.36).round(), semibold()), g.ink);
        return;
    }
    let pad = (side * 0.085).round();
    let width = rect.width() - 2.0 * pad;
    let title_size = (side * 0.105).clamp(11.0, 30.0).round();
    let byline_size = (title_size * 0.7).max(10.0).round();
    let byline = cover.byline.filter(|b| !b.trim().is_empty()).map(|b| {
        let mut job = LayoutJob::single_section(b.to_owned(), TextFormat::simple(FontId::proportional(byline_size), g.quiet));
        job.wrap = TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        painter.layout_job(job)
    });
    let foot = byline.as_ref().map_or(0.0, |b| b.size().y + pad * 0.5);
    let font = FontId::new(title_size, semibold());
    let line = ui.fonts_mut(|f| f.row_height(&font));
    let rows = (((rect.height() - 2.0 * pad - foot) / line).floor() as usize).clamp(1, 4);
    let mut job = LayoutJob::single_section(cover.title.to_owned(), TextFormat::simple(font, g.ink));
    job.wrap = TextWrapping { max_width: width, max_rows: rows, break_anywhere: false, overflow_character: Some('…') };
    painter.galley(pos2(rect.left() + pad, rect.top() + pad * 0.9), painter.layout_job(job), g.ink);
    if let Some(b) = byline {
        painter.galley(pos2(rect.left() + pad, rect.bottom() - pad - b.size().y), b, g.quiet);
    }
}

/// Only the ground of the cover `seed` gets: where the title is already
/// shown beside it and controls sit over it (Rede's Mini).
pub fn generated_ground(ui: &Ui, rect: Rect, radius: impl Into<CornerRadius>, seed: &str) {
    ui.painter().rect_filled(rect, radius, ground(seed, ui.visuals().dark_mode).fill);
}

/// A person without a photo: a disc in the ground `seed` picks, holding
/// `name`'s [`initials`].
pub fn generated_disc(ui: &Ui, rect: Rect, name: &str, seed: &str) {
    let g = ground(seed, ui.visuals().dark_mode);
    let r = rect.width().min(rect.height()) / 2.0;
    ui.painter().circle_filled(rect.center(), r, g.fill);
    if r * 2.0 >= INITIALS_FROM {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, initials(name), FontId::new((r * 0.56).min(40.0).round(), semibold()), g.ink);
    }
}

/// How tall the line along a cover's foot is.
pub const COVER_LINE_H: f32 = 4.0;
/// The badge's diameter, and how far it sits in from the cover's corner.
const BADGE: f32 = 22.0;
const BADGE_INSET: f32 = 7.0;

/// A track under the cover's foot, its first `fraction` in the accent. The
/// band follows the cover's rounded corners: it's the cover's own shape,
/// clipped to its last few points.
pub fn cover_progress(painter: &Painter, cover: Rect, radius: f32, fraction: f32, t: &Tokens) {
    let band = Rect::from_min_max(pos2(cover.left(), cover.bottom() - COVER_LINE_H), cover.max);
    // The muted ink, half see-through: grey on any cover, and still there on
    // a placeholder in either theme, so the cover never looks cut short.
    painter.with_clip_rect(band.intersect(painter.clip_rect())).rect_filled(cover, radius, t.muted.gamma_multiply(0.6));
    let done = Rect::from_min_max(band.min, pos2(band.left() + band.width() * fraction.clamp(0.0, 1.0), band.bottom()));
    painter.with_clip_rect(done.intersect(painter.clip_rect())).rect_filled(cover, radius, t.accent);
}

/// A round accent badge with `icon` in it, in the cover's top right corner (a
/// check mark on a finished book).
pub fn cover_badge(painter: &Painter, cover: Rect, icon: Icon, t: &Tokens) {
    let c = pos2(cover.right() - BADGE_INSET - BADGE / 2.0, cover.top() + BADGE_INSET + BADGE / 2.0);
    // A thin ring of shadow keeps it apart from a cover in the same colour.
    painter.circle_filled(c, BADGE / 2.0 + 1.0, Color32::from_black_alpha(40));
    painter.circle_filled(c, BADGE / 2.0, t.accent);
    icons::paint(painter, Rect::from_center_size(c, vec2(11.0, 11.0)), icon, t.on_accent);
}

/// A thin rounded bar, `fraction` of it in the accent, across `rect` (a
/// card's progress under its text).
pub fn progress_bar(painter: &Painter, rect: Rect, fraction: f32, t: &Tokens) {
    let r = rect.height() / 2.0;
    painter.rect_filled(rect, r, t.muted.gamma_multiply(0.28));
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction > 0.0 {
        // Never narrower than the bar is tall, so a book just begun still
        // shows a round dot rather than a sliver.
        let w = (rect.width() * fraction).max(rect.height());
        painter.rect_filled(Rect::from_min_size(rect.min, vec2(w, rect.height())), r, t.accent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::{AA, contrast};
    use crate::kit::themed;

    #[test]
    fn the_same_seed_gets_the_same_ground_on_every_run() {
        // Fixed figures: a change to the hash would recolour everyone's shelf.
        assert_eq!(seed_hash("Bill Evans"), 0x4a7c_2c4a_8509_7236);
        assert_eq!(seed_hash("Kind of Blue"), seed_hash("Kind of Blue"));
        assert_ne!(seed_hash("Kind of Blue"), seed_hash("Kind of Blues"));
        assert_eq!(ground("book-4", false), ground("book-4", false));
    }

    #[test]
    fn the_grounds_spread_and_every_one_reads_in_both_themes() {
        let seeds: Vec<String> = (0..200).map(|n| format!("al-{n:02}")).collect();
        for dark in [false, true] {
            let grounds: Vec<Ground> = seeds.iter().map(|s| ground(s, dark)).collect();
            let fills: std::collections::HashSet<_> = grounds.iter().map(|g| g.fill).collect();
            assert_eq!(fills.len(), HUES.len(), "every hue is reached");
            for g in grounds {
                assert!(contrast(g.ink, g.fill) >= AA, "title on {:?} (dark {dark})", g.fill);
                assert!(contrast(g.quiet, g.fill) >= AA, "byline on {:?} (dark {dark}): {}", g.fill, contrast(g.quiet, g.fill));
            }
        }
    }

    #[test]
    fn initials_are_the_first_and_last_words() {
        assert_eq!(initials("Bill Evans"), "BE");
        assert_eq!(initials("Bill Evans Trio"), "BT");
        assert_eq!(initials("The Beatles"), "Be");
        assert_eq!(initials("The Left Hand of Darkness"), "LD");
        assert_eq!(initials("Björk"), "Bj");
        assert_eq!(initials("AC/DC"), "Ac");
        assert_eq!(initials("坂本龍一"), "坂本");
        assert_eq!(initials("  "), "#");
        assert_eq!(initials("…"), "#");
        assert_eq!(initials("é"), "É");
    }

    #[test]
    fn a_generated_cover_sets_its_title_inside_its_square_and_allocates_no_texture() {
        let rect = Rect::from_min_size(pos2(10.0, 10.0), egui::vec2(180.0, 180.0));
        let mut h = themed((), move |ui, _| {
            generated_cover(ui, rect, 6, &Generated { title: "The Ocean at the End of the Lane", byline: Some("Neil Gaiman"), seed: "book-7" });
        });
        h.run();
        let out = h.output();
        assert!(out.textures_delta.set.iter().all(|(id, _)| *id == egui::TextureId::default()), "only the font atlas");
        let texts: Vec<_> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2, "the title and the byline");
        for t in texts {
            assert!(rect.contains_rect(t.visual_bounding_rect()), "{:?} spills out of the cover", t.galley.text());
        }
    }
}
