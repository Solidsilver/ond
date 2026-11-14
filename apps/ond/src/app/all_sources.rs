//! Concurrent sources and the scope's in-memory projection. Each source owns
//! its bridge (including cancellation, reconnect and its folder watcher),
//! session and data versions. A source message is checked before entering its
//! handler and handled against only that source's data. The shared projection
//! is rebuilt from versioned snapshots; a scope change never cancels work.

use super::{Action, Data, OndApp};
use crate::bridge::{Bridge, LibraryMsg, Msg};
use crate::nav::Route;
use crate::source::identity;
use accounts::{AccountId, Scope};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct AllSources {
    pub bridges: BTreeMap<AccountId, Bridge>,
    pub signing_in: Option<AccountId>,
    pub data: BTreeMap<AccountId, Data>,
    pub downloads: BTreeMap<AccountId, super::Downloads>,
    pub pending_playlist: Option<PendingMixedPlaylist>,
    pub ond_generation: u64,
    pub ond_playlists: Vec<crate::source::Playlist>,
    pub ond_tracks: BTreeMap<String, Arc<Vec<library::Track>>>,
    pub ond_entries: HashMap<String, crate::bridge::OndPlaylistEntries>,
    revisions: BTreeMap<AccountId, u64>,
    artist_catalog: crate::memo::Memo<Vec<(AccountId, u64, u64)>, Vec<crate::source::artists::MergedArtist>>,
    pub(super) projection: Option<Vec<(AccountId, u64)>>,
    scope: Scope,
}

#[derive(Debug, Clone)]
pub enum ScopeAction {
    One(AccountId),
    All,
    Chip(shell::kit::SourceChip),
}

impl OndApp {
    pub(super) fn load_favorites(&mut self, starred: crate::favorites::Starred, ratings: Vec<(String, u8)>) {
        if let Some(account) = self.session.focus.filter(|a| self.multi.bridges.contains_key(a)) {
            self.favorites.load_source(account, starred, ratings);
        } else {
            self.favorites.load(starred, ratings);
        }
    }

    pub(super) fn downloads_for(&self, account: AccountId) -> Option<&super::Downloads> {
        if self.downloads.owner == Some(account) { Some(&self.downloads) } else { self.multi.downloads.get(&account) }
    }

    fn ensure_source_downloads(&mut self, account: AccountId, settings: &crate::settings::Settings, bridge: &Bridge) {
        let network = settings.primary().is_some_and(|s| crate::source::Kind::of(s.kind).capabilities.downloads);
        if network && self.downloads.owner != Some(account) && !self.multi.downloads.contains_key(&account) {
            let downloads = self.downloads.for_source(settings, bridge);
            self.multi.downloads.insert(account, downloads);
        }
    }

    pub(super) fn start_sources(&mut self) {
        let settings = self.prefs.snapshot();
        if !settings.sources.is_empty() {
            self.analysis.runner.use_account_stores();
        }
        for entry in &settings.sources {
            let mut own = settings.clone();
            own.make_primary(entry.id);
            own.signed_out = entry.signed_out || (settings.signed_out && settings.account() == Some(entry.id));
            self.start_source(own);
        }
        self.session.focus = settings.scope.single().filter(|a| self.session.accounts.contains_key(a)).or_else(|| settings.sources.first().map(|e| e.id));
    }

    pub(super) fn start_source(&mut self, settings: crate::settings::Settings) {
        let Some(account) = settings.account() else { return };
        self.art.add_account(account);
        let network = settings.primary().is_none_or(|s| crate::source::Kind::of(s.kind).capabilities.network);
        let connect = !(network && (self.session.work_offline || settings.signed_out));
        let bridge = if let Some(b) = self.multi.bridges.get(&account) {
            b.end_session();
            b.clone()
        } else {
            let library = if self.bridge.storage_account() == Some(account) {
                self.bridge.library_handle()
            } else {
                Arc::new(library::Library::in_memory().expect("empty mirror"))
            };
            self.bridge.fork_source(account, library)
        };
        self.session.accounts.insert(
            account,
            super::session::SourceSession {
                settings: Some(settings.clone()),
                connecting: connect,
                sign_in: (network && settings.signed_out).then(|| "Sign in again".into()),
                ..Default::default()
            },
        );
        self.multi.bridges.insert(account, bridge.clone());
        self.ensure_source_downloads(account, &settings, &bridge);
        let keep = (network && self.session.work_offline).then(|| self.offline_filter(account));
        bridge.open_source(settings, keep, connect);
    }

    pub(super) fn connect_source_form(&mut self, settings: crate::settings::Settings, password: String) {
        let account = settings.account().expect("sign-in account");
        let bridge = if let Some(b) = self.multi.bridges.get(&account) {
            b.end_session();
            b.clone()
        } else {
            let library = Arc::new(library::Library::in_memory().expect("empty mirror"));
            self.bridge.fork_source(account, library)
        };
        self.session.accounts.insert(account, super::session::SourceSession { settings: Some(settings.clone()), connecting: true, ..Default::default() });
        self.multi.bridges.insert(account, bridge.clone());
        self.ensure_source_downloads(account, &settings, &bridge);
        self.multi.signing_in = Some(account);
        bridge.connect(settings, Some(password));
    }

    fn offline_filter(&self, account: AccountId) -> crate::local::Playable {
        crate::local::Playable {
            account: Some(account),
            downloads_account: self.downloads_for(account).and_then(|d| d.owner),
            downloads: self.downloads_for(account).map(|d| d.snap.clone()).unwrap_or_default(),
            cache: self.cache.clone(),
            transcode: self.output.transcode(),
        }
    }

    /// Source controls and their results use that source's bridge and state.
    /// The browsing data stays merged except while ingesting a library batch.
    pub(super) fn with_source<R>(&mut self, account: AccountId, f: impl FnOnce(&mut Self) -> R) -> R {
        let bridge = self.multi.bridges[&account].clone();
        let old = std::mem::replace(&mut self.bridge, bridge);
        let focus = self.session.focus.replace(account);
        let mut previous_downloads = None;
        let previous_owner = self.downloads.owner;
        let swapped_downloads = self.downloads.owner != Some(account) && self.multi.downloads.contains_key(&account);
        if swapped_downloads {
            let downloads = self.multi.downloads.remove(&account).expect("registered downloads");
            let previous = std::mem::replace(&mut self.downloads, downloads);
            if let Some(owner) = previous_owner {
                self.multi.downloads.insert(owner, previous);
            } else {
                previous_downloads = Some(previous);
            }
        }
        let result = f(self);
        if swapped_downloads {
            let previous = previous_owner.and_then(|a| self.multi.downloads.remove(&a)).or(previous_downloads).expect("previous downloads");
            let downloads = std::mem::replace(&mut self.downloads, previous);
            if self.session.accounts.contains_key(&account) {
                self.multi.downloads.insert(account, downloads);
            }
        }
        self.bridge = old;
        self.session.focus = focus.filter(|a| self.session.accounts.contains_key(a)).or_else(|| self.session.accounts.keys().next().copied());
        result
    }

    pub(super) fn source_message(&mut self, account: AccountId, session: u64, msg: Msg) {
        let Some(bridge) = self.multi.bridges.get(&account) else { return };
        if bridge.session() != session {
            return;
        }
        self.with_source(account, |app| {
            app.handle_source_result(account, msg);
        });
    }

    fn handle_source_result(&mut self, account: AccountId, msg: Msg) {
        match msg {
            Msg::Scoped { session, message } if session == self.bridge.session() => self.handle_source_result(account, *message),
            Msg::Scoped { .. } => {}
            Msg::Library(LibraryMsg::SearchIndex { .. }) => self.handle(msg),
            Msg::Library(_) => {
                let refresh_ond = matches!(msg, Msg::Library(LibraryMsg::AllTracks(_)));
                let changed = matches!(
                    msg,
                    Msg::Library(
                        LibraryMsg::Albums(_)
                            | LibraryMsg::Artists(_)
                            | LibraryMsg::AllTracks(_)
                            | LibraryMsg::AlbumUpdated(_)
                            | LibraryMsg::Tracks { .. }
                            | LibraryMsg::PlaylistTracks { .. }
                            | LibraryMsg::Newest(_)
                            | LibraryMsg::Playlists(_)
                            | LibraryMsg::Saved(_)
                            | LibraryMsg::Credited { .. }
                            | LibraryMsg::ArtistMbids(_)
                    )
                );
                let mut data = self.multi.data.remove(&account).unwrap_or_default();
                data.account = Some(account);
                let projection = std::mem::replace(&mut self.data, data);
                self.handle(msg);
                self.multi.data.insert(account, std::mem::replace(&mut self.data, projection));
                if changed {
                    *self.multi.revisions.entry(account).or_default() += 1;
                    self.project_sources();
                }
                if refresh_ond {
                    self.refresh_ond_playlists();
                }
            }
            Msg::ConnectFailed { error, .. } => {
                self.set_offline();
                self.session.connecting = false;
                self.session.sign_in = Some(error.clone());
                self.session.unreachable = false;
                self.session.sync.label = "Sign in again".into();
                if self.multi.signing_in == Some(account) {
                    self.multi.signing_in = None;
                    if let Some(form) = self.login.as_mut() {
                        form.busy = false;
                        form.error = Some(error);
                    }
                }
                self.server_gone();
            }
            Msg::Unreachable { username, error, .. } => {
                let login = self.login.take();
                self.unreachable(username, error);
                self.login = login;
            }
            Msg::Connected { source, stores, settings, server, password_saved, .. } => {
                let owns_form = self.multi.signing_in == Some(account) || self.onboarding.as_ref().is_some_and(|o| o.folders.account == Some(account));
                if let Some(stores) = stores {
                    let store = self.bridge.install_storage(account, stores, &[account]);
                    self.bridge.load_analyses(store);
                }
                if !password_saved {
                    self.show_notice("Ond couldn't save your password in the system keychain.");
                }
                let name = settings.source_name().unwrap_or(&server).to_owned();
                self.session.source = Some(source.clone());
                self.session.online = true;
                self.session.connecting = false;
                self.session.unreachable = false;
                self.session.sign_in = None;
                self.session.server_info = server.clone();
                self.session.server_label =
                    if !source.capabilities().network || settings.username().is_empty() { name } else { format!("{} · {server}", settings.username()) };
                self.session.settings = Some(*settings.clone());
                if let Some(entry) = settings.primary().cloned() {
                    self.prefs.edit(|s| {
                        if let Some(existing) = s.sources.iter_mut().find(|e| e.id == account) {
                            existing.signed_out = false;
                        } else {
                            s.sources.push(entry.clone());
                        }
                        s.signed_out = false;
                    });
                }
                if self.multi.signing_in == Some(account) {
                    self.multi.signing_in = None;
                }
                self.refresh_ond_playlists();
                self.session.sync.begin();
                self.bridge.sync_source(&source);
                self.bridge.fetch_home(source.clone());
                self.bridge.fetch_starred(Some(source.clone()));
                self.art.add_source(source.clone());
                self.now_playing.add_source(source.clone());
                self.refresh_analysis_sources();
                self.connect_downloads(&source);
                if owns_form {
                    self.login = None;
                    self.source_added(settings.server(), account);
                }
                self.with_player(|p, cx| {
                    p.forget_next();
                    p.resume_held(cx);
                    p.sync_next(cx);
                });
            }
            other => self.handle(other),
        }
    }

