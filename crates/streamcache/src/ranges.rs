/// Sorted, disjoint, non-adjacent half-open byte spans `[start, end)`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RangeSet {
    spans: Vec<(u64, u64)>,
}

impl RangeSet {
    /// Adds `[start, end)`, merging with any span it overlaps or touches.
    pub fn insert(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        let (mut s, mut e) = (start, end);
        let i = self.spans.partition_point(|&(_, b)| b < s);
        let mut j = i;
        while j < self.spans.len() && self.spans[j].0 <= e {
            s = s.min(self.spans[j].0);
            e = e.max(self.spans[j].1);
            j += 1;
        }
        self.spans.splice(i..j, [(s, e)]);
    }

    /// Bytes available contiguously from `pos` (0 if `pos` isn't downloaded).
    pub fn available_from(&self, pos: u64) -> u64 {
        let i = self.spans.partition_point(|&(_, b)| b <= pos);
        match self.spans.get(i) {
            Some(&(a, b)) if a <= pos => b - pos,
            _ => 0,
        }
    }

    /// The first missing span at or after `from`, clipped to `len`.
    pub fn first_gap(&self, from: u64, len: u64) -> Option<(u64, u64)> {
        let mut pos = from;
        for &(a, b) in &self.spans {
            if pos >= len {
                return None;
            }
            if b <= pos {
                continue;
            }
            if a > pos {
                return Some((pos, a.min(len)));
            }
            pos = b;
        }
        (pos < len).then_some((pos, len))
    }

    pub fn covers(&self, len: u64) -> bool {
        self.first_gap(0, len).is_none()
    }

    pub fn total(&self) -> u64 {
        self.spans.iter().map(|(a, b)| b - a).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_overlapping_and_adjacent() {
        let mut r = RangeSet::default();
        r.insert(10, 20);
        r.insert(30, 40);
        r.insert(20, 25);
        assert_eq!(r.spans, [(10, 25), (30, 40)]);
        r.insert(24, 31);
        assert_eq!(r.spans, [(10, 40)]);
        r.insert(0, 5);
        r.insert(50, 60);
        r.insert(3, 55);
        assert_eq!(r.spans, [(0, 60)]);
        assert_eq!(r.total(), 60);
    }

    #[test]
    fn availability_and_gaps() {
        let mut r = RangeSet::default();
        r.insert(0, 100);
        r.insert(200, 300);
        assert_eq!(r.available_from(0), 100);
        assert_eq!(r.available_from(99), 1);
        assert_eq!(r.available_from(100), 0);
        assert_eq!(r.available_from(250), 50);
        assert_eq!(r.first_gap(0, 400), Some((100, 200)));
        assert_eq!(r.first_gap(150, 400), Some((150, 200)));
        assert_eq!(r.first_gap(250, 400), Some((300, 400)));
        assert_eq!(r.first_gap(250, 300), None);
        assert!(!r.covers(300));
        r.insert(100, 200);
        assert!(r.covers(300));
    }
}
