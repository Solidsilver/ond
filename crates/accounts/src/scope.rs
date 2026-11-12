//! Which sources a library view is showing.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::AccountId;

/// Which sources a view shows, and which filter chips have narrowed it.
///
/// A view is in one of two switcher modes:
/// - **All sources** (`all = true`): shows every source unless filter chips
///   (`only`) narrow it to a subset. When `only` is empty, every source is
///   included; when `only` is non-empty, only those sources are included.
/// - **One source** (`all = false`): shows a single chosen source (`only` holds
///   that one [`AccountId`]). Filter chips do not apply in this mode.
///
/// Rules:
/// - [`Scope::choose_one`] sets `all = false` and `only = {id}`.
/// - [`Scope::choose_all`] sets `all = true` and clears `only`, showing every
///   source.
/// - [`Scope::toggle_chip`] adds `id` to `only` when absent and removes it when
///   present (clearing `only` back to empty includes every source again). When
///   the scope is on a single source (`all = false`), toggling a chip does
///   nothing.
/// - [`Scope::drop_account`] removes `id` from `only`. If the scope was
///   narrowed to only that account (either `Only(id)` or `All` narrowed by
///   chips to `{id}`), it falls back to `All` with no chip filter.
///
/// Serializes as `{ "all": true, "only": [ids] }`. Deserialization is lenient:
/// non-object values, unknown fields, malformed `all`, or invalid entries in
/// `only` are ignored or fall back to defaults rather than failing the
/// settings file, and `all: false` with an empty `only` normalizes to `All`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    all: bool,
    only: BTreeSet<AccountId>,
}

impl Default for Scope {
    fn default() -> Self {
        Self::all()
    }
}

impl Scope {
    /// All sources, with no filter chips selected.
    pub fn all() -> Self {
        Self { all: true, only: BTreeSet::new() }
    }

    /// Narrowed to one source by the switcher.
    pub fn only(id: AccountId) -> Self {
        let mut only = BTreeSet::new();
        only.insert(id);
        Self { all: false, only }
    }

    /// Whether the switcher is on All sources (even when filter chips narrow
    /// which of them are shown).
    pub fn is_all(&self) -> bool {
        self.all
    }

    /// The single source when the switcher is on one source, or `None` in All
    /// mode.
    pub fn single(&self) -> Option<AccountId> {
        (!self.all).then(|| self.only.iter().next().copied()).flatten()
    }

    /// Whether `id`'s filter chip is active in All mode.
    pub fn chip_selected(&self, id: AccountId) -> bool {
        self.all && self.only.contains(&id)
    }

    /// Whether any filter chip is narrowing All mode.
    pub fn has_chips(&self) -> bool {
        self.all && !self.only.is_empty()
    }

    /// The accounts currently selected by filter chips (empty unless in All
    /// mode with chips active).
    pub fn chips(&self) -> impl Iterator<Item = AccountId> + '_ {
        self.all.then_some(&self.only).into_iter().flatten().copied()
    }

    /// Whether items from `id` belong in the current view.
    pub fn includes(&self, id: AccountId) -> bool {
        if self.all && self.only.is_empty() { true } else { self.only.contains(&id) }
    }

    /// Switches the view to one source, replacing any chip selection.
    pub fn choose_one(&mut self, id: AccountId) {
        self.all = false;
        self.only.clear();
        self.only.insert(id);
    }

    /// Switches the view to All sources and clears any chip selection.
    pub fn choose_all(&mut self) {
        self.all = true;
        self.only.clear();
    }

    /// Toggles `id`'s filter chip while in All mode; ignored in single-source
    /// mode.
    pub fn toggle_chip(&mut self, id: AccountId) {
        if !self.all {
            return;
        }
        if !self.only.remove(&id) {
            self.only.insert(id);
        }
    }

    /// Clears all filter chips while staying in All mode.
    pub fn clear_chips(&mut self) {
        if self.all {
            self.only.clear();
        }
    }

    /// Removes `id` after an account is deleted; falls back to All if the scope
    /// was narrowed to only `id`.
    pub fn drop_account(&mut self, id: AccountId) {
        self.only.remove(&id);
        if !self.all && self.only.is_empty() {
            self.all = true;
        }
    }
}

