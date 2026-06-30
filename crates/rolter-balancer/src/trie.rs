//! A minimal byte trie for approximate prefix matching.
//!
//! This is intentionally simple for the MVP: it grows without bound. Eviction
//! (LRU / max-node caps mirroring the upstream engine's cache) is tracked in the
//! roadmap and slots in behind the same interface.

use std::collections::HashMap;

/// A prefix trie keyed on raw bytes.
#[derive(Default)]
pub struct Trie {
    root: Node,
}

#[derive(Default)]
struct Node {
    children: HashMap<u8, Node>,
    terminal: bool,
}

impl Trie {
    /// Insert a string so future queries can match against its bytes.
    pub fn insert(&mut self, s: &str) {
        let mut node = &mut self.root;
        for b in s.as_bytes() {
            node = node.children.entry(*b).or_default();
        }
        node.terminal = true;
    }

    /// Return the number of leading bytes of `s` already present in the trie.
    pub fn longest_prefix(&self, s: &str) -> usize {
        let mut node = &self.root;
        let mut count = 0usize;
        for b in s.as_bytes() {
            match node.children.get(b) {
                Some(next) => {
                    count += 1;
                    node = next;
                }
                None => break,
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_shared_prefix() {
        let mut t = Trie::default();
        t.insert("system prompt: hello");
        assert_eq!(
            t.longest_prefix("system prompt: world"),
            "system prompt: ".len()
        );
        assert_eq!(t.longest_prefix("different"), 0);
    }
}
