//! Whether a newer Ond is out. GitHub's `/releases/latest` redirects to the
//! newest release's page, whose path ends in its tag, so one request with
//! redirects off tells us the version (no API, so no rate limit). The
//! release notes come from the repository at that tag, also without the API.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use tokio::sync::Notify;

pub const REPO: &str = "https://github.com/Solidsilver/ond";
pub const RELEASES: &str = "https://github.com/Solidsilver/ond/releases";

/// How this copy of Ond was built, set at build time: `github` by the release
/// workflow, `nix` by the Nix package. A plain `cargo build` sets nothing.
const DISTRIBUTION: Option<&str> = option_env!("OND_DISTRIBUTION");

/// Where this copy gets its updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// The GitHub releases: Ond looks for new ones itself.
    GitHub,
    /// A package manager (Nix): it updates Ond, so Ond doesn't ask.
    Packaged(&'static str),
    /// Built from source: nothing to compare against.
    Source,
}

pub fn channel() -> Channel {
    match DISTRIBUTION {
        Some("github") => Channel::GitHub,
        Some("nix") => Channel::Packaged("Nix"),
        Some(_) => Channel::Packaged("your package manager"),
        None => Channel::Source,
    }
}

/// A release version, `major.minor.patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u64, u64, u64);

impl Version {
    /// This build's.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is major.minor.patch")
    }

    /// `0.4.2` or `v0.4.2`; prereleases (`0.5.0-beta.1`) aren't releases here.
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.strip_prefix('v').unwrap_or(s).split('.').map(|p| p.parse::<u64>().ok());
        let v = Self(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(v)
    }

    /// The release's page.
    pub fn page(self) -> String {
        format!("{RELEASES}/tag/v{self}")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The version a `/releases/latest` redirect points at.
fn from_location(location: &str) -> Option<Version> {
    Version::parse(location.trim_end_matches('/').rsplit_once("/tag/")?.1)
}

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Ond/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
}

/// Looks 30 seconds after launch, then once a day (by the clock, so a night
/// asleep counts), while `on` is set, and right away whenever `now` is
/// notified (Check Now, whatever the setting). Hands `checked` the latest
/// release each time, or why it couldn't tell; a failed daily look tries
/// again an hour later.
pub fn start(rt: &tokio::runtime::Handle, on: Arc<AtomicBool>, now: Arc<Notify>, checked: impl Fn(Result<Version, String>) + Send + 'static) {
    if channel() != Channel::GitHub {
        return;
    }
    rt.spawn(async move {
        let client = match client() {
            Ok(c) => c,
            Err(e) => return tracing::warn!("update check: {e}"),
        };
        let mut asked = tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(30)) => false,
            _ = now.notified() => true,
        };
        let mut last: Option<SystemTime> = None;
        loop {
            // A clock set back makes `elapsed` fail: then look again.
            let due = last.is_none_or(|t| t.elapsed().map_or(true, |e| e >= DAY));
            if asked || (due && on.load(Ordering::Relaxed)) {
                let result = latest(&client).await;
                match &result {
                    Ok(v) => {
                        last = Some(SystemTime::now());
                        tracing::info!("update check: {v} is the latest release");
                    }
                    Err(e) => tracing::info!("update check: {e}"),
                }
                checked(result);
            }
            asked = tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(60 * 60)) => false,
                _ = now.notified() => true,
            };
        }
    });
}

async fn latest(client: &reqwest::Client) -> Result<Version, String> {
    let resp = client.head(format!("{RELEASES}/latest")).send().await.map_err(|e| e.to_string())?;
    let location = resp.headers().get(reqwest::header::LOCATION).and_then(|l| l.to_str().ok()).ok_or_else(|| format!("no redirect ({})", resp.status()))?;
    from_location(location).ok_or_else(|| format!("not a release: {location}"))
}

