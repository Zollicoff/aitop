//! Narrowing a tool's summary to one project, model or session.

use std::sync::Arc;

use crate::event::Event;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    Project,
    Model,
    Session,
    /// A machine.
    Host,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    pub kind: FilterKind,
    /// Project path, model id, session id or machine name (the row key).
    pub key: Arc<str>,
    /// What to show in breadcrumbs.
    pub label: String,
}

impl Filter {
    pub fn matches(&self, e: &Event) -> bool {
        let field = match self.kind {
            FilterKind::Project => &e.project_path,
            FilterKind::Model => &e.model,
            FilterKind::Session => &e.session,
            FilterKind::Host => &e.host,
        };
        **field == *self.key
    }
}

/// True when `e` passes every filter.
pub fn matches_all(filters: &[Filter], e: &Event) -> bool {
    filters.iter().all(|f| f.matches(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::testutil::ev;
    use crate::event::{Interner, Tool};

    #[test]
    fn matching() {
        let mut i = Interner::default();
        let e = ev(&mut i, Tool::Claude, 0, "/code/web", "s1", 1.0);
        let f = |kind, key: &str| Filter { kind, key: key.into(), label: String::new() };
        assert!(f(FilterKind::Project, "/code/web").matches(&e));
        assert!(!f(FilterKind::Project, "/code/api").matches(&e));
        assert!(f(FilterKind::Session, "s1").matches(&e));
        assert!(f(FilterKind::Model, "m").matches(&e));
        assert!(f(FilterKind::Host, "here").matches(&e));
        assert!(!f(FilterKind::Host, "laptop").matches(&e));
        assert!(matches_all(&[], &e));
        assert!(!matches_all(&[f(FilterKind::Session, "s1"), f(FilterKind::Model, "x")], &e));
    }
}
