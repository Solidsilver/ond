//! Update checks for apps that share a release repository. A single Atom
//! feed supplies both the newest full release and the newest version including
//! betas, so changing channels needs no network request. Apps supply their
//! own build version and distribution; account sessions do not scope these
//! repository-wide results.

pub mod install;
pub use install::PUBLIC_KEY;

use serde::{Deserialize, Serialize};
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
    /// The app's name, for errors and the HTTP user agent.
    pub app: &'static str,
    /// What comes before the version in the app's tags: `ond-v` for
    /// `ond-v0.5.0`. Any one of them makes a tag the app's.
    pub tags: &'static [&'static str],
    /// The folder under `.github/release-notes/` that has the app's notes.
    pub notes: &'static str,
    /// Whether this app publishes Windows archives.
    pub windows: bool,
}

/// Ond's: `ond-v0.5.0` from 0.5 on, and the `v0.4.4` tags from before Rede.
/// Rede's tags (`rede-v`) start with neither.
pub const OND: Releases = Releases { app: "Ond", tags: &["ond-v", "v"], notes: "ond", windows: true };

/// Rede's releases and hand-written notes.
pub const REDE: Releases = Releases { app: "Rede", tags: &["rede-v"], notes: "rede", windows: false };

/// Whether an app looks for newer versions (GitHub distributions only).
/// Installation is staged in the background and applied only after playback stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateCheck {
    Off,
    /// Says when one is out, with a link to it.
    #[default]
    Notify,
    /// Fetches a verified update and installs it on quit.
    Install,
}

/// Which versions the user wants to hear about. Betas includes full releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateChannel {
    Releases,
    Betas,
}

impl UpdateChannel {
    /// An absent choice follows the kind of build the user installed.
    pub fn effective(choice: Option<Self>, current: &Version) -> Self {
        choice.unwrap_or(if current.is_prerelease() { Self::Betas } else { Self::Releases })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Releases => "Releases",
            Self::Betas => "Betas",
        }
    }
}

/// Unknown channel choices follow the build, just like an absent choice.
pub fn lenient_update_channel<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<UpdateChannel>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or(None))
}

/// Where this copy gets its updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// The GitHub releases: the app looks for new ones itself.
    GitHub,
    /// A package manager updates the app, so the app doesn't ask.
    Packaged(&'static str),
    /// Built from source: nothing to compare against.
    Source,
}

pub fn channel(distribution: Option<&str>) -> Channel {
    match distribution {
        Some("github") => Channel::GitHub,
        Some("nix") => Channel::Packaged("Nix"),
        Some(_) => Channel::Packaged("your package manager"),
        None => Channel::Source,
    }
}

/// A semantic version and the prefix its release tag has. Equality and
/// ordering use semver precedence, ignoring the tag prefix and build metadata.
#[derive(Debug, Clone)]
pub struct Version {
    number: semver::Version,
    prefix: &'static str,
}

impl Version {
    /// `0.4.2`, `v0.4.2`, or a prerelease such as `0.1.0-beta.2`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(Self { number: semver::Version::parse(s.strip_prefix('v').unwrap_or(s)).ok()?, prefix: "v" })
    }

    fn from_tag(tag: &str, prefixes: &'static [&'static str]) -> Option<Self> {
        prefixes.iter().find_map(|&prefix| Some(Self { number: semver::Version::parse(tag.strip_prefix(prefix)?).ok()?, prefix }))
    }

    pub fn is_prerelease(&self) -> bool {
        !self.number.pre.is_empty()
    }

    pub fn tag(&self) -> String {
        format!("{}{self}", self.prefix)
    }

    pub fn page(&self) -> String {
        format!("{RELEASES}/tag/{}", self.tag())
    }

    /// The hand-written notes at this release's own tag.
    pub fn notes_url(&self, app: &Releases) -> String {
        format!("https://raw.githubusercontent.com/Solidsilver/ond/{}/.github/release-notes/{}/v{self}.md", self.tag(), app.notes)
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
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
        self.number.cmp_precedence(&other.number)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.number.fmt(f)
    }
}

/// Both answers from one feed, retained even when the user changes channel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Candidates {
    pub release: Option<Version>,
    /// The newest version, whether it is a beta or a full release.
    pub beta: Option<Version>,
}

impl Candidates {
    pub fn latest(&self, channel: UpdateChannel) -> Option<&Version> {
        match channel {
            UpdateChannel::Releases => self.release.as_ref(),
            UpdateChannel::Betas => self.beta.as_ref(),
        }
    }
}

