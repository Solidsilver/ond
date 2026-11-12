//! Forgiving decoders for the parts of a reply that vary between servers.
//!
//! A library page holds hundreds of books; one with a bad field shouldn't cost
//! the others. Lists decode item by item and drop (and log) the items that
//! don't decode. Numbers take what Audiobookshelf actually sends: the paging
//! fields `limit` and `page` are echoed back from the query string, so they
//! arrive as strings when they were sent and as `0` when they weren't. Fields
//! whose absence means the reply is wrong (an item's `id`) stay strict.
//!
//! These go through [`serde_json::Value`], so they only work with JSON, which
//! is all the server speaks.

use serde::Deserialize;
use serde::de::{DeserializeOwned, Deserializer};
use serde_json::Value;

/// A list where an item that doesn't decode is skipped with a warning instead
/// of failing the whole reply. Also takes `null` (empty) and a lone object (a
/// one-item list: with a series filter the server sends one series object
/// where it otherwise sends an array).
pub(crate) fn list<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let items = match Value::deserialize(d)? {
        Value::Array(items) => items,
        Value::Null => Vec::new(),
        one @ Value::Object(_) => vec![one],
        other => {
            tracing::warn!("expected a list of {}, got a {}; ignoring it", short_name::<T>(), kind(&other));
            Vec::new()
        }
    };
    Ok(items.iter().filter_map(item).collect())
}

/// One list item, or `None` (logged) when it doesn't decode.
fn item<T: DeserializeOwned>(v: &Value) -> Option<T> {
    match T::deserialize(v) {
        Ok(t) => Some(t),
        Err(e) => {
            let id = v.get("id").and_then(Value::as_str).unwrap_or("?");
            tracing::warn!("skipping a {} the server sent malformed (id {id}): {e}", short_name::<T>());
            None
        }
    }
}

/// An optional object that is dropped (logged) rather than failing its parent
/// when it doesn't decode.
pub(crate) fn opt<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(match Value::deserialize(d)? {
        Value::Null => None,
        v => item(&v),
    })
}

/// An optional integer from a number, a float (rounded) or a numeric string.
/// Anything else, or a value out of range for `T`, is `None`.
pub(crate) fn opt_int<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<i128>,
{
    Ok(int(&Value::deserialize(d)?))
}

/// An optional float from a number or a numeric string.
pub(crate) fn opt_float<'de, D>(d: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|f| f.is_finite()))
}

/// Optional text from a string, or from a number or a boolean written out
/// (an `ino` or a `publishedYear` some servers store as a number).
pub(crate) fn opt_text<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(d)? {
        Value::String(s) => Some(s),
        v @ (Value::Number(_) | Value::Bool(_)) => Some(v.to_string()),
        _ => None,
    })
}

fn int<T: TryFrom<i128>>(v: &Value) -> Option<T> {
    let from_float = |f: f64| f.is_finite().then(|| f.round() as i128);
    let n = match v {
        Value::Number(n) => n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from)).or_else(|| n.as_f64().and_then(from_float))?,
        Value::String(s) => {
            let s = s.trim();
            s.parse::<i128>().ok().or_else(|| s.parse::<f64>().ok().and_then(from_float))?
        }
        _ => return None,
    };
    T::try_from(n).ok()
}

/// What kind of JSON value this is, for a log line. Never the value itself:
/// a reply can carry tokens.
fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "object",
    }
}

fn short_name<T>() -> &'static str {
    let full = std::any::type_name::<T>();
    full.rsplit("::").next().unwrap_or(full)
}