    pub(super) fn project_sources(&mut self) {
        let scope = self.session.scope.clone();
        let key: Vec<_> =
            self.multi.data.keys().filter(|a| scope.includes(**a)).map(|a| (*a, self.multi.revisions.get(a).copied().unwrap_or_default())).collect();
        if self.multi.projection.as_ref() == Some(&key) && self.multi.scope == scope {
            return;
        }
        let sources: Vec<&Data> = key.iter().map(|(a, _)| &self.multi.data[a]).collect();
        self.data.merge_sources(&sources);
        self.data.playlists.retain(|p| !p.id.starts_with("ond:"));
        self.data.playlists.extend(
            self.multi
                .ond_playlists
                .iter()
                .filter(|p| self.multi.ond_tracks.get(&p.id).is_none_or(|ts| ts.iter().any(|t| identity::account(&t.id).is_some_and(|a| scope.includes(a)))))
                .cloned(),
        );
        self.data.playlist_tracks.extend(self.multi.ond_tracks.iter().map(|(id, ts)| (id.clone(), ts.clone())));
        let artist_key = self.multi.data.iter().map(|(a, d)| (*a, d.version, d.artists_version)).collect();
        let catalog = self.multi.artist_catalog.get(artist_key, || Data::artist_catalog(&self.multi.data.values().collect::<Vec<_>>()));
        self.data.remember_artist_members(&catalog);
        self.multi.projection = Some(key);
        self.multi.scope = scope.clone();
        self.favorites.set_scope(scope);
        self.search.stale = true;
        // Advance the generation now so an earlier build can't install over the
        // new scope and `is_current()` flips false until the rebuild lands.
        self.search.generation += 1;
    }

    pub(super) fn apply_scope(&mut self, action: ScopeAction) {
        let mut scope = self.session.scope.clone();
        match action {
            ScopeAction::One(account) if self.session.accounts.contains_key(&account) => scope.choose_one(account),
            ScopeAction::One(_) => return,
            ScopeAction::All => scope.choose_all(),
            ScopeAction::Chip(shell::kit::SourceChip::All) => scope.clear_chips(),
            // From one source, a box in the switcher's menu adds another to
            // it (or, on the one itself, takes it out, leaving every source),
            // which is All sources narrowed to them.
            ScopeAction::Chip(shell::kit::SourceChip::Toggle(account)) if let Some(one) = scope.single() => {
                scope.choose_all();
                if account != one {
                    scope.toggle_chip(one);
                    scope.toggle_chip(account);
                }
            }
            ScopeAction::Chip(shell::kit::SourceChip::Toggle(account)) => scope.toggle_chip(account),
        }
        self.session.scope = scope.clone();
        self.prefs.edit(|s| s.scope = scope.clone());
        if let Some(account) = scope.single() {
            self.session.focus = Some(account);
        }
        self.project_sources();
        let route = crate::nav::route_in_scope(self.ui.nav.current(), &scope, &self.data);
        if &route != self.ui.nav.current() {
            self.scope_section_to_top(&route);
        }
        self.ui.nav.go(route);
        self.ui.track_sel = None;
        self.ui.reveal = None;
        self.ui.palette = None;
        self.on_arrive();
    }

    fn scope_section_to_top(&self, route: &Route) {
        match route {
            Route::Albums => {
                super::alpha::to_top(&self.ctx, ("grid", route));
                super::alpha::to_top(&self.ctx, ("albums-list", route));
            }
            Route::Artists => {
                super::alpha::to_top(&self.ctx, "artists-grid");
                super::alpha::to_top(&self.ctx, "artists");
            }
            Route::Songs => super::alpha::to_top(&self.ctx, "songs"),
            Route::Playlists => super::alpha::to_top(&self.ctx, "playlists"),
            _ => {}
        }
    }

    pub(super) fn sign_out_source(&mut self, account: AccountId) {
        self.bridge.end_session();
        self.forget_source_requests(account);
        self.session.source = None;
        self.session.online = false;
        self.session.connecting = false;
        self.session.unreachable = false;
        self.session.sign_in = Some("Sign in again".into());
        self.session.sync = Default::default();
        self.analysis.runner.cancel_account(account);
        self.refresh_analysis_sources();
        self.art.offline_source(account);
        self.now_playing.remove_source(account);
        if let Some(store) = &self.downloads.store {
            store.set_source(None);
        }
        let settings = self.session.settings.clone();
        if let Some(settings) = settings {
            std::thread::spawn(move || settings.forget_password());
        }
        self.prefs.edit(|s| {
            if let Some(e) = s.sources.iter_mut().find(|e| e.id == account) {
                e.signed_out = true;
            }
        });
        self.ui.overlay = None;
        if self.session.accounts.len() == 1 {
            self.with_player(|p, cx| p.reset(cx));
            self.favorites = Default::default();
            self.lyrics.clear();
            self.song_stats.clear();
            let settings = self.session.settings.clone().unwrap_or_default();
            self.login =
                Some(super::session::LoginForm { server: settings.server().to_owned(), username: settings.username().to_owned(), ..Default::default() });
            self.onboarding = Some(Default::default());
            self.session.work_offline = false;
            self.prefs.edit(|s| s.work_offline = false);
        } else {
            self.with_player(|p, cx| p.suspend_account(account, cx));
        }
    }

    pub(super) fn remove_live_source(&mut self, account: AccountId) {
        let removed = self.prefs.get(|s| s.source(account).cloned());
        let Some(bridge) = self.multi.bridges.remove(&account) else { return };
        bridge.end_session();
        self.forget_source_requests(account);
        self.with_player(|p, cx| p.remove_account(account, cx));
        self.multi.data.remove(&account);
        if self.downloads.owner == Some(account) {
            self.downloads.remove_files(&bridge);
        }
        if let Some(mut downloads) = self.multi.downloads.remove(&account) {
            downloads.remove_files(&bridge);
        }
        self.multi.revisions.remove(&account);
        self.session.accounts.remove(&account);
        self.session.scope.drop_account(account);
        if self.session.focus == Some(account) {
            self.session.focus = self.session.accounts.keys().next().copied();
        }
        self.prefs.edit(|s| {
            s.remove_source(account);
            s.scope.drop_account(account);
        });
        self.favorites.remove_account(account);
        self.lyrics.retain(|id, _| identity::account(id) != Some(account));
        self.song_stats.retain(|id, _| identity::account(id) != Some(account));
        self.art.remove_source(account);
        self.now_playing.remove_source(account);
        if self.multi.pending_playlist.as_ref().is_some_and(|p| p.source.account == account || p.entries.iter().any(|k| k.account == account)) {
            self.multi.pending_playlist = None;
        }
        if self.bridge.storage_account() == Some(account) {
            self.bridge.forget_account(account);
        }
        self.analysis.runner.remove_account(account);
        self.analysis.records.retain(|id, _| identity::account(id) != Some(account));
        self.analysis.version += 1;
        bridge.forget_account(account);
        self.multi.projection = None;
        self.project_sources();
        self.refresh_ond_playlists();
        let route = crate::nav::route_in_scope(self.ui.nav.current(), &self.session.scope, &self.data);
        if &route != self.ui.nav.current() {
            self.scope_section_to_top(&route);
        }
        self.ui.nav.go(route);
        self.ui.overlay = None;
        if let Some(entry) = removed {
            let name = entry.name.clone().unwrap_or_else(|| super::sidebar::default_name(&entry));
            self.show_notice(super::sources::removed_notice(&name, entry.kind));
        }
        if self.session.accounts.is_empty() {
            self.login = Some(super::session::LoginForm::default());
            self.onboarding = Some(Default::default());
        }
    }

    pub(super) fn fetch_scope_tracks(&self, key: crate::bridge::ListKey, ids: Vec<String>) {
        let jobs: Vec<_> = ids
            .into_iter()
            .filter_map(|id| {
                let account = identity::account(&id)?;
                let bridge = self.multi.bridges.get(&account)?.clone();
                let session = bridge.session();
                let source = self.session.get(account)?.live().cloned();
                Some((account, session, bridge, source, id))
            })
            .collect();
        let post = self.bridge.poster();
        self.bridge.runtime().spawn(async move {
            let mut parts = Vec::new();
            for (account, session, bridge, source, id) in jobs {
                let tracks = bridge.resolve_album(source, id).await;
                parts.push((account, session, tracks));
            }
            post(Msg::Library(LibraryMsg::ScopeTracks { key, parts }));
        });
    }

    pub(super) fn refresh_analysis_sources(&self) {
        let sources: BTreeMap<_, _> = self.session.accounts.iter().filter_map(|(a, s)| Some((*a, s.live()?.clone()))).collect();
        self.analysis.runner.set_server(Some(Arc::new(move |key| sources.get(&key.account)?.download_url(&key.item))));
    }

    pub(super) fn load_source_local(&mut self, account: AccountId) {
        let keep = (self.session.work_offline && self.session.source_caps().network).then(|| self.offline_filter(account));
        self.bridge.load_local(keep);
    }