/// Commands shared by both apps' About pages and update notices.
pub enum UpdateAction {
    /// Opens the version's release page, to download it by hand.
    Open(Version),
    /// Downloads and stages this version now, as Install updates
    /// automatically would; Restart to Update or quitting installs it.
    Install(Version),
    Skip(Version),
    Check(UpdateCheck),
    Channel(Option<UpdateChannel>),
    CheckNow,
    /// Closes through the normal quit path, which stops playback first.
    Restart,
    /// Tries a failed download again.
    Retry,
    Notes(Version),
}

/// Messages about updates, separate from any account's server work.
pub enum UpdateMsg {
    Checked(Result<Candidates, String>),
    ReleaseNotes(Version, Notes),
    Install(install::Event),
    /// An earlier quit could not finish installation, or startup cleanup failed.
    InstallError(install::Error),
}

/// What an app knows about updates. `latest` follows the effective channel;
/// `candidates` keeps both answers from the last successful check.
#[derive(Default)]
pub struct Updates {
    /// Both channels from the last successful check.
    pub candidates: Option<Candidates>,
    /// The selected channel's newest version, even if skipped or not newer.
    pub latest: Option<Version>,
    /// Check Now is waiting for an answer.
    pub checking: bool,
    /// When the last check succeeded.
    pub checked: Option<SystemTime>,
    /// Why the last check failed, without discarding its earlier candidates.
    pub error: Option<String>,
    /// The requested version and its notes; the inner `None` means loading.
    pub notes: Option<(Version, Option<Notes>)>,
    /// Installation state for About and the update notice; no polling is needed.
    pub install: install::State,
    pub install_error: Option<install::Error>,
    /// Background job ownership; use `stage_update` and `cancel_install`.
    pub installation: install::Installation,
    /// A version the user asked to install while automatic installs are
    /// off. It stays asked for until another version is the one on offer.
    pub requested: Option<Version>,
}

