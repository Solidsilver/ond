//! The diagnostics bar: a thin strip along the bottom that gathers the
//! technical state in one place (signal path, output, engine, buffer,
//! download, sync, analysis, server, frame time). Off by default; nothing it
//! shows is removed from anywhere else. Values refresh at most twice a second
//! and the bar asks for no repaints of its own.

use super::ListenerApp;
use crate::theme::Tokens;
use audio::{BitPerfectStatus, PlayState, PlaybackMode};
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const BAR_H: f32 = 22.0;
/// How often the bar's values are recomputed.
const REFRESH: Duration = Duration::from_millis(500);
/// A server round trip older than this is measured again (while playing).
const LATENCY_MAX_AGE: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct Diagnostics {
    pub shown: bool,
    latency: Option<Duration>,
    latency_at: Option<Instant>,
    latency_pending: bool,
    /// The last download sample (when, bytes) and the rate derived from it.
    sample: Option<(Instant, u64)>,
    rate: Option<f64>,
    /// Smoothed time spent building a frame, in ms.
    frame_ms: Option<f32>,
    cached: Option<(Instant, Arc<Vec<Segment>>)>,
}

/// One labelled reading in the bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub text: String,
    /// Shown on hover.
    pub detail: String,
    pub tone: Tone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Good,
    Warn,
}

impl Diagnostics {
    pub fn new(shown: bool) -> Self {
        Self { shown, ..Default::default() }
    }

    pub fn latency(&self) -> Option<Duration> {
        self.latency
    }

    /// A ping is on its way; don't send another.
    pub fn latency_asked(&mut self) {
        self.latency_pending = true;
    }

    pub fn latency_stale(&self) -> bool {
        !self.latency_pending && self.latency_at.is_none_or(|t| t.elapsed() > LATENCY_MAX_AGE)
    }

    pub fn set_latency(&mut self, rtt: Option<Duration>) {
        self.latency = rtt;
        self.latency_at = Some(Instant::now());
        self.latency_pending = false;
    }

    pub fn note_frame(&mut self, took: Duration) {
        let ms = took.as_secs_f32() * 1000.0;
        self.frame_ms = Some(self.frame_ms.map_or(ms, |m| m * 0.8 + ms * 0.2));
    }

    /// Bytes per second of the current download, from successive samples
    /// (`fetched` bytes now). Resets when the count goes backwards (a new track).
    fn sample_download(&mut self, fetched: u64, now: Instant) {
        match self.sample {
            Some((at, bytes)) if fetched >= bytes => {
                let dt = now.duration_since(at).as_secs_f64();
                if dt >= REFRESH.as_secs_f64() {
                    self.rate = Some((fetched - bytes) as f64 / dt);
                    self.sample = Some((now, fetched));
                }
            }
            _ => {
                self.rate = None;
                self.sample = Some((now, fetched));
            }
        }
    }
}

