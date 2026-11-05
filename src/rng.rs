//! SplitMix64, a small seeded pseudo-random number generator.
//!
//! The generator is used by the `gen` command and by the tests. It is not
//! cryptographically secure. It only uses integer arithmetic, so a seed gives
//! the same sequence on every platform.

/// SplitMix64 state: a single `u64` that is advanced by a fixed odd constant
/// and then scrambled into the output.
#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Create a generator from a seed. Every seed, including 0, is valid.
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform `f64` in `[0, 1)`, built from the top 53 bits of `next_u64`.
    pub fn next_f64(&mut self) -> f64 {
        const SCALE: f64 = 1.0 / (1u64 << 53) as f64;
        (self.next_u64() >> 11) as f64 * SCALE
    }

    /// Integer in `0..n` (returns 0 when `n` is 0).
    ///
    /// Uses the multiply-and-shift method: the high 64 bits of the 128-bit
    /// product `random * n`. The bias is at most `n / 2^64`, which is
    /// negligible for the sizes used here.
    pub fn below(&mut self, n: u64) -> u64 {
        ((u128::from(self.next_u64()) * u128::from(n)) >> 64) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_reference_sequence_for_seed_zero() {
        // Reference values from an independent Python implementation.
        let mut rng = SplitMix64::new(0);
        assert_eq!(rng.next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(rng.next_u64(), 0x6e78_9e6a_a1b9_65f4);
        assert_eq!(rng.next_u64(), 0x06c4_5d18_8009_454f);
    }

    #[test]
    fn below_stays_in_range() {
        let mut rng = SplitMix64::new(7);
        for n in [1u64, 2, 3, 10, 1000, u64::MAX] {
            for _ in 0..1000 {
                assert!(rng.below(n) < n);
            }
        }
        assert_eq!(rng.below(0), 0);
    }

    #[test]
    fn next_f64_is_in_unit_interval() {
        let mut rng = SplitMix64::new(3);
        for _ in 0..10_000 {
            let x = rng.next_f64();
            assert!((0.0..1.0).contains(&x));
        }
    }
}
