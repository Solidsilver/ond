//! Whether a newer Ond is out. Ond and Rede publish their releases from one
//! repository, so GitHub's "latest release" may be the other app's. Instead
//! Ond reads the repository's releases feed (`releases.atom`: no API, so no
//! key and no rate limit) and takes the highest version among the tags that
//! are its own ([`Releases`]). The release notes come from the repository at
//! that tag, also without the API.
//!
//! Nothing here is Ond's alone except [`OND`] and the URLs, so the module can
//! move to a shared crate when Rede needs it.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use tokio::sync::Notify;

pub const REPO: &str = "https://github.com/Solidsilver/ond";
pub const RELEASES: &str = "https://github.com/Solidsilver/ond/releases";

/// One app's releases in the shared repository: which tags are its own and
/// where its notes are kept.
pub struct Releases {
    /// The app's name, for errors.
    pub app: &'static str,
    /// What comes before the version in the app's tags: `ond-v` for
    /// `ond-v0.5.0`. Any one of them makes a tag the app's.
    pub tags: &'static [&'static str],
    /// The folder under `.github/release-notes/` that has the app's notes.
    pub notes: &'static str,
}

/// Ond's: `ond-v0.5.0` from 0.5 on, and the `v0.4.4` tags from before Rede.
/// Rede's tags (`rede-v`) start with neither.
pub const OND: Releases = Releases { app: "Ond", tags: &["ond-v", "v"], notes: "ond" };

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

/// A release version, `major.minor.patch`, and the prefix its tag has.
///
/// Versions compare and are equal by their numbers alone: `v0.4.4` and
/// `ond-v0.4.4` would be the same release.
#[derive(Debug, Clone, Copy)]
pub struct Version {
    number: (u64, u64, u64),
    /// What came before the number in the release's tag: `v` or `ond-v`.
    prefix: &'static str,
}

impl Version {
    /// This build's.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is major.minor.patch")
    }

    /// `0.4.2` or `v0.4.2`; prereleases (`0.5.0-beta.1`) aren't releases here.
    pub fn parse(s: &str) -> Option<Self> {
        Some(Self { number: number(s.strip_prefix('v').unwrap_or(s))?, prefix: "v" })
    }

    /// The version a tag names, if the tag starts with one of `prefixes` and
    /// the rest is a release's number: `ond-v0.5.0` with `ond-v`.
    fn from_tag(tag: &str, prefixes: &'static [&'static str]) -> Option<Self> {
        prefixes.iter().find_map(|&prefix| Some(Self { number: number(tag.strip_prefix(prefix)?)?, prefix }))
    }

    /// The release's tag.
    pub fn tag(self) -> String {
        format!("{}{self}", self.prefix)
    }

    /// The release's page.
    pub fn page(self) -> String {
        format!("{RELEASES}/tag/{}", self.tag())
    }
}

/// `0.4.2` as numbers; anything more or less (`0.5`, `0.5.0-beta.1`) isn't one.
fn number(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.split('.').map(|p| p.parse::<u64>().ok());
    let n = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(n)
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.number == other.number
    }
}

impl Eq for Version {}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.number.cmp(&other.number)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (major, minor, patch) = self.number;
        write!(f, "{major}.{minor}.{patch}")
    }
}

/// The highest version among the releases in a releases feed whose tags are
/// the app's, or `None` if the feed has none of them.
///
/// The feed is Atom, and each release is an `entry` whose `alternate` link is
/// its page, `…/releases/tag/<tag>`. The feed lists the latest few releases
/// of every app by date, so the order is no guide and the versions decide.
/// Release notes in an entry are escaped HTML, so a link in the notes is text
/// to the parser and can't be taken for the release's own.
fn newest(feed: &str, tags: &'static [&'static str]) -> Result<Option<Version>, String> {
    const ATOM: &str = "http://www.w3.org/2005/Atom";
    let doc = roxmltree::Document::parse(feed).map_err(|e| format!("the releases feed doesn't read: {e}"))?;
    let newest = doc
        .descendants()
        .filter(|n| n.has_tag_name((ATOM, "entry")))
        .filter_map(|entry| {
            let link = entry.children().find(|n| n.has_tag_name((ATOM, "link")) && n.attribute("rel").is_none_or(|r| r == "alternate"))?;
            let (_, tag) = link.attribute("href")?.trim_end_matches('/').rsplit_once("/releases/tag/")?;
            Version::from_tag(tag, tags)
        })
        .max();
    Ok(newest)
}

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

fn client() -> reqwest::Result<reqwest::Client> {
    // Redirects are followed, so a renamed repository still answers.
    reqwest::Client::builder().user_agent(concat!("Ond/", env!("CARGO_PKG_VERSION"))).timeout(Duration::from_secs(20)).build()
}

