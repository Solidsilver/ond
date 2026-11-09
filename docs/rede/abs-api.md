# Audiobookshelf server API: verified spec for a Rust client

Source: https://github.com/advplyr/audiobookshelf, `master` at `f57ca09` (2026-11-05). `package.json` version **2.37.2**; latest release tag **v2.37.2** (published 2026-10-23). All `file:line` references are to that checkout. Paths below are server-relative. `ROUTER_BASE_PATH` is prepended when a request does not already start with it (`server/Server.js:325-334`), so `/api/...` works with or without the base path. If the user's server URL includes a reverse-proxy prefix, join paths onto it.

Conventions in the JSON:
- Timestamps called `*At`, `lastUpdate`, `lastSeen`, `mtimeMs` and so on are **integer milliseconds since epoch** (`Date.valueOf()`).
- Durations, `currentTime`, `startOffset` and chapter `start`/`end` are **float seconds**.
- IDs are UUID strings. `ino` is a **string**.
- Many fields can be `null`. Treat every non-ID field as `Option<_>` with `#[serde(default)]`, and ignore unknown fields.

---

## 1. Login: `POST /login` (claim TRUE)

`server/Auth.js:322-328`. The rate limiter runs first, then `passport.authenticate('local')`.

Request: JSON `{"username": "...", "password": "..."}`; urlencoded also works. The username is lowercased before lookup (`server/auth/LocalAuthStrategy.js` `verifyCredentials`). A passwordless root user logs in only when `password` is empty.

Header `x-return-tokens: true`. It must be the exact string `true` (`Auth.js:324`). With it, `user.refreshToken` is in the body and **no cookie is set**. Without it, `user.refreshToken` is `null` and the refresh token is set only as the httpOnly cookie `refresh_token` (`Auth.js:296-311`, `TokenManager.js:65-73`).

Bad credentials: passport fails with **401** and a plain-text `Unauthorized` body. Too many requests: **429** `{"error":"Too many authentication requests"}`.

Response 200 (`Auth.js:96-104`, `User.toOldJSONForBrowser` at `server/models/User.js:603-637`):
```jsonc
{
  "user": {
    "id": "uuid", "username": "str", "email": "str|null",
    "type": "root|admin|user|guest",
    "token": "str|null",            // legacy non-expiring JWT, still issued and still accepted (see surprises)
    "isOldToken": "bool|undefined", // usually absent
    "mediaProgress": [MediaProgress, ...],   // every progress row for the user
    "seriesHideFromContinueListening": ["seriesId", ...],
    "bookmarks": [Bookmark, ...],
    "isActive": true, "isLocked": false,
    "lastSeen": 1730000000000,      // or null
    "createdAt": 1730000000000,
    "permissions": { "download": bool, "update": bool, "delete": bool, "upload": bool,
                     "createEreader": bool, "accessAllLibraries": bool, "accessAllTags": bool,
                     "accessExplicitContent": bool, "selectedTagsNotAccessible": bool },
    "librariesAccessible": ["libId", ...],
    "itemTagsSelected": ["tag", ...],
    "hasOpenIDLink": bool,
    "accessToken": "jwt",           // added by handleLoginSuccess
    "refreshToken": "jwt|null"      // password: non-null with x-return-tokens: true; mobile OpenID: always returned
  },
  "userDefaultLibraryId": "uuid|null",
  "serverSettings": { /* ServerSettings.toJSONForBrowser, see below */ },
  "ereaderDevices": [ { "name": "...", "email": "...", "availabilityOption": "...", "users": [...] } ],
  "Source": "docker|debian|..."     // global.Source string
}
```
Useful `serverSettings` keys (`server/objects/settings/ServerSettings.js:230-310`): `version` (server version string), `buildNumber`, `language`, `sortingIgnorePrefix`, `sortingPrefixes`, `dateFormat`, `timeFormat`, `authActiveAuthMethods` (`["local","openid"]`), `chromecastEnabled`, `timeZone` (added in 2.36.0), plus many scanner/backup settings. `tokenSecret` and the OIDC secrets are stripped.

Related: `POST /api/authorize` (Bearer) returns the same payload without tokens (`MiscController.js:285-288`). Use it to re-read the user and settings with an access token.

Unauthenticated discovery: `GET /status` returns `{app:"audiobookshelf", serverVersion, isInit, language, authMethods, authFormData}` (`Server.js:375-391`). `GET /ping` returns `{success:true}`. `GET /healthcheck` returns 200.

## 2. Refresh: `POST /auth/refresh` (claim TRUE; `x-return-tokens` is NOT needed)

`Auth.js:331-359`, `TokenManager.js:350-455`.
- The token is read from the `refresh_token` cookie. If the header `x-refresh-token` is present, it overrides the cookie, and only then is the new refresh token returned in the body. `x-return-tokens` is ignored on this route.
- No body is needed.
- **Rotation on every refresh**: a new access token and a new refresh token are issued. The session row is updated with a compare-and-swap on the old token (`TokenManager.js:210-270`). The cookie is also re-set.
- **Grace period**: the previous refresh token stays valid for `REFRESH_TOKEN_GRACE_PERIOD` seconds (default 600). Presenting the old token in that window returns the **already-rotated current** refresh token plus a fresh access token without rotating again (`TokenManager.js:374-418`). After the window, the old token gets `401 {"error":"Invalid refresh token"}`.
- Response 200: **the same shape as login** (`getUserLoginResponsePayload`). The tokens are at `user.accessToken` and `user.refreshToken`; `refreshToken` is `null` unless the `x-refresh-token` header was used.
- Errors: 401 with `{"error": "No refresh token provided" | "Invalid token type" | "Invalid refresh token" | "Refresh token expired" | "User not found or inactive"}`. An expired session row is deleted.
- Every refresh rebuilds the whole user payload, including all `mediaProgress`.

