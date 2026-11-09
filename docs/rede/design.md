# Rede: design

Rede is a native audiobook player for [Audiobookshelf](https://www.audiobookshelf.org) and audiobook folders on disk, written in Rust with egui. It's the second app in the Ond family: it shares Ond's playback engine, caching, offline downloads and platform code, and has its own interface built for listening to books instead of albums.

The name: in German *Rede* is a speech or a talk, and in old English to *rede* is to tell, or to give counsel. It reads like "read". It fits podcasts too, which come later.

Status: design, being built. Written November 2026. Claims about Ond's code, the Audiobookshelf server (checked against its source at 2.37.2) and the crates named here were verified on 7 November 2026; corrections are folded in below.

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

Code moves out of Ond's app crate into `crates/shell` when Rede needs it, not ahead of time: the second use shows what the shared shape really is. Ond's snapshot tests (`apps/ond/src/app/snapshots.rs`) guard it while things move. Likely early candidates, in the order Rede needs them: theme and widgets, window and chrome (`window.rs`, `chrome.rs`, `csd.rs`), settings and keyring, managed settings, the artwork cache, toasts, the command palette and fuzzy search, now-playing integrations, the update check.

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

When Rede starts, regains focus or opens a book, it fetches the server's progress for that book and compares it with the newest local entry by time:

- The newer one wins (last write wins, as ABS itself does).
- If the server is newer and differs by more than a minute (you listened on your phone), Rede moves to it and shows a toast: "Picked up where you left off on another device, 1h 12m in. Undo." Undo restores the local position. This avoids both silently rewinding you and silently ignoring the phone.
- Pending offline entries are sent before the comparison, so they aren't lost to it.

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
- **Bookmarks:** item, position, note, created.
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

**4. Silence removal.** Before the stretch: measure level in 10 ms windows; a run below a threshold (about −45 dBFS, relative to the book's loudness) for longer than about 400 ms is shortened to about 200 ms, with short fades at the cut. Removed time is a new segment, so positions stay correct. The total time saved is shown, as podcast apps do. Off by default; one switch, with a strength setting perhaps later.

**5. Voice boost.** After the stretch: a gentle compressor (slow attack, so consonants stay crisp), a few dB of presence boost around 2–4 kHz, makeup gain and a limiter. One switch. Loudness from the `ebur128` measurement Ond's `analysis` crate does could set a per-book target level, so quiet and loud recordings come out alike.

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
- **Player** (Ond's Player mode): large cover, book and chapter titles readable across the room, a chapter scrubber with the whole book shown as a thin bar under it, large skip back and forward buttons (15 and 30 seconds by default, both settable), speed, sleep timer, bookmarks, voice boost and silence removal switches, the output device.
- **Mini** (Ond's Mini mode): cover, chapter, play/pause, skip back and forward. Stays on top.
- **Chapters panel:** takes the place of Ond's queue panel.
- **Bookmarks:** per book, with notes; jump to one with a click.
- **Downloads:** what's downloaded, what's in progress, space used, per source.
- **Sources:** the accounts, their state, add and remove, which are listener only.
- **Settings:** playback (skip intervals, default speed, rewind a few seconds after a long pause), sleep timer defaults, audio (voice boost, silence removal, mono), downloads, appearance, updates.

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

**M3: listening.**
- Voice boost and silence removal.
- Bookmarks.
- Reconciling with other devices (the "picked up where you left off" toast).

**M4: offline.**
- Downloads.
- The progress log sending offline listening when back online.
- Starting with the server unreachable.

**M5: local folders.** A second source, which proves the source trait and the account-qualified ids.

**M6: several accounts.** Mixed library with source dots and filters, listener-only accounts, listening with others.

**M7: Linux and Windows.** MPRIS and SMTC changes, packaging, Nix.

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