/// Looks 30 seconds after launch, then once a day (by the clock, so a night
/// asleep counts), while `on` is set, and right away whenever `now` is
/// notified (Check Now, whatever the setting). Hands `checked` the app's
/// latest release each time, or why it couldn't tell; a failed daily look
/// tries again an hour later.
pub fn start(
    rt: &tokio::runtime::Handle,
    app: &'static Releases,
    on: Arc<AtomicBool>,
    now: Arc<Notify>,
    checked: impl Fn(Result<Version, String>) + Send + 'static,
) {
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
                let result = latest(&client, app).await;
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

async fn latest(client: &reqwest::Client, app: &Releases) -> Result<Version, String> {
    let resp = client.get(format!("{RELEASES}.atom")).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("no releases feed ({})", resp.status()));
    }
    let feed = resp.text().await.map_err(|e| e.to_string())?;
    // The feed has only the latest few releases of all the apps: a run of
    // the others' can push this one's out of it until its next release.
    newest(&feed, app.tags)?.ok_or_else(|| format!("no {} release among GitHub's latest", app.app))
}

/// The hand-written notes for release `v`, from the repository at its tag.
pub fn fetch_notes(rt: &tokio::runtime::Handle, app: &'static Releases, v: Version, done: impl FnOnce(Notes) + Send + 'static) {
    rt.spawn(async move {
        let url = format!("https://raw.githubusercontent.com/Solidsilver/ond/{}/.github/release-notes/{}/v{v}.md", v.tag(), app.notes);
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

    fn v(major: u64, minor: u64, patch: u64) -> Version {
        Version { number: (major, minor, patch), prefix: "v" }
    }

    /// A releases feed as GitHub writes it, with Ond's old and new tags
    /// between Rede's, a prerelease, and a release listed out of order.
    const FEED: &str = include_str!("../testdata/releases.atom");

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(Version::parse("v0.4.2"), Some(v(0, 4, 2)));
        assert_eq!(Version::parse("1.10.0"), Some(v(1, 10, 0)));
        assert_eq!(Version::parse("0.5.0-beta.1"), None);
        assert_eq!(Version::parse("0.5"), None);
        assert_eq!(Version::parse("0.5.0.1"), None);
        assert!(v(0, 10, 0) > v(0, 9, 9));
        assert!(v(1, 0, 0) > v(0, 99, 99));
        assert_eq!(v(0, 4, 2).page(), "https://github.com/Solidsilver/ond/releases/tag/v0.4.2");
    }

    #[test]
    fn ond_takes_both_its_tag_forms_and_none_of_redes() {
        let ond = Version::from_tag("ond-v0.5.0", OND.tags).unwrap();
        assert_eq!((ond, ond.tag().as_str()), (v(0, 5, 0), "ond-v0.5.0"));
        assert_eq!(ond.page(), "https://github.com/Solidsilver/ond/releases/tag/ond-v0.5.0");
        assert_eq!(Version::from_tag("v0.4.4", OND.tags).map(Version::tag).as_deref(), Some("v0.4.4"));
        for not_ond in ["rede-v0.1.0", "rede-v0.1", "0.4.4", "ond-0.5.0", "ond-v0.5.0-beta.1", "ond-v", "vond-v0.5.0", "nightly"] {
            assert_eq!(Version::from_tag(not_ond, OND.tags), None, "{not_ond}");
        }
        assert_eq!(Version::from_tag("rede-v0.1.0", &["rede-v"]).map(Version::tag).as_deref(), Some("rede-v0.1.0"));
        assert_eq!(Version::from_tag("v0.4.4", &["rede-v"]), None);
        assert_eq!(Version::from_tag("ond-v0.5.0", &["rede-v"]), None);
    }

    #[test]
    fn the_feed_names_each_apps_newest_release() {
        // 0.5.10 is listed below 0.5.9, and 0.6.0-beta.1 above both.
        let ond = newest(FEED, OND.tags).unwrap().unwrap();
        assert_eq!(ond.tag(), "ond-v0.5.10");
        assert_eq!(newest(FEED, &["v"]).unwrap().map(Version::tag).as_deref(), Some("v0.4.4"));
        assert_eq!(newest(FEED, &["rede-v"]).unwrap().map(Version::tag).as_deref(), Some("rede-v1.0.0"));
    }

    #[test]
    fn a_feed_without_the_apps_releases_names_none() {
        let theirs = FEED.replace("/tag/ond-v", "/tag/rede-v").replace("/tag/v", "/tag/rede-v");
        assert_eq!(newest(&theirs, OND.tags), Ok(None));
        let empty = r#"<?xml version="1.0" encoding="UTF-8"?><feed xmlns="http://www.w3.org/2005/Atom"><title>Release notes from ond</title></feed>"#;
        assert_eq!(newest(empty, OND.tags), Ok(None));
        // A page where the feed should be (a captive portal, an error page).
        assert!(newest("<!DOCTYPE html><html><body><p>Sign in<br></body></html>", OND.tags).is_err());
    }

    /// Asks GitHub: `cargo test -p ond-player updates::tests::github_names_the_latest -- --ignored`.
    #[test]
    #[ignore = "needs the network"]
    fn github_names_the_latest() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let v = rt.block_on(latest(&client().unwrap(), &OND)).unwrap();
        assert!(v >= Version::parse("0.4.4").unwrap(), "{v}");
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
