//! A set of target indices, small enough to build on every attempt.
//!
//! The per-target registries ([`crate::cooldowns`], [`crate::breaker`],
//! [`crate::health`]) each sit behind one lock. Asking them about a target at a
//! time takes that lock once per target, so a pick over an `n`-target route paid
//! about `3n` acquisitions (#1942). They answer for a whole route in one
//! acquisition instead, as a [`TargetMask`], and the eligibility closure a pick
//! hands the balancer then reads bits.
//!
//! The first 64 targets live in a word, so a route that size allocates nothing;
//! a wider one allocates its overflow words on the first insert past 63.

/// A set of target indices.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TargetMask {
    first: u64,
    rest: Vec<u64>,
}

impl TargetMask {
    /// The empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add target `idx`.
    pub fn insert(&mut self, idx: usize) {
        if idx < 64 {
            self.first |= 1 << idx;
            return;
        }
        let (word, bit) = ((idx - 64) / 64, (idx - 64) % 64);
        if self.rest.len() <= word {
            self.rest.resize(word + 1, 0);
        }
        self.rest[word] |= 1 << bit;
    }

    /// Whether target `idx` is in the set.
    pub fn contains(&self, idx: usize) -> bool {
        if idx < 64 {
            return self.first & (1 << idx) != 0;
        }
        let (word, bit) = ((idx - 64) / 64, (idx - 64) % 64);
        self.rest.get(word).is_some_and(|w| w & (1 << bit) != 0)
    }

    /// Whether no target is in the set.
    pub fn is_empty(&self) -> bool {
        self.first == 0 && self.rest.iter().all(|w| *w == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_targets_either_side_of_the_first_word() {
        let mut mask = TargetMask::new();
        assert!(mask.is_empty());
        for idx in [0, 63, 64, 127, 128, 300] {
            assert!(!mask.contains(idx));
            mask.insert(idx);
            assert!(mask.contains(idx));
        }
        assert!(!mask.contains(1) && !mask.contains(65) && !mask.contains(299));
        assert!(!mask.is_empty());
    }

    #[test]
    fn a_route_within_one_word_allocates_nothing() {
        let mut mask = TargetMask::new();
        for idx in 0..64 {
            mask.insert(idx);
        }
        assert_eq!(mask.rest.capacity(), 0);
    }
}