## 3. Default lifetimes (claim FALSE for current versions)

`server/auth/TokenManager.js:15-31`:
- `ACCESS_TOKEN_EXPIRY` env, seconds; default **3600 (1 hour)**.
- `REFRESH_TOKEN_EXPIRY` env, seconds; default **2592000 (30 days)**. The DB session `expiresAt` uses the same value.
- `REFRESH_TOKEN_GRACE_PERIOD` env, seconds; default **600**.
- `JWT_SECRET_KEY` env, or a generated secret stored in server settings (`initTokenSecret`).

History: v2.26.0 through v2.30.x defaulted to **12 h access / 7 d refresh**. That is the claim, and it is out of date. From **v2.31.0** onward the defaults are **1 h / 30 d**. I checked these tags with the GitHub contents API: v2.26.0 and v2.30.0 have 12 h/7 d; v2.31.0, v2.35.0 and v2.36.0 have 1 h/30 d.

JWT payloads: access `{userId, username, jti, type:"access", iat, exp}`; refresh has `type:"refresh"`; API keys have `type:"api", keyId`. Read `exp` from the access token to schedule refreshes instead of hard-coding a lifetime.

## 4. Logout: `POST /logout` (claim TRUE)

`Auth.js:486-525`. The refresh token comes from the cookie or the `x-refresh-token` header. That session row is deleted. `?allDevices=1` deletes **all** of the user's sessions (`TokenManager.invalidateAllSessionsForRefreshToken`). There is no rate limiter and no Bearer requirement. Response 200 `{"redirect_url": null | "<oidc end-session url>"}`. With no token present it still returns 200.

## 5. Auth on `/api` and `?token=` (claim TRUE, broader than stated)

- `/api/*` goes through `passport.authenticate('jwt')` (`Server.js:345`, `Auth.js:61-63`). The extractor is `fromAuthHeaderAsBearerToken()` **or** `fromUrlQueryParameter('token')` (`Auth.js:126`). **Every** `/api` route therefore accepts `?token=<accessToken>`, not only file routes. A refresh token is rejected on `/api` (`TokenManager.js:81-85`, `306-310`). Failure returns **401** with a plain-text `Unauthorized` body.
- Accepted credential types: an access JWT (with `exp` checked); an API key JWT (`type:"api"`, checked against the `apiKeys` table, deactivated on expiry); a **legacy JWT with no `exp`** (`user.token`), which is still accepted and flagged `isOldToken`.
- **No auth** for `GET /api/items/:id/cover` and `GET /api/authors/:id/image` (`Auth.js:21`, `ignorePatterns`).
- `/hls/:stream/:file` has **no auth**; the session id is the capability (`server/routers/HlsRouter.js`). It is mounted outside `/api` (`Server.js:346`).
- `/public/session/:id/track/:index` has **no auth**; it streams the file for an open session id (`PublicRouter.js:21`, `SessionController.getTrack` at `SessionController.js:279-330`). The index is the track's `index` field, which is 1-based for books.
- `/public/share/:slug/...` covers media shares and needs no auth.

**`contentUrl`** values in a play session:
- Direct play (books): `/api/items/{libraryItemId}/file/{ino}` (`server/models/Book.js:293-305`). Podcasts use the same pattern (`PodcastEpisode.js:189`). The path is **relative**, has no base path or host, and carries **no token**. The client must join it to the server URL and authenticate with `Authorization: Bearer` (preferred) or by appending `?token=<accessToken>`.
- Transcode: `/hls/{sessionId}/output.m3u8` (`Stream.js:100-102`), with `mimeType: "application/vnd.apple.mpegurl"`. No auth is needed.
- `GET /api/items/:id/file/:ino` uses Express `res.sendFile`, which supports `Range` and returns 206. The `Content-Type` comes from the extension map (m4b becomes `audio/mp4`). If `USE_X_ACCEL` is set, the server answers **204** with an `X-Accel-Redirect` header, and nginx serves the file. It requires library and tag access but not the `download` permission (`LibraryItemController.js:989-1004`, middleware at `1220-1249`).
- An access token in `?token=` expires after 1 h by default. A long stream started with a query token keeps working only while the HTTP connection lives; a re-open after expiry gets 401. Prefer the header.

## 6. `GET /api/libraries` (claim TRUE)

`LibraryController.js:188-213`, `server/models/Library.js:202-219`. `?include=stats` adds `stats`.
```jsonc
{ "libraries": [ {
    "id": "uuid", "name": "str",
    "folders": [ { "id": "uuid", "fullPath": "str", "libraryId": "uuid", "addedAt": ms } ],
    "displayOrder": 1, "icon": "str",
    "mediaType": "book" | "podcast",
    "provider": "str",
    "settings": { "coverAspectRatio": 0|1, "disableWatcher": bool, "skipMatchingMediaWithAsin": bool,
                  "skipMatchingMediaWithIsbn": bool, "autoScanCronExpression": str|null,
                  "audiobooksOnly": bool, "epubsAllowScriptedContent": bool, "hideSingleBookSeries": bool,
                  "onlyShowLaterBooksInContinueSeries": bool, "metadataPrecedence": [str],
                  "markAsFinishedTimeRemaining": 10, "markAsFinishedPercentComplete": null|number, ... },
    "lastScan": ms|null, "lastScanVersion": str|null,
    "createdAt": ms, "lastUpdate": ms
} ] }
```
`GET /api/libraries*` responses are cached server-side per username and URL (`ApiCacheManager`). Any non-progress DB write clears the cache, so results are not stale in practice.

