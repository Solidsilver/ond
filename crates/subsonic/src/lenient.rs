//! Forgiving decoders for the parts of a reply that servers get wrong.
//!
//! A library page holds hundreds of songs; one with a bad field shouldn't cost
//! the other 499. Lists decode item by item and drop (and log) the items that
//! don't decode, and optional numbers take what servers actually send: strings,
//! floats, out-of-range values (which become `None`). Fields whose absence
//! means the reply is wrong (an item's `id`, a payload's top-level key) stay
//! strict.
//!
//! These go through [`serde_json::Value`], so they only work with
//! self-describing formats. The wire types are only ever read from JSON.

use serde::Deserialize;
use serde::de::{DeserializeOwned, Deserializer};
use serde_json::Value;

/// A list where an item that doesn't decode is skipped with a warning instead
/// of failing the whole reply. Also takes `null` (empty) and a lone object
/// (a one-item list, as servers that convert from XML send it).
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
            tracing::warn!("expected a list of {}, got {other}; ignoring it", short_name::<T>());
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

/// [`opt_int`] for counts that default to zero.
pub(crate) fn int_or_zero<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<i128> + Default,
{
    Ok(int(&Value::deserialize(d)?).unwrap_or_default())
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

fn short_name<T>() -> &'static str {
    let full = std::any::type_name::<T>();
    full.rsplit("::").next().unwrap_or(full)
}
