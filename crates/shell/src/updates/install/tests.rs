use super::*;
use crate::updates::{OND, REDE, UpdateCheck, Updates};
use std::fs;
use testkit::{RangeServer, Serve, wait_until};

const BODY: &[u8] = include_bytes!("../../../testdata/updates/archive");
const SIGNATURE: &str = include_str!("../../../testdata/updates/archive.minisig");
const KEY_FILE: &str = include_str!("../../../testdata/updates/test.pub");

fn key() -> &'static str {
    KEY_FILE.lines().nth(1).unwrap()
}

fn version() -> Version {
    Version::parse("1.2.3-beta.4").unwrap()
}

#[test]
fn only_the_signed_file_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("archive");
    fs::write(&file, BODY).unwrap();
    verify(&file, SIGNATURE, key()).unwrap();
    assert!(matches!(verify(&file, SIGNATURE, PUBLIC_KEY), Err(Error::Signature)));
    fs::write(&file, b"a different release, signed by the same key").unwrap();
    assert!(matches!(verify(&file, SIGNATURE, key()), Err(Error::Signature)));
    fs::write(&file, BODY).unwrap();
    let bad = SIGNATURE.replace("trusted comment:", "broken comment:");
    assert!(verify(&file, &bad, key()).is_err());
    assert!(verify(&file, "unsigned", key()).is_err());
    // Decode the production key too: the test fixture never replaces it.
    minisign_verify::PublicKey::from_base64(PUBLIC_KEY).unwrap();
}

#[test]
fn asset_names_match_each_apps_published_platforms_and_architectures() {
    for app in [&OND, &REDE] {
        for arch in ["aarch64", "x86_64"] {
            assert_eq!(asset_name(app, &version(), Platform::Macos, arch).unwrap(), format!("{}-1.2.3-beta.4-macos-universal.zip", app.app));
            assert_eq!(asset_name(app, &version(), Platform::Linux, arch).unwrap(), format!("{}-1.2.3-beta.4-linux-{arch}.tar.gz", app.app));
        }
        assert!(asset_name(app, &version(), Platform::Linux, "riscv64").is_none());
        assert!(asset_name(app, &version(), Platform::Windows, "aarch64").is_none());
    }
    assert_eq!(asset_name(&OND, &version(), Platform::Windows, "x86_64").unwrap(), "Ond-1.2.3-beta.4-windows-x86_64.zip");
    assert!(asset_name(&REDE, &version(), Platform::Windows, "x86_64").is_none());
}

#[test]
fn linux_layout_detection_requires_the_binary_and_its_own_desktop_entry() {
    let dir = tempfile::tempdir().unwrap();
    let prefix = dir.path();
    let ond = prefix.join("bin/ond");
    let desktop = prefix.join("share/applications/dev.solidsilver.ond.desktop");
    assert_eq!(linux_prefix(&ond, "ond", |p| p == desktop), Some(prefix.to_owned()));
    assert_eq!(linux_prefix(&ond, "ond", |_| false), None);
    assert_eq!(linux_prefix(&prefix.join("bin/rede"), "ond", |_| true), None);
    assert_eq!(linux_prefix(&prefix.join("ond"), "ond", |_| true), None);
    assert_eq!(linux_prefix(&prefix.join("sbin/ond"), "ond", |_| true), None);
}

#[test]
fn translocation_detection_uses_a_whole_path_component() {
    assert!(translocated(Path::new("/private/var/folders/ab/AppTranslocation/random/d/Ond.app/Contents/MacOS/Ond")));
    assert!(!translocated(Path::new("/Applications/Ond.app/Contents/MacOS/Ond")));
    assert!(!translocated(Path::new("/Users/me/AppTranslocation-backup/Ond.app")));
}

