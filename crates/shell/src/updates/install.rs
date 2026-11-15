//! Verified, disposable stages and platform installation plans. Only a
//! `Prepared` update can be installed; downloads never expose that capability.
//! Account sessions do not scope this work because releases belong to the app.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod stage;
#[cfg(unix)]
mod unix;
#[cfg(target_os = "windows")]
mod windows;

use super::{Releases, Version};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use stage::{clean_stale, verify};

/// Minisign key 85322167E18204BC. Rotation requires a release signed by the
/// old key that carries its successor before CI switches to the new key.
pub const PUBLIC_KEY: &str = "RWS8BILhZyEyhZhjHWOEkGgfkz6ZofGPH/EdWBRiGwB59QyV943Cn4BP";

/// Reasons to keep notifying instead of attempting to install. The drawing
/// layer supplies the short user-facing wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    UnsupportedPlatform,
    UnknownLayout,
    Translocated,
    DiskImage,
    ReadOnly,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("the update signature did not verify")]
    Signature,
    #[error("the update archive is incomplete or has an unexpected layout")]
    Archive,
    #[error("{0} failed")]
    Command(&'static str),
    #[error("the update's signing team does not match this app")]
    SigningTeam,
    #[error("the installer could not finish: {0}")]
    Worker(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Macos,
    Linux,
    Windows,
}

impl Platform {
    pub fn current() -> Option<Self> {
        match std::env::consts::OS {
            "macos" => Some(Self::Macos),
            "linux" => Some(Self::Linux),
            "windows" => Some(Self::Windows),
            _ => None,
        }
    }
}

/// Uses the build target, never the host reported by `uname` (Rosetta and
/// cross builds can differ). Linux's aarch64 name matches the bundle scripts.
pub fn asset_name(app: &Releases, version: &Version, platform: Platform, arch: &str) -> Option<String> {
    let suffix = match (platform, arch) {
        (Platform::Macos, "aarch64" | "x86_64") => "macos-universal.zip".to_owned(),
        (Platform::Linux, "aarch64" | "x86_64") => format!("linux-{arch}.tar.gz"),
        (Platform::Windows, "x86_64") if app.windows => "windows-x86_64.zip".to_owned(),
        _ => return None,
    };
    Some(format!("{}-{version}-{suffix}", app.app))
}

/// All decisions that can be tested without running a platform tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    MacBundle(PathBuf),
    LinuxLayout { executable: PathBuf, prefix: PathBuf },
    Binary(PathBuf),
}

/// Recognizes exactly the install.sh layout, including its desktop entry.
pub fn linux_prefix(executable: &Path, app: &str, desktop_exists: impl FnOnce(&Path) -> bool) -> Option<PathBuf> {
    let bin = executable.parent()?;
    let prefix = bin.parent()?;
    (executable.file_name()? == app && bin.file_name()? == "bin" && desktop_exists(&prefix.join(format!("share/applications/dev.solidsilver.{app}.desktop"))))
        .then(|| prefix.to_owned())
}

pub fn translocated(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "AppTranslocation")
}

pub fn plan(
    platform: Platform,
    executable: &Path,
    app: &str,
    writable: bool,
    read_only_mount: bool,
    desktop_exists: impl FnOnce(&Path) -> bool,
) -> Result<Plan, Fallback> {
    if platform == Platform::Macos {
        if translocated(executable) {
            return Err(Fallback::Translocated);
        }
        if executable.starts_with("/Volumes") && read_only_mount {
            return Err(Fallback::DiskImage);
        }
        let bundle = executable.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).ok_or(Fallback::UnknownLayout)?;
        if !writable {
            return Err(Fallback::ReadOnly);
        }
        return Ok(Plan::MacBundle(bundle.to_owned()));
    }
    if !writable {
        return Err(Fallback::ReadOnly);
    }
    if platform == Platform::Linux
        && let Some(prefix) = linux_prefix(executable, app, desktop_exists)
    {
        return Ok(Plan::LinuxLayout { executable: executable.to_owned(), prefix });
    }
    Ok(Plan::Binary(executable.to_owned()))
}

