# 021 · Offline: tell the user when the state file was set aside and cleanup is off

- Area: `crates/offline`, `crates/app/src/app/settings_page.rs`, `offline_page.rs` · Kind: enhancement · Priority: P3
- Status: done (branch fix/followups) · Claimed by: —
- Source: follow-up from ticket 001's resolution.

## Problem

Since ticket 001, an `offline.json` that can't be read or parsed (or is missing beside downloads) is moved aside as `offline.json.bad-<ms>`, and while any such file is in the downloads folder the store never removes stray files (`load_pins` / `has_set_aside` in `crates/offline/src/lib.rs`). That's deliberately conservative, but nothing in the UI says so. The only way to turn cleanup back on is to delete the file by hand, and the user doesn't know the downloads they had are no longer listed.

## Fix

- Expose it: e.g. `Snapshot::set_aside: Option<PathBuf>` (or a count of unknown files and their bytes).
- Settings › Offline (and/or the Offline page): a short notice, "Ond couldn't read its list of downloads. N files (X GB) it no longer knows are kept in the folder." with two actions: "Show in Finder/File Manager" and "Remove them", which deletes the unknown files and the `.bad-` file(s) after a confirm, and re-enables cleanup.
- Optional: try to recover pins from the set-aside file if it parses partially (likely not worth it).

## Tests

Store opened with a bad state file reports it in the snapshot; the remove action deletes only files the pins don't name, then the `.bad-` file, and the next open prunes normally.

## Resolution

- `Snapshot::set_aside: Option<SetAside>`: the `offline.json.bad-*` paths, how many files in the folder the pins don't name and their bytes, and whether a removal is under way. When the store opens without trusting its pins it lists those files once (it already reads the folder there), so building a snapshot touches no disk. A file that's pinned again stops counting; one the deleter removes (pinned, then unpinned) leaves the list.
- `OfflineStore::remove_set_aside()` hands the work to the deleter thread (the channel now carries `Chore::Delete` or `Chore::Sweep`), so nothing runs on the UI thread. The sweep lists the folder, then under the state lock keeps only names the pins don't account for (finished or `.part`) and marks them in `State::sweeping`. A track pinned meanwhile whose file is among them isn't downloaded until the files are gone, as with `deleting`. Hidden files and the store's own `offline.json*` are never touched. The `.bad-` files go last, so if Ond quits halfway cleanup stays off and the notice comes back. `remove_strays` now shares the listing (`listing`/`strays`/`known_names`).
- UI: one notice (`offline_page::set_aside_notice`) at the top of the Offline page (above the "Nothing downloaded yet" state, which is what such a store usually shows) and of Settings › Offline: "Ond couldn't read its list of downloads", "N files (X GB) it no longer knows are kept in the downloads folder.", then "Show in Finder" (new `Action::RevealSetAside`, which opens the account's store folder rather than the downloads base) and "Remove them…". That asks inline ("Delete N files (X GB) from the downloads folder? This can't be undone.", Delete / Cancel), so it doesn't need a sheet and Settings stays open. While it runs the notice says "Removing…". With no unknown files left, the only button is "Dismiss", which deletes just the `.bad-` file. `Action::RemoveSetAside` isn't deferred while a store opens, because that store may be another account's.
- Not done: recovering pins from a set-aside file that partly parses.
- Tests (offline): `a_set_aside_state_file_shows_in_the_snapshot_with_the_files_it_keeps`, `removing_what_was_set_aside_deletes_only_unknown_files_and_turns_cleanup_back_on` (a pinned track's `.part` and `.DS_Store` stay; the next open prunes a stray), `a_track_pinned_while_unknown_files_are_removed_downloads_after`. App: `a_set_aside_list_of_downloads_is_shown_and_removed_after_asking` (kittest: the notice shows, nothing is deleted before Delete, then the file and the `.bad-` file go and the notice with them). Review PNGs: `offline-set-aside`, `offline-set-aside-confirm`, `settings-offline-set-aside`.
