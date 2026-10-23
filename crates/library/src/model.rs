use serde::{Deserialize, Serialize};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// Current schema versions. When a model changes, add a `V2` struct, point the
/// alias at it and rewrite rows in `Library::migrate`.
pub type Artist = ArtistV1;
pub type Album = AlbumV3;
pub type Track = TrackV3;

/// Sort-key normalization: case- and accent-insensitive, so "Björk" sorts with
/// "Bjork" and "bill evans" with "Bill Evans".
pub fn fold(s: &str) -> String {
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

/// Schema v2: adds record labels and all genres.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AlbumV2 {
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
    pub labels: Vec<String>,
    pub genres: Vec<String>,
}

/// An artist as a linkable entity: Navidrome splits "A, B & C" credits into
/// separate artists with ids.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArtistLink {
    pub id: Option<String>,
    pub name: String,
}

impl From<subsonic::ArtistRef> for ArtistLink {
    fn from(a: subsonic::ArtistRef) -> Self {
        Self { id: a.id, name: a.name }
    }
}

/// Schema v3: adds the album artists as entities.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AlbumV3 {
    pub id: String,
    pub name: String,
    pub sort: Option<String>,
    /// Display string, all album artists joined.
    pub artist: Option<String>,
    pub artist_ref: Option<String>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub song_count: u32,
    pub duration_secs: u32,
    pub cover_art: Option<String>,
    pub created: Option<String>,
    pub labels: Vec<String>,
    pub genres: Vec<String>,
    pub artists: Vec<ArtistLink>,
}

impl AlbumV3 {
    /// Index key parts: artist, year, title, id. Groups an artist's albums
    /// chronologically; the id keeps same-named albums distinct.
    pub(crate) fn sort_parts(&self) -> (String, i32, String) {
        (
            fold(self.artist.as_deref().unwrap_or("")),
            self.year.unwrap_or(0),
            fold(self.sort.as_deref().unwrap_or(&self.name)),
        )
    }

    /// Every genre: the OpenSubsonic list, else the single legacy field.
    pub fn all_genres(&self) -> Vec<&str> {
        if self.genres.is_empty() { self.genre.as_deref().into_iter().collect() } else { self.genres.iter().map(String::as_str).collect() }
    }
}

impl From<AlbumV1> for AlbumV2 {
    fn from(a: AlbumV1) -> Self {
        Self {
            id: a.id,
            name: a.name,
            sort: a.sort,
            artist: a.artist,
            artist_ref: a.artist_ref,
            year: a.year,
            genre: a.genre,
            song_count: a.song_count,
            duration_secs: a.duration_secs,
            cover_art: a.cover_art,
            created: a.created,
            labels: Vec::new(),
            genres: Vec::new(),
        }
    }
}

impl AlbumV3 {
    /// Fingerprint of what the album list says about an album. When it matches
    /// the stamp stored with the album's tracks, they don't need fetching again.
    /// Navidrome's cover art id carries the album's update time, so edits show up.
    pub fn sync_stamp(&self) -> u64 {
        let mut h = Fnv::default();
        h.str(&self.name);
        h.str(self.artist.as_deref().unwrap_or(""));
        h.str(self.cover_art.as_deref().unwrap_or(""));
        h.str(self.created.as_deref().unwrap_or(""));
        h.u64(u64::from(self.song_count));
        h.u64(u64::from(self.duration_secs));
        h.u64(self.year.unwrap_or(0) as u64);
        h.0
    }

    /// Whether `artist` (by id, else by name) is one of this album's artists.
    pub fn has_artist(&self, id: Option<&str>, name: &str) -> bool {
        if let Some(id) = id
            && (self.artist_ref.as_deref() == Some(id) || self.artists.iter().any(|a| a.id.as_deref() == Some(id)))
        {
            return true;
        }
        self.artist.as_deref() == Some(name) || self.artists.iter().any(|a| a.name == name)
    }
}

/// FNV-1a: stable across Rust versions, unlike `DefaultHasher`, so stored stamps stay valid.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }

    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
        self.bytes(&[0xff]);
    }

    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
}

