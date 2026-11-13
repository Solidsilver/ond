# Sources: several libraries in one player

Status: milestone 1 is built on `feat/sources`; milestone 2's kit (form, onboarding, switcher, dots, Sources rows and filter chips) is built, with Rede holding several accounts and both apps showing a Sources tab in Settings. Milestone 3 is built except Ond's own favourites and playlists and watching folders for changes: the `scan` crate, the local source, the `sources` list in `settings.json`, Music folder in Ond's first-run sheet, folder sources in Settings › Sources, and the views hiding what a source's capabilities rule out. Ond still holds one source live at a time. Milestones 4–5 are design. Written 11 November 2026. Covers Ond, and the parts Rede shares with it. Rede's own plan for sources is in [its design](rede/design.md#sources-and-accounts); this document follows it and says where the two apps meet.

Ond plays from one Navidrome server. This design makes the server one **source** among several: a Navidrome (or other OpenSubsonic) server, a local folder of music, and later Jellyfin and Plex. Setup starts with one or more of them, and the library shows one source at a time or all of them together.

## Goals

- Local folders as a first-class source: no server, no sign-in.
- More than one source at once, each with its own state; one unreachable server doesn't stop the others.
- Switch between sources, or see them merged into one library, and move between the two without losing your place.
- Setup that asks "where's your music?" rather than "what's your server?".
- Adding Jellyfin or Plex later is a new implementation of a trait, not a change to the app.
- The onboarding, the source switcher and the merge/switch controls are drawn by `shell::kit`, so Ond and Rede look and behave alike. Rede needs the same things for its own several accounts.

## Not now