#[test]
fn the_planner_selects_bundles_layouts_and_loose_binaries_or_a_typed_fallback() {
    let mac = Path::new("/Applications/Ond.app/Contents/MacOS/Ond");
    assert_eq!(plan(Platform::Macos, mac, "ond", true, |_| false), Ok(Plan::MacBundle("/Applications/Ond.app".into())));
    assert_eq!(plan(Platform::Macos, mac, "ond", false, |_| false), Err(Fallback::ReadOnly));
    assert_eq!(plan(Platform::Macos, Path::new("/tmp/Ond"), "ond", true, |_| false), Err(Fallback::UnknownLayout));
    assert_eq!(plan(Platform::Macos, Path::new("/Volumes/Ond/Ond.app/Contents/MacOS/Ond"), "ond", true, |_| false), Err(Fallback::DiskImage));
    assert_eq!(plan(Platform::Macos, Path::new("/tmp/AppTranslocation/id/d/Ond.app/Contents/MacOS/Ond"), "ond", false, |_| false), Err(Fallback::Translocated));
    let linux = Path::new("/home/me/.local/bin/ond");
    assert_eq!(plan(Platform::Linux, linux, "ond", true, |_| true), Ok(Plan::LinuxLayout { executable: linux.into(), prefix: "/home/me/.local".into() }));
    assert_eq!(plan(Platform::Linux, linux, "ond", true, |_| false), Ok(Plan::Binary(linux.into())));
    for platform in [Platform::Linux, Platform::Windows] {
        let exe = Path::new("/tmp/app");
        assert_eq!(plan(platform, exe, "ond", true, |_| false), Ok(Plan::Binary(exe.into())));
        assert_eq!(plan(platform, exe, "ond", false, |_| false), Err(Fallback::ReadOnly));
    }
}

async fn fetch(root: &Path, server: &RangeServer, signature: &RangeServer) -> Result<stage::Stage, Error> {
    stage::fetch(&reqwest::Client::new(), root, "update.zip", &server.url(), &signature.url(), key(), |_, _| {}).await
}