impl From<AlbumV2> for AlbumV3 {
    fn from(a: AlbumV2) -> Self {
        Self {
            id: a.id,
            name: a.name,
            sort: a.sort,
            artist: a.artist,
            artist_ref: a.artist_ref,
            year: a.year,
            genre: a.genre,
            song_count: a.song_count,
            duration_secs: a.duration_secs,
            cover_art: a.cover_art,
            created: a.created,
            labels: a.labels,
            genres: a.genres,
            artists: Vec::new(),
        }
    }
}

impl From<subsonic::Album> for AlbumV3 {
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
            labels: a.record_labels.into_iter().map(|l| l.name).collect(),
            genres: a.genres.into_iter().map(|g| g.name).collect(),
            artists: a.artists.into_iter().map(ArtistLink::from).collect(),
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

/// A credit on a track: who, in what role ("composer", "performer" + "piano").
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Contributor {
    pub role: String,
    pub sub_role: Option<String>,
    pub name: String,
    pub artist_id: Option<String>,
}

/// Schema v2: adds the artist id, composer, credits and per-track cover art.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TrackV2 {
    pub id: String,
    pub album_id: String,
    pub title: String,
    pub artist: Option<String>,
    pub artist_ref: Option<String>,
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
    pub composer: Option<String>,
    pub contributors: Vec<Contributor>,
    pub cover_art: Option<String>,
}

impl From<TrackV1> for TrackV2 {
    fn from(t: TrackV1) -> Self {
        Self {
            id: t.id,
            album_id: t.album_id,
            title: t.title,
            artist: t.artist,
            artist_ref: None,
            track: t.track,
            disc: t.disc,
            duration_secs: t.duration_secs,
            suffix: t.suffix,
            content_type: t.content_type,
            size: t.size,
            bit_rate_kbps: t.bit_rate_kbps,
            bit_depth: t.bit_depth,
            sample_rate: t.sample_rate,
            channels: t.channels,
            track_gain: t.track_gain,
            album_gain: t.album_gain,
            track_peak: t.track_peak,
            album_peak: t.album_peak,
            composer: None,
            contributors: Vec::new(),
            cover_art: None,
        }
    }
}

/// Schema v3: adds the track artists as entities.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TrackV3 {
    pub id: String,
    pub album_id: String,
    pub title: String,
    /// Display string, all track artists joined.
    pub artist: Option<String>,
    pub artist_ref: Option<String>,
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
    pub composer: Option<String>,
    pub contributors: Vec<Contributor>,
    pub cover_art: Option<String>,
    pub artists: Vec<ArtistLink>,
}

impl From<TrackV2> for TrackV3 {
    fn from(t: TrackV2) -> Self {
        Self {
            id: t.id,
            album_id: t.album_id,
            title: t.title,
            artist: t.artist,
            artist_ref: t.artist_ref,
            track: t.track,
            disc: t.disc,
            duration_secs: t.duration_secs,
            suffix: t.suffix,
            content_type: t.content_type,
            size: t.size,
            bit_rate_kbps: t.bit_rate_kbps,
            bit_depth: t.bit_depth,
            sample_rate: t.sample_rate,
            channels: t.channels,
            track_gain: t.track_gain,
            album_gain: t.album_gain,
            track_peak: t.track_peak,
            album_peak: t.album_peak,
            composer: t.composer,
            contributors: t.contributors,
            cover_art: t.cover_art,
            artists: Vec::new(),
        }
    }
}

impl TrackV3 {
    /// Everyone this track credits, folded for lookup: its artists, composer
    /// and contributors, each name once.
    pub fn credited_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .chain(self.artist.as_deref())
            .chain(self.composer.as_deref())
            .chain(self.contributors.iter().map(|c| c.name.as_str()))
            .map(fold)
            .filter(|n| !n.is_empty())
            .collect();
        names.sort();
        names.dedup();
        names
    }

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
            artist_ref: s.artist_id,
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
            composer: s.display_composer.filter(|c| !c.is_empty()),
            contributors: s
                .contributors
                .into_iter()
                .map(|c| Contributor { role: c.role, sub_role: c.sub_role, name: c.artist.name, artist_id: c.artist.id })
                .collect(),
            cover_art: s.cover_art,
            artists: s.artists.into_iter().map(ArtistLink::from).collect(),
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