## 7. `GET /api/libraries/:id/items` (claim PARTLY: there is no "since" filter, and `minified` is a no-op)

`LibraryController.js:604-641`, `LibraryItem.getByFilterAndSort` at `server/models/LibraryItem.js:294-328`, `libraryItemsBookFilters.js`.

Query parameters:
- `limit`: 0 or absent means **all items**.
- `page`: 0-based. `offset = page * limit`.
- `sort`: one of `addedAt`, `size`, `birthtimeMs`, `mtimeMs`, `media.duration`, `media.metadata.publishedYear`, `media.metadata.authorNameLF`, `media.metadata.authorName`, `media.metadata.title`, `sequence` (only with a series filter), `progress`, `progress.createdAt`, `progress.finishedAt`, `random` (`libraryItemsBookFilters.js:254-300`). **There is no `updatedAt` sort.** An unknown or absent sort gives **no ORDER BY**, so paging order is unspecified. Use `sort=addedAt` for stable paging; ties on the same millisecond are still possible.
- `desc=1`.
- `filter=<group>.<base64(value)>`, where the value is base64 and then URL-encoded (`libraryFilters.decode`). Groups: `genres, tags, series, authors, progress, narrators, publishers, publishedDecades, missing, languages, tracks, ebooks`, plus the bare groups `issues`, `feed-open`, `share-open`, `recent`. Progress values: `finished`, `in-progress`, `not-started`, `not-finished`. **No updatedAt or since filter exists.** Incremental sync has to page everything and diff on `updatedAt`. Alternatively, listen on the socket for `item_added`, `item_updated` and `item_removed`.
- `collapseseries=1` and `include=rssfeed,...` are optional.
- `minified=1` is echoed back but **ignored**: results are always minified (`toOldJSONMinified`).

Response:
```jsonc
{
  "results": [LibraryItemMinified, ...],
  "total": 123,                 // number (undefined/absent with collapseseries+series filter)
  "limit": "50",                // !!! echoed from the query string: a STRING if given, number 0 if absent
  "page": "0",                  // !!! same: string or 0
  "sortBy": "addedAt"|absent, "sortDesc": bool, "filterBy": str|absent,
  "mediaType": "book"|"podcast", "minified": bool, "collapseseries": bool,
  "include": "", "offset": 0    // number
}
```
`LibraryItemMinified` (`LibraryItem.js:1000-1029` plus `Book.js:643-662` and `575-595`):
```jsonc
{
  "id": "uuid", "ino": "str", "oldLibraryItemId": str|null, "libraryId": "uuid", "folderId": "uuid",
  "path": "str", "relPath": "str", "isFile": bool,
  "mtimeMs": ms, "ctimeMs": ms, "birthtimeMs": ms,
  "addedAt": ms,          // libraryItem.createdAt
  "updatedAt": ms,        // libraryItem.updatedAt
  "isMissing": bool, "isInvalid": bool, "mediaType": "book"|"podcast",
  "numFiles": int, "size": int,
  "media": {              // book
    "id": "uuid",
    "metadata": {
      "title": str|null, "titleIgnorePrefix": str, "subtitle": str|null,
      "authorName": "A, B",           // comma-joined
      "authorNameLF": "Last, First, ...",
      "narratorName": "N1, N2",       // comma-joined
      "seriesName": "Series #1, Other",  // comma-joined, "#seq" appended when set
      "genres": [str], "publishedYear": str|null, "publishedDate": str|null, "publisher": str|null,
      "description": str|null, "isbn": str|null, "asin": str|null, "language": str|null,
      "explicit": bool, "abridged": bool
      // with a series filter a "series" object is added: { id, name, sequence }
    },
    "coverPath": str|null, "tags": [str],
    "numTracks": int, "numAudioFiles": int, "numChapters": int,
    "duration": float, "size": int, "ebookFormat": str|undefined
  }
  // optional: collapsedSeries, rssFeed, mediaItemShare, numEpisodesIncomplete (podcasts), media.numEpisodes
}
```
Caveat for diffing: `libraryItem.updatedAt` is bumped by `PATCH /api/items/:id/media` (`LibraryItemController.updateMedia`, which calls `changed('updatedAt', true)`). Some media-only writes, such as `PATCH /api/items/:id/tracks`, save only the Book row, so the item's `updatedAt` may not move. Pair `updatedAt` with the socket `item_updated` event, or with `numTracks`, `duration` and `size`, when diffing.

## 8. `GET /api/items/:id` (claim TRUE; use `?expanded=1`)

`LibraryItemController.js:61-101`. Without `expanded=1` the old full JSON is returned, without `tracks` and without the minified metadata strings. With `?expanded=1&include=progress` you get `toOldJSONExpanded` (`LibraryItem.js:1032-1041`, `Book.js:671-690`) and `userMediaProgress` (MediaProgress or null). `&episode=<id>` selects the episode for a podcast.

