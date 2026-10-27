//! The deterministic vote schedule: which pairs each voter sees, in what order,
//! and which item sits in the A slot.
//!
//! Two shared biases are neutralized here, and neither uses randomness where
//! parity does the job better:
//!
//! - **Position bias is shared**, so a majority vote amplifies it rather than
//!   cancelling it — the one failure mode where an ensemble is actively worse
//!   than a single judge. Orientation alternates on `(pair + voter) % 2`, which
//!   splits every pair 3/2 across a 5-panel. That is the best an odd panel can
//!   do, and a coin flip only approaches it in expectation.
//! - **Order effects are per-voter**, so a seeded Fisher-Yates shuffle per voter
//!   turns what would be shared drift into noise the ensemble can absorb.
//!
//! Everything is a pure function of `(seed, voter index)`, so a run reproduces
//! exactly. A run you cannot reproduce is a run you cannot defend.

/// splitmix64 — small, stateless-to-seed, and good enough to shuffle with.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`, rejection-sampled so the shuffle carries no modulo bias.
    pub fn below(&mut self, n: u64) -> u64 {
        if n <= 1 {
            return 0;
        }
        let limit = u64::MAX - (u64::MAX % n) - 1;
        loop {
            let v = self.next_u64();
            if v <= limit {
                return v % n;
            }
        }
    }
}

/// Every unordered pair of item indices, in canonical (ascending) order. The
/// position of a pair in this list is its stable id, independent of any shuffle.
pub fn all_pairs(items: usize) -> Vec<(usize, usize)> {
    let mut pairs = Vec::with_capacity(items * items.saturating_sub(1) / 2);
    for a in 0..items {
        for b in (a + 1)..items {
            pairs.push((a, b));
        }
    }
    pairs
}

/// Canonical pair ids in the order this voter is asked them.
pub fn schedule_for_voter(pair_count: usize, seed: u64, voter: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..pair_count).collect();
    let mut rng = Rng::new(seed ^ (0xA076_1D64_78BD_642F_u64.wrapping_mul(voter as u64 + 1)));
    for i in (1..order.len()).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        order.swap(i, j);
    }
    order
}

/// True when this voter sees the pair's lower-indexed item in the A slot.
pub fn a_first(pair_id: usize, voter: usize) -> bool {
    (pair_id + voter) % 2 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_pairs_is_n_choose_2_and_canonical() {
        let pairs = all_pairs(12);
        assert_eq!(pairs.len(), 66);
        assert!(pairs.iter().all(|(a, b)| a < b));
        let unique: std::collections::HashSet<_> = pairs.iter().collect();
        assert_eq!(unique.len(), 66);
    }

    #[test]
    fn a_schedule_is_a_permutation_and_reproducible() {
        let first = schedule_for_voter(66, 7, 2);
        assert_eq!(first, schedule_for_voter(66, 7, 2));
        let mut sorted = first.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..66).collect::<Vec<_>>());
    }

    #[test]
    fn voters_get_different_orders_and_seeds_change_them() {
        assert_ne!(schedule_for_voter(66, 7, 0), schedule_for_voter(66, 7, 1));
        assert_ne!(schedule_for_voter(66, 7, 0), schedule_for_voter(66, 8, 0));
    }

    #[test]
    fn orientation_splits_every_pair_as_evenly_as_an_odd_panel_can() {
        // The bias that matters is per-pair across the panel: if all 5 voters
        // saw the same item first, a majority would amplify position bias.
        for pair in 0..66 {
            let a_firsts = (0..5).filter(|v| a_first(pair, *v)).count();
            assert!(
                a_firsts == 2 || a_firsts == 3,
                "pair {pair} split {a_firsts}/5"
            );
        }
    }

    #[test]
    fn orientation_also_balances_within_a_single_voter() {
        let a_firsts = (0..66).filter(|p| a_first(*p, 3)).count();
        assert_eq!(a_firsts, 33);
    }

    #[test]
    fn below_is_in_range_including_degenerate_bounds() {
        let mut rng = Rng::new(1);
        assert_eq!(rng.below(0), 0);
        assert_eq!(rng.below(1), 0);
        for _ in 0..1000 {
            assert!(rng.below(7) < 7);
        }
    }
}