impl ListenerApp {
    /// The readings, recomputed at most every [`REFRESH`].
    fn diagnostics_segments(&mut self) -> Arc<Vec<Segment>> {
        let now = Instant::now();
        if let Some((at, segs)) = &self.diagnostics.cached
            && now.duration_since(*at) < REFRESH
        {
            return Arc::clone(segs);
        }
        let status = &self.engine.status;
        let state = status.state();
        let mut out = Vec::new();

        // Signal path and output.
        if let Some(p) = self.output.signal().filter(|_| state != PlayState::Stopped) {
            let tone = match &p.bit_perfect {
                BitPerfectStatus::Verified => Tone::Good,
                BitPerfectStatus::Unavailable { .. } => Tone::Warn,
                _ => Tone::Normal,
            };
            out.push(Segment { text: p.summary(), detail: format!("Signal path\n{}", p.summary()), tone });
            let mode = match status.mode() {
                PlaybackMode::BitPerfect => "bit-perfect",
                PlaybackMode::Normal => "normal",
            };
            out.push(Segment { text: format!("{} · {mode}", p.output), detail: format!("Output: {}\nMode: {mode}", p.output), tone: Tone::Normal });
        }

        // Engine.
        if state != PlayState::Stopped {
            let label = match state {
                PlayState::Loading => "loading",
                PlayState::Buffering => "buffering",
                PlayState::Playing => "playing",
                PlayState::Paused => "paused",
                PlayState::Stopped => "stopped",
            };
            let ahead = status.buffered_secs();
            out.push(Segment {
                text: format!("{label} · buf {ahead:.1} s"),
                detail: format!("Engine: {label}\nDecoded audio queued ahead of the output: {ahead:.2} s"),
                tone: if state == PlayState::Buffering { Tone::Warn } else { Tone::Normal },
            });
            let underruns = status.underruns.load(std::sync::atomic::Ordering::Relaxed);
            if underruns > 0 {
                out.push(Segment { text: format!("{underruns} underrun{}", if underruns == 1 { "" } else { "s" }), detail: "Times the output ran out of audio this session".into(), tone: Tone::Warn });
            }
        }

        // Download.
        let fetched = status.fetched_bytes.load(std::sync::atomic::Ordering::Relaxed);
        let total = status.total_bytes.load(std::sync::atomic::Ordering::Relaxed);
        self.diagnostics.sample_download(fetched, now);
        if state != PlayState::Stopped && (total > 0 || fetched > 0) {
            let text = if total > 0 && fetched >= total {
                if status.downloading() { "track cached · prefetching next".to_owned() } else { "track cached".to_owned() }
            } else {
                let pct = if total > 0 { format!("{:.0}%", 100.0 * fetched as f64 / total as f64) } else { offline::human_bytes(fetched) };
                match self.diagnostics.rate.filter(|r| *r > 0.0) {
                    Some(r) => format!("dl {pct} · {}/s", offline::human_bytes(r as u64)),
                    None => format!("dl {pct}"),
                }
            };
            let detail = format!(
                "Current file: {} of {}{}",
                offline::human_bytes(fetched),
                if total > 0 { offline::human_bytes(total) } else { "unknown size".into() },
                if status.downloading() { "\nStill downloading (this track or the next one's prefetch)" } else { "" }
            );
            out.push(Segment { text, detail, tone: Tone::Normal });
        }

        // Background work.
        if let Some(p) = self.sync_progress {
            out.push(Segment { text: format!("sync {}/{}", p.done, p.total), detail: p.label(), tone: Tone::Normal });
        } else if self.album_syncing {
            out.push(Segment { text: "sync albums…".into(), detail: "Fetching the album list".into(), tone: Tone::Normal });
        }
        let a = self.analyzer.progress();
        if a.active() {
            let state = if a.paused {
                " paused"
            } else if a.waiting {
                " waiting"
            } else {
                ""
            };
            out.push(Segment { text: format!("analysis {}/{}{state}", a.done.min(a.total), a.total), detail: "Library analysis (loudness, dynamic range, hi-res checks)".into(), tone: Tone::Normal });
        }

        // Server.
        if let Some(s) = &self.settings {
            let host = server_host(&s.server);
            let (text, tone) = if !self.online {
                (format!("{host} · offline"), Tone::Warn)
            } else {
                match self.diagnostics.latency {
                    Some(rtt) => (format!("{host} · {} ms", rtt.as_millis()), Tone::Normal),
                    None => (host.clone(), Tone::Normal),
                }
            };
            let detail = format!("{}\n{}", s.server, if self.server_info.is_empty() { "Not connected" } else { &self.server_info });
            out.push(Segment { text, detail, tone });
        }

        if let Some(ms) = self.diagnostics.frame_ms {
            out.push(Segment { text: format!("ui {ms:.1} ms"), detail: "Time to build the last frames (smoothed)".into(), tone: Tone::Normal });
        }
        let out = Arc::new(out);
        self.diagnostics.cached = Some((now, Arc::clone(&out)));
        out
    }