#[tokio::test]
async fn verified_downloads_are_renamed_and_skipping_deletes_the_stage() {
    let dir = tempfile::tempdir().unwrap();
    let body = RangeServer::start(BODY, Serve::fast());
    let sig = RangeServer::start(SIGNATURE.as_bytes(), Serve::fast());
    let stage = fetch(dir.path(), &body, &sig).await.unwrap();
    let path = stage.path().to_owned();
    assert_eq!(fs::read(&stage.archive).unwrap(), BODY);
    assert!(!path.join("download.part").exists());
    clean_stale(dir.path()).unwrap();
    assert!(path.exists(), "a live stage is locked against another instance's sweep");
    let mut state = Updates {
        install: State::Ready {
            version: version(),
            prepared: Prepared { replacement: stage.archive.clone(), plan: Plan::Binary(dir.path().join("target")), stage, neighbor: None },
        },
        ..Default::default()
    };
    state.stage_update(None, UpdateCheck::Install, &tokio::runtime::Handle::current(), &OND, dir.path().into(), |_| {});
    assert!(!path.exists());
    assert!(!state.install.ready());
    assert!(state.install_on_quit().is_ok());
    assert!(!dir.path().join("target").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_partial_download_never_becomes_installable_and_is_deleted_on_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let server = RangeServer::start(BODY, Serve { gate_after: Some(4), ..Serve::fast() });
    let sig = RangeServer::start(SIGNATURE.as_bytes(), Serve::fast());
    let root = dir.path().to_owned();
    let url = server.url();
    let signature_url = sig.url();
    let job = tokio::spawn(async move { stage::fetch(&reqwest::Client::new(), &root, "update.zip", &url, &signature_url, key(), |_, _| {}).await });
    wait_until("a partial download", || fs::read_dir(dir.path()).unwrap().filter_map(Result::ok).any(|e| e.path().join("download.part").is_file()));
    let stage = fs::read_dir(dir.path()).unwrap().filter_map(Result::ok).find(|e| e.file_type().unwrap().is_dir()).unwrap().path();
    assert!(!stage.join("update.zip").exists());
    job.abort();
    let _ = job.await;
    wait_until("the cancelled stage to be deleted", || !stage.exists());
    server.open_gate();
}

#[tokio::test]
async fn truncated_downloads_and_wrong_signatures_delete_every_stage() {
    let dir = tempfile::tempdir().unwrap();
    let good_sig = RangeServer::start(SIGNATURE.as_bytes(), Serve::fast());
    let truncated = RangeServer::start(BODY, Serve { cut: Some(4), ..Serve::fast() });
    assert!(fetch(dir.path(), &truncated, &good_sig).await.is_err());
    let wrong_body = RangeServer::start(b"a different signed file".to_vec(), Serve::fast());
    assert!(matches!(fetch(dir.path(), &wrong_body, &good_sig).await, Err(Error::Signature)));
    let body = RangeServer::start(BODY, Serve::fast());
    let no_sig = RangeServer::start(b"not signed".to_vec(), Serve::fast());
    assert!(matches!(fetch(dir.path(), &body, &no_sig).await, Err(Error::Signature)));
    assert!(!fs::read_dir(dir.path()).unwrap().filter_map(Result::ok).any(|e| e.file_type().unwrap().is_dir()));
}

#[test]
fn stale_stages_are_removed_without_deleting_unrelated_cache_files() {
    let dir = tempfile::tempdir().unwrap();
    let stale = dir.path().join("stage-interrupted");
    fs::create_dir(&stale).unwrap();
    fs::write(stale.join("download.part"), b"partial").unwrap();
    fs::write(dir.path().join("unrelated"), b"keep").unwrap();
    clean_stale(dir.path()).unwrap();
    assert!(!stale.exists());
    assert!(dir.path().join("unrelated").exists());
}

#[tokio::test]
async fn stale_stages_remove_only_the_replacement_copy_they_own() {
    let cache = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let body = RangeServer::start(BODY, Serve::fast());
    let signature = RangeServer::start(SIGNATURE.as_bytes(), Serve::fast());
    let stage = fetch(cache.path(), &body, &signature).await.unwrap();
    let neighbor = stage.neighbor(destination.path()).unwrap();
    fs::write(neighbor.path().join("replacement"), b"new").unwrap();
    let stage_path = stage.keep();
    let neighbor_path = neighbor.keep();
    let unrelated = destination.path().join(".update-unrelated");
    fs::create_dir(&unrelated).unwrap();
    clean_stale(cache.path()).unwrap();
    assert!(!stage_path.exists());
    assert!(!neighbor_path.exists());
    assert!(unrelated.exists());
}

#[tokio::test]
async fn a_failed_rollback_keeps_its_backup_for_manual_recovery() {
    let cache = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let body = RangeServer::start(BODY, Serve::fast());
    let signature = RangeServer::start(SIGNATURE.as_bytes(), Serve::fast());
    let stage = fetch(cache.path(), &body, &signature).await.unwrap();
    let neighbor = stage.neighbor(destination.path()).unwrap();
    fs::write(neighbor.path().join("previous"), b"old").unwrap();
    fs::write(stage.path().join("recovery"), []).unwrap();
    let stage_path = stage.keep();
    let neighbor_path = neighbor.keep();
    clean_stale(cache.path()).unwrap();
    assert!(stage_path.exists());
    assert_eq!(fs::read(neighbor_path.join("previous")).unwrap(), b"old");
}

#[test]
fn stale_cleanup_preserves_a_replacement_with_another_owner() {
    let cache = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let stage = cache.path().join("stage-interrupted");
    let neighbor = destination.path().join(".update-unrelated");
    fs::create_dir(&stage).unwrap();
    fs::create_dir(&neighbor).unwrap();
    fs::write(stage.join("neighbor.json"), serde_json::to_vec(&neighbor).unwrap()).unwrap();
    fs::write(neighbor.join("stage-origin.json"), serde_json::to_vec(&cache.path().join("stage-other")).unwrap()).unwrap();
    fs::write(neighbor.join("precious"), b"keep").unwrap();
    assert!(clean_stale(cache.path()).is_err());
    assert_eq!(fs::read(neighbor.join("precious")).unwrap(), b"keep");
    assert!(stage.exists());
}

#[test]
fn late_results_cannot_restore_a_cancelled_or_retried_download() {
    let mut state = Updates::default();
    state.cancel_install();
    state.install_event(Event { generation: 0, kind: EventKind::NotifyOnly(Fallback::ReadOnly) });
    assert!(matches!(state.install, State::Idle));
    state.install = State::Downloading { version: version(), received: 0, total: None };
    state.install_event(Event { generation: 1, kind: EventKind::NotifyOnly(Fallback::ReadOnly) });
    assert!(matches!(state.install, State::NotifyOnly { reason: Fallback::ReadOnly, .. }));
    state.cancel_install();
    state.install_event(Event { generation: 1, kind: EventKind::Finished(Err(Error::Signature)) });
    assert!(matches!(state.install, State::Idle));
}

#[cfg(unix)]
fn swap_fixture(fail: bool, bundle: bool) {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("app with 'quotes' and $spaces");
    let replacement = dir.path().join("new");
    let neighbor = dir.path().join("neighbor");
    let stage = dir.path().join("stage-fixture");
    fs::create_dir(&neighbor).unwrap();
    fs::create_dir(&stage).unwrap();
    if bundle {
        fs::create_dir(&target).unwrap();
        fs::write(target.join("binary"), b"old").unwrap();
        if !fail {
            fs::create_dir(&replacement).unwrap();
            fs::write(replacement.join("binary"), b"new").unwrap();
        }
    } else {
        fs::write(&target, b"old").unwrap();
        if !fail {
            fs::write(&replacement, b"new").unwrap();
        }
    }
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(super::unix::SCRIPT)
        .arg("update-test")
        .arg("0")
        .arg(&target)
        .arg(&replacement)
        .arg(neighbor.join("previous"))
        .arg(&stage)
        .arg(&neighbor)
        .arg("swap")
        .arg("")
        .arg("none")
        .status()
        .unwrap();
    assert_eq!(status.success(), !fail);
    let installed = if bundle { target.join("binary") } else { target };
    assert_eq!(fs::read(installed).unwrap(), if fail { b"old" } else { b"new" });
    assert!(!stage.exists());
    assert!(!neighbor.exists());
    assert_eq!(dir.path().join("last-install-error").exists(), fail);
}

#[test]
#[cfg(unix)]
fn the_helper_swaps_a_bundle_and_a_loose_binary_end_to_end() {
    swap_fixture(false, true);
    swap_fixture(false, false);
}

#[test]
#[cfg(unix)]
fn a_failed_move_restores_the_old_bundle_and_binary() {
    swap_fixture(true, true);
    swap_fixture(true, false);
}

#[test]
#[cfg(unix)]
fn the_helper_waits_for_exit_before_touching_the_installed_bundle() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("app");
    let replacement = dir.path().join("new");
    let stage = dir.path().join("stage-fixture");
    let neighbor = dir.path().join("neighbor");
    fs::write(&target, b"old").unwrap();
    fs::write(&replacement, b"new").unwrap();
    fs::create_dir(&stage).unwrap();
    fs::create_dir(&neighbor).unwrap();
    let mut app = Command::new("/bin/sh").args(["-c", "read answer"]).stdin(std::process::Stdio::piped()).spawn().unwrap();
    let started = dir.path().join("started");
    let script = format!("touch \"${{10}}\"\n{}", super::unix::SCRIPT);
    let mut helper = Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("update-test")
        .arg(app.id().to_string())
        .arg(&target)
        .arg(&replacement)
        .arg(neighbor.join("previous"))
        .arg(&stage)
        .arg(&neighbor)
        .arg("swap")
        .arg("")
        .arg("none")
        .arg(&started)
        .spawn()
        .unwrap();
    wait_until("the helper to start", || started.exists());
    assert_eq!(fs::read(&target).unwrap(), b"old");
    assert!(helper.try_wait().unwrap().is_none());
    // Closing stdin lets the fake application exit without a sleep budget.
    drop(app.stdin.take());
    app.wait().unwrap();
    wait_until("the helper to finish", || helper.try_wait().unwrap().is_some());
    assert_eq!(fs::read(&target).unwrap(), b"new");
}

