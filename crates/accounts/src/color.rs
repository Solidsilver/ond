//! A fixed eight-colour palette index for sources.

use serde::{Deserialize, Serialize};

/// An index into the shared eight-colour palette for sources.
///
/// Stored as a bare integer (`0..8`) with each account so a source keeps its
/// colour when another is removed. The actual RGB values for light and dark
/// themes live in `shell::theme`.
///
/// Out-of-range non-negative integers wrap modulo [`Self::COUNT`] when read;
/// anything else (negatives, strings, nulls) falls back to `0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct SourceColor(u8);

impl SourceColor {
    /// How many colours the shared palette has.
    pub const COUNT: u8 = 8;

    /// All eight palette entries in order.
    pub const ALL: [Self; Self::COUNT as usize] = [Self(0), Self(1), Self(2), Self(3), Self(4), Self(5), Self(6), Self(7)];

    /// The colour at `index`, wrapped into `0..COUNT`.
    pub const fn new(index: u8) -> Self {
        Self(index % Self::COUNT)
    }

    /// The palette index, always in `0..8`.
    pub const fn index(self) -> u8 {
        self.0
    }

    /// The palette index as `usize`, for indexing a theme table.
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl From<u8> for SourceColor {
    fn from(index: u8) -> Self {
        Self::new(index)
    }
}

impl From<SourceColor> for u8 {
    fn from(color: SourceColor) -> Self {
        color.0
    }
}

impl Serialize for SourceColor {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SourceColor {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Saved {
            Index(u64),
            Garbage(serde::de::IgnoredAny),
        }
        Ok(match Saved::deserialize(deserializer)? {
            Saved::Index(n) => Self((n % u64::from(Self::COUNT)) as u8),
            Saved::Garbage(_) => Self::default(),
        })
    }
}

/// Picks the colour for a newly added source given `used`.
///
/// Picks the lowest index in `0..8` not currently in use, so removing a source
/// frees its colour for the next addition without recolouring existing ones.
/// Once all eight colours are taken, picks the least-used index (lowest index
/// on a tie), wrapping around `0, 1, …, 7, 0, …` instead of piling every extra
/// source onto `0`.
pub fn next_color(used: impl IntoIterator<Item = SourceColor>) -> SourceColor {
    let mut counts = [0usize; SourceColor::COUNT as usize];
    for c in used {
        counts[c.as_usize()] += 1;
    }
    let idx = counts.iter().enumerate().min_by_key(|&(i, count)| (*count, i)).map_or(0, |(i, _)| i);
    SourceColor::new(idx as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_handed_out_in_order_and_reused_after_removal() {
        let mut used = Vec::new();
        for expected in 0..4 {
            let c = next_color(used.iter().copied());
            assert_eq!(c, SourceColor::new(expected));
            used.push(c);
        }
        // Removing source 1 frees index 1 while keeping 0, 2 and 3 untouched.
        used.remove(1);
        assert_eq!(next_color(used.iter().copied()), SourceColor::new(1));
        used.push(SourceColor::new(1));
        // Next addition continues at 4.
        assert_eq!(next_color(used.iter().copied()), SourceColor::new(4));
    }

    #[test]
    fn colors_wrap_around_evenly_once_all_eight_are_taken() {
        let mut used = SourceColor::ALL.to_vec();
        for expected in 0..SourceColor::COUNT {
            let c = next_color(used.iter().copied());
            assert_eq!(c, SourceColor::new(expected));
            used.push(c);
        }
        // Dropping one of the two copies of 3 leaves 3 used only once, so it
        // becomes the next colour picked again.
        let pos = used.iter().position(|c| *c == SourceColor::new(3)).unwrap();
        used.remove(pos);
        assert_eq!(next_color(used), SourceColor::new(3));
    }

    #[test]
    fn source_color_serializes_as_a_bare_number_and_reads_leniently() {
        #[derive(Deserialize)]
        struct Entry {
            #[serde(default)]
            color: SourceColor,
            #[serde(default)]
            name: String,
        }

        assert_eq!(serde_json::to_string(&SourceColor::new(3)).unwrap(), "3");
        assert_eq!(serde_json::from_str::<SourceColor>("5").unwrap(), SourceColor::new(5));
        assert_eq!(serde_json::from_str::<SourceColor>("8").unwrap(), SourceColor::new(0));
        assert_eq!(serde_json::from_str::<SourceColor>("11").unwrap(), SourceColor::new(3));

        for (json, expected) in [
            (r#"{"name":"a"}"#, 0),
            (r#"{"color":2,"name":"a"}"#, 2),
            (r#"{"color":9,"name":"a"}"#, 1),
            (r#"{"color":-1,"name":"a"}"#, 0),
            (r#"{"color":1.5,"name":"a"}"#, 0),
            (r#"{"color":"blue","name":"a"}"#, 0),
            (r#"{"color":null,"name":"a"}"#, 0),
            (r#"{"color":{},"name":"a"}"#, 0),
        ] {
            let entry: Entry = serde_json::from_str(json).unwrap();
            assert_eq!(entry.color, SourceColor::new(expected), "{json}");
            assert_eq!(entry.name, "a", "{json}");
        }
    }
}
