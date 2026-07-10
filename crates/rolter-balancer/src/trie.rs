//! A minimal byte trie for approximate prefix matching, with an optional
//! node-count cap and LRU eviction so a long-running process mirrors the
//! upstream engine's bounded KV cache instead of growing without limit.
//!
//! Each node carries a refcount of how many resident keys pass through it, so
//! evicting a key prunes exactly the nodes that become unreferenced. Eviction
//! order is least-recently-inserted (an `insert` of an already-resident key
//! refreshes its recency), tracked with a lazily-compacted access log.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};

/// A prefix trie keyed on raw bytes. Unbounded by default; use
/// [`Trie::with_capacity`] to cap the node count and enable LRU eviction.
#[derive(Default)]
pub struct Trie {
    root: Node,
    /// number of nodes below the root (the root itself is not counted)
    nodes: usize,
    /// node-count ceiling; `0` means unbounded (no eviction)
    max_nodes: usize,
    /// monotonic recency stamp handed to keys on insert
    seq: u64,
    /// resident keys mapped to their latest recency stamp
    keys: HashMap<String, u64>,
    /// access log (key, stamp); front is oldest. entries whose stamp no longer
    /// matches `keys` are stale and skipped during eviction (lazy compaction)
    order: VecDeque<(String, u64)>,
    /// keys evicted since construction (observability)
    evictions: u64,
}

#[derive(Default)]
struct Node {
    children: HashMap<u8, Node>,
    terminal: bool,
    /// number of resident keys whose path traverses this node
    refs: u32,
}

impl Trie {
    /// A trie capped at `max_nodes` nodes with LRU eviction. `0` is unbounded.
    pub fn with_capacity(max_nodes: usize) -> Self {
        Self {
            max_nodes,
            ..Default::default()
        }
    }

    /// Insert a string so future queries can match against its bytes. Inserting
    /// an already-resident key refreshes its recency without changing the trie.
    /// After insertion, evicts least-recently-used keys until the node count is
    /// within the cap.
    pub fn insert(&mut self, s: &str) {
        if self.keys.contains_key(s) {
            self.touch(s);
            return;
        }
        let created = insert_path(&mut self.root, s.as_bytes());
        self.nodes += created;
        self.seq += 1;
        self.keys.insert(s.to_string(), self.seq);
        self.order.push_back((s.to_string(), self.seq));
        self.evict_to_capacity();
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

    /// Current node count (below the root).
    pub fn len(&self) -> usize {
        self.nodes
    }

    /// Whether the trie holds no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes == 0
    }

    /// Number of keys evicted since construction.
    pub fn evictions(&self) -> u64 {
        self.evictions
    }

    /// Refresh a resident key's recency to the newest stamp.
    fn touch(&mut self, s: &str) {
        self.seq += 1;
        if let Some(stamp) = self.keys.get_mut(s) {
            *stamp = self.seq;
            self.order.push_back((s.to_string(), self.seq));
        }
    }

    /// Evict least-recently-used keys until the node count is within the cap.
    fn evict_to_capacity(&mut self) {
        if self.max_nodes == 0 {
            return;
        }
        while self.nodes > self.max_nodes {
            if !self.evict_one() {
                break;
            }
        }
    }

    /// Evict the oldest live key. Returns `false` when nothing is left to evict.
    fn evict_one(&mut self) -> bool {
        while let Some((key, stamp)) = self.order.pop_front() {
            // skip stale log entries (superseded by a later touch/insert)
            match self.keys.get(&key) {
                Some(&cur) if cur == stamp => {
                    self.keys.remove(&key);
                    let removed = remove_path(&mut self.root, key.as_bytes());
                    self.nodes -= removed;
                    self.evictions += 1;
                    return true;
                }
                _ => continue,
            }
        }
        false
    }
}

/// Insert `bytes` under `root`, bumping the refcount of every node on the path
/// and marking the last terminal. Returns the number of nodes newly created.
fn insert_path(root: &mut Node, bytes: &[u8]) -> usize {
    let mut node = root;
    let mut created = 0;
    for b in bytes {
        match node.children.entry(*b) {
            Entry::Occupied(e) => node = e.into_mut(),
            Entry::Vacant(e) => {
                created += 1;
                node = e.insert(Node::default());
            }
        }
        node.refs += 1;
    }
    node.terminal = true;
    created
}

/// Remove the key `bytes` from `root`, decrementing refcounts along its path and
/// pruning any subtree that becomes unreferenced. Returns the number of nodes
/// removed.
fn remove_path(node: &mut Node, bytes: &[u8]) -> usize {
    let Some((&b, rest)) = bytes.split_first() else {
        // end of the key path: it is no longer a resident terminal
        node.terminal = false;
        return 0;
    };
    let Some(child) = node.children.get_mut(&b) else {
        return 0;
    };
    child.refs = child.refs.saturating_sub(1);
    if child.refs == 0 {
        // no resident key traverses the child any longer: drop its whole subtree
        let removed = count_nodes(child);
        node.children.remove(&b);
        removed
    } else {
        remove_path(child, rest)
    }
}

/// Count the nodes in a subtree, including `node` itself.
fn count_nodes(node: &Node) -> usize {
    1 + node.children.values().map(count_nodes).sum::<usize>()
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

    #[test]
    fn shared_prefix_counts_nodes_once() {
        let mut t = Trie::default();
        t.insert("abc");
        t.insert("abd");
        // "ab" is shared -> 4 nodes total: a, b, c, d
        assert_eq!(t.len(), 4);
    }

    #[test]
    fn reinsert_does_not_grow() {
        let mut t = Trie::default();
        t.insert("abc");
        let n = t.len();
        t.insert("abc");
        assert_eq!(t.len(), n);
    }

    #[test]
    fn evicts_lru_key_over_capacity() {
        // cap at 3 nodes: inserting "abc" (3) then "xyz" (3) must evict "abc"
        let mut t = Trie::with_capacity(3);
        t.insert("abc");
        assert_eq!(t.len(), 3);
        t.insert("xyz");
        assert_eq!(t.len(), 3, "should have evicted to stay within cap");
        assert_eq!(t.evictions(), 1);
        // "abc" evicted, "xyz" resident
        assert_eq!(t.longest_prefix("abc"), 0);
        assert_eq!(t.longest_prefix("xyz"), 3);
    }

    #[test]
    fn eviction_prunes_only_unshared_nodes() {
        // cap 7: "share-a" (7 nodes) fits; adding "share-b" needs the 'b' node
        // (8) so the LRU key "share-a" is evicted — but only its unshared 'a'
        // node is pruned, the shared "share-" prefix stays for "share-b"
        let mut t = Trie::with_capacity(7);
        t.insert("share-a");
        t.insert("share-b");
        assert_eq!(t.len(), 7);
        assert_eq!(t.evictions(), 1);
        // "share-a" evicted: its terminal is gone, but the shared prefix remains
        assert_eq!(t.longest_prefix("share-a"), 6);
        assert_eq!(t.longest_prefix("share-b"), 7);
    }

    #[test]
    fn touch_refreshes_recency() {
        // cap 3: insert abc, touch it, then insert xyz -> xyz is now LRU-newer
        // than the refreshed abc only if abc was touched; here abc is touched so
        // xyz eviction target is abc still (single-key cap). verify no panic and
        // capacity is held.
        let mut t = Trie::with_capacity(3);
        t.insert("abc");
        t.insert("abc"); // touch
        t.insert("xyz");
        assert_eq!(t.len(), 3);
        assert_eq!(t.evictions(), 1);
    }
}