The expanded item is a superset of the minified item, plus `lastScan`, `scanVersion` and `libraryFiles: [...]`, and `media` becomes:
```jsonc
{
  ...minified media,
  "libraryItemId": "uuid",
  "metadata": { "title", "subtitle", "authors": [{"id","name"}], "narrators": [str],
                "series": [{"id","name","sequence": str|null}], "genres", "publishedYear", "publishedDate",
                "publisher", "description", "isbn", "asin", "language", "explicit", "abridged",
                "titleIgnorePrefix", "authorName", "authorNameLF", "narratorName", "seriesName",
                "descriptionPlain": str|null },
  "audioFiles": [AudioFile, ...],      // all, including exclude:true
  "chapters": [ { "id": int, "start": float, "end": float, "title": str } ],
  "ebookFile": {...}|null,
  "tracks": [AudioTrack, ...]          // excludes exclude:true files, see below
}
```
`AudioFile` (`server/objects/files/AudioFile.js:43-70`, `FileMetadata.js:31-42`):
```jsonc
{ "index": int, "ino": "str",
  "metadata": { "filename": str, "ext": ".m4b", "path": "/server/abs/path", "relPath": str,
                "size": int, "mtimeMs": ms, "ctimeMs": ms, "birthtimeMs": ms },
  "addedAt": ms, "updatedAt": ms,
  "trackNumFromMeta": int|null, "discNumFromMeta": int|null,
  "trackNumFromFilename": int|null, "discNumFromFilename": int|null,
  "manuallyVerified": bool, "exclude": bool, "error": str|null,
  "format": str, "duration": float, "bitRate": int, "language": str|null, "codec": str,
  "timeBase": str, "channels": int, "channelLayout": str,
  "chapters": [Chapter], "embeddedCoverArt": str|null, "metaTags": {...},
  "mimeType": "audio/mp4" }
```
`AudioTrack` for a book is the AudioFile **plus** `title` (= `metadata.filename`), `startOffset` (cumulative float seconds) and `contentUrl` (`Book.getTracklist` at `Book.js:293-305`). Tracks are the `includedAudioFiles`, those without `exclude`, in stored order.

Mime types the server assigns (`server/utils/constants.js:36-58`): `audio/mpeg, audio/mp4, audio/ogg, audio/aac, audio/flac, audio/x-ms-wma, audio/x-aiff, audio/webm, audio/x-matroska, audio/amr-wb, audio/x-caf`.

## 9. `POST /api/items/:id/play` (claim TRUE, with caveats)

Routes: `/api/items/:id/play` and `/api/items/:id/play/:episodeId` (`ApiRouter.js:118-119`). 404 if the book has no audio tracks.

Request body (`PlaybackSessionManager.js:52-86`, `313-364`; `DeviceInfo.js`):
```jsonc
{
  "deviceInfo": {
    "deviceId": "stable-string",   // reuse forever; device row is looked up by it
    "clientName": "Ond",
    "clientVersion": "0.5.0",
    "manufacturer": "str?", "model": "str?", "sdkVersion": "str|number? (Android only)"
    // deviceName is computed server side, not taken from the client
  },
  "supportedMimeTypes": ["audio/mpeg","audio/mp4","audio/flac","audio/ogg", ...],
  "forceDirectPlay": true,     // recommended
  "forceTranscode": false,
  "mediaPlayer": "ond"         // free string; default "unknown"
}
```
- Direct play happens when `forceDirectPlay` is set, **or** when `supportedMimeTypes` is an array containing every track's mimeType. **If `supportedMimeTypes` is missing or not an array and `forceDirectPlay` is not set, the server transcodes to HLS** with ffmpeg (`Book.checkCanDirectPlay` at `Book.js:278-284`).
- Starting a session **closes any other open session for the same user and the same device row** (`PlaybackSessionManager.js:315-319`). A stable `deviceId` matters.
- `deviceDescription` uses `model`; without it the text comes from User-Agent parsing. Sending `model` (for example "macOS") gives a readable session list.
- The start position comes from server progress. If the item is `isFinished`, it starts at 0 (`:324-334`).

Response = `PlaybackSession.toJSONForClient` (`server/objects/PlaybackSession.js:86-116`):
```jsonc
{
  "id": "uuid", "userId": "uuid", "libraryId": "uuid", "libraryItemId": "uuid",
  "bookId": "uuid|null", "episodeId": "uuid|null",
  "mediaType": "book"|"podcast",
  "mediaMetadata": { /* Book.oldMetadataToJSON: title, subtitle, authors[{id,name}], narrators[], series[{id,name,sequence}], genres, publishedYear, ... */ },
  "chapters": [ { "id": int, "start": float, "end": float, "title": str } ],
  "displayTitle": str, "displayAuthor": str, "coverPath": str|null,
  "duration": float,
  "playMethod": 0,            // 0 DIRECTPLAY, 1 DIRECTSTREAM, 2 TRANSCODE, 3 LOCAL
  "mediaPlayer": str,
  "deviceInfo": { "id","userId","deviceId","ipAddress","browserName","browserVersion","osName","osVersion",
                  "deviceType","clientVersion","manufacturer","model","sdkVersion","clientName","deviceName" }, // null keys omitted
  "serverVersion": "2.37.2",
  "date": "YYYY-MM-DD", "dayOfWeek": "Monday",
  "timeListening": 0, "startTime": float, "currentTime": float,
  "startedAt": ms, "updatedAt": ms,
  "audioTracks": [ ... ],
  "libraryItem": { /* LibraryItem expanded */ }
}
```
`audioTracks`:
- Direct play: the full Book track objects, meaning AudioFile plus `title`, `startOffset` and `contentUrl` (section 8). These include `ino`, `metadata.size`, `metadata.path` (a server-side path) and `mimeType`. They are **not** `AudioTrack.toJSON`.
- Transcode: one object `{index:1, startOffset:0, duration, title, contentUrl:"/hls/<id>/output.m3u8", mimeType:"application/vnd.apple.mpegurl", codec:null, metadata:null}` (`server/objects/files/AudioTrack.js:13-45`).

