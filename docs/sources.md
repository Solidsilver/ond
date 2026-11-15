# Sources: several libraries in one player

Status: milestones 1–5 are built, UI included. Milestone 4 brings concurrent sources with independent sessions and watchers, account-qualified in-memory identity, persisted scope and chips, merged libraries and artists, scope-aware search and navigation, and mixed Ond playlists; in Ond's UI, the switcher with each source's own state (an unreachable server flagged, a narrowed scope named, boxes to toggle sources), source dots on covers, the queue, playlists, the album page, search and what's playing, a Source column in lists that mix sources, filter chips under the library pages, a merged artist's albums grouped by source, the Make Ond playlist offer, and each source's live state in Settings › Sources. Milestone 5 combines matching albums across sources as an option, in three stages, all built. Shipped in Ond 0.6.0. Updated 14 November 2026. Covers Ond, and the parts Rede shares with it. Rede's own plan for sources is in [its design](rede/design.md#sources-and-accounts); this document follows it and says where the two apps meet.

Ond plays from one Navidrome server. This design makes the server one **source** among several: a Navidrome (or other OpenSubsonic) server, a local folder of music, and later Jellyfin and Plex. Setup starts with one or more of them, and the library shows one source at a time or all of them together.

## Goals

- Local folders as a first-class source: no server, no sign-in.
- More than one source at once, each with its own state; one unreachable server doesn't stop the others.
- Switch between sources, or see them merged into one library, and move between the two without losing your place.
- Setup that asks "where's your music?" rather than "what's your server?".
- Adding Jellyfin or Plex later is a new implementation of a trait, not a change to the app.
- The onboarding, the source switcher and the merge/switch controls are drawn by `shell::kit`, so Ond and Rede look and behave alike. Rede needs the same things for its own several accounts.

## Not now

- Merging the same album from two sources into one. Milestone 4 shows both, each marked with its source; milestone 5 combines them as an option (see [Combined albums](#combined-albums-milestone-5)).
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
- Signing in to the same server and user reuses its account, mirror and downloads, including after signing out. Another server or user adds a separate account; it does not replace the other configured sources. The first entry in a request's settings snapshot selects its credentials and storage helpers, while the persisted `Scope` selects the library on screen. Every configured source opens independently; signing out ends only its session, and removing it deletes only Ond's copy of that account.
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
| stars, ratings, playlists | yes, on the server | no; Ond keeps its own |
| lyrics | server | `.lrc` beside the file, then embedded |
| scrobbling | through the server | no at first; later direct to ListenBrainz and Last.fm |
| transcoding, downloads, network | yes | no |

As in Rede, the app never asks which kind of source it has. It reads capabilities and hides what a source can't do: a local album has no download button, and Work Offline doesn't touch it.

`bridge.rs` holds one `Arc<dyn MusicSource>` per account instead of an `Option<Client>`. Session scoping stays, per account: removing a source, or signing one out, drops results from before it, and doesn't touch the others.

## The local source

- **What it reads:** one or more folders per source, rescanned at start and on Rescan by size and modification time and watched recursively while Ond runs with `notify` (bursts settle for two seconds before one incremental rescan). Files symphonia can't decode are skipped and counted, and Settings says how many. A folder that's gone (a drive unplugged) keeps its albums: the source shows Folder missing, a track that can't be opened fails with a plain message and is skipped, and a rescan once it's back clears it.
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

### Combined albums (milestone 5)

The same album on two sources shows twice in milestone 4. With **Combine matching albums** on (Settings › Sources, off by default, a `combine_copies` setting Home Manager may manage), views that show more than one source treat copies as one album and one song.

**Matching albums.** Two albums from different sources are one when both have a MusicBrainz release id and the ids are equal. When either lacks one, they match on their merged artist (the artist groups above) and their normalised title (the same folding as artist names). Different release ids never match, and neither do two albums from the same source. Year and track count are ignored: rips of one album often disagree on the year, and a partial copy should still join the full one. Editions stay apart when their titles differ ("Deluxe", "Live"), since the title is compared whole. A user can keep two albums apart (stage 3), and that choice outlives syncs.

**Matching songs.** Within a combined album, two songs are one when both have a MusicBrainz recording id and the ids are equal. Otherwise they match on disc and track number together with their normalised title, or on the title alone when the song has no number and only one song on the album has that title. A song only one source has stays a song of its own. The album's track list is the union of every copy's, in disc and track order.

**Identity.** Nothing new is stored as a key. A combined album or song is addressed by one member's real `ItemKey`, its *lead*: the member from the first source in settings order. `Data` keeps the copies beside it (`album_copies` and `track_copies`, from a lead key to every member key, the lead first, in source order), so routes, selections, the queue, playlists and pins all keep holding ordinary keys, and turning the switch off loses nothing. Frozen postcard rows are unchanged. Chips and a one-source scope narrow the copies first: a combined album with one copy left in scope is just that copy.

**On screen.** A combined album has one card with a dot for each source that has a copy, and its formats on its page ("FLAC · MP3", from the tracks' suffixes). A combined song has one row with a dot for each copy. The Source column lists them all. Artists count a combined album once.

**Which copy plays (stage 2).** A combined song plays its *preferred* copy: among copies whose source is reachable, by the **Prefer** setting: *Best quality* (lossless before lossy, then the higher bitrate, then source order) or *Source order* (settings order; the default). If the preferred copy fails to start because its source is unreachable or the file is gone, the player moves to the next copy of that song before skipping. This applies to anything queued from a combined view and to playlist entries whose song has copies.

**Stars, ratings and downloads (stage 3).** A combined song shows as starred when any copy is. Starring it stars every copy (on each server that holds stars, in Ond for the rest), and unstarring clears every copy. A rating is shown from the preferred copy and set on every copy. Play counts shown are the sum. Downloading a combined song pins its preferred copy, and a combined album counts as downloaded when every song has a pinned copy. **Keep apart** on a combined album's menu splits it back into its copies and stores the pair in `ond.redb` (account-qualified album keys), and **Combine again** on either copy undoes it.

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
- **`kit::source_switcher`**: the sidebar's foot, replacing the account row each app draws now. It shows the scope: the one source's name and state, or "All sources" with its sources' dots stacked. With the filter chips narrowing All sources to some of them, it names them ("Home server, Music folder", or "2 of 3 sources" where that doesn't fit) over their dots. Its menu lists each source (dot, name, state, a check on the one in scope; in an app that can show some sources together, a box instead, ticked as its chip is lit, that adds it to or takes it from the set while the row still picks it alone), then All sources, then **Add source…** and **Manage sources…**, then the app's own rows (Settings, shortcuts). A source that needs attention (signed out, folder missing) shows its state in place of the detail in both the row and the menu. A server that can't be reached is flagged too (its line in the warning colour, and a mark at the end of All sources' row), keeping the app's own words for it. In the folded rail it's the stacked dots alone.
- **`kit::source_dot`**: the small mark on covers and rows, in the source's colour, drawn only when the app says there's more than one source. Under a title it follows the line (`text_then_dots`), so the lines start together; in a list with columns it's a Source column of its own (`source_cell`).
- **`kit::source_chips`**: the filter chips above a library page: All sources, then a chip per source (dot and name), lit as the `Scope`'s chips are. They hand back what was clicked (`SourceChip::All` or `Toggle(id)`) for the app to apply with `clear_chips` or `toggle_chip`, and draw nothing with one source or the switcher on one. Built and snapshotted in the kit; neither app draws them until All sources (milestone 4).
- **`kit::source_row`**: a source in Settings' Sources tab: dot, name, kind, state (in place of the detail when the source needs attention, as in the switcher), detail, and a menu: Rename (in place, in the row), a colour picker over the eight `SOURCE_COLORS`, the app's sync word (Sync now, Rescan), Sign out… and Remove…. The app says which entries apply and gets the choice back as a `SourceRowEvent`; asking first and doing it are the app's. Remove asks and says what goes with it (Rede: its downloads and its library on this computer, and listening not yet sent; Ond, once it can remove: its downloads, Ond's playlists from it, favourites kept only by Ond). A locked source (managed) shows the app's lock words and offers no Rename or Remove. Rede lists every account with Add source…, then the account in view's address, who's signed in and how, and its last sync with Sync now (this replaced its Account tab); Ond lists every configured source with its own connection and sync state; a folder source's sync word is Rescan, its line is its folders (and the files its last scan skipped), it offers Remove… and no Sign out…, and while it's in use its folders are listed under the rows to add to or take from (which rescans). The switcher's Manage sources… opens the tab in both.

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
4. **All sources.** The merged views, merged artists, filter chips, scope-aware search, Ond playlists that mix sources. Built, UI included. Done when a Navidrome source and a local one show as one library, and switching scope keeps your place.
5. **Combined albums** (see "Combined albums" above), in three stages. Stage 1: matching albums and songs, the setting, and the combined views with dots and formats. Stage 2: the preferred copy and falling back to another copy. Stage 3: stars, ratings and play counts across copies, downloads, and Keep apart. Done when the same album on two servers shows as one album whose songs are the union of both, and pulling one server's network cable leaves it playable.
6. **Later:** direct scrobbling for local tracks; Jellyfin; Plex.

## Ond backend handoff (milestone 4)

- `Session.accounts[AccountId]` holds each source's handle, connection flags,
  sign-in reason, request settings, server label/info, missing folders, skipped
  count and `sync` (label, album/track flags, progress, last sync). Its
  `state(work_offline)` produces the kit's `SourceState`. `Session.work_offline`
  is global; `Session.scope` is the current persisted scope. Source bridges,
  data snapshots and download stores live in `OndApp.multi` by account.
- Switcher rows are the configured sources in settings order, then All
  sources, Add source…, and Manage sources…. A source picks
  `Action::Scope(ScopeAction::One(id))`; All picks `ScopeAction::All`. Add uses
  `Action::Onboarding(AddAnother)` and Manage opens Settings › Sources.
  Existing one-source drawing stays on its original path.
- Dots obtain the account from `source::identity::account(&item.id)` and the
  colour/name from `Settings::source(account)`. All in-memory item ids,
  links, cover ids, route ids and selection ids carry the canonical ItemKey
  spelling. Per-source database rows and source API arguments remain bare.
- Chips read `Session.scope` (also copied into `PrefsView.scope`) and dispatch
  `PrefsAction::Scope(ScopeAction::Chip(SourceChip::{All, Toggle(id)}))`.
  The action uses `Scope::clear_chips` / `toggle_chip`; it never cancels work.
- `Data.merged_artists` holds each artist's original members and
  `albums: BTreeMap<AccountId, Vec<String>>` for the current scope. These are
  the groups to draw under each source's dot. Members outside the scope stay
  as aliases so changing scope can retain the same artist page. MusicBrainz
  ids have a separate `artist_mbids_v1` saved blob; frozen rows are unchanged.
- A foreign addition to a server playlist fetches the server's complete
  listing without editing it. `Msg::MixedPlaylistReady` installs
  `multi.pending_playlist` (`source: ItemKey`, `name`, ordered `entries`) and
  opens the minimal `Overlay::MixedPlaylist` confirmation. Dispatch
  `PlaylistAction::AcceptMixed(true)` to create the Ond copy, or `false` to
  decline. Ond playlist ids are `ond:<uuid>` in memory; their unchanged store
  rows hold ordered ItemKeys. Resolving uses each entry's source; M3U export
  writes local paths and skips server tracks to keep credentials out of files.
- Source row controls dispatch `Action::ForSource(id, Box<Action>)` for
  SyncNow, StopSync, AskSignOut and SignInAgain. Removal uses
  `SourceAction::AskRemove(id)` / `Remove(id)`. The UI pass can use the
  per-account state above for both Settings and sidebar details without
  selecting a different browsing scope.

The UI pass is built on this: dots on covers, the queue, playlists, the
album page, search, the toolbar's display, the Player and Mini (the playing
song's, whenever more than one source is configured), a Source column in
lists that mix sources, per-source artist headings,
each source's live state in the switcher and Settings, and the finished
mixed-playlist offer. The backend does not merge album copies. Rede's
source/session implementation is unchanged.

## Later sources

- **Jellyfin:** a username and password for an access token; music libraries through `/Items` with paging, favourites and playlists on the server, `/Audio/{id}/universal` for streaming and transcoding, lyrics on recent servers. Fits the trait as it stands.
- **Plex:** sign-in goes through plex.tv (a PIN or browser flow), then to one of the account's servers, which may have several addresses (local, remote, relay). Ratings are on a 10-point scale; there are no stars as such. Fits the trait, but its sign-in is a form of its own, as Rede's OpenID is.

## Open questions

- Should a local folder that holds both music and audiobooks be offered to both apps, or should each ignore what isn't its own (by folder, by genre tag, by file type)?
- **Settled:** Work Offline is one persisted switch for every networked source. It cancels their sessions and reconnect attempts together; local folders stay open and their watchers keep running. An unreachable server otherwise changes only its own state.
- When a track is starred in All sources and its source can't hold stars, Ond keeps the star. If that source later gains the capability (a server upgrade), are Ond's stars sent to it?
- **Settled:** Mini and the Player show the playing song's source dot, and nothing else of the merged view.

## Ond backend handoff (milestone 5, stage 1)

- `Settings.combine_copies` and `PrefsView.combine_copies` are off by
  default. Dispatch `Action::Prefs(PrefsAction::CombineCopies(on))` from
  Settings › Sources' **Combine matching albums** row. Home Manager may
  manage `combine_copies`; use the usual managed lock on that row. The
  action rebuilds the projection and invalidates search immediately.
- Per-source mirrors keep their original albums and tracks. Release ids
  live in `album_mbids_v1`, recording ids in `recording_mbids_v1`, both
  versioned saved `BTreeMap<String, String>` blobs with bare source ids.
  OpenSubsonic album lists, album fetches and bulk song syncs collect them;
  local scans use `mb_album_id` and `mb_recording_id`, never release-group
  or release-track ids. Loading, saving and messages discard blank ids
  through `clean_mbid`; a cleared server tag removes a previously saved id.
  `Data.album_mbids` and `Data.recording_mbids` have qualified in-memory keys.
  Frozen postcard rows and their schema versions are unchanged.
- `source::copies::{albums, songs}` is the pure matcher. Album name
  fallback uses the merged artist groups and `normalize_name` on the whole
  title. Known releases are grouped before untagged copies, so an untagged
  copy cannot bridge conflicting release ids. A group never has two members
  of one source. Songs try recording identity first, then position and title;
  an unnumbered song needs a unique title in each participating album copy.
  Missing or zero disc numbers mean disc 1; zero track numbers are unnumbered.
- The projection filters scope first and combines only with the setting on
  and at least two source snapshots in scope. `Data.albums`, `tracks.all`,
  `tracks.album(lead)` and search contain the union once. Songs unique to a
  later source retain their real song key, with their visible `album_id`
  pointing to the album lead. Album counts and duration describe that union;
  artists count and list it once. One-source chips and scopes expose plain
  source rows. Switching off restores the milestone 4 projection.
- `Data.album_copies` and `Data.track_copies` contain combined groups only,
  from the lead's canonical ItemKey spelling to `Vec<ItemKey>` (lead first,
  settings source order). `copies_of_album(id)` and `copies_of_track(id)`
  also handle singletons and non-lead members. Every ItemKey carries its
  account for dot colours/names. `album_lead(id)` and `track_lead(id)` resolve
  members to their current lead without inventing identities. Album routes
  follow an available member when combination, source order or chips change.
- `album_copy(id)` and `track_copy(id)` expose the source's original row.
  `copy_format(track_id)` exposes `suffix`, `bitrate_kbps` and `lossless`;
  `album_copy_formats(album_id)` returns those formats grouped by account,
  in source order, including formats of songs absent on the lead source.
  The UI pass should draw one dot per copy on combined covers and song rows,
  list every copy in the Source column, and show distinct album suffixes as
  **FLAC · MP3**. It should avoid source headings that repeat a combined album
  and use the union counts. The accessors and preference action awaiting
  that pass have narrow dead-code allowances, removable once the UI uses them.
- Projection memos include source order, scope, the combine switch and each
  source's album, track, artist, identity and list versions. Copy metadata
  changes rebuild the union and advance the search generation, even when
  the frozen rows did not change.

This stage adds no visual controls or marks. The existing scenes retain their
milestone 4 drawing. Preferred-copy choice and playback fallback (stage 2),
and shared stars, ratings, play counts, downloads and Keep apart (stage 3)
remain for their own passes. Copies stay in source order for those stages;
queue, playlist and pin identities remain ordinary member keys.

## Ond backend handoff (milestone 5, stage 2)

- `Settings.prefer_copy` and `PrefsView.prefer_copy` use
  `settings::PreferCopy::{SourceOrder, BestQuality}`. Source order is the
  default; JSON and Home Manager use `"source-order"` / `"best-quality"`.
  Unknown values fall back to source order. The future Sources control
  should dispatch `Action::Prefs(PrefsAction::PreferCopy(value))` and show
  the usual `prefer_copy` managed lock. This stage adds no Settings row.
- `player::Sources::copies` supplies real member keys and original track
  rows in preference order. `AppSources` in `app/playing.rs` implements it
  using the current combined projection and each account's session. The
  shared `Player::enqueue` path chooses the first playable copy for Play,
  Shuffle, Play Next and Add, including non-lead playlist entries. Quality
  sorts lossless first, then descending bitrate (unknown is zero), with
  settings source order breaking ties. With combination off, rows keep
  their requested copy. Queue keys and the saved queue layout are unchanged.
- Reachability is per account, independent of the focused source. A
  connecting, signed-out or unreachable server is unavailable; Work Offline
  also excludes server streams while local sources stay available. Pinned
  downloads and complete stream-cache files remain playable without their
  server. Selection checks availability without opening a stream.
- `Player::start_current` checks alternatives if a queued source has gone
  away. A pre-start `audio::Event::Error` with `ErrorKind::Unavailable`
  tries the remaining playable members before the existing next-song skip.
  The actual copy replaces the current slot's key and row, retaining its
  uid, queue source and position. Each member is tried once per start
  attempt; retry state is runtime only. Failed gapless prefetches are also
  remembered until that slot starts. A later source recovery never changes
  a song already playing.
- Failure policy: missing local files and stream acquisition failures
  (including exhausted network retries and HTTP/server rejections) permit
  fallback. Remote I/O failures while probing or reading also permit it, except
  cancellation and unexpected EOF. Available files that fail probing or
  decoding, local permission failures, output/device errors, and failures
  after TrackStarted do not switch copies. An undecodable or truncated
  available file is not evidence that its source disappeared. Successful
  fallback probes the failed account without showing a skipped-song error.
- The UI can show the playing member directly from
  `player.queue.current.key.account` and its original `track` format.
  `player.playing_from` and `playing::origin` still describe download,
  cache, local file or stream. New enqueue actions use the current setting;
  changing it or restoring a source does not rewrite existing queue slots.

The Transport/Sources/Note boundary, session-scoped bridge work and audio
callback are unchanged. No view drawing code or snapshot scene changes are
part of this stage. Shared stars, ratings, play counts, downloads across
copies and Keep apart remain stage 3.

## Ond backend handoff (milestone 5, stage 3)

- `Action::ToggleStar` now applies one desired state to every current song or
  album copy. `Favorites::is_on` derives the visible heart from any copy;
  `Favorites.view` maps stars onto union rows and lists each song and album
  once. Source snapshots and optimistic changes retain their own raw values,
  sequence protection and rollback. Each source's scoped bridge writes to
  that server, or to Ond's existing stars/ratings tables when the source
  lacks the capability. A refusal restores that copy's own previous value.
- `Favorites::rating` reads the preferred song copy. The preference order is
  shared with playback through `Data::preferred_tracks` / `preferred_track`:
  source order, or lossless then descending bitrate with stable source-order
  ties. Ratings and downloads use that order independently of temporary
  reachability; playback still filters for playable copies. `Action::Rate`
  writes every song copy. Changing `prefer_copy`, scope, source order or
  combination immediately rebuilds the derived metadata lookups.
- `Action::NeedSongStats` asks each copy once, through its scoped bridge.
  `song_stats` exposes the summed known play counts under every member key
  and the latest reported last-played time. Raw answers remain separate so
  disabling combination or changing scope restores each source's count.
  Adding copies to an already requested song fetches the missing answers.
  Counts are currently shown only in Info; no stored track layout changes.
  Scrobbling and now-playing reporting still use the member that played.
- Download actions select the preferred real member of each combined song.
  A combined album downloads its union using individual song pins, including
  songs absent from the lead source. Those exact selections survive source
  syncs; changing preference does not delete already pinned copies. Removing
  a combined song removes its copies from all pins that hold them, preserving
  other songs. `OfflineStore::unpin_tracks` performs that explicit removal
  through the existing saver/deleter; unknown files are never inferred from
  the combined projection.
- `ViewCtx::download_state`, `track_pin` and `album_pin` expose derived download
  state. Any downloaded copy lights a song. A combined album is complete only
  when every union song has a downloaded copy, including a mixture across
  accounts. The versioned, read-only download projection supplied to views
  lists that completed album once in Offline; unfinished individual song
  pins retain their progress rows. Real stores and file ownership stay per
  source. With combination off, all these actions and lookups retain their
  milestone 4 behavior.
- The UI pass should add **Keep apart** to combined album menus, dispatching
  `Action::KeepApart(album_lead)`, and **Combine again** on either separated
  member, dispatching `Action::CombineAgain(album_key)`. Both arguments use
  canonical account-qualified album keys. Keep apart records all pairs in
  the current group; Combine again clears only pairs involving the supplied
  member, including absent sources. `Data.apart_pairs` exposes the choices
  for menu availability or an optional separated-copy indicator.
- Exclusions live in the new `album_apart_v1` table in `ond.redb`, with a
  frozen `ApartV1` record and schema v1 → v2 migration. Existing stored rows
  are unchanged. The matcher checks every pair in a candidate group, so an
  A/B exclusion cannot be bridged by C, while C remains free to combine with
  either compatible group. Serialized background writes publish the saved
  choices, rebuild the projection and advance search/memo versions. Startup
  reloads them before source projection; syncs never replace them.

This pass adds no menu items, settings rows, view drawing or snapshot scenes.
The UI pass can keep the existing star, rating and download lookups, add the
album menu actions above, and optionally identify the preferred/downloaded
member using the real keys already exposed by the copy accessors.