impl Updates {
    pub fn receive(&mut self, result: Result<Candidates, String>) {
        self.checking = false;
        match result {
            Ok(candidates) => {
                self.candidates = Some(candidates);
                self.checked = Some(SystemTime::now());
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// Selects the cached answer and returns an eligible notice. About can
    /// still show a skipped release through `latest`. Neither path downgrades.
    pub fn select(&mut self, current: &Version, choice: Option<UpdateChannel>, check: UpdateCheck, skipped: Option<&str>) -> Option<Version> {
        let channel = UpdateChannel::effective(choice, current);
        self.latest = self.candidates.as_ref().and_then(|c| c.latest(channel)).cloned();
        self.latest.as_ref().filter(|v| *v > current && check != UpdateCheck::Off && skipped.and_then(Version::parse).as_ref() != Some(v)).cloned()
    }
}

/// The newest full release and the newest version among the app's own tags.
/// Either candidate can be absent when the feed has no matching releases.
///
/// The feed is Atom, and each release is an `entry` whose `alternate` link is
/// its page, `…/releases/tag/<tag>`. The feed lists the latest few releases
/// of every app by date, so the order is no guide and the versions decide.
/// Release notes in an entry are escaped HTML, so a link in the notes is text
/// to the parser and can't be taken for the release's own.
pub fn newest(feed: &str, tags: &'static [&'static str]) -> Result<Candidates, String> {
    const ATOM: &str = "http://www.w3.org/2005/Atom";
    let doc = roxmltree::Document::parse(feed).map_err(|e| format!("the releases feed doesn't read: {e}"))?;
    let versions = doc.descendants().filter(|n| n.has_tag_name((ATOM, "entry"))).filter_map(|entry| {
        let link = entry.children().find(|n| n.has_tag_name((ATOM, "link")) && n.attribute("rel").is_none_or(|r| r == "alternate"))?;
        let (_, tag) = link.attribute("href")?.trim_end_matches('/').rsplit_once("/releases/tag/")?;
        Version::from_tag(tag, tags)
    });
    let mut found = Candidates::default();
    for v in versions {
        if !v.is_prerelease() && found.release.as_ref().is_none_or(|r| v > *r) {
            found.release = Some(v.clone());
        }
        if found.beta.as_ref().is_none_or(|b| v > *b) {
            found.beta = Some(v);
        }
    }
    Ok(found)
}

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

fn client(app: &Releases, version: &str) -> reqwest::Result<reqwest::Client> {
    // Redirects are followed, so a renamed repository still answers.
    reqwest::Client::builder().user_agent(format!("{}/{version}", app.app)).timeout(Duration::from_secs(20)).build()
}

/// Looks 30 seconds after launch, then once a day (by the clock, so a night
/// asleep counts), while `on` is set, and right away whenever `now` is
/// notified (Check Now, whatever the setting). Hands `checked` the app's
/// candidates each time, or why it couldn't tell; a failed daily look
/// tries again an hour later.
pub fn start(
    rt: &tokio::runtime::Handle,
    app: &'static Releases,
    version: &'static str,
    distribution: Channel,
    on: Arc<AtomicBool>,
    now: Arc<Notify>,
    checked: impl Fn(Result<Candidates, String>) + Send + 'static,
) {
    if distribution != Channel::GitHub {
        return;
    }
    rt.spawn(async move {
        let client = match client(app, version) {
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
                        tracing::info!("update check: latest versions {v:?}");
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

async fn latest(client: &reqwest::Client, app: &Releases) -> Result<Candidates, String> {
    let resp = client.get(format!("{RELEASES}.atom")).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("no releases feed ({})", resp.status()));
    }
    let feed = resp.text().await.map_err(|e| e.to_string())?;
    // The feed has only the latest few releases of all the apps: a run of
    // the others' can push this one's out of it until its next release.
    let found = newest(&feed, app.tags)?;
    if found.beta.is_none() {
        return Err(format!("no {} release among GitHub's latest", app.app));
    }
    Ok(found)
}

/// The hand-written notes for release `v`, from the repository at its tag.
pub fn fetch_notes(rt: &tokio::runtime::Handle, app: &'static Releases, version: &'static str, v: Version, done: impl FnOnce(Notes) + Send + 'static) {
    rt.spawn(async move {
        let url = v.notes_url(app);
        let text = async {
            let resp = client(app, version).map_err(|e| e.to_string())?.get(url).send().await.map_err(|e| e.to_string())?;
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

impl Updates {
    /// Reconciles installation with the app's eligible notice. Changing the
    /// setting/channel or skipping aborts work and releases its disposable stage.
    pub fn stage_update(
        &mut self,
        eligible: Option<&Version>,
        check: UpdateCheck,
        rt: &tokio::runtime::Handle,
        app: &'static Releases,
        cache: std::path::PathBuf,
        send: impl Fn(install::Event) + Send + Sync + 'static,
    ) {
        let asked = |v: &&Version| check == UpdateCheck::Install || (check == UpdateCheck::Notify && self.requested.as_ref() == Some(*v));
        let wanted = eligible.filter(asked).cloned();
        let wanted = wanted.as_ref();
        if self.install.version() == wanted {
            return;
        }
        self.cancel_install();
        if let Some(version) = wanted {
            self.install_error = None;
            self.install = install::State::Downloading { version: version.clone(), received: 0, total: None };
            self.installation.job = Some(install::start(rt, app, version.clone(), cache, self.installation.generation, send));
        }
    }

    pub fn cancel_install(&mut self) {
        self.installation.relaunch = false;
        self.installation.generation = self.installation.generation.wrapping_add(1);
        if let Some(job) = self.installation.job.take() {
            drop(job);
        }
        self.install = install::State::Idle;
    }

    /// Discards late results by generation; dropping an obsolete Prepared
    /// value also deletes its stage.
    pub fn install_event(&mut self, event: install::Event) {
        if event.generation != self.installation.generation {
            return;
        }
        let Some(version) = self.install.version().cloned() else { return };
        match event.kind {
            install::EventKind::Progress { received, total } => {
                if matches!(self.install, install::State::Downloading { .. }) {
                    self.install = install::State::Downloading { version, received, total };
                }
            }
            install::EventKind::Finished(result) => {
                self.installation.job = None;
                self.install = match result {
                    Ok(prepared) => install::State::Ready { version, prepared },
                    Err(error) => install::State::Failed { version, error },
                };
            }
            install::EventKind::NotifyOnly(reason) => {
                self.installation.job = None;
                self.install = install::State::NotifyOnly { version, reason };
            }
        }
    }

    /// Marks an explicit restart before the app requests its normal close.
    /// A plain quit installs without reopening, including after rollback.
    pub fn request_restart(&mut self) -> bool {
        let ready = self.install.ready();
        self.installation.relaunch = ready;
        ready
    }

    /// Playback must already have stopped through the app's normal quit path.
    pub fn install_on_quit(&mut self) -> Result<(), install::Error> {
        let relaunch = std::mem::take(&mut self.installation.relaunch);
        if self.install.ready()
            && let install::State::Ready { prepared, .. } = std::mem::take(&mut self.install)
        {
            prepared.install(relaunch)?;
        }
        self.cancel_install();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = include_str!("../testdata/releases.atom");

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn prerelease_versions_parse_and_keep_their_full_links() {
        let beta = Version::from_tag("rede-v0.1.0-beta.2", REDE.tags).unwrap();
        assert_eq!(beta.to_string(), "0.1.0-beta.2");
        assert_eq!(beta.tag(), "rede-v0.1.0-beta.2");
        assert_eq!(beta.page(), "https://github.com/Solidsilver/ond/releases/tag/rede-v0.1.0-beta.2");
        assert_eq!(beta.notes_url(&REDE), "https://raw.githubusercontent.com/Solidsilver/ond/rede-v0.1.0-beta.2/.github/release-notes/rede/v0.1.0-beta.2.md");
        assert_eq!(v("v0.4.2"), v("0.4.2"));
        for invalid in ["0.5", "0.5.0.1", "0.1.0-", "0.1.0-beta..2", "0.1.0-beta.02", "01.1.0", "0.1.0-β"] {
            assert!(Version::parse(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn prerelease_identifiers_follow_semver_precedence() {
        let ordered = ["1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0"];
        for pair in ordered.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{pair:?}");
        }
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0-beta.99999999999999999999999999999") > v("1.0.0-beta.100"));
        assert!(v("1.0.0-1") < v("1.0.0-a"));
        assert_eq!(v("1.0.0+build.1"), v("1.0.0+build.2"));
        assert_eq!(Version::from_tag("ond-v0.5.0", OND.tags), Some(v("0.5.0")));
    }

    #[test]
    fn the_feed_keeps_both_channels_without_crossing_apps() {
        let ond = newest(FEED, OND.tags).unwrap();
        assert_eq!(ond.release.unwrap().tag(), "ond-v0.5.10");
        assert_eq!(ond.beta.unwrap().tag(), "ond-v0.6.0-beta.1");
        let rede = newest(FEED, REDE.tags).unwrap();
        assert!(rede.release.is_none());
        assert_eq!(rede.beta.unwrap().tag(), "rede-v0.1.0-beta.2");
        assert_eq!(newest(FEED, &["v"]).unwrap().release.unwrap().tag(), "v0.4.4");
        for other in ["rede-v0.1.0-beta.2", "nightly", "ond-v", "vond-v0.5.0"] {
            assert!(Version::from_tag(other, OND.tags).is_none());
        }
    }

    #[test]
    fn an_empty_feed_has_no_candidates_and_a_broken_one_fails() {
        let empty = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>Releases</title></feed>"#;
        assert_eq!(newest(empty, OND.tags).unwrap(), Candidates::default());
        assert!(newest("<html><br></html>", OND.tags).is_err());
    }

    #[test]
    fn an_unset_channel_follows_the_build_and_an_explicit_choice_wins() {
        assert_eq!(UpdateChannel::effective(None, &v("0.6.0")), UpdateChannel::Releases);
        assert_eq!(UpdateChannel::effective(None, &v("0.6.0-beta.1")), UpdateChannel::Betas);
        assert_eq!(UpdateChannel::effective(Some(UpdateChannel::Releases), &v("0.6.0-beta.1")), UpdateChannel::Releases);
        assert_eq!(UpdateChannel::effective(Some(UpdateChannel::Betas), &v("0.6.0")), UpdateChannel::Betas);
    }

    #[test]
    fn selecting_a_channel_never_offers_a_downgrade_and_skipping_keeps_about() {
        let mut state = Updates::default();
        state.receive(Ok(Candidates { release: Some(v("0.5.10")), beta: Some(v("0.6.0-beta.2")) }));
        let current = v("0.6.0-beta.1");
        assert!(state.select(&current, Some(UpdateChannel::Releases), UpdateCheck::Notify, None).is_none());
        assert_eq!(state.select(&current, None, UpdateCheck::Notify, None), Some(v("0.6.0-beta.2")));
        assert!(state.select(&current, None, UpdateCheck::Notify, Some("0.6.0-beta.2")).is_none());
        assert_eq!(state.latest, Some(v("0.6.0-beta.2")));
        assert!(state.select(&current, None, UpdateCheck::Off, None).is_none());
        state.receive(Ok(Candidates { release: Some(v("0.6.0")), beta: Some(v("0.6.0")) }));
        assert_eq!(state.select(&current, Some(UpdateChannel::Releases), UpdateCheck::Notify, None), Some(v("0.6.0")));
        assert_eq!(state.select(&current, Some(UpdateChannel::Betas), UpdateCheck::Notify, None), Some(v("0.6.0")));
        state.receive(Err("offline".into()));
        assert_eq!(state.error.as_deref(), Some("offline"));
        assert_eq!(state.select(&current, None, UpdateCheck::Notify, None), Some(v("0.6.0")));
    }

    #[test]
    fn distribution_controls_whether_the_app_checks() {
        assert_eq!(channel(Some("github")), Channel::GitHub);
        assert_eq!(channel(Some("nix")), Channel::Packaged("Nix"));
        assert_eq!(channel(Some("other")), Channel::Packaged("your package manager"));
        assert_eq!(channel(None), Channel::Source);
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
                Block::Bullet("Second.".into())
            ]
        );
    }

    #[test]
    #[ignore = "needs the network"]
    fn github_names_the_latest_release() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let found = rt.block_on(latest(&client(&OND, "0.6.0").unwrap(), &OND)).unwrap();
        assert!(found.beta.unwrap() >= v("0.4.4"));
    }
}
