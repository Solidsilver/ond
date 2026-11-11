# Rede: design

Rede is a native audiobook player for [Audiobookshelf](https://www.audiobookshelf.org) and audiobook folders on disk, written in Rust with egui. It's the second app in the Ond family: it shares Ond's playback engine, caching, offline downloads and platform code, and has its own interface built for listening to books instead of albums.

The name: in German *Rede* is a speech or a talk, and in old English to *rede* is to tell, or to give counsel. It reads like "read". It fits podcasts too, which come later.

Status: [first beta, for macOS and Linux](README.md). Local folders, several accounts and podcasts below are plans, not beta features. Written November 2026. Claims about Ond's code, the Audiobookshelf server (checked against its source at 2.37.2) and the crates named here were verified on 7 November 2026; corrections are folded in below.

## Goals

- A fast, native desktop client. The existing ones are mostly web views (Tauri, Electron) or Flutter; the native ones are Mac-only.
- Built for books: resume where you left off, chapters, speed, sleep timer, bookmarks, time left.
- Good offline support: downloaded books play with no server, and progress made offline syncs when the server is back.
- Plays local audiobook folders too, with no server at all.
- Several sources at once (more than one ABS server, local folders, later podcasts), shown together in one library, with a quiet indication of where each book comes from.
- Clear speech when listening around the house: voice boost and silence removal early on, and AirPlay speakers.
- macOS first, then Linux and Windows, as Ond did.

## Non-goals

- Mobile. The ABS mobile apps are good; Rede is for the desktop.
- Ebooks and read-along (EPUB with synced audio).
- Managing files: Rede reads local folders in place. Editing metadata and chapters, importing and organising belong to ABS.
- Bit-perfect playback. Rede processes the signal anyway (speed, voice boost, silence removal).
- A music mode. Music stays in Ond.

## Decisions so far

| Question | Decision |
|---|---|
| One app with modes, or a family of apps | A family: Ond for music, Rede for spoken word, maybe video much later. Shared core, shared general look, each app tuned to its use. |
| Books and podcasts | Same app (Rede), browsed separately: a top-level switch between Books and Podcasts, with an optional combined view. Podcasts come after books. |
| Repository | Same repo and Cargo workspace as Ond. A separate core repo only when something outside the family uses it. |
| First source | Audiobookshelf, 2.26 or newer (the JWT login with refresh tokens). Recommend 2.36+, which fixes CVE-2026-73085. |
| Accounts | Usually one per server, but several accounts can be signed in, mainly to share progress (see [Listening with others](#listening-with-others)). |
| Local files | Read in place. |
| Podcasts | Later, as sources: ABS podcast libraries, and RSS feeds Rede subscribes to itself. Designed for now. |
| Platform | macOS first. |
| Network speakers | AirPlay through the system, as Ond does now. Chromecast and older Sonos later. |
| Early audio features | Speed with pitch kept, voice boost, silence removal. |
| Where playback controls live | In the title bar, as Ond's, and nothing else there: the transport at the left; a capsule centred on the window that widens with it (0.47 of it, 330 to 1000 pt) holding the cover (a click opens the Player), the book, the chapter with the book's time left, the chapter scrubber, and the speed and sleep chips stacked at its right end where Ond's heart sits; the chapters column's toggle at the right end. Chapters, bookmarks and info sit in Ond's right-hand panel slot. The account, the places and the shortcuts are the sidebar's; Mini is ⇧⌘M and a button in the Player. A bottom bar cost too much height on every screen; a bar holding the account and the mode buttons too was crowded and lopsided. |
| Sidebar | Ond's: the places under the libraries' name (Library now; Series, Authors, Collections and Downloads join it), the account at the foot with its state (connected, syncing, connecting, offline) and menu, and the shortcuts beside it. Folds to a rail of icons (⌥⌘S, remembered), and a window too narrow for it shows the rail. The account's libraries share one shelf, so the heading names them; switching between them comes with per-library browsing. |
| Sign-in | Password and OpenID. OpenID uses ABS's app flow with PKCE, redirecting to `rede://oauth`, falling back to `audiobookshelf://oauth` (the only URI a server allows by default). Done before M3, since most accounts use it. |

## The family and the repository

### Layout

Today the workspace is `crates/*`, with Ond's app in `crates/app` (package `ond-player`). With a second app, apps and shared libraries go in separate folders:

```text
apps/
  ond/            Ond (moved from crates/app; package stays ond-player)
  rede/           Rede
crates/
  audio/          playback engine (shared)
  streamcache/    progressive cache for streamed files (shared)
  offline/        offline download store (shared)
  shell/          window, chrome, theme, widgets, palette, settings,
                  keyring, updates, now-playing integrations (shared; new)
  analysis/       Ond: loudness, DR, upsampling checks
  subsonic/       Ond: OpenSubsonic client
  library/        Ond: music library db
  abs/            Rede: Audiobookshelf client (new)
  sources/        Rede: source and account model, progress log (new)
  shelf/          Rede: book library db (new; name open)
docs/
  rede/           this document, and Rede's own docs later
```

Moving `crates/app` touches `nix/package.nix`, the three bundle scripts and the release workflow's comment; they find the app by package name (`-p ond-player`), so the move itself is small. It's one commit with no behaviour change.

### Extracting shared code

The widgets both apps draw with (Ond's buttons, segmented pill, sliders, switch, settings and info rows, popovers, sheets, the shortcuts sheet and toasts, and the chips and steppers Rede added) are in `shell::kit`, and a new widget either app could use goes there first. Other shared code (the window, settings, the artwork cache) still moves out of Ond's app crate when Rede needs it. Ond's snapshot tests (`apps/ond/src/app/snapshots.rs`) guard it while things move. Likely early candidates, in the order Rede needs them: theme and widgets, window and chrome (`window.rs`, `chrome.rs`, `csd.rs`), settings and keyring, managed settings, the artwork cache, toasts, the command palette and fuzzy search, now-playing integrations, the update check.

The shared crates stay free of anything specific to one app, so a third app (podcasts split out, or video) is possible later. The engine is audio only; a video app would use its own (libmpv) and share the rest.

### Releases

- Tags get an app prefix: `ond-v0.5.0`, `rede-v0.1.0`. The release workflow triggers on both patterns and builds only the tagged app. Ond's existing `v*` tags stay as they are; the update check accepts both forms for Ond.
- **The update check has to change.** `updates.rs` reads where GitHub's `/releases/latest` redirects to (the `Location` header, without following it), and with two apps in one repo "latest" can be the other app's release. Each app instead looks for its own newest release by tag prefix. The redirect trick doesn't do that, so this means either the releases Atom feed (`/releases.atom`, no API key, no rate limit) filtered by prefix, or a small per-app manifest file published with each release. The Atom feed is simpler.
- Release notes go in `.github/release-notes/<app>/`.
- The Nix flake exposes a `rede` package next to Ond's, and the Home Manager module gets a Rede section.
- The README becomes a short index of the apps, with a page for each.
- The repo could be renamed to a family name later; GitHub redirects renamed repos, but check `nix run github:Solidsilver/ond` keeps working.

## Architecture

```text
             ┌──────────────── Rede (apps/rede) ────────────────┐
             │ views: home, library, book, player, chapters,    │
             │ bookmarks, downloads, sources, settings          │
             │ book timeline · sleep timer · listening sessions │
             └──────┬──────────────┬──────────────────┬─────────┘
                    │              │                  │
             ┌──────▼─────┐  ┌─────▼──────┐   ┌───────▼────────┐
             │  sources   │  │   shelf    │   │     shell      │
             │ accounts,  │  │ book db,   │   │ window, theme, │
             │ trait,     │  │ one file   │   │ settings, now  │
             │ progress   │  │ per account│   │ playing, ...   │
             │ log        │  └────────────┘   └────────────────┘
             └──┬───┬───┬─┘
         ┌──────┘   │   └───────┐
     ┌───▼──┐  ┌────▼───┐  ┌────▼────┐
     │ abs  │  │ local  │  │ rss     │ (later; also jellyfin,
     └──────┘  │ folders│  │ (later) │  subsonic bookmarks)
               └────────┘  └─────────┘
     ┌───────────────── shared with Ond ─────────────────┐
     │ audio (engine) · streamcache · offline            │
     └───────────────────────────────────────────────────┘
```

## Sources and accounts

### Identity

Every book, file, chapter and progress record is identified by the account it came from as well as its own id:

```rust
pub struct AccountId(Uuid);                  // assigned when the account is added
pub struct ItemKey { account: AccountId, item: String }
```

- An account's id is local and permanent. The server URL is a property of the account and can change (new domain, local vs remote address) without orphaning progress or downloads.
- Two servers can both have an item `abc`; with the account in the key they never collide. This is the part that's expensive to retrofit, so it's in from the start, even with one account.
- On-disk names include the account: the stream cache and offline store name files `<account>/<file id>.<suffix>`. (Ond keys these by bare server id today; Rede passes the account-qualified id through `CacheKey`, which takes any string.)

### Accounts

An account is one source with its credentials:

| Kind | Sign-in | Notes |
|---|---|---|
| Audiobookshelf | URL, browser OpenID or username/password | Access and refresh tokens in the keyring. |
| Local folders | none | A list of folders. |
| RSS (later) | none | The subscriptions. |
| Jellyfin, Subsonic (maybe later) | as their servers want | Subsonic's bookmark endpoints exist for keeping audiobook positions. |

Each account has its own state shown in the UI: online, offline (server unreachable, playing from downloads), signed out (refresh token expired), syncing. One unreachable server doesn't stop the others.

An account can be marked **listener only**: it receives progress (see [Listening with others](#listening-with-others)) but its libraries aren't shown.

### The source trait

```rust
#[async_trait]
pub trait Source {
    fn capabilities(&self) -> Capabilities;
    async fn libraries(&self) -> Result<Vec<Library>>;
    async fn items(&self, lib: &LibraryId, since: Option<Stamp>) -> Result<Changes<ItemSummary>>;
    async fn item(&self, id: &str) -> Result<Item>;              // tracks, chapters, full metadata
    async fn cover(&self, id: &str, size: Option<u32>) -> Result<Bytes>;
    async fn open(&self, id: &str, device: &DeviceInfo) -> Result<PlaySession>;
    async fn report(&self, s: &PlaySession, at: Position, listened: Duration) -> Result<()>;
    async fn close(&self, s: &PlaySession, at: Position) -> Result<()>;
    async fn progress(&self) -> Result<Vec<Progress>>;           // everything, for reconciling
    async fn set_progress(&self, batch: &[ProgressUpdate]) -> Result<()>;
    // optional, per capabilities: bookmarks, search, subscribe, live updates
}

pub struct Capabilities {
    pub server_search: bool,
    pub bookmarks: bool,
    pub live_updates: bool,      // ABS socket.io
    pub subscriptions: bool,     // RSS, ABS podcast libraries
    pub transcoding: bool,
}
```

The app never checks which kind of source it has; it checks capabilities and hides what a source can't do. Adding Jellyfin later is a new implementation, not a change to the app.

### Libraries have a kind

ABS libraries are either book or podcast libraries. Every library in Rede carries a kind (`Books`, later `Podcasts`), which decides which side of the app shows it. Local folders are book libraries; RSS feeds are podcast libraries.

### Audiobookshelf

OpenID sign-in uses ABS's PKCE mobile flow in the system browser, alongside
password sign-in when `/status` offers it. Rede first tries `rede://oauth`, then
`audiobookshelf://oauth` only on the server's exact invalid-redirect rejection.
The [verified protocol](abs-api.md#18-native-openid-sign-in-verified-mobile-flow)
records the cookie/session exchange. On macOS the app bundle registers both
schemes and an Apple Event receiver handles running and newly launched apps.
Linux `.desktop` `x-scheme-handler` entries and a Windows registry protocol
handler come later; there is no URL handler registration for those platforms yet.


What the `abs` crate talks to. The official API docs are marked as no longer maintained, so everything here gets checked against the server source before it's relied on. The checked request and response shapes are in [abs-api.md](abs-api.md).

- **Login:** `POST /login` with `username` and `password` and the header `x-return-tokens: true`, which returns `user.accessToken` and `user.refreshToken` in the body (without the header the refresh token is only set as a cookie).
- **Refresh:** `POST /auth/refresh` with `x-refresh-token`. The server rotates the refresh token on every refresh, so the stored copy is replaced each time, atomically. Two refreshes racing would spend the same token twice: one refresh in flight per account, others wait on it.
- **Lifetimes** (defaults since 2.31; 2.26 to 2.30 used 12 hours and 7 days): access token 1 hour, refresh token 30 days, set by the server's `ACCESS_TOKEN_EXPIRY` and `REFRESH_TOKEN_EXPIRY`. A Rede that isn't opened for longer than the refresh lifetime has to sign in again; the UI says so plainly instead of failing quietly. After a rotation the old refresh token keeps working for 10 minutes and returns the current pair, which softens a crash between receiving and storing a new token.
- **Rate limit:** login, refresh and password changes share one limiter, 40 requests per 10 minutes per IP, successful ones included. Refreshing only when a token is near expiry or rejected, never per request, keeps well inside it.
- **The old `user.token`** (never expires) still comes back in the login response. Rede ignores it and never stores or logs it.
- **Logout:** `POST /logout` with `x-refresh-token`.
- **Requests** carry `Authorization: Bearer <access token>`. Every `/api` route also takes `?token=` instead, which is what the stream cache and offline store use for files: they take a URL and nothing else. A session's `contentUrl` is a relative `/api/items/:id/file/:ino` without a token; Rede adds the server address and the token. Covers need no auth. Expiry while streaming: a 401 on a file fetch triggers a refresh and a retry with the new URL.
- **Libraries and items:** `GET /api/libraries` (`{libraries: [...]}`, `mediaType` `book` or `podcast`), `GET /api/libraries/:id/items` (paged with `limit` and `page`; always minified, the `minified` flag is ignored; pass `sort=addedAt`, as with no sort the order is undefined and pages can overlap; `limit` and `page` come back as strings), `GET /api/items/:id?expanded=1` for one book with its `audioFiles`, `chapters` and `tracks`.
- **Playing:** `POST /api/items/:id/play` with `deviceInfo` (client name `Rede`, version, a stable device id), `supportedMimeTypes` (what symphonia decodes) and `forceDirectPlay: true`. The session it returns has `audioTracks` (index, `startOffset`, duration, `contentUrl`, mime type), the chapters and the resume position (`currentTime`). Rede plays the files directly; ABS's HLS transcoding isn't needed. Without `supportedMimeTypes` and `forceDirectPlay` the server transcodes to HLS, so both are always sent. Opening a session closes any other open session for the same user and device id. Sessions live in the server's memory: after a server restart a sync answers 404, and Rede opens a new session.
- **While playing:** sync the session every 15 seconds and on pause, seek and stop (`/api/session/:id/sync` with the current time and the time listened since the last sync), and close it at the end (`/api/session/:id/close`).
- **Progress:** `/api/me/progress` endpoints, including a batch update, for writes that aren't part of a live session (offline listening, marking finished). A `PATCH` overwrites without comparing times (`lastUpdate` only sets the stored stamp), so last-write-wins is Rede's job. Two server rules to respect: `isFinished: false` on a finished item resets its position to 0 and ignores any `currentTime` sent with it, and a position within 10 seconds of the end marks the item finished.
- **Offline listening:** ABS has endpoints for syncing a "local session", the listening a client did offline. Rede uses them so offline listening counts in ABS's listening stats too. The server skips a local session whose `updatedAt` is older than the progress it holds, so an old offline session can't rewind newer listening from another device; a session without `updatedAt` always overwrites, so Rede always sends it. When it does write, the server keeps the client's timestamp, so a skewed clock decides who wins.
- **Live updates** (later): socket.io events when progress changes on another device (`user_item_progress_updated`; the client authenticates by emitting `auth` with its access token). Until then, Rede pulls progress when it starts, when the window gains focus, and before playing a book.

Paths for sessions, progress and local sessions, confirmed in the server source: `/api/session/:id/sync`, `/api/session/:id/close`, `/api/session/local`, `/api/session/local-all`, `PATCH /api/me/progress/:itemId`, `PATCH /api/me/progress/batch/update`, `GET /api/me/items-in-progress`, `GET /api/items/:id/cover`, and bookmarks under `/api/me/item/:id/bookmark`.

### Local folders

- A book is a folder of audio files, or a single file (`.m4b`, `.m4a`, `.mp3`, `.opus`, `.flac`). Nested folders: the deepest folder that holds audio files is the book, so `Author/Series/01 Title/` works.
- Metadata from tags: album or title → title, album artist or artist → author, composer → narrator (the common convention), plus series and sequence from the tags ABS also reads. Folder names fill gaps. Symphonia reads the tags. Ond reads only ReplayGain tags from files (everything else comes from the server), so this is new code.
- Chapters: embedded `.m4b` chapters (Nero `chpl` and QuickTime chapter tracks, via `mp4ameta` 0.13, which reads both; `chpl` timescales differ between writers), ID3 `CHAP` frames in MP3, FLAC cuesheets and Matroska chapters (symphonia 0.6 reads these itself; it has no MP4 chapter support), otherwise one chapter per file.
- Covers: embedded art, else `cover.jpg`/`folder.jpg` in the folder.
- Ids: a hash of the path relative to the library folder, so moving the whole library keeps progress.
- Rescans on start and on demand; watching the folders for changes comes later.
- Progress lives only in Rede's database, via the progress log below.

### Podcasts (later)

- ABS podcast libraries: episodes are items with their own progress; ABS handles downloading episodes on the server.
- RSS: a source that stores subscriptions, polls feeds, and downloads episodes into the offline store. An episode's id is its feed URL plus its `<guid>`.
- Inbox, auto-download, auto-delete when finished, and per-show speed belong to the Podcasts side; the engine, progress log and offline store are the same as for books.

## Progress

### The progress log

All progress goes through one local, append-only log per account, in that account's database:

```rust
pub struct ProgressUpdate {
    pub item: ItemKey,
    pub position: f64,           // seconds into the book
    pub finished: bool,
    pub at: SystemTime,          // when it happened on this device
    pub listened: Duration,      // listening time since the previous entry
    pub synced: bool,
}
```

- For ABS while online, entries are written and sent together; offline, they wait and are sent in order when the server is back (as a local session where possible, else as a progress batch).
- For local and RSS sources the log is the progress; nothing is sent.
- So offline sync, local books and podcasts later all use the same piece.

### Reconciling

When Rede starts, regains focus, opens a book or resumes a paused book, its
listening worker replays pending progress, then pulls the server's positions.
Focus pulls are independent of the five-minute library-listing throttle. Opens,
reports, bookmark sync and reconciliation share that worker, so a pull cannot
race another worker's blind progress PATCH.

- The newer timestamp wins; equal timestamps choose the server. Accepted server
  positions are saved as snapshots, without inventing local listening or pending
  progress entries. A newer local position is reported back when needed.
- If the server is newer and differs by more than a minute in either direction,
  Rede saves both positions in `Reconciled` state. Its message is: "Picked up where
  you left off on another device, 1h 12m in." The toast's Undo action restores the
  saved local position. This only happens for the book loaded in the player and
  only when a local position exists to restore; books started elsewhere are
  accepted silently.
- Exactly a minute or less updates the position without the toast. A paused
  player's engine moves with the accepted snapshot and remains paused. Resuming
  waits at most 2.5 seconds for that pull before restarting from the saved local
  position, so an unreachable or lossy server cannot hold the play press.
- While the same book is actively playing on this device, local wins regardless
  of the phone's timestamp, and is reported back. Moving running audio beneath
  its listener would interrupt this device's session. The app also refuses to
  move audio if it resumed while a paused pull was in flight.
- Undo is a new durable progress entry and a server write, with a timestamp after
  both the accepted phone stamp and now. This prevents the old phone position
  returning on the next pull, even if the phone clock was ahead. Restoring an
  unfinished position over a finished ABS row clears `isFinished` first, then
  writes the position separately: ABS drops a position sent with the reset. The
  reset carries a `lastUpdate` one millisecond before the position, so a failed
  position write is replayed rather than mistaken for already-synced.
  The server can still auto-finish a restored position within its end threshold.
- Pending entries replay before comparison, oldest log first, coalesced to the
  newest position per item. ABS PATCH is a blind overwrite, so each replay checks
  server timestamps first and discards older local entries. Reports from local
  play sessions first send timestamped ABS local sessions for statistics and
  acknowledge those entries before any position PATCH. An accepted session
  whose progress was skipped never falls through to a blind PATCH.

### Finishing, starting over, and un-finishing

Sometimes progress is settled by hand rather than by listening: the book is
finished, it turns out not to be, or it starts again from the beginning. The
three are `Action::MarkFinished`, `Action::MarkUnfinished` and
`Action::StartOver`, each taking the book's id; the listening worker is started
on first use, so none of them needs the book open. Each is one durable entry in
the same progress log, stamped after both now and the server's row as Rede last
saw it, exactly as Undo is: a pull that races the write cannot bring the old
state back, a write that fails replays in order with the rest, and the app
shows the new state in `Positions` at once (bumping its version, so anything
derived from it is worked out again).

What each writes to ABS, given the rules in
[§12](abs-api.md#12-progress-endpoints-claim-partly-patch-is-a-plain-overwrite-not-last-write-wins):

- **Finished** keeps the place the book got to and sends `isFinished: true` with
  it, in one write. A finished book opens at the start, so the place costs
  nothing, and marking it unfinished again can return there.
- **Unfinished** keeps the last known place, or the start when there is none.
  ABS drops the position in the request that clears a finish, so this is two
  writes: the reset (stamped a millisecond earlier, so a failed follow-up is
  replayed rather than mistaken for synced), then the place.
- **Start over** is a place of 0 that is not finished, and takes the same two
  writes when the server had the book finished. The loaded book moves to the
  start through the player, which reports it through the session, and stays
  paused or playing as it was.

Marking the loaded book finished stops it first: stopping reports and closes the
session at the old place, and a session sync after the finish would write that
place again, which ABS treats as un-finishing the book.

Two server behaviours stay outside Rede's control. ABS finishes a place within
ten seconds of a book's end again, even on a plain write, so un-finishing a book
left that close to its end may come back finished on the next pull. And a device
whose clock is ahead can still hold the newer stamp.

### Bookmarks

Bookmarks belong to the account's shelf and use `(item, exact book time)` as their
server identity. The source model carries item, position, title and creation time.
Title formatting may round a display time; the position itself always retains
all f64 bits, including through postcard and the ABS delete URL. Renaming changes
only the title; moving a bookmark is delete followed by create.

Local create, rename and delete commit the optimistic row and an ordered journal
entry in one transaction. They survive offline use and restart. The listening
worker replays them on startup, focus, full library sync, book open/resume and a
retry that runs only while an edit is still queued, backing off from 30 seconds
when the server is unreachable. A pull installs the
authoritative server list and overlays edits queued during the request; it asks
the server again after a write, and once when there was nothing to write. Snapshot
revisions prevent a delayed UI message from erasing a newer optimistic edit. The
UI reads a memo keyed by bookmark generation and ItemKey, sorted by book time;
drawing never reads the database.

ABS has no bookmark revision timestamp. The simple conflict rule compares the
server title (or absence) with the title seen when an edit was queued. If they
differ, the server wins and the queued edit is acknowledged without overwriting
it. A missing renamed bookmark stays deleted. A row already at the desired title,
or an already absent deletion, also acknowledges the edit: this makes retrying a
lost HTTP response idempotent. A 4xx refusal other than 408 or 429 is final:
the edit is acknowledged, shown once, and the remaining edits continue; only a
timeout, rate limit, 5xx or network failure stays queued for retry, with a
backoff. Creation time is not compared, because the server assigns it when an
offline create is uploaded. Edits that delete and recreate the same title cannot
be distinguished; ABS supplies no compare-and-swap, so phone writes between the
comparison and mutation can still race. A final pull keeps whatever the server
ended with.

Schema v2 adds a frozen bookmark-edit journal without changing any v1 row bytes.
Existing v1 bookmarks become pending creates during migration, so upgrading does
not discard bookmarks saved before server sync existed.

### Listening with others

When you play a book, you choose where progress goes:

- **Me** (the default).
- **Me and others:** any of the other signed-in accounts on the same server. Every update goes to each listener's account.
- **Nobody:** progress isn't saved for this session (listening along with someone else's book, or sampling).

Details:

- A *listening session* has a set of listeners. The progress log entries are written for each of them.
- If the listeners are at different places in the book, Rede asks where to start: "Yours (2h 10m) or Sam's (1h 55m)?"
- Listener-only accounts receive progress without adding their libraries to yours.
- On the same server, the book has the same item id for every account, so this needs no matching. Across servers it would need the same-book matching that's deferred (see [Open questions](#open-questions)), so version 1 offers it for accounts on the same server only.
- The listeners are shown in the player while a session has more than one, so it's never a surprise where progress went.

## The library database (`shelf`)

One redb file per account, like Ond's `library` crate (redb with postcard-encoded records, migrations by record version). Removing an account deletes its file and its downloads. The merged view across accounts is built in memory, as Ond's search index already is.

Records:

- **Item:** title, subtitle, authors, narrators, series (name and sequence, a book can be in several), genres, tags, published year, publisher, description, language, ASIN and ISBN, duration, cover id, added and updated time, the library and its kind.
- **Tracks:** per item, the files with their start offset, duration, suffix and size (for offline downloads and the stream cache).
- **Chapters:** start, end, title.
- **Progress log** and the current position per item.
- **Bookmarks:** item, position, title, created, and the ordered journal of edits that haven't reached the server yet (schema v2; a v1 file's bookmarks become queued creates on upgrade).
- **Sync state:** per library, where the listing got to and the newest `updatedAt` seen.

Sync for ABS: every run pages through each library's minified item list (ABS has no "changed since" filter) and compares `updatedAt`; that is cheap even for large libraries. Full item details (tracks, chapters) are fetched when a book is opened or downloaded, and kept.

## Playback

### The book timeline

A book is one timeline made of one or more files. Rede keeps the mapping between book time and (file, offset in file), using the tracks' `startOffset`:

- Playing a book plays its files as a queue. Ond's engine already plays queues gaplessly, prefetches the next file, and reports when the output reaches it, which is what a book split into many files needs.
- Seeking to a book position: if it's in the current file, `Seek`; otherwise play the right file from the right offset.
- Chapters are ranges on the book timeline. The current chapter, the time left in it and in the book are derived from the book position.

### Engine changes (`crates/audio`)

**1. Play from a position.** The engine can already start a session from a position internally (`Engine::start(src, start_at)`, private, used for rate changes, device switches and seeks the decoder can't serve; the seek is applied before any audio). `Command::Play` takes only a `TrackSource` today; it gets an optional start position so resuming doesn't play a moment of the beginning before seeking.

**2. Book time separate from output time.** The engine's position today is output frames counted by the realtime callback (`frames_played`), divided by the output rate. It is per track: the callback zeroes it at each gapless marker, and the decoder overwrites it where a seek lands. That holds while one output second is one second of the file. With speed and silence removal it doesn't: at 1.5×, one second of output is 1.5 seconds of book, and removed silence is book time that's never output.

The change: the decoder records, alongside the samples it pushes, which part of the source they came from, as segments of (ring sample where it starts, source frame where it starts, ratio). The ring already carries `Marker`s at sample positions for track boundaries; segments are the same idea. The callback publishes the source position of the frame being played, computed from the current segment. Seeks, speed changes and cut silences each start a new segment. Progress, chapters, the scrubber and the sleep timer all read the source position and stay exact.

This is the most important engine change: everything else builds on it. Ond keeps working unchanged (one segment per track at ratio 1).

**3. Speed with pitch kept.** A time-stretch stage in the decode thread (`Ctx::queue`, after the channel map and before the resampler), behind a small trait. The algorithm is a speech one: Sonic (`libsonic`, a pure-Rust port, Apache-2.0) or WSOLA (`wsola`, MIT). [`signalsmith-stretch`](https://lib.rs/crates/signalsmith-stretch) was the first idea, but it needs libclang to build (bindgen) on every platform and Nix, and its authors say it sounds best between 0.75× and 1.5×; a phase vocoder smears speech at 2–3×. Bypassed at 1.0×. Range 0.5× to 3.0× in 0.05 steps. Places that assume a fixed source-to-output ratio change with it: `Track::to_output`, marker placement, the `total_frames` conversion and the position written after a seek.

A speed change has to be heard at once, but the ring holds about 2 seconds already stretched at the old speed. So a speed change works like a seek: flush the ring and refill from the current source position at the new speed, as a seek does. A short fade at the switch may be needed to avoid a click.

**4. Silence removal.** Implemented in M3; tuned parameters and source-time accounting are in [Playing books](playback.md#speech-processing). Before the stretch: measure level in 10 ms windows; a run below a threshold (about −45 dBFS, absolute until a cheap book loudness measurement is available) for longer than about 400 ms is shortened to about 200 ms, with short fades at the cut. Removed time is a new segment, so positions stay correct. The total time saved is shown, as podcast apps do. Off by default; one switch, with a strength setting perhaps later.

**5. Voice boost.** Implemented in M3, with per-book loudness levelling deferred. After the stretch: a gentle compressor (slow attack, so consonants stay crisp), a few dB of presence boost around 2–4 kHz, makeup gain and a limiter. One switch. Loudness from the `ebur128` measurement Ond's `analysis` crate does could set a per-book target level, so quiet and loud recordings come out alike.

**6. Mono** (small): a downmix option, for one earbud or one speaker.

Order of the stages: decode → silence removal → time stretch → voice boost → (resample to the device) → volume.

**What Rede doesn't use:** bit-perfect mode, ReplayGain (voice boost and loudness levelling replace it), matching the device rate. The engine keeps them for Ond; Rede runs it in Normal mode.

### Output

- AirPlay speakers are system output devices on macOS and already appear in the engine's device list (`DeviceKind::AirPlay`). AirPlay's latency (about 2 seconds) is fine for books; the position shown follows the audio that's heard, as the engine reports it.
- The output picker from Ond carries over, without its bit-perfect parts.
- Chromecast and older Sonos later, as separate outputs that are sent a URL instead of samples. They'd bypass the engine's processing, so speed would need the receiver's own support. Worth its own design when it comes.

### Caching and downloads

- **Streaming:** `streamcache`'s range-request seeking suits a 500 MB `.m4b` well: seeking to 10 hours in starts at once. Its default 2 GB cap is a few books; Rede keeps that cap but evicts by book, never the book that's playing.
- **Prefetch:** when a book is split into files, the next file is fetched as the current one nears its end. `SetNext` takes no lead time; the engine starts the next download when the current one is complete or 30 seconds remain (`PREFETCH_LEAD_SECS`).
- **Downloads:** `offline` pins whole books (later podcast episodes) instead of albums and playlists. Downloads resume after interruption and restart, are checked against the size the server reports, and hard-link files the stream cache already holds. A downloaded book shows in the library offline, plays from disk, and its progress goes to the log.
- `streamcache` checks responses for Subsonic error replies (`fetch.rs`, `lib.rs`); that check becomes a function each app passes in, so `streamcache` stops depending on `subsonic`.

## The interface

Rede borrows Ond's look (type, spacing, colours, the quiet style) and its three window sizes, tuned for books.

### Screens

- **Home:** Continue Listening (books in progress, newest first, with progress and time left), then Recently Added, Series in Progress (the next unread book of each), Recently Finished.
- **Library:** books as a grid of covers or a list, sorted by title, author, recently added, recently listened or progress; filters for in progress, not started, finished, downloaded, and by source.
- **Series:** books in order with their sequence numbers; what's next is highlighted.
- **Authors** and **Narrators:** their books.
- **Book page:** cover, title, author, narrator, series and number, length, time left at the current speed, description, and the chapter list (each with its length; the current one marked; finished ones dimmed). Play or resume, download, mark finished, the progress choice for sharing.
- **Player** (Ond's Player mode): large cover, book and chapter titles readable across the room, a chapter scrubber with the whole book shown as a thin bar under it, large skip back and forward buttons (15 and 30 seconds by default, both settable), and a last row with a bookmark button, the sleep timer, and the time left at the speed (whose popover holds the voice boost and skip silence switches). Its toasts hang near the top, over the cover, since its controls run to the foot. The output device comes later.
- **Mini** (Ond's Mini mode): cover, chapter, play/pause, skip back and forward. Stays on top.
- **Chapters panel:** takes the place of Ond's queue panel.
- **Bookmarks:** the Bookmarks tab of the column at the right, beside Chapters and Info, for the book playing. Rows as the chapters tab sets them: a bookmark glyph, the title, the book time at the right; the one at or just behind where the book is has its glyph in the accent, and hovering a row says its chapter. A click jumps there. A row's … (or a right-click) renames it in place (Enter keeps the new title, Esc the old, an empty title isn't taken) or deletes it, with Undo in the toast that says so. Add bookmark at the top does what ⌘B does: a bookmark at the current position titled with its chapter (the row shows the time beside it), confirmed by a toast ("Bookmarked Chapter 5 at 3:56:40") that offers to show the tab when it isn't in view. With none yet, the tab says "No bookmarks yet" and how to add one. Titles are the notes; there are no separate notes yet.
- **Downloads:** what's downloaded, what's in progress, space used, per source.
- **Sources:** the accounts, their state, add and remove, which are listener only.
- **Settings:** playback (skip intervals, default speed, rewind a few seconds after a long pause), sleep timer defaults, audio (voice boost, silence removal, mono), downloads, appearance, updates. Settings is Ond's Settings window, the same one from `shell::kit` (the dimmed app behind it, the grey tab column, the "esc" hint), opened with ⌘,, the gear in the sidebar's footer (and the rail's), Settings… in the account menu, or Settings… in the Playback popover, which opens it on Playback; from the Player or Mini it brings the full window back first, and Esc or a click outside closes it. It starts with three tabs: Playback (skip intervals, the rewinds after a long pause and after sleep, the timer's length, voice boost and skip silence), Account (the server, the last sync with Sync now, who's signed in and how, and Sign out…, which asks first as Ond's does) and About (Ond's About without the updates). The rest join it as tabs when they come. The speed isn't there: the one saved is the one playing, set from the speed chip's Playback popover, which holds only what you change while listening: the speed, voice boost and skip silence.

Podcasts, when they come, get their own Home (new episodes as an inbox), Shows and Episodes, behind the Books/Podcasts switch, with an optional combined Home.

### Where things come from

- With one source, nothing indicates where a book comes from.
- With more than one, each source gets a colour; a small dot of it marks books on covers and in lists, and filter chips in the library narrow to one or more sources.
- The source's name is on the book page and in Downloads, where it matters.
- The same book on two sources shows twice, each with its own dot, until matching exists.

### Playback details

- **Sleep timer:** after 5 to 120 minutes, or at the end of the current chapter. Fades out over the last 10 seconds. The engine's volume ramp is a fixed 15 ms glide, so this needs a long fade in the engine (or the app stepping the volume). Stopping saves progress at the faded-out point minus a few seconds, so you don't lose the last bit you half heard. Extend with one click from a notice before it ends.
- **Rewind on resume:** after a pause longer than a few minutes, resume a few seconds earlier (settable, off for short pauses).
- **Time left** everywhere is shown at the current speed ("4h 10m left at 1.5×").
- **Mark finished** at the end of the book, and on demand; finishing removes the download if the user chose that.

### Keyboard

Space play/pause; ← and → skip back and forward; ⇧← and ⇧→ previous and next chapter; `[` and `]` speed down and up; ⌥⌘U the chapters column (Ond's queue toggle); ⌘B add a bookmark; ⌘K search; ⌘/ the shortcuts list, as in Ond.

### System integration (macOS first)

Ond's now-playing code (`now_playing/macos.rs`) carries over with changes for books. It already registers skip forward and back (switched off, `ENABLE_SKIP = false`) and reports a rate of 1.0 or 0.0:

- Skip forward and skip backward commands with their intervals instead of next and previous track (headphone and keyboard media keys still skip).
- The playback rate in the now-playing info, so the system's progress bar moves at the right speed.
- Chapter number and count, and the chapter title as the subtitle.
- Seeking from the system controls goes to the book position.

MPRIS (Linux) and SMTC (Windows) get the same changes when those platforms come.

## Milestones

**M0: groundwork in the Ond repo.** No change for Ond users.
- Move `crates/app` to `apps/ond`.
- Per-app release tags and the update check by tag prefix.
- `streamcache` takes the error check from the app, and drops its `subsonic` dependency.
- An empty `apps/rede` that opens a window with Ond's theme, extracting the first pieces of `crates/shell`.

**M1: the engine.**
- Play from a position.
- Book time separate from output time (segments).
- Speed with pitch kept.
- Tests for positions under speed changes and seeks, as `audio/src/tests.rs` does for gapless.

**M2: Audiobookshelf, playing books.** The first usable Rede on macOS.
- Sign in, token refresh, one ABS account.
- Library sync, Home, Library, Book page, search.
- Play with resume, chapters, skip, speed, sleep timer, session sync to ABS.
- Player and Mini modes, now playing and media keys.

**M2.5: done after M2.** OpenID sign-in; the player moved to the title bar with a chapters panel; book descriptions rendered from their HTML; a sidebar with the library and the account, and the bar cleared down to the transport, the capsule (with speed and sleep inside it) and the chapters toggle.

**M3: listening.** Done.
- Voice boost and silence removal: switches in the Playback popover (the speed chip and the Player's speed pill) and on Settings › Playback, with what skipping has saved in the book under them.
- Bookmarks: the Bookmarks tab in the column at the right, ⌘B anywhere a book is loaded, and a bookmark button in the Player; a toast confirms each one.
- Reconciling with other devices (the "picked up where you left off" toast, with Undo).

**M4: offline.**
- Downloads.
- The progress log sending offline listening when back online.
- Starting with the server unreachable.

How M4 is built:

- **The store.** `offline` gains a pin for a whole book, `Pin::Book(String)` holding the account-qualified item key, with one `TrackRef` per audio file, keyed by the same id `SessionSources` gives the stream cache, so a book listened to all the way through is hard-linked rather than fetched again. `offline.json` is JSON and `Pin` is a serde enum, so the new variant needs no migration and Ond's files still read. Each Rede account has its own store (`offline::account_store`). The file URL comes from the source while it is reachable (`/api/items/:id/file/:ino?token=`), and is rebuilt after a token refresh.
- **Playing a download.** Playing needs the item's tracks and chapters, which the shelf already keeps from the last expanded fetch (`set_details`); a download first makes sure they are there. When every file of a book is downloaded, its tracks play as `file://` sources from the store, online or not. Online, the server session is still opened for statistics and progress; offline, the listening worker opens a local session instead (an id Rede makes, the shelf's item, the saved position) and the reports go to the progress log as they do now.
- **Offline listening.** Listening done without a server session is grouped by book and local playback session into ABS local sessions (`timeListening`, start and current time, `updatedAt` from the last entry) and sent with `POST /api/session/local-all` when the server is back, before the position replay. The server skips a session older than the progress it holds, so this cannot rewind newer listening from another device. A server too old for local sessions falls back to the progress batch alone. The
  session's UUID, starting position and start timestamp are saved with each
  report in frozen `ProgressEntryV2` rows (shelf schema 3, migrated from V1).
  Unsent reports alone cannot distinguish offline listening from a failed
  online report. Retries reuse that UUID; subsequent batches include already
  acknowledged listening in their cumulative totals because ABS upserts the
  session total. Successful per-session results are acknowledged individually,
  including sessions whose progress was skipped in favour of newer server
  progress.
- **Starting unreachable.** The library, Home and book pages read from the shelf, downloaded books play, and anything that needs the server (books not downloaded, search on the server, sign-in checks) says so plainly. Nothing is cleared or signed out because the server can't be reached.
- **The interface.** A Download button on the book page and in a book's context menu (with progress, pause and remove); a Downloads place in the sidebar listing what's downloaded and downloading; a Downloaded option in the library's listening filter; a mark on downloaded covers; and, while offline, books that aren't downloaded are dimmed and say "Not downloaded". Settings › Downloads has the storage limit, the folder, and "Remove downloads when finished" (off by default).

The backend exposes `DownloadBook(ItemKey)`, `RemoveDownload(ItemKey)`,
`PauseDownloads(bool)` and `RetryDownloads`. `ViewCtx.downloads` holds the store's
snapshot plus preparing/error maps; `book(key)` returns the pin summary (counts,
bytes, fraction and failure state), and an absent pin means not downloaded.
`ViewCtx.download_settings` and `PrefsAction::RemoveDownloadsWhenFinished` /
`DownloadLimit` supply the two download preferences; `RemoveAllDownloads`
(asked first through `AskRemoveDownloads`) unpins every book. The store sends scoped
`Msg::Offline` notifications, with no timer while idle. Download folders use the
full UUID's numeric value through `offline::account_store`; Ond's existing
32-bit folder names keep their spelling.


**M5: local folders.** A second source, which proves the source trait and the account-qualified ids.

**M6: several accounts.** Mixed library with source dots and filters, listener-only accounts, listening with others.

**M7: platform integration.** The first beta includes Linux x86_64/ARM64 bundles with password sign-in. Linux OpenID redirects and MPRIS/media keys, Windows integration and bundles, and Rede's Nix packaging remain to do.

**Later:** podcasts (ABS podcast libraries, then RSS), live updates over socket.io, Chromecast and Sonos, matching the same book across sources, Jellyfin.

## Open questions

- **The shelf crate's name**, and whether Rede's crates get a prefix (`rede-shelf`) to keep the family's crates apart.
- **Matching the same book across sources:** by ASIN or ISBN, else title, author and length. When it exists, which copy's progress wins? Per-source progress with the newest shown is the likely answer, but it's deferred until there's a real need.
- **Socket.io:** `rust_socketio` is stagnant (last release April 2024); its active fork `tf-rust-socketio` or a thin client over `tokio-tungstenite` are the options. Pull on focus may be enough for a long time.
- **ABS transcoding:** direct play covers what symphonia decodes. Is there anything ABS libraries commonly hold that it doesn't (old WMA)? If so, `supportedMimeTypes` leaving it out makes ABS transcode it, and Rede would play the HLS stream, which the engine doesn't do today.
- **Local session sync:** answered (an older local session is skipped; see [Audiobookshelf](#audiobookshelf)). Rede sends offline listening as local sessions, always with `updatedAt`.
- **The time-stretch library:** Sonic or WSOLA, chosen by listening at 1.5×, 2× and 3×.

## References

- Audiobookshelf API docs (unmaintained): <https://api.audiobookshelf.org/>
- ABS 2.26 authentication: <https://github.com/advplyr/audiobookshelf/discussions/4460>
- ABS API keys (for scripts, not apps): <https://www.audiobookshelf.org/guides/api-keys/>
- CVE-2026-73085 (refresh tokens accepted on API endpoints before 2.36.0, GHSA-798w-wv6c-2c3h): <https://osv.dev/vulnerability/CVE-2026-73085>
- GHSA-m5hp-x2xc-7jqq (unauthenticated path traversal, published October 2026): a reason to warn about old servers.
- Other ABS clients: <https://audiobookshelf.org/docs/documentation/community/community-apps/>; the closest to Rede is Skald (Tauri, Windows and Linux): <https://github.com/Fireb-08/Skald>
- signalsmith-stretch: <https://lib.rs/crates/signalsmith-stretch>
