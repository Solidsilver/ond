//! Who an item belongs to.
//!
//! Two servers can both hold an item called `abc`, so every item Rede stores,
//! caches, downloads or sends progress for is keyed by the account it came
//! from as well as by its own id. An account's id is a UUID made when the
//! account is added and never changes: the server address can change (a new
//! domain, a local address at home and a remote one away), and a key carrying
//! the address would orphan progress and downloads when it did. This is the
//! part that is expensive to retrofit, so it's in from the start even though
//! most people will have one account.
//!
//! [`ItemKey`]'s string form is `<account>/<item>`, which is what the stream
//! cache and the offline store name files with. Both pass it through
//! `streamcache::escape_id`, which percent-escapes everything outside
//! `[A-Za-z0-9_-]`, so the `/` becomes `%2F`: the pair stays one flat file
//! name, can't be read as a directory, and the escaping is reversible, so two
//! keys never share a name.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An account's permanent id, local to this installation.
///
/// It is not the server's user id: signing the same server out and back in
/// makes a new account, and the server can be reached at more than one
/// address.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(transparent)]
pub struct AccountId(Uuid);

impl AccountId {
    /// A fresh id, for an account that is being added.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for AccountId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for AccountId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::from_str(s)?))
    }
}

impl From<Uuid> for AccountId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<AccountId> for Uuid {
    fn from(id: AccountId) -> Self {
        id.0
    }
}

/// One item, on one account.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemKey {
    pub account: AccountId,
    /// The source's own id for the item. Meaningful only with the account.
    pub item: String,
}

impl ItemKey {
    pub fn new(account: AccountId, item: impl Into<String>) -> Self {
        Self { account, item: item.into() }
    }

    /// [`ItemKey`]'s string form, `<account>/<item>`; [`FromStr`] is its
    /// inverse. It's what names a file in the stream cache and the offline
    /// store, so it's part of the on-disk format.
    pub fn parse(s: &str) -> Result<Self, KeyError> {
        let Some((account, item)) = s.split_once('/') else {
            return Err(KeyError::Format(s.to_owned()));
        };
        if item.is_empty() {
            return Err(KeyError::EmptyItem);
        }
        Ok(Self { account: account.parse()?, item: item.to_owned() })
    }
}

impl fmt::Display for ItemKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.account, self.item)
    }
}

impl FromStr for ItemKey {
    type Err = KeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Why a string isn't an [`ItemKey`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("an item key is \"<account>/<item>\", not {0:?}")]
    Format(String),
    #[error("an item key needs an item id after the account")]
    EmptyItem,
    #[error("not an account id: {0}")]
    Account(#[from] uuid::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(last: u8) -> AccountId {
        AccountId::from_uuid(Uuid::from_u128(u128::from(last)))
    }

    #[test]
    fn an_account_id_is_a_uuid_and_parses_back() {
        let id = AccountId::new();
        assert_eq!(id.to_string().parse::<AccountId>().unwrap(), id);
        assert_eq!(id.as_uuid().get_version_num(), 4, "made, not derived from anything");
        assert_ne!(AccountId::new(), AccountId::new());
        assert!("not-a-uuid".parse::<AccountId>().is_err());
    }

    #[test]
    fn an_item_key_round_trips_through_its_string_form() {
        for item in ["abc", "01J9-book.m4b", "with space", "ünïcode", "a/b/c", "%2F"] {
            let key = ItemKey::new(account(3), item);
            let shown = key.to_string();
            assert_eq!(shown.parse::<ItemKey>().unwrap(), key, "{shown}");
            assert_eq!(ItemKey::parse(&shown).unwrap(), key);
        }
    }

    #[test]
    fn a_key_that_isnt_two_parts_is_refused() {
        assert!(matches!(ItemKey::parse("abc").unwrap_err(), KeyError::Format(_)));
        assert!(matches!(ItemKey::parse("").unwrap_err(), KeyError::Format(_)));
        assert_eq!(ItemKey::parse("6f1c1f6e-a0a2-4b8f-9d7f-0d51b0f6b0a7/").unwrap_err(), KeyError::EmptyItem);
        assert!(matches!(ItemKey::parse("not-a-uuid/abc").unwrap_err(), KeyError::Account(_)));
    }

    #[test]
    fn item_keys_from_two_accounts_never_collide() {
        let (one, two) = (account(1), account(2));
        let a = ItemKey::new(one, "abc");
        let b = ItemKey::new(two, "abc");
        assert_ne!(a, b);
        assert_ne!(a.to_string(), b.to_string());
        assert_ne!(streamcache::file_name(&a.to_string(), Some("m4b")), streamcache::file_name(&b.to_string(), Some("m4b")));
    }

    /// The string form has to survive the escaping the cache does, or two keys
    /// could name one file.
    #[test]
    fn the_string_form_becomes_one_safe_file_name() {
        for item in ["abc", "with space", "a/b", "ünïcode", "..", ".", "a. m4b"] {
            let key = ItemKey::new(account(4), item);
            let name = streamcache::escape_id(&key.to_string());
            assert!(!name.contains('/'), "{name} is a path");
            assert!(!name.contains('.'), "{name} holds a dot");
            assert!(!name.contains('\\'), "{name} is a path on Windows");
            // The account reads as itself: a UUID is all letters, digits and
            // hyphens, which the escaper keeps.
            assert!(name.starts_with(&key.account.to_string()), "{name} lost its account");
            // And the whole name is unique per key, since the escaping is.
            let other = ItemKey::new(account(5), item);
            assert_ne!(name, streamcache::escape_id(&other.to_string()), "two accounts share a name");
        }
    }
}