    /// Draws the bar: segments left to right, divided by hairlines, dropping
    /// whatever doesn't fit.
    pub(super) fn diagnostics_bar(&mut self, ui: &mut Ui) {
        let t = Tokens::of(ui);
        let segs = self.diagnostics_segments();
        let rect = ui.available_rect_before_wrap();
        ui.painter().hline(rect.x_range(), rect.top() - 0.5, egui::Stroke::new(1.0, t.hairline));
        let font = FontId::monospace(10.0);
        // Text stays muted; a segment worth a glance gets a small coloured dot.
        const DOT: f32 = 10.0;
        let mut x = rect.left();
        for (i, s) in segs.iter().enumerate() {
            let dot = match s.tone {
                Tone::Normal => None,
                Tone::Good => Some(t.ok),
                Tone::Warn => Some(t.warn),
            };
            let g = ui.painter().layout_no_wrap(s.text.clone(), font.clone(), t.muted);
            let gap = if i == 0 { 0.0 } else { 17.0 };
            let lead = if dot.is_some() { DOT } else { 0.0 };
            if x + gap + lead + g.size().x > rect.right() {
                break;
            }
            if i > 0 {
                let mid = x + gap / 2.0;
                ui.painter().vline(mid, rect.center().y - 5.0..=rect.center().y + 5.0, egui::Stroke::new(1.0, t.hairline));
            }
            x += gap;
            if let Some(c) = dot {
                ui.painter().circle_filled(pos2(x + 3.0, rect.center().y), 3.0, c);
                x += lead;
            }
            let r = Rect::from_min_size(pos2(x, rect.center().y - g.size().y / 2.0), g.size());
            ui.painter().galley(r.min, g, t.muted);
            let resp = ui.interact(r.expand2(vec2(4.0, 4.0)), ui.scope_id().with(("diag", i)), Sense::hover());
            let detail = s.detail.clone();
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &detail));
            resp.on_hover_text(&s.detail);
            x = r.right();
        }
    }
}

/// "https://music.example.com:4533/" → "music.example.com".
pub fn server_host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    // Keep IPv6 brackets intact; strip a trailing :port.
    let host = match host.rfind(':') {
        Some(i) if !host[i..].contains(']') && host[i + 1..].chars().all(|c| c.is_ascii_digit()) => &host[..i],
        _ => host,
    };
    if host.is_empty() { url.to_owned() } else { host.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_read_short() {
        assert_eq!(server_host("https://music.example.com:4533/"), "music.example.com");
        assert_eq!(server_host("http://navidrome.lan"), "navidrome.lan");
        assert_eq!(server_host("http://192.168.1.4:4533"), "192.168.1.4");
        assert_eq!(server_host("http://[::1]:4533/x"), "[::1]");
        assert_eq!(server_host("navidrome.lan:4533"), "navidrome.lan");
    }

    #[test]
    fn download_rate_comes_from_samples_and_resets_on_a_new_track() {
        let mut d = Diagnostics::default();
        let t0 = Instant::now();
        d.sample_download(0, t0);
        assert_eq!(d.rate, None);
        d.sample_download(1_000_000, t0 + Duration::from_secs(1));
        assert_eq!(d.rate.map(|r| r.round()), Some(1_000_000.0));
        // Too soon: keep the last rate.
        d.sample_download(1_100_000, t0 + Duration::from_millis(1100));
        assert_eq!(d.rate.map(|r| r.round()), Some(1_000_000.0));
        // A new track starts over.
        d.sample_download(10, t0 + Duration::from_secs(2));
        assert_eq!(d.rate, None);
    }

    #[test]
    fn latency_is_measured_again_only_when_old_and_not_already_asked() {
        let mut d = Diagnostics::default();
        assert!(d.latency_stale());
        d.latency_asked();
        assert!(!d.latency_stale());
        d.set_latency(Some(Duration::from_millis(12)));
        assert!(!d.latency_stale());
        assert_eq!(d.latency(), Some(Duration::from_millis(12)));
    }
}
