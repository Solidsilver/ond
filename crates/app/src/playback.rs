//! Normal-mode playback preferences: ReplayGain and streaming quality, plus the
//! home-network check behind "transcode only when away". What they are and how
//! they turn into the engine's settings; `output::Outputs` holds the ones in
//! use (with the device and its mode) and saves changes to the preferences.

use audio::replaygain::{AlbumPos, album_context};
use audio::{GainConfig, GainMode, ReplayGain, Transcode};
use library::Track;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RgMode {
    #[default]
    Off,
    Track,
    Album,
    Auto,
}

impl RgMode {
    pub const ALL: [RgMode; 4] = [RgMode::Off, RgMode::Auto, RgMode::Album, RgMode::Track];

    pub fn label(self) -> &'static str {
        match self {
            RgMode::Off => "Off",
            RgMode::Track => "Track",
            RgMode::Album => "Album",
            RgMode::Auto => "Auto",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            RgMode::Off => "Tracks play at their mastered level",
            RgMode::Track => "Every track evened out to the same loudness",
            RgMode::Album => "Albums evened out, so quiet and loud songs on one album keep their contrast",
            RgMode::Auto => "Album gain while an album plays in order, track gain otherwise",
        }
    }
}

/// Where audio comes from: the original file, or a server-side transcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QualityMode {
    #[default]
    Original,
    /// Original on the home network, transcoded elsewhere.
    Auto,
    Transcode,
}

impl QualityMode {
    pub const ALL: [QualityMode; 3] = [QualityMode::Original, QualityMode::Auto, QualityMode::Transcode];

    pub fn label(self) -> &'static str {
        match self {
            QualityMode::Original => "Original",
            QualityMode::Auto => "Auto",
            QualityMode::Transcode => "Transcoded",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            QualityMode::Original => "Original files",
            QualityMode::Auto => "Original at home, transcoded away",
            QualityMode::Transcode => "Always transcoded",
        }
    }
}

/// Formats the decoder can play from a transcode. (Opus would need a decoder
/// the app doesn't ship yet.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TranscodeFormat {
    #[default]
    Aac,
    Mp3,
}

impl TranscodeFormat {
    pub const ALL: [TranscodeFormat; 2] = [TranscodeFormat::Aac, TranscodeFormat::Mp3];

    pub fn id(self) -> &'static str {
        match self {
            TranscodeFormat::Aac => "aac",
            TranscodeFormat::Mp3 => "mp3",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TranscodeFormat::Aac => "AAC",
            TranscodeFormat::Mp3 => "MP3",
        }
    }
}

pub const BITRATES: [u32; 4] = [128, 192, 256, 320];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackPrefs {
    pub replay_gain: RgMode,
    pub preamp_db: f32,
    /// For tracks without ReplayGain tags. See [`GainConfig::fallback_db`].
    pub untagged_db: f32,
    pub quality: QualityMode,
    pub format: TranscodeFormat,
    pub kbps: u32,
}

impl Default for PlaybackPrefs {
    fn default() -> Self {
        let gain = GainConfig::default();
        Self {
            replay_gain: RgMode::Off,
            preamp_db: gain.preamp_db,
            untagged_db: gain.fallback_db,
            quality: QualityMode::Original,
            format: TranscodeFormat::Aac,
            kbps: 256,
        }
    }
}

/// The engine's mode for the one the user picked (the engine has no serde;
/// this is the only place the two meet).
impl From<RgMode> for GainMode {
    fn from(m: RgMode) -> Self {
        match m {
            RgMode::Off => GainMode::Off,
            RgMode::Track => GainMode::Track,
            RgMode::Album => GainMode::Album,
            RgMode::Auto => GainMode::Auto,
        }
    }
}

impl PlaybackPrefs {
    pub fn gain_config(&self) -> GainConfig {
        GainConfig { mode: self.replay_gain.into(), preamp_db: self.preamp_db, fallback_db: self.untagged_db }
    }

    /// The transcode to ask for, given where the server is. An unknown network
    /// (still checking, or the lookup failed) streams originals.
    pub fn transcode(&self, network: Network) -> Option<Transcode> {
        let transcode = match self.quality {
            QualityMode::Original => false,
            QualityMode::Transcode => true,
            QualityMode::Auto => network == Network::Away,
        };
        transcode.then(|| Transcode { format: self.format.id().into(), kbps: self.kbps })
    }
}

/// Where the server is, as seen from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    Unknown,
    /// A private, loopback or link-local address (or a `.local` name).
    Home,
    Away,
}

impl Network {
    pub fn label(self) -> &'static str {
        match self {
            Network::Unknown => "Checking the network…",
            Network::Home => "Server is on this network",
            Network::Away => "Server is remote",
        }
    }
}

/// Classifies one server address. Tailscale and other CGNAT (100.64/10)
/// addresses count as away: they're how people reach home from elsewhere.
pub fn classify(ip: IpAddr) -> Network {
    let home = match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            let cgnat = o[0] == 100 && (o[1] & 0xC0) == 64;
            !cgnat && (v4.is_private() || v4.is_loopback() || v4.is_link_local())
        }
        IpAddr::V6(v6) => {
            let seg = v6.segments()[0];
            v6.is_loopback() || (seg & 0xfe00) == 0xfc00 || (seg & 0xffc0) == 0xfe80
        }
    };
    if home { Network::Home } else { Network::Away }
}

