//! Bounded page ownership for virtual TUI collections.
//!
//! Navigation, request scheduling, and data-source cursors deliberately remain
//! outside this type. It owns only the reusable invariant shared by virtual
//! lists: at most `CAPACITY` materialized pages survive, and least-recently-used
//! pages are retired deterministically.

use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone)]
pub(crate) struct BoundedPageWindow<V, const CAPACITY: usize> {
    pages: HashMap<usize, V>,
    lru: VecDeque<usize>,
}

impl<V, const CAPACITY: usize> Default for BoundedPageWindow<V, CAPACITY> {
    fn default() -> Self {
        assert!(
            CAPACITY > 0,
            "bounded page window capacity must be non-zero"
        );
        Self {
            pages: HashMap::new(),
            lru: VecDeque::new(),
        }
    }
}

impl<V, const CAPACITY: usize> BoundedPageWindow<V, CAPACITY> {
    pub(crate) fn contains(&self, page: usize) -> bool {
        self.pages.contains_key(&page)
    }

    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.pages.values_mut()
    }

    #[cfg(test)]
    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        self.pages.values()
    }

    pub(crate) fn take(&mut self, page: usize) -> Option<V> {
        self.lru.retain(|cached| *cached != page);
        self.pages.remove(&page)
    }

    /// Insert or refresh one page and return every value evicted by the hard
    /// capacity. Returning ownership lets callers choose whether a large value
    /// should be dropped synchronously or retired on a background thread.
    pub(crate) fn insert(&mut self, page: usize, value: V) -> Vec<V> {
        let mut retired = self
            .pages
            .insert(page, value)
            .into_iter()
            .collect::<Vec<_>>();
        self.touch(page);
        while self.pages.len() > CAPACITY {
            let Some(evicted_page) = self.lru.pop_front() else {
                break;
            };
            if let Some(evicted) = self.pages.remove(&evicted_page) {
                retired.push(evicted);
            }
        }
        retired
    }

    pub(crate) fn clear(&mut self) -> Vec<V> {
        self.lru.clear();
        self.pages.drain().map(|(_, value)| value).collect()
    }

    fn touch(&mut self, page: usize) {
        self.lru.retain(|cached| *cached != page);
        self.lru.push_back(page);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.pages.len()
    }
}

#[cfg(test)]
mod tests {
    use super::BoundedPageWindow;

    #[test]
    fn replacement_and_eviction_return_owned_values_in_lru_order() {
        let mut window = BoundedPageWindow::<String, 2>::default();
        assert!(window.insert(1, "one".to_string()).is_empty());
        assert!(window.insert(2, "two".to_string()).is_empty());

        assert_eq!(window.insert(1, "one-new".to_string()), vec!["one"]);
        assert_eq!(window.insert(3, "three".to_string()), vec!["two"]);
        assert!(window.contains(1));
        assert!(window.contains(3));
        assert!(!window.contains(2));
    }

    #[test]
    fn take_and_clear_keep_lru_and_values_coherent() {
        let mut window = BoundedPageWindow::<usize, 2>::default();
        window.insert(4, 40);
        window.insert(5, 50);
        assert_eq!(window.take(4), Some(40));
        assert_eq!(window.len(), 1);
        assert_eq!(window.clear(), vec![50]);
        assert_eq!(window.len(), 0);
    }
}