/// The hand-written notes for release `v`, from the repository at its tag.
pub fn fetch_notes(rt: &tokio::runtime::Handle, v: Version, done: impl FnOnce(Notes) + Send + 'static) {
    rt.spawn(async move {
        let url = format!("https://raw.githubusercontent.com/Solidsilver/ond/v{v}/.github/release-notes/v{v}.md");
        let text = async {
            let resp = client().map_err(|e| e.to_string())?.get(url).send().await.map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(format!("no release notes ({})", resp.status()));
            }
            resp.text().await.map_err(|e| e.to_string())
        };
        done(text.await.map(|t| notes(&t)));
    });
}

/// A release's notes as Settings › About shows them, or why they didn't load.
pub type Notes = Result<Vec<Block>, String>;

/// A piece of release notes, as Settings › About shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(String),
    Paragraph(String),
    Bullet(String),
}

/// The notes' Markdown as headings, paragraphs and bullets in plain text.
/// The Downloads section (a table of files) is left out: the release page has
/// the files.
fn notes(md: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut skipping = false;
    for line in md.lines().map(str::trim_end) {
        if let Some(h) = line.strip_prefix('#') {
            let h = h.trim_start_matches('#').trim();
            skipping = h.eq_ignore_ascii_case("downloads");
            if !skipping {
                out.push(Block::Heading(plain(h)));
            }
            continue;
        }
        if skipping || line.trim().is_empty() || line.trim_start().starts_with('|') {
            continue;
        }
        if let Some(b) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            out.push(Block::Bullet(plain(b)));
        } else if line.starts_with(' ') && matches!(out.last(), Some(Block::Bullet(_))) {
            // A bullet's wrapped continuation.
            if let Some(Block::Bullet(b)) = out.last_mut() {
                b.push(' ');
                b.push_str(&plain(line.trim()));
            }
        } else {
            out.push(Block::Paragraph(plain(line)));
        }
    }
    out
}

/// Inline Markdown as plain text: `[text](url)` keeps the text, and the
/// marks for code and bold go.
fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|i| open + i) else { break };
        let Some(end) = rest[close..].find(')').map(|i| close + i) else { break };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..close]);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out.replace("**", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(Version::parse("v0.4.2"), Some(Version(0, 4, 2)));
        assert_eq!(Version::parse("1.10.0"), Some(Version(1, 10, 0)));
        assert_eq!(Version::parse("0.5.0-beta.1"), None);
        assert_eq!(Version::parse("0.5"), None);
        assert_eq!(Version::parse("0.5.0.1"), None);
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        assert_eq!(Version(0, 4, 2).page(), "https://github.com/Solidsilver/ond/releases/tag/v0.4.2");
    }

    #[test]
    fn the_latest_redirect_names_the_version() {
        assert_eq!(from_location("https://github.com/Solidsilver/ond/releases/tag/v0.4.1"), Some(Version(0, 4, 1)));
        // No releases yet: GitHub sends you to the list.
        assert_eq!(from_location("https://github.com/Solidsilver/ond/releases"), None);
    }

    /// Asks GitHub: `cargo test -p ond-player updates::tests::github_names_the_latest -- --ignored`.
    #[test]
    #[ignore = "needs the network"]
    fn github_names_the_latest() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let v = rt.block_on(latest(&client().unwrap())).unwrap();
        assert!(v >= Version(0, 4, 1), "{v}");
    }

    #[test]
    fn release_notes_read_as_plain_blocks() {
        let md = "Opus files play, and `Ond` tells you.\n\n### Downloads\n\n| File | For |\n| --- | --- |\n| `a.dmg` | macOS |\n\nOn Nix: see the [README](https://x).\n\n### Opus\n\n- `.opus` files **play**.\n  They didn't before.\n- Second.\n";
        assert_eq!(
            notes(md),
            vec![
                Block::Paragraph("Opus files play, and Ond tells you.".into()),
                Block::Heading("Opus".into()),
                Block::Bullet(".opus files play. They didn't before.".into()),
                Block::Bullet("Second.".into()),
            ]
        );
    }

    #[test]
    fn this_build_has_a_version() {
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }
}
