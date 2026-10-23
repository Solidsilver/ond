//! A one-slot cache for values derived from versioned data: rebuilt when its
//! key changes, otherwise handed out as a cheap `Arc` clone. Views use it so a
//! frame never copies a whole collection.

use std::sync::Arc;

pub struct Memo<K, V> {
    cached: Option<(K, Arc<V>)>,
}

impl<K, V> Default for Memo<K, V> {
    fn default() -> Self {
        Self { cached: None }
    }
}

impl<K: PartialEq, V> Memo<K, V> {
    /// The value for `key`, building it only when the key changed.
    pub fn get(&mut self, key: K, build: impl FnOnce() -> V) -> Arc<V> {
        match &self.cached {
            Some((k, v)) if *k == key => Arc::clone(v),
            _ => {
                let v = Arc::new(build());
                self.cached = Some((key, Arc::clone(&v)));
                v
            }
        }
    }

    /// Forgets the value, so the next [`get`](Self::get) rebuilds.
    pub fn clear(&mut self) {
        self.cached = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_once_per_key() {
        let mut m: Memo<u64, Vec<u32>> = Memo::default();
        let mut built = 0;
        let a = m.get(1, || {
            built += 1;
            vec![1, 2]
        });
        let b = m.get(1, || {
            built += 1;
            vec![9]
        });
        assert!(Arc::ptr_eq(&a, &b));
        let c = m.get(2, || {
            built += 1;
            vec![3]
        });
        assert_eq!(*c, [3]);
        assert_eq!(built, 2);
        m.clear();
        m.get(2, || {
            built += 1;
            vec![]
        });
        assert_eq!(built, 3);
    }
}
