//! The shipped install.sh refreshes desktop integration; loose binaries use
//! a same-filesystem rename. Both happen only after the helper observes exit.

use super::{Error, Fallback, Plan, Prepared, stage::Stage};
use crate::updates::Releases;
use std::process::Command;

pub(super) fn check_writable(plan: &Plan) -> Result<(), Fallback> {
    if let Plan::LinuxLayout { prefix, .. } = plan {
        // Only this app's integration directories matter. A writable prefix
        // can still contain bin/share directories owned by root.
        let mut paths = vec![prefix.join("bin")];
        for suffix in ["share", "share/applications", "share/metainfo", "share/icons", "share/icons/hicolor"] {
            paths.push(prefix.join(suffix));
        }
        let icons = prefix.join("share/icons/hicolor");
        if icons.is_dir() {
            for entry in std::fs::read_dir(icons).map_err(|_| Fallback::ReadOnly)? {
                let entry = entry.map_err(|_| Fallback::ReadOnly)?;
                if entry.file_type().map_err(|_| Fallback::ReadOnly)?.is_dir() {
                    paths.push(entry.path());
                    paths.push(entry.path().join("apps"));
                }
            }
        }
        for path in paths.into_iter().filter(|p| p.is_dir()) {
            let _probe = tempfile::Builder::new().prefix(".update-write-").tempfile_in(path).map_err(|_| Fallback::ReadOnly)?;
        }
    }
    Ok(())
}

pub(super) fn prepare(stage: Stage, plan: Plan, app: &Releases) -> Result<Prepared, Error> {
    let unpack = stage.path().join("unpacked");
    std::fs::create_dir(&unpack)?;
    if !Command::new("tar").arg("-xzf").arg(&stage.archive).arg("-C").arg(&unpack).status()?.success() {
        return Err(Error::Command("tar"));
    }
    let name = stage.archive.file_name().and_then(|s| s.to_str()).and_then(|s| s.strip_suffix(".tar.gz")).ok_or(Error::Archive)?;
    let folder = unpack.join(name);
    let binary = folder.join(app.app.to_lowercase());
    if !binary.is_file() {
        return Err(Error::Archive);
    }
    let executable = match &plan {
        Plan::LinuxLayout { executable, .. } | Plan::Binary(executable) => executable,
        _ => return Err(Error::Archive),
    };
    let neighbor = stage.neighbor(executable.parent().ok_or(Error::Archive)?)?;
    let replacement = if matches!(plan, Plan::LinuxLayout { .. }) {
        if !folder.join("install.sh").is_file() {
            return Err(Error::Archive);
        }
        folder
    } else {
        let replacement = neighbor.path().join("replacement");
        std::fs::copy(binary, &replacement)?;
        replacement
    };
    Ok(Prepared { stage, replacement, plan, neighbor: Some(neighbor) })
}

pub(super) fn install(prepared: Prepared) -> Result<(), Error> {
    super::unix::install(prepared, "binary")
}