/// A verified archive, unpacked and checked by this platform. Its private
/// ownership keeps partial downloads out of the quit path.
pub struct Prepared {
    stage: stage::Stage,
    replacement: PathBuf,
    plan: Plan,
    /// A same-filesystem copy for an atomic rename, when needed.
    neighbor: Option<tempfile::TempDir>,
}

impl Prepared {
    /// Called only after the app's normal playback shutdown. Unix helpers
    /// wait for process exit as well, before changing the installation.
    pub fn install(self, relaunch: bool) -> Result<(), Error> {
        #[cfg(any(test, feature = "preview"))]
        if self.replacement.as_os_str().is_empty() {
            return Err(Error::Worker("a preview can't be installed".into()));
        }
        #[cfg(target_os = "macos")]
        return macos::install(self, relaunch);
        #[cfg(target_os = "linux")]
        return linux::install(self, relaunch);
        #[cfg(target_os = "windows")]
        return windows::install(self, relaunch);
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        Err(Error::Archive)
    }
}

#[derive(Default)]
pub enum State {
    #[default]
    Idle,
    Downloading {
        version: Version,
        received: u64,
        total: Option<u64>,
    },
    Ready {
        version: Version,
        prepared: Prepared,
    },
    Failed {
        version: Version,
        error: Error,
    },
    NotifyOnly {
        version: Version,
        reason: Fallback,
    },
}

impl State {
    /// A ready update for snapshots and UI tests in the apps. It has nothing
    /// to install: its empty replacement makes [`Prepared::install`] refuse.
    #[cfg(any(test, feature = "preview"))]
    pub fn preview_ready(version: Version) -> Self {
        let stage = stage::Stage::preview().expect("a temporary folder for the preview's stage");
        Self::Ready { version, prepared: Prepared { stage, replacement: PathBuf::new(), plan: Plan::Binary(PathBuf::new()), neighbor: None } }
    }

    pub fn version(&self) -> Option<&Version> {
        match self {
            Self::Idle => None,
            Self::Downloading { version, .. } | Self::Ready { version, .. } | Self::Failed { version, .. } | Self::NotifyOnly { version, .. } => Some(version),
        }
    }