Sessions live **in memory**. They are lost on a server restart, and stale ones (no update for 36 h) are dropped (`closeStaleOpenSessions`). After that, `/api/session/:id/*` returns **404**.

## 10. Session sync and close (claim TRUE)

`POST /api/session/:id/sync` (`SessionController.js:166-168`, `PlaybackSessionManager.syncSession` at `:373-412`)
```jsonc
{ "currentTime": float, "timeListened": float /* seconds since last sync, added to timeListening */, "duration": float /* optional */ }
```
- Writes progress **unconditionally**, with no timestamp comparison. It passes `markAsFinishedTimeRemaining` / `markAsFinishedPercentComplete` from library settings, so the item is auto-finished near the end.
- Bumps the session's `updatedAt` only if `timeListened > 0`.
- Emits the socket event `user_item_progress_updated`.
- Response: **200**, empty. **500** if the item or library is gone. **404** if the session is not open. **403** if the session belongs to another user and the caller is not an admin.

`POST /api/session/:id/close` (`SessionController.js:178-182`): the body is the same as sync, or `{}`/empty to just close. It syncs if the body is non-empty, saves the session (only if `timeListening > 0`), emits `user_session_closed` and returns **200**.

`GET /api/session/:id` returns the open session in the same shape as the play response.

## 11. Local sessions: `POST /api/session/local` and `/api/session/local-all` (claim PARTLY; read the semantics)

`SessionController.js:254-271`, `PlaybackSessionManager.js:103-290`.

`POST /api/session/local`: the body **is** a PlaybackSession JSON, and `deviceInfo` inside it is used as the client device info. Response **200** empty on success; **500** with a text error such as `Media item not found` or `Library not found`.

`POST /api/session/local-all`:
```jsonc
{ "deviceInfo": { ...as in play... },
  "sessions": [ PlaybackSessionJson, ... ] }
```
Response: `{ "results": [ { "id": "uuid", "success": true, "progressSynced": bool } | { "id", "success": false, "error": "Media item not found"|"Library not found" } ] }`

Fields of a PlaybackSession JSON the server reads (`PlaybackSession.construct` at `:118-169`): `id` (use a UUID you generate; ids prefixed `play_local_` get remapped in memory), `libraryItemId`, `episodeId`, `libraryId`, `bookId`, `mediaType`, `displayTitle`, `displayAuthor`, `duration`, `playMethod` (use 3 = LOCAL), `mediaPlayer`, `chapters`, `mediaMetadata`, `coverPath`, `date`, `dayOfWeek`, `timeListening` (seconds), `startTime`, `currentTime` (seconds), `startedAt` (ms), **`updatedAt` (ms)**. For a new session, `duration` is replaced by the item's duration; missing metadata and title fields are filled in from the item.

How a local session updates progress (`:179-271`):
1. Session upsert: if the id is unknown, a new playback-session row is inserted. If it is known, the server overwrites `currentTime`, `timeListening` and `updatedAt` from the JSON and recomputes `date`.
2. Progress decision: the server finds the user's existing MediaProgress for the book or episode.
   - **If `progress.updatedAt (ms) > session.updatedAt`**, progress is **not touched** (`progressSynced: false`; the session row is still saved for stats). An older local session therefore **cannot** overwrite newer server progress.
   - Otherwise, including **equal timestamps**, progress is overwritten with `{duration: session.duration, currentTime: session.currentTime, progress: currentTime/duration, lastUpdate: session.updatedAt}` plus the library's mark-as-finished thresholds. `lastUpdate` then **force-sets** the progress row's `updatedAt` to the session's `updatedAt` with raw SQL (`MediaProgress.js:252-262`), so the stored `lastUpdate` equals the client's timestamp, not the server's clock.
   - If no progress exists, it is created from the session.
3. On success it emits `user_item_progress_updated` `{id, sessionId, deviceDescription, data: MediaProgress}`.

Pitfalls:
- `session.updatedAt` falls back to `startedAt`. If both are missing, the comparison `x > undefined` is false and the session **always overwrites**, with `updatedAt` stamped as now. **Always send `updatedAt`.**
- The comparison uses the *session's* `updatedAt`, not a separate progress timestamp. A session that was paused long ago but whose `updatedAt` was bumped later still wins. Set `updatedAt` to the time of the last real position change.
- Clock skew between client and server decides the winner, since client ms is compared with server ms.
- There is no `isFinished` in the payload. Finish status comes only from the thresholds: if the stored progress was finished and the new `currentTime` is not past the threshold, the item becomes **un-finished** (`MediaProgress.js:233-241`).

## 12. Progress endpoints (claim PARTLY: PATCH is a plain overwrite, not last-write-wins)

