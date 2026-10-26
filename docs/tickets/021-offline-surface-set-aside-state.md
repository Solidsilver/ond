# 021 · Offline: tell the user when the state file was set aside and cleanup is off

- Area: `crates/offline`, `crates/app/src/app/settings_page.rs`, `offline_page.rs` · Kind: enhancement · Priority: P3
- Status: open · Claimed by: —
- Source: follow-up from ticket 001's resolution.

## Problem

Since ticket 001, an `offline.json` that can't be read or parsed (or is missing beside downloads) is moved aside as `offline.json.bad-<ms>`, and while any such file is in the downloads folder the store never removes stray files (`load_pins` / `has_set_aside` in `crates/offline/src/lib.rs`). That's deliberately conservative, but nothing in the UI says so. The only way to turn cleanup back on is to delete the file by hand, and the user doesn't know the downloads they had are no longer listed.

## Fix

- Expose it: e.g. `Snapshot::set_aside: Option<PathBuf>` (or a count of unknown files and their bytes).
- Settings › Offline (and/or the Offline page): a short notice, "Ond couldn't read its list of downloads. N files (X GB) it no longer knows are kept in the folder." with two actions: "Show in Finder/File Manager" and "Remove them", which deletes the unknown files and the `.bad-` file(s) after a confirm, and re-enables cleanup.
- Optional: try to recover pins from the set-aside file if it parses partially (likely not worth it).

## Tests

Store opened with a bad state file reports it in the snapshot; the remove action deletes only files the pins don't name, then the `.bad-` file, and the next open prunes normally.
