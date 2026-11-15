//! Bundle validation belongs here; minisign is required even for ad-hoc apps.

use super::{Error, Plan, Prepared, stage::Stage};
use crate::updates::Releases;
use std::path::Path;
use std::process::Command;

pub(super) fn prepare(stage: Stage, plan: Plan, app: &Releases) -> Result<Prepared, Error> {
    let Plan::MacBundle(target) = &plan else { return Err(Error::Archive) };
    let unpack = stage.path().join("unpacked");
    std::fs::create_dir(&unpack)?;
    if !Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(&stage.archive).arg(&unpack).status()?.success() {
        return Err(Error::Command("ditto"));
    }
    let bundle = unpack.join(format!("{}.app", app.app));
    verify_bundle(&bundle, target)?;
    let neighbor = stage.neighbor(target.parent().ok_or(Error::Archive)?)?;
    let replacement = neighbor.path().join(format!("{}.app", app.app));
    // A cache can be on another volume. Copy before Ready, then the helper's
    // final move stays atomic and cannot leave half a bundle at the target.
    if !Command::new("/usr/bin/ditto").arg(&bundle).arg(&replacement).status()?.success() {
        return Err(Error::Command("ditto"));
    }
    verify_bundle(&replacement, target)?;
    Ok(Prepared { stage, replacement, plan, neighbor: Some(neighbor) })
}

fn identity(bundle: &Path) -> Result<(bool, Option<String>), Error> {
    let output = Command::new("/usr/bin/codesign").args(["-d", "--verbose=4"]).arg(bundle).output()?;
    if !output.status.success() {
        return Err(Error::Command("codesign"));
    }
    let text = String::from_utf8_lossy(&output.stderr);
    let adhoc = text.lines().any(|line| line == "Signature=adhoc" || line.starts_with("CodeDirectory ") && line.contains("(adhoc)"));
    let team = text.lines().find_map(|l| l.strip_prefix("TeamIdentifier=")).filter(|s| *s != "not set").map(str::to_owned);
    Ok((adhoc, team))
}

fn verify_bundle(bundle: &Path, running: &Path) -> Result<(), Error> {
    if !Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(bundle).status()?.success() {
        return Err(Error::Command("codesign"));
    }
    let (adhoc, team) = identity(running)?;
    if !adhoc && (team.is_none() || identity(bundle)?.1 != team) {
        return Err(Error::SigningTeam);
    }
    Ok(())
}

pub(super) fn install(prepared: Prepared) -> Result<(), Error> {
    super::unix::install(prepared, "macos")
}