#[test]
#[cfg(unix)]
fn the_layout_helper_runs_the_archives_installer_and_rolls_back_a_failure() {
    use std::process::Command;
    for fail in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("prefix with spaces");
        fs::create_dir_all(prefix.join("bin")).unwrap();
        let target = prefix.join("bin/ond");
        fs::write(&target, b"old").unwrap();
        let stage = dir.path().join("stage-fixture");
        let neighbor = dir.path().join("neighbor");
        let payload = stage.join("payload");
        fs::create_dir_all(&payload).unwrap();
        fs::create_dir(&neighbor).unwrap();
        fs::write(
            payload.join("install.sh"),
            if fail { "printf broken > \"$1/bin/ond\"\nexit 1\n" } else { "printf new > \"$1/bin/ond\"\nprintf desktop > \"$1/desktop\"\n" },
        )
        .unwrap();
        let result = Command::new("/bin/sh")
            .arg("-c")
            .arg(super::unix::SCRIPT)
            .arg("update-test")
            .arg("0")
            .arg(&target)
            .arg(&payload)
            .arg(neighbor.join("previous"))
            .arg(&stage)
            .arg(&neighbor)
            .arg("layout")
            .arg(&prefix)
            .arg("none")
            .status()
            .unwrap();
        assert_eq!(result.success(), !fail);
        assert_eq!(fs::read(&target).unwrap(), if fail { b"old" } else { b"new" });
        assert_eq!(prefix.join("desktop").exists(), !fail);
        assert!(!stage.exists());
    }
}

