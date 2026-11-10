//! Where the window is, and how it got there: the pages Rede has, and a back
//! trail through them.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Route {
    /// Where the listening is: the books in progress, and the newest.
    Home,
    /// The books the shelf has.
    Library,
    /// One book, by its own id.
    Book(String),
}

#[derive(Debug)]
pub struct Nav {
    back: Vec<Route>,
    current: Route,
}

impl Default for Nav {
    fn default() -> Self {
        Self { back: Vec::new(), current: Route::Library }
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
    }

    pub fn back(&mut self) {
        if let Some(previous) = self.back.pop() {
            self.current = previous;
        }
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// Whether the window is still where it opened: nowhere gone, nothing to
    /// go back to.
    pub fn untouched(&self) -> bool {
        self.back.is_empty() && self.current == Route::Library
    }

    /// Opens on `at` instead, as if it had started there: no step back.
    pub fn land(&mut self, at: Route) {
        self.current = at;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