Routes (`ApiRouter.js:177-192`):
- `GET /api/me` returns the same user object as `login.user` (without tokens), **including `mediaProgress` for every item and episode** and `bookmarks` (`MeController.js:27-29`).
- `GET /api/me/progress` returns `{ "mediaProgress": [MediaProgress] }` (added in 2.36.0).
- `GET /api/me/progress/:libraryItemId/:episodeId?` returns MediaProgress or **404**.
- `PATCH /api/me/progress/:libraryItemId/:episodeId?` returns **200** empty; 404 `Library item not found`/`Episode not found`, 400 `Library item is not a book` (a podcast item without an episodeId).
- `PATCH /api/me/progress/batch/update`: the body is a **JSON array** of payloads, each carrying `libraryItemId` and optional `episodeId`. Returns **200** empty even if some entries fail (failures are only logged); **400** `Missing request payload` on an empty array.
- `DELETE /api/me/progress/:progressId` takes the **MediaProgress id**, not the item id. Returns 200 or 404.
- `GET /api/me/progress/:id/remove-from-continue-listening` (GET despite mutating).
- `GET /api/me/items-in-progress?limit=25` returns `{ "libraryItems": [ LibraryItemMinified + { "progressLastUpdate": ms, "recentEpisode"?: Episode } ] }`. Only non-finished items with `currentTime > 0` or `ebookProgress > 0`, sorted by lastUpdate descending, across all libraries (`MeController.js:485-523`).

PATCH payload (`User.js:67-81` `ProgressUpdatePayload`):
```jsonc
{ "duration": float?, "progress": float(0..1)?, "currentTime": float?, "isFinished": bool?,
  "hideFromContinueListening": bool?, "ebookLocation": str?, "ebookProgress": float?,
  "finishedAt": ms?, "lastUpdate": ms?,
  "markAsFinishedTimeRemaining": number?, "markAsFinishedPercentComplete": number? }
```
MediaProgress (`MediaProgress.js:155-174` `getOldMediaProgress`):
```jsonc
{ "id": "uuid", "userId": "uuid",
  "libraryItemId": "uuid|null", "episodeId": "uuid|null",
  "mediaItemId": "uuid",            // book id or episode id
  "mediaItemType": "book"|"podcastEpisode",
  "duration": float, "progress": float, "currentTime": float,
  "isFinished": bool, "hideFromContinueListening": bool,
  "ebookLocation": str|null, "ebookProgress": float|null,
  "lastUpdate": ms,                 // row updatedAt
  "startedAt": ms,                  // row createdAt
  "finishedAt": ms|null }
```
Semantics (`MediaProgress.applyProgressUpdate` at `:190-264`; `User.createUpdateMediaProgressFromPayload` at `User.js:730-831`):
- **No comparison of `lastUpdate`.** A PATCH always overwrites. If `lastUpdate` is supplied, it **force-sets** `updatedAt` afterward, even to an older time. The client has to implement last-write-wins itself by reading `lastUpdate` first.
- The body is applied with `this.set(progressPayload)`. Send only the fields listed above.
- Setting `isFinished: false` on a finished item **resets `currentTime` to 0 and drops any `currentTime` sent in the same request**. To un-finish at a position, send two PATCHes: `{isFinished:false}`, then `{currentTime}`.
- Setting `isFinished: true` sets `finishedAt` (from the payload or now) and `progress = 1`.
- Auto-finish: with `duration` set, the item is finished when the time remaining is under `markAsFinishedTimeRemaining`, which **defaults to 10 s even on a plain PATCH**. If the item was finished and `currentTime` changes to a point before that threshold, it is **un-finished**.
- A change to `currentTime` resets `hideFromContinueListening` unless the payload sets it.
- When `progress` is sent without `isFinished`, it is stored in `extraData.progress`. The returned `progress` is that stored value, not `currentTime/duration`.
- Each PATCH emits the socket event `user_updated` (the full user JSON), not `user_item_progress_updated`.

## 13. Cover: `GET /api/items/:id/cover` (claim TRUE; no auth needed)

`LibraryItemController.js:401-432`, `CacheManager.handleCoverCache` at `server/managers/CacheManager.js:43-85`. The route is listed in `ignorePatterns`, so no token is required, though sending one does no harm.

Query: `width` (default **400**, clamped to ≤ 4096), `height` (optional), `format` = `webp` | `jpeg` | `png` (default `webp` if `Accept` includes `image/webp`, else `jpeg`; anything else returns **400**), `raw=1` (the original file, no resize), `ts=<anything>` (adds `Cache-Control: private, max-age=86400`). The id must be a UUID. 404 if there is no cover. With `USE_X_ACCEL` set, `raw` returns 204 with `X-Accel-Redirect`.

## 14. Bookmarks (claim TRUE)

`MeController.js:321-424`, `User.js:840-915`. Bookmarks are keyed by `(libraryItemId, time)`.
- `POST /api/me/item/:libraryItemId/bookmark` with body `{"time": number (seconds), "title": "non-empty string"}` returns the Bookmark. An existing bookmark at the same time has its title updated instead.
- `PATCH /api/me/item/:libraryItemId/bookmark` with body `{"time": number, "title": str}` matches on `time` and renames. Returns the Bookmark; **404** if there is no bookmark at that time. The time cannot be changed: delete and recreate instead.
- `DELETE /api/me/item/:libraryItemId/bookmark/:time` takes `time` as a number in the path and returns 200 or 404.
- `GET /api/me/bookmarks` returns `{bookmarks:[...]}`. `GET /api/me/bookmarks/:libraryItemId` returns the same filtered to one item. Both arrived in 2.36.0.
- Bookmark: `{ "libraryItemId": "uuid", "time": number, "title": str, "createdAt": ms }`.
- 400 `Invalid time` / `Invalid title`; 403 without library access; 404 for an unknown item.
- Equality: lookup uses `==`, but removal filters with `!==`. Always send `time` as a JSON **number**, never a string, or the delete will 200 without removing anything.
- Every write emits `user_updated`.

