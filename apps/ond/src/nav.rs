//! Where the main view is, and how it got there: back/forward stacks plus the
//! trail of recent places shown as breadcrumbs.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Route {
    Recent,
    Albums,
    Artists,
    /// Every track, as a sortable table.
    Songs,
    Playlists,
    /// An album artist's page; `id` when the server gave one.
    Artist {
        name: String,
        id: Option<String>,
    },
    Album(String),
    Playlist {
        id: String,
        name: String,
    },
    /// Albums sharing a genre, year or label.
    Filter {
        kind: FilterKind,
        value: String,
    },
    /// Loved songs, albums and artists.
    Favorites,
    Offline,
}

/// Keeps a detail page when its entity survives in the new projection.
/// A merged artist may be reached through any of its original members.
pub fn route_in_scope(route: &Route, scope: &accounts::Scope, data: &crate::app::Data) -> Route {
    let included = |id: &str| crate::source::identity::account(id).is_none_or(|a| scope.includes(a));
    match route {
        Route::Album(id) if !included(id) || data.album(id).is_none() => Route::Albums,
        Route::Playlist { id, .. } if !included(id) || !data.playlists.iter().any(|p| p.id == *id) => Route::Playlists,
        Route::Artist { name, id } => {
            if let Some(group) = data.merged_artist(id.as_deref(), name)
                && let Some(member) = group.members.iter().find(|m| scope.includes(m.key.account))
            {
                return Route::Artist { name: member.name.clone(), id: (!member.key.item.starts_with('\0')).then(|| member.key.to_string()) };
            }
            let matched = data.artists.iter().find(|a| crate::source::artists::normalize_name(&a.name) == crate::source::artists::normalize_name(name));
            if let Some(artist) = matched {
                Route::Artist { name: artist.name.clone(), id: Some(artist.id.clone()) }
            } else if data.albums.iter().any(|a| a.has_artist(id.as_deref(), name)) {
                route.clone()
            } else {
                Route::Artists
            }
        }
        _ => route.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterKind {
    Genre,
    Year,
    Label,
}

impl Route {
    /// Top-level places the sidebar links to; going to one starts a fresh trail.
    pub fn is_root(&self) -> bool {
        matches!(
            self,
            Route::Recent | Route::Albums | Route::Artists | Route::Songs | Route::Playlists | Route::Favorites | Route::Offline | Route::Playlist { .. }
        )
    }

    /// A track's artist page: its first credited artist (name and id) when the
    /// server split them out, else the artist tag with the track's artist id.
    pub fn artist_of(track: &library::Track) -> Option<Route> {
        let first = track.artists.first();
        let name = first.map(|a| a.name.clone()).or_else(|| track.artist.clone())?;
        let id = first.and_then(|a| a.id.clone()).or_else(|| track.artist_ref.clone());
        Some(Route::Artist { name, id })
    }
}

#[derive(Debug)]
pub struct Nav {
    back: Vec<Route>,
    current: Route,
    forward: Vec<Route>,
}

/// Breadcrumbs shown at most.
pub const TRAIL_LEN: usize = 4;

impl Default for Nav {
    fn default() -> Self {
        Self { back: Vec::new(), current: Route::Recent, forward: Vec::new() }
    }
}

impl Nav {
    pub fn current(&self) -> &Route {
        &self.current
    }

    pub fn go(&mut self, to: Route) {
        if to == self.current {
            return;
        }
        let from = std::mem::replace(&mut self.current, to);
        self.back.push(from);
        self.forward.clear();
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn back(&mut self) -> bool {
        let Some(prev) = self.back.pop() else { return false };
        self.forward.push(std::mem::replace(&mut self.current, prev));
        true
    }

    pub fn forward(&mut self) -> bool {
        let Some(next) = self.forward.pop() else { return false };
        self.back.push(std::mem::replace(&mut self.current, next));
        true
    }

    /// The breadcrumb trail ending at the current place: recent history back to
    /// (and including) the last top-level place, at most [`TRAIL_LEN`] long.
    /// A lone top-level place has no trail (the sidebar already says where you are).
    pub fn trail(&self) -> Vec<&Route> {
        let mut out = vec![&self.current];
        if !self.current.is_root() {
            for r in self.back.iter().rev() {
                if out.len() == TRAIL_LEN {
                    break;
                }
                out.push(r);
                if r.is_root() {
                    break;
                }
            }
        }
        out.reverse();
        if out.len() == 1 { Vec::new() } else { out }
    }

    /// A playlist was renamed: show the new name wherever it's been visited.
    pub fn rename_playlist(&mut self, id: &str, name: &str) {
        for r in self.back.iter_mut().chain(self.forward.iter_mut()).chain(std::iter::once(&mut self.current)) {
            if let Route::Playlist { id: pid, name: n } = r
                && pid == id
            {
                *n = name.to_owned();
            }
        }
    }

    /// Steps back `steps` times (clicking an earlier breadcrumb).
    pub fn back_by(&mut self, steps: usize) {
        for _ in 0..steps {
            self.back();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str) -> Route {
        Route::Album(id.into())
    }

    fn artist(name: &str) -> Route {
        Route::Artist { name: name.into(), id: None }
    }

    #[test]
    fn back_forward() {
        let mut n = Nav::default();
        n.go(Route::Albums);
        n.go(album("a"));
        assert!(n.back());
        assert_eq!(n.current(), &Route::Albums);
        assert!(n.forward());
        assert_eq!(n.current(), &album("a"));
        assert!(!n.forward());
        // Going somewhere new drops the forward stack.
        n.back();
        n.go(Route::Artists);
        assert!(!n.can_forward());
        // Same place twice isn't a step.
        n.go(Route::Artists);
        n.back();
        assert_eq!(n.current(), &Route::Albums);
    }

    #[test]
    fn trail_runs_back_to_the_last_root() {
        let mut n = Nav::default();
        assert!(n.trail().is_empty());
        n.go(Route::Albums);
        n.go(album("a"));
        n.go(artist("Bill Evans"));
        assert_eq!(n.trail(), [&Route::Albums, &album("a"), &artist("Bill Evans")]);
        n.go(Route::Artists);
        assert!(n.trail().is_empty());
        n.go(artist("Scott LaFaro"));
        n.go(album("b"));
        n.go(artist("Paul Motian"));
        n.go(album("c"));
        // Capped: the oldest crumbs fall off.
        assert_eq!(n.trail(), [&artist("Scott LaFaro"), &album("b"), &artist("Paul Motian"), &album("c")]);
        n.go(artist("Jim Hall"));
        assert_eq!(n.trail(), [&album("b"), &artist("Paul Motian"), &album("c"), &artist("Jim Hall")]);
        n.back();
        n.back_by(2);
        assert_eq!(n.current(), &album("b"));
    }
}