    pub(super) fn work_offline_for_sources(&mut self, on: bool) {
        self.session.work_offline = on;
        self.prefs.edit(|s| s.work_offline = on);
        let accounts: Vec<_> = self.session.accounts.iter().filter(|(_, s)| s.source_caps().network).map(|(a, _)| *a).collect();
        for account in accounts {
            self.with_source(account, |app| {
                app.bridge.end_session();
                app.forget_source_requests(account);
                app.session.source = None;
                app.session.online = false;
                app.session.connecting = !on && app.session.sign_in.is_none();
                app.session.unreachable = false;
                if let Some(settings) = app.session.settings.clone() {
                    let keep = on.then(|| app.offline_filter(account));
                    app.bridge.load_local(keep);
                    if !on && app.session.sign_in.is_none() {
                        app.bridge.connect(settings, None);
                    }
                }
                app.art.offline_source(account);
                app.now_playing.remove_source(account);
                if let Some(store) = &app.downloads.store {
                    store.set_source(None);
                }
            });
        }
        self.refresh_analysis_sources();
        self.with_player(|p, cx| {
            p.forget_next();
            p.resume_held(cx);
            p.sync_next(cx);
        });
    }

    pub(super) fn forget_source_requests(&mut self, account: AccountId) {
        self.lyrics.retain(|id, fetch| identity::account(id) != Some(account) || *fetch != crate::lyrics::Fetch::Loading);
        self.song_stats.retain(|id, stats| identity::account(id) != Some(account) || stats.is_some());
        self.pending.retain(|p| !super::data::belongs_to(&p.key, account));
        self.downloads.pending_pins.retain(|(key, _)| !super::data::belongs_to(key, account));
    }
}

pub(super) fn action_account(action: &Action) -> Option<AccountId> {
    use super::PlaylistAction as P;
    let id = match action {
        Action::Go(Route::Album(id) | Route::Playlist { id, .. })
        | Action::Album(id, _)
        | Action::Playlist(id, _)
        | Action::NeedSongStats(id)
        | Action::Rate(id, _)
        | Action::ToggleStar { id, .. } => id,
        Action::Go(Route::Artist { id: Some(id), .. }) | Action::Artist(_, Some(id), _) => id,
        Action::NeedLyrics(t) => &t.id,
        Action::Offline(super::OfflineAction::Unpin(offline::Pin::Album(id) | offline::Pin::Playlist(id) | offline::Pin::Track(id))) => id,
        Action::Offline(super::OfflineAction::Keep(super::Keep::Album(id) | super::Keep::Playlist(id), _)) => id,
        Action::Offline(super::OfflineAction::Keep(super::Keep::Tracks(ts), _)) => &ts.first()?.id,
        Action::Reload(crate::bridge::ListKey::Album(id) | crate::bridge::ListKey::Playlist(id)) => id,
        Action::PlaylistEdit(
            P::Add { id, .. }
            | P::Rename { id, .. }
            | P::Delete { id }
            | P::Remove { id, .. }
            | P::Restore { id, .. }
            | P::Move { id, .. }
            | P::ExportM3u { id, .. },
        ) => id,
        Action::PlaylistEdit(P::Create { tracks, .. }) => &tracks.first()?.id,
        _ => return None,
    };
    identity::account(id)
}

/// A server playlist cannot accept these foreign keys. The UI can show the
/// source playlist's name and ask to copy these entries into an Ond playlist.
pub struct PendingMixedPlaylist {
    pub source: accounts::ItemKey,
    pub name: String,
    pub entries: Vec<accounts::ItemKey>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{PlaylistAction, fixture};
    use crate::bridge::ListKey;
    use crate::favorites::StarKind;
    use crate::player::QueueAction;
    use crate::settings::{Settings, SourceEntry};
    use crate::source::{Src, TrackKey, subsonic::SubsonicSource};
    use library::{AlbumTracks, Artist};
    use serde_json::json;
    use std::time::Duration;
    use testkit::{Answer, Deadline, JsonFake};

    fn account(n: u32) -> AccountId {
        format!("00000000-0000-4000-8000-{n:012x}").parse().unwrap()
    }
    fn key(a: AccountId, id: &str) -> String {
        TrackKey::new(a, id).to_string()
    }

    fn server() -> JsonFake {
        JsonFake::start(|request| {
            let body = match request.path() {
                "/rest/getPlaylists" => json!({"playlists":{"playlist":[{"id":"1","name":"Server list","owner":"me","songCount":1}]}}),
                "/rest/getPlaylist" => json!({"playlist":{"id":"1","name":"Server list","entry":[{"id":"1","albumId":"1","title":"Existing","duration":60}]}}),
                "/rest/getAlbum" => json!({"album":{"id":"1","name":"Album","songCount":1,"song":[{"id":"1","title":"Song","duration":60}]}}),
                "/rest/getSong" => json!({"song":{"id":"1","title":"Song"}}),
                "/rest/getLyricsBySongId" => json!({"lyricsList":{"structuredLyrics":[{"line":[{"value":"Words"}]}]}}),
                "/rest/getStarred2" => json!({"starred2":{}}),
                _ => json!({}),
            };
            let mut response = json!({"status":"ok","version":"1.16.1"});
            response.as_object_mut().unwrap().extend(body.as_object().unwrap().clone());
            Answer::ok(json!({"subsonic-response":response}).to_string())
        })
    }

    fn rig(dir: &std::path::Path, one: &JsonFake, two: &JsonFake) -> OndApp {
        rig_with(&egui::Context::default(), dir, one, two)
    }

    fn rig_with(ctx: &egui::Context, dir: &std::path::Path, one: &JsonFake, two: &JsonFake) -> OndApp {
        let mut app = fixture::populated(ctx, dir);
        app.data.clear();
        app.player = Default::default();
        app.favorites = Default::default();
        app.session = Default::default();
        let entries = [SourceEntry::subsonic(account(1), one.base(), "me"), SourceEntry::subsonic(account(2), two.base(), "me")];
        app.prefs.edit(|s| {
            s.sources = entries.to_vec();
            s.scope = Scope::all();
        });
        for entry in entries {
            let a = entry.id;
            let source: Src = Arc::new(SubsonicSource::new(a, subsonic::Client::new(&entry.server, subsonic::Credentials::new("me", "p")).unwrap()));
            let stores = crate::storage::Stores::in_memory().unwrap();
            let mut album = fixture::album(0);
            album.id = "1".into();
            album.artist = Some("The Björk & Friends".into());
            album.artist_ref = Some("1".into());
            album.artists.clear();
            album.song_count = 1;
            let mut track = crate::test_tracks::track("1", "1", if a == account(1) { "One" } else { "Two" });
            track.artist_ref = Some("1".into());
            stores.library.upsert_album_tracks(AlbumTracks { album, tracks: vec![track] }).unwrap();
            stores
                .library
                .replace_artists(vec![Artist {
                    id: "1".into(),
                    name: if a == account(1) { "The Björk & Friends".into() } else { "bjork and friends".into() },
                    sort: None,
                    album_count: 1,
                    cover_art: None,
                }])
                .unwrap();
            let bridge = app.bridge.fork_source(a, stores.library.clone());
            bridge.configure_storage(dir.to_owned(), dir.join("art"), Some(a), &stores.analyses);
            let mut settings = Settings { sources: app.prefs.snapshot().sources, ..Default::default() };
            settings.make_primary(a);
            app.session
                .accounts
                .insert(a, super::super::session::SourceSession { source: Some(source), online: true, settings: Some(settings), ..Default::default() });
            app.multi.bridges.insert(a, bridge.clone());
            bridge.load_local(None);
        }
        app.session.focus = Some(account(1));
        pump(&mut app, |a| a.data.tracks.all.len() == 2 && a.data.albums.len() == 2 && a.data.artists.len() == 1);
        app
    }

    fn pump(app: &mut OndApp, mut done: impl FnMut(&OndApp) -> bool) {
        let deadline = Deadline::new();
        while !done(app) {
            assert!(!deadline.passed(), "source results did not settle");
            if let Ok(msg) = app.inbox.recv_timeout(Duration::from_millis(10)) {
                app.handle(msg);
            }
        }
    }

    fn inject(app: &mut OndApp, msg: Msg) {
        let msg = match msg {
            Msg::Source { account, session, message } => Msg::Source { account, session, message: Box::new(identity::message(account, *message)) },
            msg => msg,
        };
        app.handle(msg);
    }

