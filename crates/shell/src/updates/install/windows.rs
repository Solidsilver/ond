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

pub(super) fn install(prepared: Prepared, relaunch: bool) -> Result<(), Error> {
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
    if relaunch && let Err(error) = Command::new(target).arg("--after-update").arg(std::process::id().to_string()).spawn() {
        fs::remove_file(target)?;
        fs::rename(&old, target)?;
        return Err(error.into());
    }
    Ok(())
}

/// A process handle stays tied to this predecessor even if Windows later
/// reuses its PID. Request only synchronization access and never wait forever.
pub(super) fn wait_for_predecessor(pid: u32, timeout: std::time::Duration) -> std::io::Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};

    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if raw.is_null() {
        let error = std::io::Error::last_os_error();
        // The predecessor may have exited before the new image reached main.
        if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            return Ok(());
        }
        return Err(error);
    }
    // OpenProcess returned a non-null, owned handle; RAII keeps it open across
    // the wait and closes it on every result path.
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let milliseconds = timeout.as_millis().min(30_000) as u32;
    match unsafe { WaitForSingleObject(process.as_raw_handle(), milliseconds) } {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the previous app is still running")),
        _ => Err(std::io::Error::last_os_error()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::time::Duration;

    #[test]
    fn waiting_is_bounded_and_succeeds_once_the_predecessor_exits() {
        let mut child = Command::new("cmd").args(["/C", "more >NUL"]).stdin(Stdio::piped()).spawn().unwrap();
        let error = wait_for_predecessor(child.id(), Duration::ZERO).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        drop(child.stdin.take());
        child.wait().unwrap();
        wait_for_predecessor(child.id(), Duration::ZERO).unwrap();
    }
}