impl Serialize for Scope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Raw<'a> {
            all: bool,
            only: &'a BTreeSet<AccountId>,
        }
        Raw { all: self.all, only: &self.only }.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum MaybeBool {
            Bool(bool),
            Garbage(serde::de::IgnoredAny),
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum MaybeId {
            Id(AccountId),
            Garbage(serde::de::IgnoredAny),
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum MaybeIds {
            Ids(Vec<MaybeId>),
            Garbage(serde::de::IgnoredAny),
        }

        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            all: Option<MaybeBool>,
            #[serde(default)]
            only: Option<MaybeIds>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum MaybeScope {
            Map(Raw),
            Garbage(serde::de::IgnoredAny),
        }

        let MaybeScope::Map(raw) = MaybeScope::deserialize(deserializer)? else {
            return Ok(Self::default());
        };
        let all = match raw.all {
            Some(MaybeBool::Bool(b)) => b,
            _ => true,
        };
        let ids: Vec<AccountId> = match raw.only {
            Some(MaybeIds::Ids(items)) => items
                .into_iter()
                .filter_map(|item| match item {
                    MaybeId::Id(id) => Some(id),
                    MaybeId::Garbage(_) => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        if all {
            Ok(Self { all: true, only: ids.into_iter().collect() })
        } else if let Some(first) = ids.into_iter().next() {
            Ok(Self::only(first))
        } else {
            Ok(Self::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn account(n: u8) -> AccountId {
        AccountId::from_uuid(Uuid::from_u128(u128::from(n)))
    }

    #[test]
    fn all_scope_includes_every_account_and_has_no_single_or_chips() {
        let s = Scope::default();
        assert_eq!(s, Scope::all());
        assert!(s.is_all());
        assert_eq!(s.single(), None);
        assert!(!s.has_chips());
        assert!(s.includes(account(1)));
        assert!(s.includes(account(2)));
        assert!(!s.chip_selected(account(1)));
    }

    #[test]
    fn choosing_one_source_limits_the_view_until_all_is_chosen() {
        let (a, b) = (account(1), account(2));
        let mut s = Scope::all();
        s.toggle_chip(a);
        assert!(s.has_chips());

        s.choose_one(b);
        assert!(!s.is_all());
        assert_eq!(s.single(), Some(b));
        assert!(!s.has_chips());
        assert!(!s.includes(a));
        assert!(s.includes(b));

        s.choose_all();
        assert!(s.is_all());
        assert_eq!(s.single(), None);
        assert!(s.includes(a));
        assert!(s.includes(b));
    }

    #[test]
    fn toggling_chips_narrows_all_sources_and_returns_to_all_when_cleared() {
        let (a, b, c) = (account(1), account(2), account(3));
        let mut s = Scope::all();

        s.toggle_chip(a);
        assert!(s.is_all());
        assert_eq!(s.single(), None);
        assert!(s.chip_selected(a));
        assert!(s.includes(a));
        assert!(!s.includes(b));
        assert!(!s.includes(c));

        s.toggle_chip(b);
        assert_eq!(s.chips().collect::<Vec<_>>(), vec![a, b]);
        assert!(s.includes(a));
        assert!(s.includes(b));
        assert!(!s.includes(c));

        s.toggle_chip(a);
        assert!(!s.chip_selected(a));
        assert!(s.chip_selected(b));
        assert!(!s.includes(a));
        assert!(s.includes(b));

        s.toggle_chip(b);
        assert!(!s.has_chips());
        assert!(s.includes(a));
        assert!(s.includes(b));
        assert!(s.includes(c));

        s.toggle_chip(a);
        s.clear_chips();
        assert_eq!(s, Scope::all());
    }

    #[test]
    fn toggling_a_chip_is_ignored_while_viewing_one_source() {
        let (a, b) = (account(1), account(2));
        let mut s = Scope::only(a);
        s.toggle_chip(b);
        s.toggle_chip(a);
        s.clear_chips();
        assert_eq!(s, Scope::only(a));
        assert!(s.includes(a));
        assert!(!s.includes(b));
    }

    #[test]
    fn dropping_the_sole_included_account_falls_back_to_all() {
        let (a, b, c) = (account(1), account(2), account(3));

        // Dropping the chosen single source falls back to All.
        let mut single = Scope::only(a);
        single.drop_account(b);
        assert_eq!(single, Scope::only(a));
        single.drop_account(a);
        assert_eq!(single, Scope::all());
        assert!(single.includes(b));

        // Dropping the only selected chip in All mode clears the filter back to All.
        let mut one_chip = Scope::all();
        one_chip.toggle_chip(a);
        one_chip.drop_account(a);
        assert_eq!(one_chip, Scope::all());
        assert!(one_chip.includes(b));

        // Dropping one of several selected chips keeps the rest.
        let mut two_chips = Scope::all();
        two_chips.toggle_chip(a);
        two_chips.toggle_chip(b);
        two_chips.drop_account(a);
        assert!(two_chips.is_all());
        assert!(two_chips.includes(b));
        assert!(!two_chips.includes(c));
    }

    #[test]
    fn scope_serializes_and_reads_the_settings_example() {
        let a: AccountId = "6f1c1f6e-a0a2-4b8f-9d7f-0d51b0f6b0a7".parse().unwrap();
        let b: AccountId = "a03e1f6e-a0a2-4b8f-9d7f-0d51b0f6b0a7".parse().unwrap();
        let json = r#"{"all":true,"only":["6f1c1f6e-a0a2-4b8f-9d7f-0d51b0f6b0a7","a03e1f6e-a0a2-4b8f-9d7f-0d51b0f6b0a7"]}"#;
        let parsed: Scope = serde_json::from_str(json).unwrap();
        assert!(parsed.is_all());
        assert!(parsed.includes(a));
        assert!(parsed.includes(b));
        assert!(!parsed.includes(account(9)));
        assert_eq!(serde_json::to_string(&parsed).unwrap(), json);

        let only = Scope::only(a);
        let round_trip: Scope = serde_json::from_str(&serde_json::to_string(&only).unwrap()).unwrap();
        assert_eq!(round_trip, only);
    }

    #[test]
    fn scope_reads_leniently_when_fields_or_entries_are_garbage() {
        #[derive(Deserialize)]
        struct Settings {
            #[serde(default)]
            scope: Scope,
            #[serde(default)]
            theme: String,
        }

        let valid = "6f1c1f6e-a0a2-4b8f-9d7f-0d51b0f6b0a7";
        let valid_id: AccountId = valid.parse().unwrap();

        // Missing or top-level garbage falls back to All without failing the file.
        for json in [
            r#"{"theme":"dark"}"#,
            r#"{"scope":null,"theme":"dark"}"#,
            r#"{"scope":"all","theme":"dark"}"#,
            r#"{"scope":42,"theme":"dark"}"#,
            r#"{"scope":[1,2],"theme":"dark"}"#,
            r#"{"scope":{},"theme":"dark"}"#,
            r#"{"scope":{"all":false,"only":[]},"theme":"dark"}"#,
            r#"{"scope":{"all":"yes","only":"nope","extra":true},"theme":"dark"}"#,
        ] {
            let s: Settings = serde_json::from_str(json).unwrap();
            assert_eq!(s.scope, Scope::all(), "{json}");
            assert_eq!(s.theme, "dark", "{json}");
        }

        // Valid ids survive beside garbage entries and unknown fields.
        let mixed = format!(r#"{{"scope":{{"all":true,"only":["not-a-uuid",123,null,"{valid}"],"future":9}},"theme":"light"}}"#);
        let s: Settings = serde_json::from_str(&mixed).unwrap();
        assert!(s.scope.is_all());
        assert!(s.scope.includes(valid_id));
        assert!(!s.scope.includes(account(2)));
        assert_eq!(s.theme, "light");

        // `all: false` with multiple ids keeps the first valid one as a single source.
        let second = "a03e1f6e-a0a2-4b8f-9d7f-0d51b0f6b0a7";
        let two_only = format!(r#"{{"scope":{{"all":false,"only":["bad","{second}","{valid}"]}}}}"#);
        let s: Settings = serde_json::from_str(&two_only).unwrap();
        assert_eq!(s.scope, Scope::only(second.parse().unwrap()));
    }
}
