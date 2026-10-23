use serde::{Deserialize, Serialize};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// Current schema versions. When a model changes, add a `V2` struct, point the
/// alias at it and rewrite rows in `Library::migrate`.
pub type Artist = ArtistV1;
pub type Album = AlbumV1;
pub type Track = TrackV1;

/// Sort-key normalization: case- and accent-insensitive, so "Björk" sorts with
/// "Bjork" and "bill evans" with "Bill Evans".
pub(crate) fn fold(s: &str) -> String {
    s.trim().nfkd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase).collect()
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ArtistV1 {
    pub id: String,
    pub name: String,
    pub sort: Option<String>,
    pub album_count: u32,
    pub cover_art: Option<String>,
}

impl ArtistV1 {
    pub(crate) fn sort_name(&self) -> String {
        fold(self.sort.as_deref().unwrap_or(&self.name))
    }
}

impl From<subsonic::Artist> for ArtistV1 {
    fn from(a: subsonic::Artist) -> Self {
        Self { id: a.id, name: a.name, sort: a.sort_name, album_count: a.album_count, cover_art: a.cover_art }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AlbumV1 {
    pub id: String,
    pub name: String,
    pub sort: Option<String>,
    pub artist: Option<String>,
    pub artist_ref: Option<String>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub song_count: u32,
    pub duration_secs: u32,
    pub cover_art: Option<String>,
    pub created: Option<String>,
}

impl AlbumV1 {
    /// Index key parts: artist, year, title, id. Groups an artist's albums
    /// chronologically; the id keeps same-named albums distinct.
    pub(crate) fn sort_parts(&self) -> (String, i32, String) {
        (
            fold(self.artist.as_deref().unwrap_or("")),
            self.year.unwrap_or(0),
            fold(self.sort.as_deref().unwrap_or(&self.name)),
        )
    }
}

impl From<subsonic::Album> for AlbumV1 {
    fn from(a: subsonic::Album) -> Self {
        Self {
            id: a.id,
            name: a.name,
            sort: a.sort_name,
            artist: a.artist,
            artist_ref: a.artist_id,
            year: a.year,
            genre: a.genre,
            song_count: a.song_count,
            duration_secs: a.duration,
            cover_art: a.cover_art,
            created: a.created,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TrackV1 {
    pub id: String,
    pub album_id: String,
    pub title: String,
    pub artist: Option<String>,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub duration_secs: Option<u32>,
    pub suffix: Option<String>,
    pub content_type: Option<String>,
    pub size: Option<u64>,
    pub bit_rate_kbps: Option<u32>,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub track_gain: Option<f32>,
    pub album_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_peak: Option<f32>,
}

impl TrackV1 {
    /// Index key parts after the album id: disc, track. Missing disc sorts as 1.
    pub(crate) fn order_parts(&self) -> (u32, u32) {
        (self.disc.unwrap_or(1), self.track.unwrap_or(0))
    }

    pub fn from_song(s: subsonic::Song, album_id: &str) -> Self {
        let rg = s.replay_gain.unwrap_or_default();
        Self {
            id: s.id,
            album_id: s.album_id.unwrap_or_else(|| album_id.to_owned()),
            title: s.title,
            artist: s.artist,
            track: s.track,
            disc: s.disc_number,
            duration_secs: s.duration,
            suffix: s.suffix,
            content_type: s.content_type,
            size: s.size,
            bit_rate_kbps: s.bit_rate,
            bit_depth: s.bit_depth,
            sample_rate: s.sampling_rate,
            channels: s.channel_count,
            track_gain: rg.track_gain,
            album_gain: rg.album_gain,
            track_peak: rg.track_peak,
            album_peak: rg.album_peak,
        }
    }

    /// e.g. `FLAC 24/88.2`, or just the codec when the server omits depth/rate.
    pub fn quality_label(&self) -> String {
        let codec = self.suffix.as_deref().unwrap_or("?").to_uppercase();
        match (self.bit_depth, self.sample_rate) {
            (Some(d), Some(r)) if d > 0 => format!("{codec} {d}/{}", khz(r)),
            (_, Some(r)) => format!("{codec} {}", khz(r)),
            _ => codec,
        }
    }
}

fn khz(hz: u32) -> String {
    if hz.is_multiple_of(1000) { (hz / 1000).to_string() } else { format!("{:.1}", hz as f32 / 1000.0) }
}
