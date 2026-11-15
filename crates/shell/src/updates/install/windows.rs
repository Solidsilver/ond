//! Windows permits renaming a running executable, but not overwriting it.
//! Keep the previous image until the next launch releases its file handle.

use super::{Error, Plan, Prepared, stage::Stage};
use crate::updates::Releases;
use std::fs;
use std::process::Command;

pub(super) fn prepare(stage: Stage, plan: Plan, app: &Releases) -> Result<Prepared, Error> {
    if !app.windows {
        return Err(Error::Archive);
    }
    let mut archive = zip::ZipArchive::new(fs::File::open(&stage.archive)?).map_err(|_| Error::Archive)?;
    let mut found = None;
    let expected = format!("{}.exe", app.app);
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|_| Error::Archive)?;
        if entry.enclosed_name().is_some_and(|p| p.file_name().is_some_and(|n| n == expected.as_str())) && found.replace(i).is_some() {
            return Err(Error::Archive);
        }
    }
    let Plan::Binary(target) = &plan else { return Err(Error::Archive) };
    let neighbor = stage.neighbor(target.parent().ok_or(Error::Archive)?)?;
    let replacement = neighbor.path().join("replacement.exe");
    let mut file = fs::File::create(&replacement)?;
    std::io::copy(&mut archive.by_index(found.ok_or(Error::Archive)?).map_err(|_| Error::Archive)?, &mut file)?;
    file.sync_all()?;
    Ok(Prepared { stage, replacement, plan, neighbor: Some(neighbor) })
}

pub(super) fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        // The new process can start before the previous image exits. Bound
        // retries so startup cleanup cannot leave background work waiting forever.
        let old = exe.with_extension("exe.old");
        for _ in 0..50 {
            match fs::remove_file(&old) {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        }
    }
}

pub(super) fn install(prepared: Prepared) -> Result<(), Error> {
    let Prepared { stage: _stage, replacement, plan, neighbor: _neighbor } = prepared;
    let Plan::Binary(target) = &plan else { return Err(Error::Archive) };
    let old = target.with_extension("exe.old");
    if old.exists() {
        fs::remove_file(&old)?;
    }
    fs::rename(target, &old)?;
    if let Err(error) = fs::rename(&replacement, target) {
        fs::rename(&old, target)?;
        return Err(error.into());
    }
    if let Err(error) = Command::new(target).spawn() {
        fs::remove_file(target)?;
        fs::rename(&old, target)?;
        return Err(error.into());
    }
    Ok(())
}