/// Host and port from a server URL like `https://music.example.com:4533/navidrome`
/// (a bare `host:port` is taken as http).
pub fn host_port(server: &str) -> Option<(String, u16)> {
    let url = if server.contains("://") { url::Url::parse(server) } else { url::Url::parse(&format!("http://{server}")) }.ok()?;
    let host = match url.host()? {
        url::Host::Domain(d) => d.to_owned(),
        url::Host::Ipv4(v4) => v4.to_string(),
        url::Host::Ipv6(v6) => v6.to_string(),
    };
    Some((host, url.port_or_known_default()?))
}

/// Resolves the server once, off the UI thread, and remembers the answer.
#[derive(Clone)]
pub struct NetworkProbe(Arc<AtomicU8>);

impl Default for NetworkProbe {
    fn default() -> Self {
        Self(Arc::new(AtomicU8::new(0)))
    }
}

impl NetworkProbe {
    pub fn start(server: &str) -> Self {
        let probe = Self::default();
        let Some((host, port)) = host_port(server) else { return probe };
        if host.to_ascii_lowercase().ends_with(".local") {
            probe.set(Network::Home);
            return probe;
        }
        let out = probe.clone();
        let spawned = std::thread::Builder::new().name("network-probe".into()).spawn(move || {
            let network = match (host.as_str(), port).to_socket_addrs() {
                // Any home address wins: a name that resolves both ways is reachable locally.
                Ok(addrs) => addrs
                    .map(|a| classify(a.ip()))
                    .reduce(|a, b| if a == Network::Home || b == Network::Home { Network::Home } else { Network::Away })
                    .unwrap_or(Network::Unknown),
                Err(e) => {
                    tracing::warn!("resolving {host}: {e}");
                    Network::Unknown
                }
            };
            out.set(network);
        });
        if let Err(e) = spawned {
            tracing::warn!("network probe: {e}");
        }
        probe
    }

    fn set(&self, n: Network) {
        self.0.store(n as u8, Ordering::Relaxed);
    }

    pub fn get(&self) -> Network {
        match self.0.load(Ordering::Relaxed) {
            x if x == Network::Home as u8 => Network::Home,
            x if x == Network::Away as u8 => Network::Away,
            _ => Network::Unknown,
        }
    }
}

/// The library's ReplayGain for a track (the decoder fills gaps from tags).
pub fn replay_gain(t: &Track) -> ReplayGain {
    ReplayGain { track_gain: t.track_gain, album_gain: t.album_gain, track_peak: t.track_peak, album_peak: t.album_peak }
}

fn pos(t: &Track) -> AlbumPos<'_> {
    AlbumPos { album_id: &t.album_id, disc: t.disc.unwrap_or(1), track: t.track.unwrap_or(0) }
}

/// Whether `cur` plays as part of its album, given its queue neighbours.
pub fn in_album(prev: Option<&Track>, cur: &Track, next: Option<&Track>) -> bool {
    album_context(prev.map(pos), pos(cur), next.map(pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_classify() {
        let home = ["192.168.1.20", "10.0.0.5", "172.16.3.1", "127.0.0.1", "169.254.1.1", "::1", "fd12::1", "fe80::1"];
        for a in home {
            assert_eq!(classify(a.parse().unwrap()), Network::Home, "{a}");
        }
        let away = ["100.101.1.2", "8.8.8.8", "2001:db8::1", "172.32.0.1"];
        for a in away {
            assert_eq!(classify(a.parse().unwrap()), Network::Away, "{a}");
        }
    }

    #[test]
    fn server_urls_parse_to_host_and_port() {
        assert_eq!(host_port("http://localhost:4533"), Some(("localhost".into(), 4533)));
        assert_eq!(host_port("https://music.example.com/navidrome"), Some(("music.example.com".into(), 443)));
        assert_eq!(host_port("http://[fd00::2]:4533/x"), Some(("fd00::2".into(), 4533)));
        assert_eq!(host_port("192.168.1.9:4533"), Some(("192.168.1.9".into(), 4533)));
        assert_eq!(host_port("http://user@nas.local"), Some(("nas.local".into(), 80)));
        assert_eq!(host_port("http://"), None);
    }

    #[test]
    fn quality_follows_the_network_only_in_auto() {
        let mut p = PlaybackPrefs::default();
        assert_eq!(p.transcode(Network::Away), None, "originals by default");
        p.quality = QualityMode::Auto;
        assert_eq!(p.transcode(Network::Home), None);
        assert_eq!(p.transcode(Network::Unknown), None, "unsure: no surprise transcoding");
        assert_eq!(p.transcode(Network::Away), Some(Transcode { format: "aac".into(), kbps: 256 }));
        p.quality = QualityMode::Transcode;
        p.format = TranscodeFormat::Mp3;
        p.kbps = 192;
        assert_eq!(p.transcode(Network::Home), Some(Transcode { format: "mp3".into(), kbps: 192 }));
    }

    #[test]
    fn prefs_round_trip_and_default_when_missing() {
        let p = PlaybackPrefs { replay_gain: RgMode::Auto, preamp_db: 2.0, ..Default::default() };
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<PlaybackPrefs>(&json).unwrap(), p);
        let partial: PlaybackPrefs = serde_json::from_str(r#"{"replay_gain":"album"}"#).unwrap();
        assert_eq!(partial.replay_gain, RgMode::Album);
        assert_eq!(partial.kbps, 256);
        let gain = partial.gain_config();
        assert_eq!(gain.mode, GainMode::Album);
    }
}