    pub fn ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

/// A generation prevents late progress, failures or ready messages from a
/// skipped/replaced download restoring it, even after retrying the same tag.
pub struct Event {
    pub generation: u64,
    pub kind: EventKind,
}

pub enum EventKind {
    Progress { received: u64, total: Option<u64> },
    Finished(Result<Prepared, Error>),
    NotifyOnly(Fallback),
}

pub(super) fn start(
    rt: &tokio::runtime::Handle,
    app: &'static Releases,
    version: Version,
    root: PathBuf,
    generation: u64,
    send: impl Fn(Event) + Send + Sync + 'static,
) -> Job {
    let send = Arc::new(send);
    Job(rt.spawn(async move {
        let inspection = tokio::task::spawn_blocking(move || inspect(app)).await;
        let plan = match inspection {
            Ok(Ok(plan)) => plan,
            Ok(Err(reason)) => return send(Event { generation, kind: EventKind::NotifyOnly(reason) }),
            Err(e) => return send(Event { generation, kind: EventKind::Finished(Err(Error::Worker(e.to_string()))) }),
        };
        let result = async {
            let platform = Platform::current().ok_or(Error::Archive)?;
            let name = asset_name(app, &version, platform, std::env::consts::ARCH).ok_or(Error::Archive)?;
            let base = format!("{}/download/{}", super::RELEASES, version.tag());
            let client = reqwest::Client::builder().user_agent(app.app).timeout(Duration::from_secs(3600)).read_timeout(Duration::from_secs(30)).build()?;
            let progress = send.clone();
            let stage =
                stage::fetch(&client, &root, &name, &format!("{base}/{name}"), &format!("{base}/{name}.minisig"), PUBLIC_KEY, move |received, total| {
                    progress(Event { generation, kind: EventKind::Progress { received, total } });
                })
                .await?;
            tokio::task::spawn_blocking(move || prepare(stage, plan, app)).await.map_err(|e| Error::Worker(e.to_string()))?
        }
        .await;
        let kind = match result {
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied => EventKind::NotifyOnly(Fallback::ReadOnly),
            result => EventKind::Finished(result),
        };
        send(Event { generation, kind });
    }))
}

/// Keeps the worker private while allowing `Updates { ..Default::default() }`
/// in app fixtures. Dropping it aborts the download and releases its stage.
#[derive(Default)]
pub struct Installation {
    pub(super) job: Option<Job>,
    pub(super) generation: u64,
    pub(super) relaunch: bool,
}

pub(super) struct Job(tokio::task::JoinHandle<()>);

impl Drop for Job {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn inspect(app: &Releases) -> Result<Plan, Fallback> {
    let platform = Platform::current().ok_or(Fallback::UnsupportedPlatform)?;
    if asset_name(app, &Version::parse("0.0.0").unwrap(), platform, std::env::consts::ARCH).is_none() {
        return Err(Fallback::UnsupportedPlatform);
    }
    let exe = std::env::current_exe().map_err(|_| Fallback::UnknownLayout)?;
    #[cfg(target_os = "macos")]
    let read_only_mount = macos::read_only_mount(&exe).map_err(|_| Fallback::ReadOnly)?;
    #[cfg(not(target_os = "macos"))]
    let read_only_mount = false;
    let tentative = plan(platform, &exe, &app.app.to_lowercase(), true, read_only_mount, |p| p.is_file())?;
    let parent = match &tentative {
        Plan::MacBundle(bundle) => bundle.parent(),
        Plan::LinuxLayout { prefix, .. } => Some(prefix.as_path()),
        Plan::Binary(executable) => executable.parent(),
    }
    .ok_or(Fallback::UnknownLayout)?;
    // Probe actual access (ACLs and read-only mounts included), not mode bits.
    let _probe = tempfile::Builder::new().prefix(".update-write-").tempfile_in(parent).map_err(|_| Fallback::ReadOnly)?;
    #[cfg(target_os = "linux")]
    linux::check_writable(&tentative)?;
    Ok(tentative)
}

fn prepare(stage: stage::Stage, plan: Plan, app: &Releases) -> Result<Prepared, Error> {
    #[cfg(target_os = "macos")]
    return macos::prepare(stage, plan, app);
    #[cfg(target_os = "linux")]
    return linux::prepare(stage, plan, app);
    #[cfg(target_os = "windows")]
    return windows::prepare(stage, plan, app);
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    Err(Error::Archive)
}

/// Called by each app before opening logs, settings or databases. Windows
/// update restarts wait for the previous process to release its resources;
/// a timeout or access error stops startup rather than racing its databases.
pub fn wait_for_predecessor() -> Result<(), Error> {
    #[cfg(target_os = "windows")]
    if let Some(pid) = predecessor(std::env::args_os().skip(1)) {
        windows::wait_for_predecessor(pid, Duration::from_secs(30))?;
    }
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
fn predecessor(mut args: impl Iterator<Item = std::ffi::OsString>) -> Option<u32> {
    args.find(|arg| arg == "--after-update")?;
    let value = args.next()?;
    let value = value.to_str()?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let pid = value.parse::<u32>().ok()?;
    (pid != 0 && pid != std::process::id()).then_some(pid)
}

/// Removes stale downloads and reports an earlier helper failure once, on
/// the next launch. This also releases Windows' previous executable image.
pub fn startup(root: &Path) -> Result<(), Error> {
    #[cfg(target_os = "windows")]
    windows::cleanup();
    clean_stale(root)?;
    let error = root.join("last-install-error");
    match std::fs::read_to_string(&error) {
        Ok(message) => {
            std::fs::remove_file(error)?;
            Err(Error::Worker(message.trim().to_owned()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests;