## 15. Socket.io (claim TRUE: `user_item_progress_updated` exists)

The server is socket.io at path `/socket.io`, and also at `<base>/socket.io` when a base path is set (`SocketAuthority.js:154-175`). After connecting, the client emits `auth` with the **access token** string (an API key also works). The server replies `init` `{userId, username, usersOnline?}`, or `auth_failed` `{message: "Invalid token"|"Token expired"|"Invalid API key"|"API key expired"|"Invalid user"}`. On a new access token, emit `auth` again; re-auth to the same user is allowed. `ping` gets `pong`.

Events relevant to a player:
- `user_item_progress_updated` `{id, sessionId, deviceDescription, data: MediaProgress}`, from session sync and local sync.
- `user_updated` (the full user JSON, including all mediaProgress and bookmarks), from PATCH progress, bookmarks and similar.
- `user_session_closed` (the session id).
- `item_added`, `item_updated`, `item_removed`, `items_added`, `items_updated`.
- `library_added`, `library_updated`, `library_removed`.
- `series_added`, `series_updated`, `series_removed`.
- `author_added`, `author_updated`, `author_removed` (plus `authors_num_books_updated`, per the 2.36.0 notes).
- Admin only: `user_stream_update`, `user_online`, `user_offline`.

## 16. History and CVE (claims TRUE)

- The JWT access and refresh system arrived in **v2.26.0** (released 2025-07-12): "JWT authentication with refresh tokens to replace old authentication system (#4444)", along with API keys, the Session model and the auth rate limiter. The `TOKEN_SECRET` env var was removed.
- **CVE-2026-73085 / GHSA-798w-wv6c-2c3h** ("Refresh Token Accepted on Resource Endpoints", medium, published 2026-08-04) lists patched version **2.36.0**. Confirmed via `gh api repos/advplyr/audiobookshelf/security-advisories`. The v2.36.0 notes say: "API and websocket authentication allowing refresh tokens by @mikiher in #5387". The fix is `TokenManager.isBearerAccessTokenPayload` (`TokenManager.js:81-85`), used in `jwtAuthCheck` and in socket auth.
- Other auth-adjacent changes in 2.36.0: the refresh grace period extended to 10 min with the `REFRESH_TOKEN_GRACE_PERIOD` env var; a password change invalidates all sessions; `/logout?allDevices=1`; `GET/DELETE /api/me/sessions`.
- Later advisory: GHSA-m5hp-x2xc-7jqq (critical, unauthenticated path traversal and arbitrary file read, published 2026-10-19). It names no CVE, and its patched-versions field reads ">= v2.36.1", which looks like the affected range was entered by mistake. A client should warn on very old servers.
- Current latest: **v2.37.2** (2026-10-23). `master` is past it (a socket API-key commit in 2.37.0, upload auth hardening, session middleware userId check).

## 17. Client-name or User-Agent checks; rate limiting (claim: no client checks; rate limiting YES)

- **No server-side gating on client name or User-Agent.** The UA is only stored (the auth Session row `userAgent`, `TokenManager.js:182-196`) and parsed for display (`PlaybackSessionManager.getDeviceInfo` at `:52-72`). `deviceInfo` string fields are HTML-stripped.
- **Rate limit** (`server/utils/rateLimiterFactory.js`): `express-rate-limit`, **40 requests per 10 minutes per client IP** by default. It is a single shared limiter instance applied to `POST /login`, **`POST /auth/refresh`**, `/auth/openid*` and `PATCH /api/me/password`, so **refreshes count against the same budget as logins**, and successful requests count too. Env: `RATE_LIMIT_AUTH_MAX` (`0` disables it), `RATE_LIMIT_AUTH_WINDOW` (ms), `RATE_LIMIT_AUTH_MESSAGE`. Over the limit: **429** `{"error": "Too many authentication requests"}` with standard `RateLimit-*` headers. The IP comes from `requestIp` (proxy headers). Behind a shared NAT, several clients share the budget.
- `POST /logout` and `/api/*` are not rate-limited.

---

## Surprises and recommendations for the Rust client

1. Defaults are **1 h access / 30 d refresh** (since 2.31.0), not 12 h/7 d. Schedule refreshes from the JWT `exp`, and refresh lazily on 401 with a single in-flight refresh.
2. On `/auth/refresh`, send `x-refresh-token`; `x-return-tokens` is irrelevant there. Persist the new refresh token **before** using the new access token. If the response is lost, retrying with the old token within 10 min returns the current pair.
3. Refreshes share the 40-per-10-min login rate limit. Don't refresh on a timer for every request; handle 429.
4. `limit` and `page` come back as **strings** in `/libraries/:id/items` when sent. Deserialize them leniently, or ignore them.
5. Without a `sort`, item paging order is undefined. Use `sort=addedAt`. There is no since or updatedAt filter: incremental sync means a full listing diffed on `updatedAt`, plus socket events.
6. Always send `forceDirectPlay: true`, or a real `supportedMimeTypes` array, on `/play`. Otherwise the server starts an ffmpeg HLS transcode.
7. `contentUrl` is a relative `/api/items/{id}/file/{ino}` path with no token. Add the Bearer header, or `?token=`.
8. `?token=` works on **every** `/api` route, not only file routes. Covers need no auth at all. `/hls/*` and `/public/session/:id/track/:index` need none either.
9. Progress PATCH is a blind overwrite. Only `/api/session/local(-all)` compares timestamps: server progress newer than `session.updatedAt` wins, and equal timestamps let the session through. For offline playback, report through `local-all` with accurate `updatedAt` values, not through PATCH.
10. `isFinished:false` resets the position to 0 and drops `currentTime` in the same PATCH.
11. Play sessions are in memory. Expect 404 on sync after a server restart, and recover by starting a new session or posting a local session.
12. The legacy non-expiring `user.token` is still returned and still accepted. Don't use it, and don't log it.
13. Delete bookmarks with a numeric `time` that exactly matches the stored value.