#[test]
#[cfg(unix)]
fn the_helper_relaunches_only_for_restart_on_success_and_after_rollback() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    for relaunch in [false, true] {
        for fail in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("app with spaces");
            let replacement = dir.path().join("new");
            let neighbor = dir.path().join("neighbor");
            let stage = dir.path().join("stage-fixture");
            fs::create_dir(&neighbor).unwrap();
            fs::create_dir(&stage).unwrap();
            for (path, version) in [(&target, "old"), (&replacement, "new")] {
                if fail && path == &replacement {
                    continue;
                }
                fs::write(path, format!("#!/bin/sh\nprintf {version} > \"$0.started\"\n")).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            // Wait for any launched child before asserting that Quit launched
            // nothing; otherwise a background exec could race the assertion.
            let script = format!("trap 'result=$?; wait; exit \"$result\"' EXIT\n{}", super::unix::SCRIPT);
            let result = Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .arg("update-test")
                .arg("0")
                .arg(&target)
                .arg(&replacement)
                .arg(neighbor.join("previous"))
                .arg(&stage)
                .arg(&neighbor)
                .arg("swap")
                .arg("")
                .arg(if relaunch { "binary" } else { "none" })
                .status()
                .unwrap();
            assert_eq!(result.success(), !fail);
            let marker = PathBuf::from(format!("{}.started", target.display()));
            if relaunch {
                wait_until("the relaunched fake app", || marker.exists());
                assert_eq!(fs::read_to_string(marker).unwrap(), if fail { "old" } else { "new" });
            } else {
                assert!(!marker.exists());
            }
        }
    }
}
