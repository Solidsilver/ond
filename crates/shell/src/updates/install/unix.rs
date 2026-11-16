//! The helper outlives the app, waits for exit and rolls back a failed move.

use super::{Error, Plan, Prepared};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

pub(super) const SCRIPT: &str = include_str!("swap.sh");

pub(super) fn install(prepared: Prepared, launch: &str) -> Result<(), Error> {
    let Prepared { stage, replacement, plan, neighbor } = prepared;
    let target = match &plan {
        Plan::MacBundle(bundle) => bundle,
        Plan::LinuxLayout { executable, .. } | Plan::Binary(executable) => executable,
    };
    let neighbor = neighbor.ok_or(Error::Archive)?;
    let (mode, prefix) = match &plan {
        Plan::LinuxLayout { prefix, .. } => ("layout", prefix.as_os_str()),
        _ => ("swap", std::ffi::OsStr::new("")),
    };
    let marker = stage.path().join("installing");
    std::fs::write(&marker, [])?;
    let spawned = Command::new("/bin/sh")
        .arg("-c")
        .arg(SCRIPT)
        .arg("update-helper")
        .arg(std::process::id().to_string())
        .arg(target)
        .arg(&replacement)
        .arg(neighbor.path().join("previous"))
        .arg(stage.path())
        .arg(neighbor.path())
        .arg(mode)
        .arg(prefix)
        .arg(launch)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    match spawned {
        Ok(_) => {
            // The helper now owns cleanup, including rollback on failure.
            stage.keep();
            let _ = neighbor.keep();
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}