## 18. Native OpenID sign-in (verified mobile flow)

Verified against the read-only 2.37.2/master source: `server/Auth.js`
(`paramsToCookies`, `handleLoginSuccessBasedOnCookie`, the three OpenID routes),
`server/auth/OidcAuthStrategy.js` (`getAuthorizationUrl`, `generatePkce`,
`isValidRedirectUri`, `handleMobileRedirect`), and
`server/objects/settings/ServerSettings.js` (`authFormData`). This is a code
exchange through ABS, not a provider token request made directly by Rede.

1. `GET /status` exposes `authMethods` (`local`, `openid`) and `authFormData`:
   `authLoginCustomMessage` (sanitized HTML), `authOpenIDButtonText` (default
   `Login with OpenId`) and `authOpenIDAutoLaunch` (default false). The OpenID
   fields appear only when that method is enabled. ABS's web form places the
   local form before the OpenID button; auto-launch goes directly to OpenID.
   Empty/missing methods retain the legacy local form. Rede checks the minimum
   version and warning version before offering either method, uses the server's
   button text verbatim, and honors auto-launch once after discovery.
2. Rede generates independent 32-byte CSPRNG values for the PKCE verifier and
   state, encoded base64url without padding. It makes **a native HTTP request**
   to `GET /auth/openid?response_type=code&redirect_uri=rede%3A%2F%2Foauth&code_challenge=<base64url(SHA256(verifier))>&code_challenge_method=S256&state=<state>`.
   `response_type=code`, a `redirect_uri`, **or** a `code_challenge` selects the
   mobile flow. The verifier is not sent yet. S256 is the only accepted method.
3. The native request disables HTTP redirects and retains the response's ABS
   session cookies, including `connect.sid` and `auth_method=openid-mobile`.
   A success is **302** with the provider authorization URL in `Location`.
   Rede opens that URL in the system browser. Opening the original ABS URL in
   the browser instead would give the browser the session that the native
   exchange needs. ABS puts the supplied state, mobile URI and provider callback
   URI in its session; a separate ten-minute LRU maps state to the mobile URI.
4. The redirect whitelist is exact membership in
   `authOpenIDMobileRedirectURIs`, default `['audiobookshelf://oauth']`. A
   single-element `['*']` accepts any URI (a `*` alongside other entries does
   not). A disallowed or missing URI returns **400**, plain text
   **`Invalid redirect_uri`**, before redirecting to the provider. Only this
   exact response triggers Rede's retry with `audiobookshelf://oauth`; it tells
   the user their admin can add `rede://oauth` to the allowed URIs. Other errors
   do not trigger fallback. Both schemes are registered by Rede on macOS.
5. ABS sends the provider an HTTP(S) redirect URI ending in
   `/auth/openid/mobile-redirect` (including its configured
   `authOpenIDSubfolderForRedirectURLs`). After the person signs in, the
   provider lands there with `code` and `state`. ABS checks its state LRU,
   consumes that entry, and returns **302** to
   `<chosen-scheme>://oauth?code=<encoded-code>&state=<encoded-state>`.
   This route forwards code and state, **not tokens**. Missing/unknown state
   returns 400 `State parameter mismatch`. The route does not forward provider
   `error` parameters; denial may arrive with `code=undefined`, which Rede
   treats as cancellation. A provider may instead leave an error in the browser;
   Cancel and the five-minute deadline cover a redirect that never arrives.
6. Rede checks both the exact redirect target and the single, matching state
   before accepting the code. Mismatches are ignored with a value-free log.
   It then requests **`GET /auth/openid/callback?code=<code>&state=<state>&code_verifier=<verifier>`**
   using its **original native ABS session cookies**, not browser cookies.
   ABS inserts the verifier into that session, and passport/openid-client
   exchanges the code at the provider, resending the saved provider redirect
   URI. No separate native token endpoint or `redirect_uri` parameter is needed
   on this request. Without the original session, the route returns 400
   `No session` when `code_verifier` is present.
7. On success `handleLoginSuccessBasedOnCookie` recognizes the
   `auth_method=openid-mobile` cookie as API-based and returns **200 JSON** in
   the same shape as password login: `user`, `user.accessToken`,
   `user.refreshToken`, `userDefaultLibraryId`, `serverSettings`. This code
   path ignores `x-return-tokens`; that header controls **only POST /login**.
   Rede installs the same token hook and uses the existing keyring, refresh and
   source machinery. The legacy `user.token` is ignored.

The callback's error branch has a server quirk: it checks
`req.session[sessionKey]?.mobile === true`, but authorization stores the URI
string there. Some callback failures therefore redirect to `/login?...` rather
than returning an API error status. Rede never follows those redirects or
quotes their bodies; it reports a typed OpenID server error. It also redacts
pending transactions and never logs state, verifier, code, cookies or provider
URLs. A pending transaction is kept only in the keyring so a macOS launch URL
can finish the same exchange after quitting; cancellation, expiry and successful
exchange clear it. Pending credentials are never written to preferences.
