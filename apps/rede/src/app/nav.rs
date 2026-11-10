//! Where the window is, and how it got there: the pages Rede has, back and
//! forward stacks through them (as Ond's), and the trail of recent places
//! the row above each page shows as breadcrumbs.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Route {
    /// Where the listening is: the books in progress, and the newest.
    Home,
    /// The books the shelf has.
    Library,
    /// One book, by its own id.
    Book(String),
}

impl Route {
    /// The places the sidebar links to; going to one starts a fresh trail.
    pub fn is_root(&self) -> bool {
        matches!(self, Route::Home | Route::Library)
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
        Self { back: Vec::new(), current: Route::Library, forward: Vec::new() }
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

    pub fn back(&mut self) {
        if let Some(previous) = self.back.pop() {
            self.forward.push(std::mem::replace(&mut self.current, previous));
        }
    }

    pub fn forward(&mut self) {
        if let Some(next) = self.forward.pop() {
            self.back.push(std::mem::replace(&mut self.current, next));
        }
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// Steps back `steps` times (an earlier breadcrumb was clicked).
    pub fn back_by(&mut self, steps: usize) {
        for _ in 0..steps {
            self.back();
        }
    }

    /// The breadcrumb trail ending at the current place: recent history back
    /// to (and including) the last place the sidebar links to, at most
    /// [`TRAIL_LEN`] long. A lone sidebar place has none: the sidebar
    /// already says where the window is.
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

    /// The sidebar place the page belongs to: itself when it is one, else the
    /// place it was opened from, as the breadcrumb says (a book reached from
    /// Home belongs to Home), and the library when the trail has no place.
    pub fn place(&self) -> &Route {
        if self.current.is_root() {
            return &self.current;
        }
        self.trail().into_iter().find(|r| r.is_root()).unwrap_or(&Route::Library)
    }

    /// Whether the window is still where it opened: nowhere gone, nothing to
    /// go back to.
    pub fn untouched(&self) -> bool {
        self.back.is_empty() && self.current == Route::Library
    }

    /// Opens on `at` instead, as if it had started there: no step back.
    pub fn land(&mut self, at: Route) {
        self.current = at;
        self.forward.clear();
    }
}

impl super::view_ctx::ViewCtx<'_> {
    /// Back, forward and the breadcrumb above every page, in the kit's row
    /// Ond's pages have.
    pub(super) fn nav_row(&mut self, ui: &mut egui::Ui) {
        let labels: Vec<String> = self.nav.trail().into_iter().map(|r| self.route_label(r)).collect();
        let action = match shell::kit::nav_row(ui, self.nav.can_back(), self.nav.can_forward(), &labels) {
            Some(shell::kit::NavClick::Back) => super::Action::Back,
            Some(shell::kit::NavClick::Forward) => super::Action::Forward,
            Some(shell::kit::NavClick::BackBy(steps)) => super::Action::BackBy(steps),
            None => return,
        };
        self.actions.push(action);
    }

    /// A place's short name, for the breadcrumb: the sidebar's words, or the
    /// book's title.
    fn route_label(&self, route: &Route) -> String {
        match route {
            Route::Home => "Home".into(),
            Route::Library => "Library".into(),
            Route::Book(id) => self.books.by_id(id).map_or_else(|| "Book".into(), |b| b.title.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_book_belongs_to_the_place_it_was_opened_from() {
        let mut nav = Nav::default();
        nav.go(Route::Book("b".into()));
        assert_eq!(nav.place(), &Route::Library);
        let mut nav = Nav::default();
        nav.go(Route::Home);
        assert_eq!(nav.place(), &Route::Home);
        nav.go(Route::Book("b".into()));
        assert_eq!(nav.place(), &Route::Home, "as its breadcrumb says");
        nav.back();
        nav.go(Route::Library);
        nav.go(Route::Book("b".into()));
        assert_eq!(nav.place(), &Route::Library);
    }

    #[test]
    fn going_somewhere_new_remembers_the_last_place() {
        let mut nav = Nav::default();
        assert_eq!(nav.current(), &Route::Library);
        assert!(!nav.can_back());
        nav.go(Route::Book("b".into()));
        assert_eq!(nav.current(), &Route::Book("b".into()));
        assert!(nav.can_back());
        // Going where we are changes nothing, so back doesn't bind a step to
        // the page's own refresh.
        nav.go(Route::Book("b".into()));
        nav.back();
        assert_eq!(nav.current(), &Route::Library);
        assert!(!nav.can_back());
        // Back at the top, back does nothing (the book gone from a fresh start).
        nav.back();
        assert_eq!(nav.current(), &Route::Library);
    }

    #[test]
    fn back_and_forward_retrace_the_steps_and_going_on_drops_forward() {
        let mut nav = Nav::default();
        assert!(!nav.can_forward());
        nav.go(Route::Book("a".into()));
        nav.back();
        assert_eq!(nav.current(), &Route::Library);
        assert!(nav.can_forward());
        nav.forward();
        assert_eq!(nav.current(), &Route::Book("a".into()));
        assert!(!nav.can_forward());
        // Forward at the end does nothing.
        nav.forward();
        assert_eq!(nav.current(), &Route::Book("a".into()));
        nav.back();
        nav.go(Route::Home);
        assert!(!nav.can_forward(), "somewhere new drops what was ahead");
    }

    #[test]
    fn the_trail_runs_back_to_where_the_sidebar_led() {
        let mut nav = Nav::default();
        assert!(nav.trail().is_empty(), "a sidebar place alone has no trail");
        nav.go(Route::Book("a".into()));
        assert_eq!(nav.trail(), [&Route::Library, &Route::Book("a".into())]);
        nav.go(Route::Home);
        assert!(nav.trail().is_empty());
        nav.go(Route::Book("b".into()));
        assert_eq!(nav.trail(), [&Route::Home, &Route::Book("b".into())]);
        for id in ["c", "d", "e"] {
            nav.go(Route::Book(id.into()));
        }
        // Capped: the oldest crumbs fall off.
        assert_eq!(nav.trail().len(), TRAIL_LEN);
        assert_eq!(nav.trail()[0], &Route::Book("b".into()));
        nav.back_by(3);
        assert_eq!(nav.current(), &Route::Book("b".into()));
        assert!(nav.can_forward());
    }

    #[test]
    fn landing_elsewhere_leaves_no_step_back() {
        let mut nav = Nav::default();
        assert!(nav.untouched());
        nav.land(Route::Home);
        assert_eq!(nav.current(), &Route::Home);
        assert!(!nav.can_back());
        assert!(!nav.untouched());
        let mut nav = Nav::default();
        nav.go(Route::Book("b".into()));
        nav.back();
        assert!(nav.untouched(), "back where it opened");
    }
}