- Merging the same album from two sources into one. Version 1 shows both, each marked with its source (see [Matching albums](#matching-albums-later)).
- Writing tags or moving files in a local folder. Ond reads a folder; it doesn't manage it.
- Syncing Ond's own playlists or favourites back to a server that doesn't hold the tracks.
- Jellyfin and Plex themselves. The trait is shaped so they fit; [Later sources](#later-sources) says what each would need.

## Decisions

| | |
|---|---|
| Identity | Every source is an account with a local, permanent `AccountId` (Rede's, from the shared crate). Track, album and artist ids are qualified by it wherever they leave their source. |
| Storage | One `library.redb` per source, as Rede has one shelf per account. Rows inside it keep bare ids, so the stored structs don't change. |
| Merging | Built in memory over the per-source databases, as the search index already is. |
| Artists | Merged across sources from the start, by MusicBrainz id, else by normalised name. |
| Albums | Not merged in version 1; each copy carries its source's dot. |
| Favourites, ratings, playlists | Written to the item's own source when it can hold them; Ond keeps its own for sources that can't, and for playlists that mix sources. |
| Kinds | Navidrome/OpenSubsonic and Local folder first. Jellyfin, then Plex, later. |
| Shared UI | `shell::kit` gains a form kit, onboarding, the source switcher, source dots and source filter chips. Neither knows what kinds of source exist; the app tells it. |

## Identity

```rust
// crates/accounts (moved out of `sources`; see below)
pub struct AccountId(Uuid);

// crates/library
pub struct TrackKey { pub account: AccountId, pub id: Arc<str> }   // likewise AlbumKey, ArtistKey
```

- An account's id is made when the source is added and never changes. A server's address and a folder's path are properties of the account and can change without orphaning favourites, downloads or the queue.
- Until the Sources rows can edit an account's address (milestone 2), Ond's sign-in form decides as its library mirror always has: signing in to the same server and user as before, even after signing out, reuses the account and keeps its folder and downloads; any other sign-in makes a new account. Ond keeps one source live at a time (the first in the `sources` list). When another takes over, the previous one's folder and cached art are removed only if it's gone from the list (another server signed in to in its place); a source still configured, such as the server behind a music folder added after signing out, keeps its library on disk and is only not in use.
- Inside one source's database, ids stay bare: the database *is* the account, so qualifying every row would only make the stored format bigger and force a migration of every struct. Ids are qualified when they leave the database: in the queue, the merged views, the saved blobs that span sources, the stream cache and the offline store.
- On disk, names follow Rede: `<account>/<id>` passed through `streamcache::escape_id`, one flat file name per key.
- The `library` crate stops depending on `subsonic`. The `From<subsonic::…>` conversions in `model.rs` and the `upsert_*` methods that take Subsonic types move to the Subsonic source, which hands the library `Track`, `Album` and `Artist` built its own way. This is the same move `streamcache` already made with `ReplyCheck`.

### Where things live

```
<data>/Sources/<account>/library.redb     the source's mirror, as library.redb is today
<data>/Sources/<account>/analysis.redb    analysis records for its tracks
<data>/ond.redb                           what spans sources: the queue, Ond's playlists and favourites, pins
<data>/Offline/<escaped account/id>.<ext> downloads, as today but account-qualified
<cache>/streams/…                          likewise
<cache>/art/<account>/…                    covers per source
```

Removing a source deletes its folder, its downloads and its cached art, after asking. For a folder source that's Ond's copy (the library mirror, the scan index, analyses, cached art), never the music files, and the question says so. Ond offers Remove… for folder sources; a server still leaves by signing out.

## The shared crate

`AccountId`, `ItemKey` and the shared source models live in the small `accounts` crate, shared by both apps and independent of books or music:

- `AccountId` and `ItemKey`.
- `SourceState`: `Online`, `Connecting`, `Syncing(Option<f32>)` (an optional `0.0..=1.0` completion fraction, `None` when the total isn't known), `Scanning`, `Offline(OfflineReason)` (`Unreachable` or `ByChoice`), `SignedOut` and `FolderMissing`.
- `Scope`: what a view is showing, stored as `{ "all": bool, "only": [ids] }` (`all: true` with an empty `only` shows every source, `all: true` with non-empty `only` is narrowed by filter chips, and `all: false` with one id is `Only(id)`). Ond and Rede both store it in their prefs and show it the same way.
- `SourceColor(u8)`: an index into a fixed palette of eight (whose light and dark RGB values live in `shell::theme`), handed out by `next_color` (the lowest unused index, or the least-used once all eight are taken) and kept when another source is removed. The colour is the account's, saved with it.

Folder scanning is the other shared piece, since Rede's design also has local folders: a crate, `scan`, that walks folders, keeps each file's size and modification time, watches for changes (the `notify` crate), finds a folder's cover image, and reads tags (the `lofty` crate). What a file *means* (a track of an album, a part of a book) stays in each app's source.

## The music source

Ond gets its own trait. Rede's `Source` is built around books (play sessions, progress, chapters), and forcing music through it would bend both.

```rust
#[async_trait]
pub trait MusicSource: Send + Sync {
    fn capabilities(&self) -> Capabilities;

    /// What changed since `since`, or everything when it's `None`, written
    /// into the source's library as it goes. Navidrome pages through search3
    /// and the album list, as sync.rs does now; a folder rescans what changed.
    async fn sync(&self, into: &Library, since: Option<Stamp>, progress: &dyn Fn(SyncProgress)) -> Result<Stamp>;

    /// What the engine plays: a stream URL with its cache key, or a file.
    fn track_source(&self, track: &Track, transcode: Option<&Transcode>) -> TrackSource;
    async fn cover(&self, id: &str, size: u32) -> Result<Option<Bytes>>;
    async fn lyrics(&self, track: &Track) -> Result<Option<Lyrics>>;

    // Each of these only when its capability says so.
    async fn set_starred(&self, kind: StarKind, id: &str, on: bool) -> Result<()>;
    async fn set_rating(&self, id: &str, rating: u8) -> Result<()>;
    async fn playlists(&self) -> Result<Vec<Playlist>>;
    async fn edit_playlist(&self, id: &str, edit: PlaylistEdit) -> Result<()>;
    async fn scrobble(&self, id: &str, at: SystemTime, submission: bool) -> Result<()>;
}

pub struct Capabilities {
    pub stars: bool,
    pub ratings: bool,
    pub playlists: bool,
    pub lyrics: bool,
    pub scrobbling: bool,
    pub transcoding: bool,
    /// Tracks stream from elsewhere, so downloading them means something.
    pub downloads: bool,
    /// The address can be unreachable: probes, Work Offline and "offline" apply.
    pub network: bool,
}
```

| | Navidrome | Local folder |
|---|---|---|
| stars, ratings, playlists | yes, on the server | no; Ond keeps its own (not built yet: hidden for now) |
| lyrics | server | `.lrc` beside the file, then embedded |
| scrobbling | through the server | no at first; later direct to ListenBrainz and Last.fm |
| transcoding, downloads, network | yes | no |

As in Rede, the app never asks which kind of source it has. It reads capabilities and hides what a source can't do: a local album has no download button, and Work Offline doesn't touch it.

`bridge.rs` holds one `Arc<dyn MusicSource>` per account instead of an `Option<Client>`. Session scoping stays, per account: removing a source, or signing one out, drops results from before it, and doesn't touch the others.

## The local source

- **What it reads:** one or more folders per source, rescanned at start and on Rescan by size and modification time (watching them while Ond runs, with `notify`, is still to come). Files symphonia can't decode are skipped and counted, and Settings says how many. A folder that's gone (a drive unplugged) keeps its albums: the source shows Folder missing, a track that can't be opened fails with a plain message and is skipped, and a rescan once it's back clears it.
- **Albums:** grouped by album artist (else the artist) and album title, with disc numbers kept; a folder holding tracks of one album with mixed artists is a compilation. MusicBrainz ids are used when the tags have them. Sort tags are honoured.
- **Ids:** a track's id is a hash of its path relative to the folder, so moving the folder (a new disk, a new mount point) keeps every id. Renaming a file changes its id; a rescan that sees a file vanish and one with the same MusicBrainz recording id, or the same tags and length, appear in one pass treats it as a move and carries favourites and playlist entries across.
- **Covers:** embedded art first, then `cover`, `folder` or `front` images in the album's folder.
- **ReplayGain:** read from tags, as the Navidrome source passes it through today.
- **Where its stars and playlists go:** Ond's own store (below).

Playback opens `file://` paths, which the engine already handles without the stream cache.

## What Ond keeps itself

`ond.redb` holds what doesn't belong to one source (it arrives with the first of these that's built; see "Moving an existing install"):

- **The queue**, as `TrackKey`s. A source that's removed takes its tracks out of the queue; one that's offline leaves them in, shown as unplayable today.
- **Ond's playlists:** playlists that may mix sources. A server's own playlists stay on the server and show under it. Creating a playlist from tracks of one Navidrome source creates it on that server, as now; adding a track from another source to it offers to make an Ond playlist instead. Ond playlists export to M3U.
- **Favourites and ratings** for sources that can't hold them. A Navidrome track's star still goes to Navidrome.
- **Pins**, account-qualified.

## Merged and single views

The sidebar's switcher sets the `Scope`: one source, or All sources.

- **One source** is today's Ond: that source's albums, artists, songs, playlists and favourites.
- **All sources** builds the library in memory from each source's database: albums and songs side by side, each with its source's dot once there's more than one source. Memos key on each source's own versions together, so a sync of one doesn't rebuild the rest.
- **Artists** are merged in both views that show more than one source: an artist with the same MusicBrainz id, or the same normalised name (case, diacritics, a leading "The", "&" and "and" folded), is one artist with albums from each source. Two different artists with one name, both without ids, merge wrongly; the artist page then shows each source's albums under its dot, so it's visible.
- **Filter chips** on the library's pages narrow All sources to some of them.
- **Search** covers whatever the scope covers.
- With one source, nothing in the UI mentions sources at all: no switcher, no dots, no chips.

Moving between scopes keeps the page you're on when it exists in the new one (an artist page stays an artist page); otherwise it goes to the same place (Albums, Songs) at the top.

### Matching albums (later)

The same album on two sources shows twice in version 1. Merging them needs answers this design leaves open: which copy plays (the local one, the better format, the one that's online), whose star counts, and what happens when the track lists differ. The likely shape is a match on the MusicBrainz release id, falling back to album artist, title and track count, with a preference order the user sets per source. It's left until people have used version 1.

## Shared UI

These go into `shell::kit`, used by both apps from the start. Each takes plain descriptions from the app and never learns what Navidrome or Audiobookshelf is.

```rust
pub struct SourceInfo<'a> {
    pub id: AccountId,
    pub name: &'a str,          // "Home server", "Music folder"
    pub kind: &'a str,          // "Navidrome", "Local folder", "Audiobookshelf"
    pub icon: Icon,
    pub color: SourceColor,
    pub state: &'a SourceState,
    pub detail: Option<&'a str>, // "Synced 2 min ago", "12 840 songs"
}

pub struct SourceKind<'a> {
    pub name: &'a str,
    pub icon: Icon,
    pub blurb: &'a str,          // "Your own server, at home or away."
    pub available: bool,         // false shows it as coming later
}
```

- **`kit::form`**: the sign-in form's pieces, moved out of Rede's `session.rs` (its helpers) and Ond's `login_ui` (drawn inline), two separate takes on one form today: a labelled field with its hint and error, the primary and busy buttons, the "or" divider, and the notice. Both apps' sign-in forms become a few calls into it, and every source's settings form (a server's address, a folder list) is drawn with it.
- **`kit::onboarding`**: the first-run sheet. A welcome line the app gives ("Where's your music?", "Where are your books?"), then the kinds as tiles; picking one shows that kind's form, which the app draws with `kit::form`. With only one kind (Rede, until it reads local folders) there are no tiles: the sheet opens on that kind's form. Where the app offers **Add another**, a source that's added joins a list above the tiles ("Home server, Navidrome ✓") with Add another and **Done**; where it doesn't (Ond, which holds one source live until milestone 4), signing in goes straight to the library. Ond's kinds are Navidrome and **Music folder** (Jellyfin and Plex shown as coming later); Music folder's form is `kit::folder_list` with Choose folder…, an optional name and Add, and stays up with how many tracks the first scan has found until it's done, with a way to carry on without waiting. Until a source is ready the window shows nothing else. The list also holds the accounts already there, so signing out of one while another is signed in leads back to it. The same sheet opens from **Add source…** later, starting on the tiles, with **Cancel** back to the library.
- **`kit::source_switcher`**: the sidebar's foot, replacing the account row each app draws now. It shows the scope: the one source's name and state, or "All sources" with its sources' dots stacked. Its menu lists each source (dot, name, state, a check on the one in scope), then All sources, then **Add source…** and **Manage sources…**, then the app's own rows (Settings, shortcuts). A source that needs attention (signed out, folder missing) shows its state in place of the detail in both the row and the menu. In the folded rail it's the stacked dots alone.
- **`kit::source_dot`**: the small mark on covers and rows, in the source's colour, drawn only when the app says there's more than one source.
- **`kit::source_chips`**: the filter chips above a library page: All sources, then a chip per source (dot and name), lit as the `Scope`'s chips are. They hand back what was clicked (`SourceChip::All` or `Toggle(id)`) for the app to apply with `clear_chips` or `toggle_chip`, and draw nothing with one source or the switcher on one. Built and snapshotted in the kit; neither app draws them until All sources (milestone 4).
- **`kit::source_row`**: a source in Settings' Sources tab: dot, name, kind, state (in place of the detail when the source needs attention, as in the switcher), detail, and a menu: Rename (in place, in the row), a colour picker over the eight `SOURCE_COLORS`, the app's sync word (Sync now, Rescan), Sign out… and Remove…. The app says which entries apply and gets the choice back as a `SourceRowEvent`; asking first and doing it are the app's. Remove asks and says what goes with it (Rede: its downloads and its library on this computer, and listening not yet sent; Ond, once it can remove: its downloads, Ond's playlists from it, favourites kept only by Ond). A locked source (managed) shows the app's lock words and offers no Rename or Remove. Rede lists every account with Add source…, then the account in view's address, who's signed in and how, and its last sync with Sync now (this replaced its Account tab); Ond lists every configured source, the one in use first and the others as not in use; a folder source's sync word is Rescan, its line is its folders (and the files its last scan skipped), it offers Remove… and no Sign out…, and while it's in use its folders are listed under the rows to add to or take from (which rescans). The switcher's Manage sources… opens the tab in both.

Snapshots: each piece gets a scene in both apps' snapshot sets, with one source, three sources, and one of them in trouble.

## Settings and Home Manager

`settings.json` gets a list:

```json
"sources": [
  { "id": "6f1c…", "kind": "subsonic", "name": "Home server", "color": 0, "server": "https://music.example.com", "username": "me" },
  { "id": "a03e…", "kind": "local", "name": "Music folder", "color": 1, "folders": ["/Users/me/Music"] }
],
"scope": { "all": true, "only": ["6f1c…", "a03e…"] }
```

- An old file with `server` and `username` and no `sources` reads as one Subsonic source with a new id, which is then saved. `kind` falls back leniently, as enum settings do.
- The keyring entry is named by the account id, as Rede's is. The old entry is read once and moved.
- `managed.json` keeps `server` and `username`, which mean "a Navidrome source, managed", so existing Home Manager configs work. A new `sources` key may list sources with their kind; managed sources can't be removed or edited, and their rows show the lock. `ALLOWED`, the `locked()` arms and `docs/home-manager.md` gain these.

## Moving an existing install

The first start of the new version, before anything else opens the library:

1. Make an account for the configured server, and write it to `settings.json`.
2. Move `library.redb`, `analysis.redb` and `scrobbles.json` (the scrobbles waiting to be sent) into `Sources/<account>/`. A move, not a copy: one rename each, on the same disk.
3. Rewrite the offline store's state with account-qualified names and rename the files, under the state file's atomic write. Stray deletion stays off until this has finished, since a half-moved store would otherwise look stray.
4. Leave the stream cache: its entries are evictable and the old names simply stop being asked for. Cover art likewise.

Each step is recorded when done, so a start cut short picks up at the next. A test builds an install in the previous release's layout and checks that downloads, favourites, saved playlists, pending scrobbles and the library all survive.

`ond.redb` isn't part of the move. Nothing saved today spans sources: the queue isn't saved, pins live in the offline store's state, and the starred list and playlists' saved copies are the server's own, so they stay in its library. `ond.redb` arrives with the first thing that does span sources: Ond's own playlists, or favourites for a source that can't hold them.

The `sources` list in `settings.json` arrives with the second kind of source (milestone 3). Until then the one account is the `account` key beside `server` and `username`, and that list will read it as its first entry.

## Milestones

1. **Accounts, with Navidrome as the only source.** `AccountId`, the per-source folder, `TrackKey` through the queue, player, offline store and stream cache, and per-account cover art; `library` freed of `subsonic`; `MusicSource` with the Subsonic implementation; the migration above. Done when Ond's snapshots come out the same as before and an existing install opens with everything intact.
2. **The shared kit.** `kit::form`, onboarding, the switcher, dots, chips and Sources rows, in both apps. Rede now holds and switches between multiple Audiobookshelf accounts via `kit::source_switcher` (one active account at a time via `Scope::only(id)`, assigning a `SourceColor` per account, reusing an existing `(server, username)` on re-sign-in, keeping signed-out rows in the list, and flushing pending progress on switch; merged "All sources" views belong to milestone 4). Done when both apps sign in through `kit::onboarding` and both sidebars draw `kit::source_switcher`.
3. **Local folders.** The `scan` crate and the local source, Ond's own favourites and playlists. Done when a first run can choose a folder instead of a server and play from it with no network.
4. **All sources.** The merged views, merged artists, filter chips, scope-aware search, Ond playlists that mix sources. Done when a Navidrome source and a local one show as one library, and switching scope keeps your place.
5. **Later:** album matching; direct scrobbling for local tracks; Jellyfin; Plex.

## Later sources

- **Jellyfin:** a username and password for an access token; music libraries through `/Items` with paging, favourites and playlists on the server, `/Audio/{id}/universal` for streaming and transcoding, lyrics on recent servers. Fits the trait as it stands.
- **Plex:** sign-in goes through plex.tv (a PIN or browser flow), then to one of the account's servers, which may have several addresses (local, remote, relay). Ratings are on a 10-point scale; there are no stars as such. Fits the trait, but its sign-in is a form of its own, as Rede's OpenID is.

## Open questions

- Should a local folder that holds both music and audiobooks be offered to both apps, or should each ignore what isn't its own (by folder, by genre tag, by file type)?
- Is Work Offline still one switch for every networked source, or does each source get its own? One switch is simpler and matches "I'm on a plane"; per source suits "my home server is down".
- When a track is starred in All sources and its source can't hold stars, Ond keeps the star. If that source later gains the capability (a server upgrade), are Ond's stars sent to it?
- How much of the merged view does Mini and the Player show? Probably the playing track's source dot, and nothing else.