    #[test]
    fn two_sources_with_the_same_bare_id_keep_tracks_albums_routes_and_writes_apart() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        assert_eq!(app.data.track(&key(account(1), "1")).unwrap().title, "One");
        assert_eq!(app.data.track(&key(account(2), "1")).unwrap().title, "Two");
        app.rebuild_search();
        let index = app.search.index.as_ref().unwrap();
        assert_eq!(index.albums.len(), 2);
        assert_eq!(index.tracks.len(), 2);
        let id = key(account(2), "1");
        app.apply(Action::ToggleStar { kind: StarKind::Song, id: id.clone(), track: app.data.track(&id).cloned().map(Box::new), name: None });
        app.apply(Action::Rate(id.clone(), 4));
        app.apply(Action::NeedLyrics(Box::new(app.data.track(&id).unwrap().clone())));
        app.apply(Action::Go(Route::Album(id.clone())));
        pump(&mut app, |_| {
            two.count("/rest/star") == 1 && two.count("/rest/setRating") == 1 && two.count("/rest/getLyricsBySongId") == 1 && two.count("/rest/getAlbum") == 1
        });
        assert_eq!(one.count("/rest/star"), 0);
        assert_eq!(one.count("/rest/setRating"), 0);
        assert_eq!(one.count("/rest/getAlbum"), 0);
        for request in two.seen() {
            if request.path() == "/rest/getAlbum" {
                assert!(request.target.split('?').nth(1).unwrap().split('&').any(|p| p == "id=1"));
            }
        }
    }

    #[test]
    fn an_offline_source_leaves_its_queue_entries_while_another_source_plays() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        inject(
            &mut app,
            Msg::Source { account: account(1), session: 0, message: Box::new(Msg::Unreachable { session: 0, username: "me".into(), error: "away".into() }) },
        );
        let offline = app.data.track(&key(account(1), "1")).unwrap().clone();
        let online = app.data.track(&key(account(2), "1")).unwrap().clone();
        assert!(app.source(1, None, &offline, None).is_none());
        assert!(app.source(2, None, &online, None).unwrap().url.starts_with(two.base()));
        app.apply(Action::Tracks { tracks: vec![offline, online], source: crate::queue::Source::Added, action: QueueAction::Add });
        assert_eq!(app.player.queue.upcoming.len(), 2);
        assert_eq!(app.player.queue.upcoming[0].key.account, account(1));
        assert_eq!(app.player.queue.upcoming[1].key.account, account(2));
        let first = app.player.queue.upcoming[0].uid;
        app.apply(Action::Queue(crate::player::QueueEdit::JumpTo(first)));
        assert_eq!(app.player.queue.current.as_ref().unwrap().key.account, account(2));
        assert!(app.player.playing_from.as_ref().unwrap().url.starts_with(two.base()));
        assert_eq!(app.player.queue.history[0].key.account, account(1));
    }

    #[test]
    fn signing_out_of_one_source_drops_only_its_late_results() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        let before = app.multi.bridges[&account(1)].session();
        app.apply(Action::ForSource(account(1), Box::new(Action::SignOut)));
        let album = || {
            let mut a = fixture::album(1);
            a.id = "late".into();
            a
        };
        for a in [account(1), account(2)] {
            inject(&mut app, Msg::Source { account: a, session: before, message: Box::new(LibraryMsg::Albums(vec![album()]).into()) });
        }
        assert!(app.data.album(&key(account(1), "late")).is_none());
        assert!(app.data.album(&key(account(2), "late")).is_some());
        assert!(app.session.get(account(2)).unwrap().online);
        assert!(app.session.get(account(1)).unwrap().sign_in.is_some());
    }

    #[test]
    fn late_saved_favourites_cannot_overwrite_live_results_from_the_same_source() {
        use crate::bridge::Saved;
        use crate::favorites::{SavedStarred, Starred};
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        let song = crate::test_tracks::track("1", "1", "Live favourite");
        inject(
            &mut app,
            Msg::Source {
                account: account(1),
                session: 0,
                message: Box::new(Msg::Starred { starred: Starred { songs: vec![song.clone()], ..Default::default() }, ratings: vec![("1".into(), 5)] }),
            },
        );
        for a in [account(1), account(2)] {
            inject(
                &mut app,
                Msg::Source {
                    account: a,
                    session: 0,
                    message: Box::new(
                        LibraryMsg::Saved(Box::new(Saved {
                            starred: Some(SavedStarred { starred: Starred::default(), ratings: vec![("1".into(), 2)] }),
                            ..Default::default()
                        }))
                        .into(),
                    ),
                },
            );
        }
        assert!(app.favorites.is_on(StarKind::Song, &key(account(1), "1")));
        assert_eq!(app.favorites.rating(&key(account(1), "1")), 5);
        assert_eq!(app.favorites.rating(&key(account(2), "1")), 2, "another source still accepts its saved snapshot");
        inject(
            &mut app,
            Msg::Source {
                account: account(2),
                session: 0,
                message: Box::new(Msg::Starred { starred: Starred { songs: vec![song], ..Default::default() }, ratings: vec![("1".into(), 4)] }),
            },
        );
        assert!(app.favorites.is_on(StarKind::Song, &key(account(2), "1")));
        assert_eq!(app.favorites.rating(&key(account(1), "1")), 5);
        assert_eq!(app.favorites.rating(&key(account(2), "1")), 4);
    }

    #[test]
    fn a_box_toggled_from_one_source_shows_both_and_on_that_source_shows_all() {
        let dir = tempfile::tempdir().unwrap();
        let (one, two) = (server(), server());
        let mut app = rig(dir.path(), &one, &two);
        let toggle = |app: &mut OndApp, a| app.apply(Action::Scope(ScopeAction::Chip(shell::kit::SourceChip::Toggle(a))));
        app.apply(Action::Scope(ScopeAction::One(account(2))));
        assert_eq!(app.data.albums.len(), 1);
        toggle(&mut app, account(1));
        let scope = app.prefs.snapshot().scope;
        assert!(scope.is_all() && scope.chip_selected(account(1)) && scope.chip_selected(account(2)), "{scope:?}");
        assert_eq!(app.data.albums.len(), 2);
        toggle(&mut app, account(1));
        assert_eq!(app.data.albums.len(), 1, "and out again, through the chips' own rule");
        assert!(app.prefs.snapshot().scope.chip_selected(account(2)));
        app.apply(Action::Scope(ScopeAction::One(account(2))));
        toggle(&mut app, account(2));
        assert_eq!(app.prefs.snapshot().scope, Scope::all());
    }

    #[test]
    fn scope_changes_preserve_details_or_return_to_their_own_section() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        let album = Route::Album(key(account(1), "1"));
        assert_eq!(crate::nav::route_in_scope(&album, &Scope::all(), &app.data), album);
        assert_eq!(crate::nav::route_in_scope(&album, &Scope::only(account(2)), &app.data), Route::Albums);
        let artist = Route::Artist { name: "The Björk & Friends".into(), id: Some(key(account(1), "1")) };
        let mapped = crate::nav::route_in_scope(&artist, &Scope::only(account(2)), &app.data);
        assert!(matches!(mapped, Route::Artist { id: Some(id), .. } if id == key(account(2), "1")));
        assert_eq!(
            crate::nav::route_in_scope(&Route::Playlist { id: key(account(1), "absent"), name: "Gone".into() }, &Scope::only(account(2)), &app.data),
            Route::Playlists
        );
        app.apply(Action::Scope(ScopeAction::One(account(2))));
        assert_eq!(app.data.albums.len(), 1);
        assert!(app.data.track(&key(account(1), "1")).is_none());
        assert_eq!(app.prefs.snapshot().scope.single(), Some(account(2)));
        app.rebuild_search();
        assert_eq!(app.search.index.as_ref().unwrap().tracks.len(), 1);
        app.apply(Action::Scope(ScopeAction::All));
        app.apply(Action::Prefs(super::super::PrefsAction::Scope(ScopeAction::Chip(shell::kit::SourceChip::Toggle(account(1))))));
        assert_eq!(app.data.albums.len(), 1);
        assert!(app.session.scope.chip_selected(account(1)));
        app.apply(Action::Scope(ScopeAction::Chip(shell::kit::SourceChip::All)));
        assert_eq!(app.data.albums.len(), 2);
    }

    #[test]
    fn merged_artists_keep_each_sources_albums_and_musicbrainz_survives_scope_changes() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        let merged = &app.data.merged_artists[0];
        assert_eq!(merged.members.len(), 2);
        assert_eq!(merged.albums.len(), 2);
        for a in [account(1), account(2)] {
            let name = if a == account(1) { "Stage name" } else { "Another language" };
            inject(
                &mut app,
                Msg::Source { account: a, session: 0, message: Box::new(LibraryMsg::ArtistMbids(vec![("1".into(), "shared-mbid".into())]).into()) },
            );
            inject(
                &mut app,
                Msg::Source {
                    account: a,
                    session: 0,
                    message: Box::new(
                        LibraryMsg::Artists(vec![Artist { id: "1".into(), name: name.into(), sort: None, album_count: 1, cover_art: None }]).into(),
                    ),
                },
            );
        }
        assert_eq!(app.data.artists.len(), 1);
        let route = Route::Artist { name: "Stage name".into(), id: Some(key(account(1), "1")) };
        app.ui.nav.go(route);
        app.apply(Action::Scope(ScopeAction::One(account(2))));
        assert!(matches!(app.ui.nav.current(), Route::Artist { id: Some(id), .. } if id == &key(account(2), "1")));
        assert_eq!(app.data.merged_artists[0].albums.keys().copied().collect::<Vec<_>>(), [account(2)]);
    }

    #[test]
    fn adding_a_foreign_song_offers_an_ond_copy_and_the_copy_resolves_both_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        let track = app.data.track(&key(account(2), "1")).unwrap().clone();
        app.apply(Action::PlaylistEdit(PlaylistAction::Add { id: key(account(1), "1"), tracks: vec![track] }));
        pump(&mut app, |a| a.multi.pending_playlist.is_some());
        assert_eq!(one.count("/rest/updatePlaylist"), 0);
        assert_eq!(two.count("/rest/updatePlaylist"), 0);
        assert_eq!(app.multi.pending_playlist.as_ref().unwrap().entries, [TrackKey::new(account(1), "1"), TrackKey::new(account(2), "1")]);
        app.apply(Action::PlaylistEdit(PlaylistAction::AcceptMixed(true)));
        pump(&mut app, |a| a.multi.ond_tracks.values().any(|ts| ts.len() == 2));
        let (id, tracks) = app.multi.ond_tracks.iter().next().map(|(id, ts)| (id.clone(), ts.clone())).unwrap();
        let mut credentials = Vec::new();
        for (i, track) in tracks.iter().enumerate() {
            let resolved = app.source(i as u64, None, track, None).unwrap();
            let expected = if i == 0 { one.base() } else { two.base() };
            assert!(resolved.url.starts_with(expected));
            let url = url::Url::parse(&resolved.url).unwrap();
            credentials.extend(url.query_pairs().filter(|(key, _)| ["u", "t", "s"].contains(&key.as_ref())).map(|(_, value)| value.into_owned()));
        }
        assert_eq!(credentials.len(), 6);
        app.apply(Action::Playlist(id.clone(), QueueAction::Add));
        assert_eq!(app.player.queue.upcoming.iter().map(|i| i.key.account).collect::<Vec<_>>(), [account(1), account(2)]);
        let destination = dir.path().join("mixed.m3u");
        app.handle(Msg::M3uPathChosen { id, path: destination.clone() });
        pump(&mut app, |a| a.ui.toasts.texts().contains(&"Exported mixed.m3u"));
        let exported = std::fs::read_to_string(destination).unwrap();
        assert_eq!(exported, "#EXTM3U\n", "server entries must not export authenticated URLs");
        for credential in credentials {
            assert!(!exported.contains(&credential));
        }
        for query in ["u=", "t=", "s="] {
            assert!(!exported.contains(query));
        }
    }

    #[test]
    fn playlist_edits_preserve_deleted_files_and_use_the_displayed_rows_raw_indices() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        let missing = TrackKey::new(account(1), "deleted-file");
        let first = TrackKey::new(account(1), "1");
        let second = TrackKey::new(account(2), "1");
        let entries = vec![missing.clone(), first.clone(), second.clone(), missing.clone()];
        let store = app.bridge.ond_store().clone();
        let playlist = store.create_playlist("Missing files", &entries).unwrap();
        let id = format!("ond:{}", playlist.id);
        app.refresh_ond_playlists();
        pump(&mut app, |a| a.multi.ond_entries.contains_key(&id));
        assert_eq!(app.multi.ond_entries[&id].visible, [1, 2]);
        app.apply(Action::PlaylistEdit(PlaylistAction::Move { id: id.clone(), from: 1, to: 0 }));
        pump(&mut app, |a| a.multi.ond_entries[&id].entries[1] == second);
        assert_eq!(store.playlist(&playlist.id).unwrap().unwrap().entries, [missing.clone(), second.clone(), first.clone(), missing.clone()]);
        let removed = app.multi.ond_tracks[&id][0].clone();
        app.apply(Action::PlaylistEdit(PlaylistAction::Remove { id: id.clone(), index: 0 }));
        pump(&mut app, |a| a.multi.ond_entries[&id].entries.len() == 3);
        assert_eq!(store.playlist(&playlist.id).unwrap().unwrap().entries, [missing.clone(), first.clone(), missing.clone()]);
        let after = app.multi.ond_tracks[&id].iter().map(|t| t.id.clone()).collect();
        app.apply(Action::PlaylistEdit(PlaylistAction::Restore { id: id.clone(), index: 0, track: Box::new(removed), after }));
        pump(&mut app, |a| a.multi.ond_entries[&id].entries.len() == 4);
        assert_eq!(store.playlist(&playlist.id).unwrap().unwrap().entries, [missing.clone(), second, first, missing]);

        // Startup's empty mirrors must retain raw entries even with no rows to draw.
        for (account, bridge) in &mut app.multi.bridges {
            *bridge = bridge.fork_source(*account, Arc::new(library::Library::in_memory().unwrap()));
        }
        app.refresh_ond_playlists();
        pump(&mut app, |a| a.multi.ond_tracks[&id].is_empty());
        app.apply(Action::PlaylistEdit(PlaylistAction::Remove { id: id.clone(), index: 0 }));
        app.apply(Action::PlaylistEdit(PlaylistAction::Move { id, from: 0, to: 1 }));
        assert_eq!(store.playlist(&playlist.id).unwrap().unwrap().entries.len(), 4);
    }

    #[test]
    fn removing_one_source_prunes_its_queue_and_leaves_the_other_library_and_queue() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        app.apply(Action::Tracks { tracks: app.data.tracks.all.as_ref().clone(), source: crate::queue::Source::Added, action: QueueAction::Add });
        app.apply(Action::Sources(super::super::SourceAction::Remove(account(1))));
        assert_eq!(app.player.queue.upcoming.len(), 1);
        assert_eq!(app.player.queue.upcoming[0].key.account, account(2));
        assert_eq!(app.data.albums.len(), 1);
        assert!(app.session.get(account(2)).unwrap().online);
        assert!(!app.multi.bridges.contains_key(&account(1)));
    }

    #[test]
    fn sync_progress_never_rebuilds_the_merged_library() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        let albums = app.data.albums.clone();
        let tracks = app.data.tracks.all.clone();
        inject(&mut app, Msg::Source { account: account(2), session: 0, message: Box::new(LibraryMsg::SyncProgress(100).into()) });
        assert!(Arc::ptr_eq(&albums, &app.data.albums));
        assert!(Arc::ptr_eq(&tracks, &app.data.tracks.all));
        assert_eq!(app.session.get(account(2)).unwrap().sync.label, "Syncing… 100");
        assert!(!app.session.get(account(1)).unwrap().sync.albums);
        inject(
            &mut app,
            Msg::Source {
                account: account(2),
                session: 0,
                message: Box::new(LibraryMsg::Credited { name: "Credit".into(), album_ids: vec!["1".into()] }.into()),
            },
        );
        assert!(Arc::ptr_eq(&albums, &app.data.albums));
        assert!(Arc::ptr_eq(&tracks, &app.data.tracks.all));
        assert_eq!(app.data.credited["Credit"], [key(account(2), "1")]);
        assert!(!super::super::data::belongs_to(&ListKey::Album(key(account(2), "1")), account(1)));
    }
    #[test]
    fn a_source_ending_while_two_requests_wait_does_not_cancel_the_other_response() {
        let dir = tempfile::tempdir().unwrap();
        let gated = || {
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let wait = std::sync::Mutex::new(wait);
            let server = JsonFake::start(move |_| {
                wait.lock().unwrap().recv_timeout(Deadline::new().left()).unwrap();
                Answer::ok(
                    json!({"subsonic-response":{"status":"ok","version":"1.16.1","lyricsList":{"structuredLyrics":[{"line":[{"value":"Words"}]}]}}})
                        .to_string(),
                )
            });
            (server, release)
        };
        let (one, release_one) = gated();
        let (two, release_two) = gated();
        let mut app = rig(dir.path(), &one, &two);
        for account in [account(1), account(2)] {
            app.apply(Action::NeedLyrics(Box::new(app.data.track(&key(account, "1")).unwrap().clone())));
        }
        testkit::wait_until("both sources' requests", || one.count("/rest/getLyricsBySongId") == 1 && two.count("/rest/getLyricsBySongId") == 1);
        app.apply(Action::ForSource(account(1), Box::new(Action::SignOut)));
        release_one.send(()).unwrap();
        release_two.send(()).unwrap();
        pump(&mut app, |a| matches!(a.lyrics.get(&key(account(2), "1")), Some(crate::lyrics::Fetch::Done(Some(_)))));
        assert!(!app.lyrics.contains_key(&key(account(1), "1")));
    }

    #[test]
    fn a_missing_folder_does_not_stop_another_folder_and_work_offline_keeps_local_watchers_live() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("music");
        std::fs::create_dir(&ready).unwrap();
        std::fs::write(ready.join("song.flac"), crate::test_tracks::flac(&[("TITLE", "Song"), ("ALBUM", "Album"), ("ARTIST", "Artist")], None)).unwrap();
        let mut entries = vec![
            SourceEntry::local(
                account(1),
                Some("Missing".into()),
                accounts::SourceColor::new(0),
                vec![crate::settings::FolderEntry::new(dir.path().join("missing"))],
            ),
            SourceEntry::local(account(2), Some("Ready".into()), accounts::SourceColor::new(1), vec![crate::settings::FolderEntry::new(&ready)]),
        ];
        let mut network = SourceEntry::subsonic(account(3), "http://127.0.0.1:9", "me");
        network.signed_out = true;
        entries.push(network);
        let settings = Settings { sources: entries, ..Default::default() };
        std::fs::write(dir.path().join("settings.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
        let paths = crate::settings::Paths {
            config_file: dir.path().join("settings.json"),
            library_db: dir.path().join("library.redb"),
            stream_cache: dir.path().join("streams"),
            art_cache: dir.path().join("art"),
            offline: dir.path().join("offline"),
        };
        let mut app = OndApp::build(&egui::Context::default(), paths, Box::new(fixture::NullOutput)).unwrap();
        pump(&mut app, |a| a.data.tracks.all.len() == 1 && !a.session.get(account(1)).unwrap().folder_missing.is_empty());
        assert!(!app.signing_in());
        let local_epochs = [app.multi.bridges[&account(1)].session(), app.multi.bridges[&account(2)].session()];
        app.apply(Action::Offline(super::super::OfflineAction::WorkOffline(true)));
        assert_eq!([app.multi.bridges[&account(1)].session(), app.multi.bridges[&account(2)].session()], local_epochs);
        assert!(app.session.get(account(2)).unwrap().online);
        assert_eq!(app.multi.bridges[&account(3)].session(), 1);
        std::fs::write(ready.join("next.flac"), crate::test_tracks::flac(&[("TITLE", "Next"), ("ALBUM", "Album"), ("ARTIST", "Artist")], None)).unwrap();
        pump(&mut app, |a| a.data.tracks.all.len() == 2);
        assert!(app.session.work_offline);
    }
    #[test]
    fn artists_without_server_ids_merge_by_name_without_inventing_a_writable_id() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        for a in [account(1), account(2)] {
            let mut album = fixture::album(0);
            album.id = "1".into();
            album.artist_ref = None;
            album.artists.clear();
            album.artist = Some(if a == account(1) { "The Björk & Friends".into() } else { "bjork and friends".into() });
            inject(&mut app, Msg::Source { account: a, session: 0, message: Box::new(LibraryMsg::Albums(vec![album]).into()) });
            inject(&mut app, Msg::Source { account: a, session: 0, message: Box::new(LibraryMsg::Artists(Vec::new()).into()) });
        }
        app.rebuild_search();
        let artists = &app.search.index.as_ref().unwrap().artists;
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0].id, None);
        assert_eq!(artists[0].albums, 2);
        assert_eq!(app.data.merged_artists[0].albums.len(), 2);
    }

    #[test]
    fn an_artist_page_tries_the_photo_from_its_own_source_then_each_other_sources() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        for a in [account(1), account(2)] {
            let mut album = fixture::album(0);
            album.id = "1".into();
            album.artist = Some("Same".into());
            album.artist_ref = Some("1".into());
            album.artists.clear();
            inject(&mut app, Msg::Source { account: a, session: 0, message: Box::new(LibraryMsg::Albums(vec![album]).into()) });
            let artist = Artist { id: "1".into(), name: "Same".into(), sort: None, album_count: 1, cover_art: Some("ar-1".into()) };
            inject(&mut app, Msg::Source { account: a, session: 0, message: Box::new(LibraryMsg::Artists(vec![artist]).into()) });
        }
        app.rebuild_search();
        let index = app.search.index.as_ref().unwrap();
        for (first, second) in [(account(2), account(1)), (account(1), account(2))] {
            let (photos, _) = super::super::artists::faces(index, &app.data, Some(&key(first, "1")), "Same");
            assert_eq!(photos, [key(first, "ar-1"), key(second, "ar-1")], "opened from {first}");
        }
    }

    #[test]
    fn every_signed_out_source_opens_its_cached_mirror_before_any_connection() {
        let dir = tempfile::tempdir().unwrap();
        let mut entries = Vec::new();
        for a in [account(1), account(2)] {
            let stores = crate::storage::open(dir.path(), Some(a), &mut Vec::new()).unwrap();
            let mut album = fixture::album(0);
            album.id = "1".into();
            album.song_count = 1;
            stores.library.upsert_album_tracks(AlbumTracks { album, tracks: vec![crate::test_tracks::track("1", "1", "Cached")] }).unwrap();
            let mut entry = SourceEntry::subsonic(a, "http://127.0.0.1:9", "me");
            entry.signed_out = true;
            entries.push(entry);
        }
        let settings = Settings { sources: entries, ..Default::default() };
        std::fs::write(dir.path().join("settings.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
        let paths = crate::settings::Paths {
            config_file: dir.path().join("settings.json"),
            library_db: dir.path().join("library.redb"),
            stream_cache: dir.path().join("streams"),
            art_cache: dir.path().join("art"),
            offline: dir.path().join("offline"),
        };
        let mut app = OndApp::build(&egui::Context::default(), paths, Box::new(fixture::NullOutput)).unwrap();
        pump(&mut app, |a| a.data.tracks.all.len() == 2);
        assert!(app.data.track(&key(account(1), "1")).is_some());
        assert!(app.data.track(&key(account(2), "1")).is_some());
        assert!(app.session.accounts.values().all(|s| !s.online && !s.connecting));
    }
    #[test]
    fn work_offline_is_restored_for_servers_even_when_a_folder_is_first() {
        let dir = tempfile::tempdir().unwrap();
        let folder = SourceEntry::local(account(1), None, accounts::SourceColor::new(0), vec![crate::settings::FolderEntry::new(dir.path().join("missing"))]);
        let server = SourceEntry::subsonic(account(2), "http://127.0.0.1:9", "me");
        let settings = Settings { sources: vec![folder, server], work_offline: true, ..Default::default() };
        std::fs::write(dir.path().join("settings.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
        let paths = crate::settings::Paths {
            config_file: dir.path().join("settings.json"),
            library_db: dir.path().join("library.redb"),
            stream_cache: dir.path().join("streams"),
            art_cache: dir.path().join("art"),
            offline: dir.path().join("offline"),
        };
        let app = OndApp::build(&egui::Context::default(), paths, Box::new(fixture::NullOutput)).unwrap();
        assert!(app.session.work_offline);
        assert!(!app.session.get(account(2)).unwrap().connecting);
        assert!(app.session.get(account(1)).unwrap().connecting);
    }

    #[test]
    fn equally_named_artists_with_different_musicbrainz_ids_play_their_own_albums() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        for a in [account(1), account(2)] {
            let mut album = fixture::album(0);
            album.id = "1".into();
            album.artist = Some("Same".into());
            album.artist_ref = Some("1".into());
            album.artists.clear();
            album.song_count = 1;
            inject(&mut app, Msg::Source { account: a, session: 0, message: Box::new(LibraryMsg::Albums(vec![album]).into()) });
            inject(&mut app, Msg::Source { account: a, session: 0, message: Box::new(LibraryMsg::ArtistMbids(vec![("1".into(), a.to_string())]).into()) });
            inject(
                &mut app,
                Msg::Source {
                    account: a,
                    session: 0,
                    message: Box::new(
                        LibraryMsg::Artists(vec![Artist { id: "1".into(), name: "Same".into(), sort: None, album_count: 1, cover_art: None }]).into(),
                    ),
                },
            );
        }
        assert_eq!(app.data.artists.len(), 2);
        app.apply(Action::Artist("Same".into(), Some(key(account(2), "1")), QueueAction::Add));
        pump(&mut app, |a| !a.player.queue.upcoming.is_empty());
        assert_eq!(app.player.queue.upcoming.len(), 1);
        assert_eq!(app.player.queue.upcoming[0].key.account, account(2));
    }

    #[test]
    fn stars_and_ratings_for_a_secondary_source_without_those_capabilities_go_to_ond() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let mut app = rig(dir.path(), &one, &two);
        let mut source = crate::source::fake::Fake::new(crate::source::Capabilities::local());
        source.account = account(2);
        app.session.accounts.get_mut(&account(2)).unwrap().source = Some(Arc::new(source));
        let id = key(account(2), "1");
        app.apply(Action::ToggleStar { kind: StarKind::Song, id: id.clone(), track: app.data.track(&id).cloned().map(Box::new), name: None });
        app.apply(Action::Rate(id, 4));
        let store = app.bridge.ond_store().clone();
        let own = TrackKey::new(account(2), "1");
        testkit::wait_until("secondary source's Ond metadata", || store.is_starred(StarKind::Song, &own).unwrap() && store.rating(&own).unwrap() == 4);
        assert!(!store.is_starred(StarKind::Song, &TrackKey::new(account(1), "1")).unwrap());
        assert_eq!(one.count("/rest/star") + two.count("/rest/star"), 0);
        assert_eq!(one.count("/rest/setRating") + two.count("/rest/setRating"), 0);
    }

    fn frame(ctx: &egui::Context, app: &mut OndApp, events: Vec<egui::Event>) {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1360.0, 860.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| app.main_ui(ui));
        out.textures_delta.clear();
    }

    fn type_in_palette(ctx: &egui::Context, app: &mut OndApp, query: &str) {
        let cmd_k = egui::Event::Key { key: egui::Key::K, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
        app.ui.palette = None;
        frame(ctx, app, vec![cmd_k]);
        assert!(app.ui.palette.is_some(), "palette should stay open while the old search index stands in");
        frame(ctx, app, Vec::new());
        frame(ctx, app, vec![egui::Event::Text(query.into())]);
        frame(ctx, app, Vec::new());
    }

    #[test]
    fn search_stays_available_across_scope_changes_and_filters_out_of_scope_hits() {
        let dir = tempfile::tempdir().unwrap();
        let one = server();
        let two = server();
        let ctx = egui::Context::default();
        let mut app = rig_with(&ctx, dir.path(), &one, &two);

        // Add one unique artist/album/track to each account alongside `rig`'s shared artist `"1"`.
        for (a, ar_id, ar_name, shared_ar_name, al_id, al_name, tr_id, tr_name) in [
            (account(1), "ar-1", "Miles Davis", "The Björk & Friends", "al-1", "Kind of Blue", "t-1", "So What"),
            (account(2), "ar-2", "John Coltrane", "bjork and friends", "al-2", "Love Supreme", "t-2", "Acknowledgement"),
        ] {
            let mut base_album = fixture::album(0);
            base_album.id = "1".into();
            base_album.artist = Some("The Björk & Friends".into());
            base_album.artist_ref = Some("1".into());
            base_album.artists.clear();
            base_album.song_count = 1;
            let mut base_track = crate::test_tracks::track("1", "1", if a == account(1) { "One" } else { "Two" });
            base_track.artist_ref = Some("1".into());

            let mut album = fixture::album(1);
            album.id = al_id.into();
            album.name = al_name.into();
            album.artist = Some(ar_name.into());
            album.artist_ref = Some(ar_id.into());
            album.artists.clear();
            album.song_count = 1;
            let mut track = crate::test_tracks::track(tr_id, al_id, tr_name);
            track.artist = Some(ar_name.into());
            track.artist_ref = Some(ar_id.into());

            let shared = Artist { id: "1".into(), name: shared_ar_name.into(), sort: None, album_count: 1, cover_art: None };
            let unique = Artist { id: ar_id.into(), name: ar_name.into(), sort: None, album_count: 1, cover_art: None };
            inject(&mut app, Msg::Source { account: a, session: 0, message: Box::new(LibraryMsg::Albums(vec![base_album, album]).into()) });
            inject(
                &mut app,
                Msg::Source {
                    account: a,
                    session: 0,
                    message: Box::new(LibraryMsg::AllTracks(Box::new(crate::tracks::TrackSet::new(vec![base_track, track]))).into()),
                },
            );
            inject(&mut app, Msg::Source { account: a, session: 0, message: Box::new(LibraryMsg::Artists(vec![shared, unique]).into()) });
        }

        app.rebuild_search();
        assert!(app.search.is_current());

        // Narrow to source 2 before any background rebuild arrives.
        app.apply(Action::Scope(ScopeAction::One(account(2))));
        assert!(app.search.index.is_some());
        assert!(!app.search.is_current());

        // Out-of-scope album, track, and artist yield no hits and cannot be acted on.
        let enter = egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let alt_enter = egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::ALT };

        type_in_palette(&ctx, &mut app, "Kind of Blue");
        assert!(app.ui.palette.as_ref().unwrap().results.top.is_none());
        assert_eq!(app.ui.palette.as_ref().unwrap().results.len(), 0);
        frame(&ctx, &mut app, vec![enter.clone()]);
        assert_eq!(app.ui.nav.current(), &crate::nav::Route::Recent);

        type_in_palette(&ctx, &mut app, "So What");
        assert_eq!(app.ui.palette.as_ref().unwrap().results.len(), 0);
        frame(&ctx, &mut app, vec![alt_enter.clone()]);
        assert!(app.player.queue.upcoming.is_empty());

        type_in_palette(&ctx, &mut app, "Miles Davis");
        assert_eq!(app.ui.palette.as_ref().unwrap().results.len(), 0);
        frame(&ctx, &mut app, vec![enter.clone()]);
        assert_eq!(app.ui.nav.current(), &crate::nav::Route::Recent);

        // Merged artist whose stale entry held account(1)'s id still opens account(2)'s member.
        type_in_palette(&ctx, &mut app, "friends");
        assert!(app.ui.palette.as_ref().unwrap().results.len() > 0);
        frame(&ctx, &mut app, vec![enter.clone()]);
        assert_eq!(app.ui.nav.current(), &crate::nav::Route::Artist { name: "bjork and friends".into(), id: Some(key(account(2), "1")) });

        // In-scope album resolves immediately from the kept index.
        type_in_palette(&ctx, &mut app, "Love Supreme");
        frame(&ctx, &mut app, vec![enter.clone()]);
        assert_eq!(app.ui.nav.current(), &crate::nav::Route::Album(key(account(2), "al-2")));

        // Start a background rebuild, change scope via chip toggle mid-flight, and verify stale builds drop.
        app.apply(Action::Scope(ScopeAction::All));
        app.after_messages();
        let stale_gen = app.search.generation;

        app.apply(Action::Scope(ScopeAction::Chip(shell::kit::SourceChip::Toggle(account(1)))));
        assert!(app.search.index.is_some());
        assert!(!app.search.is_current());
        assert!(app.search.generation > stale_gen);

        // Immediately after a chip toggle, searching finds source 1's item and skips source 2's.
        type_in_palette(&ctx, &mut app, "Love Supreme");
        assert_eq!(app.ui.palette.as_ref().unwrap().results.len(), 0);
        type_in_palette(&ctx, &mut app, "Kind of Blue");
        assert!(app.ui.palette.as_ref().unwrap().results.len() > 0);
        frame(&ctx, &mut app, vec![enter]);
        assert_eq!(app.ui.nav.current(), &crate::nav::Route::Album(key(account(1), "al-1")));

        let empty = Box::new(crate::search::SearchIndex::new(&[], &[], std::iter::empty()));
        app.handle(LibraryMsg::SearchIndex { generation: stale_gen, index: empty });
        assert!(!app.search.is_current(), "stale rebuild from before scope change must be dropped");
        assert!(!app.search.index.as_ref().unwrap().albums.is_empty());

        app.after_messages();
        let deadline = Deadline::new();
        while !app.search.is_current() {
            assert!(!deadline.passed(), "rebuilt search index did not land");
            if let Ok(msg) = app.inbox.recv_timeout(Duration::from_millis(10)) {
                app.handle(msg);
                app.after_messages();
            }
        }
        assert!(app.search.index.as_ref().unwrap().albums.iter().all(|al| identity::account(&al.id) == Some(account(1))));
    }

    #[test]
    fn navidrome_all_sources_artist_pages_merge_across_servers_and_ignore_blank_mbids() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        fn query_param(target: &str, name: &str) -> String {
            target
                .split_once('?')
                .into_iter()
                .flat_map(|(_, q)| q.split('&'))
                .filter_map(|p| p.split_once('='))
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.replace('+', " ").replace("%20", " ").replace("%26", "&"))
                .unwrap_or_default()
        }

        fn navidrome_server(
            artists: Vec<serde_json::Value>,
            albums: Vec<serde_json::Value>,
            songs_by_album: std::collections::HashMap<&'static str, Vec<serde_json::Value>>,
            search_songs: std::collections::HashMap<&'static str, Vec<serde_json::Value>>,
        ) -> JsonFake {
            JsonFake::start(move |request| {
                let path = request.path();
                let body = match path {
                    "/rest/getArtists" => json!({"artists":{"index":[{"name":"A","artist":artists}]}}),
                    "/rest/getAlbumList2" => {
                        let offset: usize = query_param(&request.target, "offset").parse().unwrap_or(0);
                        let page = if offset == 0 { albums.clone() } else { Vec::new() };
                        json!({"albumList2":{"album":page}})
                    }
                    "/rest/getAlbum" => {
                        let id = query_param(&request.target, "id");
                        let album = albums.iter().find(|a| a["id"].as_str() == Some(id.as_str())).cloned().unwrap_or_else(|| json!({"id":id,"name":id}));
                        let songs = songs_by_album.get(id.as_str()).cloned().unwrap_or_default();
                        let mut obj = album.as_object().unwrap().clone();
                        obj.insert("song".into(), json!(songs));
                        json!({"album":obj})
                    }
                    "/rest/search3" => {
                        let query = query_param(&request.target, "query");
                        let songs = search_songs.get(query.as_str()).cloned().unwrap_or_default();
                        json!({"searchResult3":{"song":songs}})
                    }
                    "/rest/getPlaylists" => json!({"playlists":{"playlist":[]}}),
                    "/rest/getStarred2" => json!({"starred2":{}}),
                    _ => json!({}),
                };
                let mut response = json!({"status":"ok","version":"1.16.1"});
                response.as_object_mut().unwrap().extend(body.as_object().unwrap().clone());
                Answer::ok(json!({"subsonic-response":response}).to_string())
            })
        }

        fn navi_artist(id: &str, name: &str, album_count: u32, mbid: Option<&str>) -> serde_json::Value {
            let mut v = json!({"id":id,"name":name,"albumCount":album_count});
            if let Some(m) = mbid {
                v.as_object_mut().unwrap().insert("musicBrainzId".into(), json!(m));
            }
            v
        }

        fn navi_album(id: &str, name: &str, artist_id: &str, artist_name: &str) -> serde_json::Value {
            json!({
                "id": id,
                "name": name,
                "artist": artist_name,
                "artistId": artist_id,
                "artists": [{"id": artist_id, "name": artist_name}],
                "songCount": 1,
                "duration": 200
            })
        }

        fn navi_song(id: &str, album_id: &str, album: &str, artist_id: &str, artist_name: &str, title: &str) -> serde_json::Value {
            json!({
                "id": id,
                "parent": album_id,
                "albumId": album_id,
                "album": album,
                "artist": artist_name,
                "artistId": artist_id,
                "artists": [{"id": artist_id, "name": artist_name}],
                "title": title,
                "duration": 200
            })
        }

        let one = navidrome_server(
            vec![
                navi_artist("ar-jg", "Josh Garrels", 2, Some("11111111-1111-1111-1111-111111111111")),
                navi_artist("ar-ss", "Shane & Shane", 1, Some("22222222-2222-2222-2222-222222222222")),
                navi_artist("ar-jw", "John Williams", 1, Some("33333333-3333-3333-3333-333333333333")),
                navi_artist("ar-stand", "Stand", 1, Some("")),
                navi_artist("ar-ps", "Paul Simon", 1, Some("")),
                navi_artist("ar-man", "Manhattan", 1, Some("")),
            ],
            vec![
                navi_album("al-a-jg-1", "Love & War & the Sea in Between", "ar-jg", "Josh Garrels"),
                navi_album("al-a-jg-2", "Jacaranda", "ar-jg", "Josh Garrels"),
                navi_album("al-a-ss-1", "Psalms II", "ar-ss", "Shane & Shane"),
                navi_album("al-a-jw-1", "Star Wars", "ar-jw", "John Williams"),
                navi_album("al-a-stand", "Stand", "ar-stand", "Stand"),
                navi_album("al-a-ps", "Graceland", "ar-ps", "Paul Simon"),
                navi_album("al-a-man", "Manhattan", "ar-man", "Manhattan"),
            ],
            [
                ("al-a-jg-1", vec![navi_song("s-a-jg-1", "al-a-jg-1", "Love & War & the Sea in Between", "ar-jg", "Josh Garrels", "White Owl")]),
                ("al-a-jg-2", vec![navi_song("s-a-jg-2", "al-a-jg-2", "Jacaranda", "ar-jg", "Josh Garrels", "A Long Way")]),
                ("al-a-ss-1", vec![navi_song("s-a-ss-1", "al-a-ss-1", "Psalms II", "ar-ss", "Shane & Shane", "Psalm 23")]),
                ("al-a-jw-1", vec![navi_song("s-a-jw-1", "al-a-jw-1", "Star Wars", "ar-jw", "John Williams", "Main Title")]),
                ("al-a-stand", vec![navi_song("s-a-stand", "al-a-stand", "Stand", "ar-stand", "Stand", "Track 1")]),
                ("al-a-ps", vec![navi_song("s-a-ps", "al-a-ps", "Graceland", "ar-ps", "Paul Simon", "Graceland")]),
                ("al-a-man", vec![navi_song("s-a-man", "al-a-man", "Manhattan", "ar-man", "Manhattan", "Manhattan")]),
            ]
            .into(),
            [("Josh Garrels", vec![navi_song("s-a-jg-1", "al-a-jg-1", "Love & War & the Sea in Between", "ar-jg", "Josh Garrels", "White Owl")])].into(),
        );
        let two = navidrome_server(
            vec![
                navi_artist("fr-jg", "Josh Garrels", 2, Some("")),
                navi_artist("fr-ss", "Shane & Shane", 1, None),
                navi_artist("fr-soj", "Sojourn Music", 1, Some("")),
                navi_artist("fr-jw", "John Williams", 1, Some("44444444-4444-4444-4444-444444444444")),
                navi_artist("fr-va", "Various Artists", 1, Some("")),
            ],
            vec![
                navi_album("al-b-jg-1", "Home", "fr-jg", "Josh Garrels"),
                navi_album("al-b-jg-2", "Chrysaline", "fr-jg", "Josh Garrels"),
                navi_album("al-b-ss-1", "The Worship Initiative", "fr-ss", "Shane & Shane"),
                navi_album("al-b-soj-1", "Over the Grave", "fr-soj", "Sojourn Music"),
                navi_album("al-b-jw-1", "Cavatina", "fr-jw", "John Williams"),
                navi_album("al-b-va-1", "Hymns Compilation", "fr-va", "Various Artists"),
            ],
            [
                ("al-b-jg-1", vec![navi_song("s-b-jg-1", "al-b-jg-1", "Home", "fr-jg", "Josh Garrels", "Home")]),
                ("al-b-jg-2", vec![navi_song("s-b-jg-2", "al-b-jg-2", "Chrysaline", "fr-jg", "Josh Garrels", "Chrysaline")]),
                ("al-b-ss-1", vec![navi_song("s-b-ss-1", "al-b-ss-1", "The Worship Initiative", "fr-ss", "Shane & Shane", "Come Behold")]),
                ("al-b-soj-1", vec![navi_song("s-b-soj-1", "al-b-soj-1", "Over the Grave", "fr-soj", "Sojourn Music", "Over the Grave")]),
                ("al-b-jw-1", vec![navi_song("s-b-jw-1", "al-b-jw-1", "Cavatina", "fr-jw", "John Williams", "Cavatina")]),
                ("al-b-va-1", vec![navi_song("s-b-va-1", "al-b-va-1", "Hymns Compilation", "fr-jg", "Josh Garrels", "Amazing Grace")]),
            ]
            .into(),
            [(
                "Josh Garrels",
                vec![
                    navi_song("s-b-jg-1", "al-b-jg-1", "Home", "fr-jg", "Josh Garrels", "Home"),
                    navi_song("s-b-jg-2", "al-b-jg-2", "Chrysaline", "fr-jg", "Josh Garrels", "Chrysaline"),
                    navi_song("s-b-va-1", "al-b-va-1", "Hymns Compilation", "fr-jg", "Josh Garrels", "Amazing Grace"),
                ],
            )]
            .into(),
        );

        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::builder().with_size(egui::vec2(1360.0, 960.0)).allow_missing_glyphs().build_ui_state(
            |ui, app: &mut Option<OndApp>| {
                if let Some(app) = app {
                    app.main_ui(ui);
                }
            },
            None,
        );

        let ctx = h.ctx.clone();
        let mut app = fixture::populated(&ctx, dir.path());
        app.data.clear();
        app.player = Default::default();
        app.favorites = Default::default();
        app.session = Default::default();
        let mut e1 = SourceEntry::subsonic(account(1), one.base(), "me");
        e1.name = Some("MyNavi".into());
        let mut e2 = SourceEntry::subsonic(account(2), two.base(), "friend");
        e2.name = Some("Friend".into());
        let entries = [e1, e2];
        app.prefs.edit(|s| {
            s.sources = entries.to_vec();
            s.scope = Scope::all();
        });
        let mut stores_by_account = std::collections::HashMap::new();
        for entry in entries {
            let a = entry.id;
            let source: Src = Arc::new(SubsonicSource::new(a, subsonic::Client::new(&entry.server, subsonic::Credentials::new("me", "p")).unwrap()));
            let stores = crate::storage::Stores::in_memory().unwrap();
            let bridge = app.bridge.fork_source(a, stores.library.clone());
            bridge.configure_storage(dir.path().to_owned(), dir.path().join("art"), Some(a), &stores.analyses);
            let mut settings = Settings { sources: app.prefs.snapshot().sources, ..Default::default() };
            settings.make_primary(a);
            app.session
                .accounts
                .insert(a, super::super::session::SourceSession { source: Some(source.clone()), online: true, settings: Some(settings), ..Default::default() });
            app.multi.bridges.insert(a, bridge.clone());
            stores_by_account.insert(a, stores);
            bridge.sync_source(&source);
        }
        app.session.focus = Some(account(1));
        pump(&mut app, |a| a.data.albums.len() == 13 && a.data.tracks.all.len() == 13 && !a.data.artists.is_empty());
        *h.state_mut() = Some(app);

        let drain = |h: &mut Harness<'_, Option<OndApp>>| {
            for _ in 0..4 {
                while let Ok(msg) = h.state_mut().as_mut().unwrap().inbox.try_recv() {
                    h.state_mut().as_mut().unwrap().handle(msg);
                }
                h.state_mut().as_mut().unwrap().after_messages();
                h.run_steps(1);
            }
        };

        let visit_album_then_click_artist = |h: &mut Harness<'_, Option<OndApp>>, album_key: String, artist: &str| {
            {
                let app = h.state_mut().as_mut().unwrap();
                app.apply(Action::Go(Route::Album(album_key)));
            }
            drain(h);
            let play_top = h.get_by_label("Play").rect().top();
            h.query_all_by_label(artist)
                .filter(|n| n.rect().bottom() < play_top)
                .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
                .unwrap_or_else(|| panic!("no album header artist link for {artist}"))
                .click();
            drain(h);
            let deadline = Deadline::new();
            while !h.state().as_ref().unwrap().data.credited.contains_key(artist) {
                assert!(!deadline.passed(), "credited lookup for {artist} did not settle");
                if let Ok(msg) = h.state_mut().as_mut().unwrap().inbox.recv_timeout(Duration::from_millis(10)) {
                    h.state_mut().as_mut().unwrap().handle(msg);
                    h.state_mut().as_mut().unwrap().after_messages();
                    h.run_steps(1);
                }
            }
            drain(h);
        };

        // Bug 1 + Bug 2: Friend album -> click "Josh Garrels" -> merged 4 albums under MyNavi & Friend,
        // none of MyNavi's unrelated albums, and "Home"/"Chrysaline" NOT under "Also credited on".
        visit_album_then_click_artist(&mut h, key(account(2), "al-b-jg-1"), "Josh Garrels");
        h.get_by_label("4 albums");
        h.get_by_label("MyNavi");
        h.get_by_label("Friend");
        h.get_by_label_contains("Love & War & the Sea in Between by Josh Garrels");
        h.get_by_label_contains("Jacaranda by Josh Garrels");
        assert_eq!(h.query_all_by_label_contains("Home by Josh Garrels").count(), 1);
        assert_eq!(h.query_all_by_label_contains("Chrysaline by Josh Garrels").count(), 1);
        assert!(h.query_by_label_contains("Stand").is_none());
        assert!(h.query_by_label_contains("Paul Simon").is_none());
        assert!(h.query_by_label_contains("Manhattan").is_none());
        // Genuine guest track credit still renders under "Also credited on", while own albums do not.
        h.get_by_label("Also credited on");
        h.get_by_label_contains("Hymns Compilation by Various Artists");

        // Same artist from MyNavi album -> same merged view, Friend's copies NOT relegated to "Also credited on".
        visit_album_then_click_artist(&mut h, key(account(1), "al-a-jg-1"), "Josh Garrels");
        h.get_by_label("4 albums");
        h.get_by_label("MyNavi");
        h.get_by_label("Friend");
        assert_eq!(h.query_all_by_label_contains("Home by Josh Garrels").count(), 1);
        assert_eq!(h.query_all_by_label_contains("Chrysaline by Josh Garrels").count(), 1);
        assert!(h.query_by_label_contains("Stand").is_none());

        // Shane & Shane (MyNavi has MBID, Friend omitted musicBrainzId) from Friend album -> merged 2 albums, no "Also credited on".
        visit_album_then_click_artist(&mut h, key(account(2), "al-b-ss-1"), "Shane & Shane");
        h.get_by_label("2 albums");
        h.get_by_label("MyNavi");
        h.get_by_label("Friend");
        assert_eq!(h.query_all_by_label_contains("Psalms II by Shane & Shane").count(), 1);
        assert_eq!(h.query_all_by_label_contains("The Worship Initiative by Shane & Shane").count(), 1);
        assert!(h.query_by_label_contains("Stand").is_none());
        assert!(h.query_by_label("Also credited on").is_none());

        // Sojourn Music (Friend only, empty MBID) -> 1 album, no MyNavi albums!
        visit_album_then_click_artist(&mut h, key(account(2), "al-b-soj-1"), "Sojourn Music");
        h.get_by_label("1 album");
        assert_eq!(h.query_all_by_label_contains("Over the Grave by Sojourn Music").count(), 1);
        assert!(h.query_by_label("MyNavi").is_none());
        assert!(h.query_by_label_contains("Stand").is_none());
        assert!(h.query_by_label_contains("Paul Simon").is_none());
        assert!(h.query_by_label_contains("Manhattan").is_none());

        // Distinct-MBID homonyms never merge across servers.
        visit_album_then_click_artist(&mut h, key(account(2), "al-b-jw-1"), "John Williams");
        h.get_by_label("1 album");
        assert_eq!(h.query_all_by_label_contains("Cavatina by John Williams").count(), 1);
        assert_eq!(h.query_all_by_label_contains("Star Wars by John Williams").count(), 0);
        assert!(h.query_by_label("Also credited on").is_none());

        visit_album_then_click_artist(&mut h, key(account(1), "al-a-jw-1"), "John Williams");
        h.get_by_label("1 album");
        assert_eq!(h.query_all_by_label_contains("Star Wars by John Williams").count(), 1);
        assert_eq!(h.query_all_by_label_contains("Cavatina by John Williams").count(), 0);
        assert!(h.query_by_label("Also credited on").is_none());

        // Command palette Route::Artist also yields the merged page.
        {
            let ctx = h.ctx.clone();
            let app = h.state_mut().as_mut().unwrap();
            app.rebuild_search();
            let enter = egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
            type_in_palette(&ctx, app, "Josh Garrels");
            frame(&ctx, app, vec![enter]);
        }
        drain(&mut h);
        h.get_by_label("4 albums");
        h.get_by_label("MyNavi");
        h.get_by_label("Friend");
        assert!(h.query_by_label_contains("Stand").is_none());

        // Reloading from saved stores whose artist_mbids_v1 already contains stale "" entries also sanitizes them.
        stores_by_account[&account(1)]
            .library
            .save(
                "artist_mbids_v1",
                &std::collections::BTreeMap::from([
                    ("ar-jg".to_owned(), "11111111-1111-1111-1111-111111111111".to_owned()),
                    ("ar-stand".to_owned(), "".to_owned()),
                    ("ar-ps".to_owned(), "".to_owned()),
                ]),
            )
            .unwrap();
        stores_by_account[&account(2)]
            .library
            .save("artist_mbids_v1", &std::collections::BTreeMap::from([("fr-jg".to_owned(), "".to_owned()), ("fr-soj".to_owned(), "".to_owned())]))
            .unwrap();
        for a in [account(1), account(2)] {
            h.state_mut().as_mut().unwrap().multi.bridges[&a].load_local(None);
        }
        drain(&mut h);
        {
            let app = h.state().as_ref().unwrap();
            let jg = app.data.merged_artist(Some(&key(account(2), "fr-jg")), "Josh Garrels").unwrap();
            assert_eq!(jg.albums.values().map(|v| v.len()).sum::<usize>(), 4);
            let soj = app.data.merged_artist(Some(&key(account(2), "fr-soj")), "Sojourn Music").unwrap();
            assert_eq!(soj.albums.values().map(|v| v.len()).sum::<usize>(), 1);
        }

        *h.state_mut() = None;
    }
}
