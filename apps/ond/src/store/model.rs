//! Frozen versioned records stored in `ond.redb`.
//!
//! Postcard is not self-describing, so a stored struct's field order and types
//! form the on-disk wire layout. Any change must introduce a new `V<n>` struct
//! and migrate existing rows during schema upgrade rather than modifying an
//! existing version in place.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ApartV1 {
    pub kept_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StarredV1 {
    pub name: Option<String>,
    pub starred_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RatingV1 {
    pub rating: u8,
    pub rated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlaylistEntryV1 {
    pub account: String,
    pub item: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlaylistV1 {
    pub id: String,
    pub name: String,
    pub entries: Vec<PlaylistEntryV1>,
    pub created_at: i64,
    pub changed_at: i64,
}
