//! SplitMix64 with the seeding, splitting and range sampling of Haskell's
//! `random` package (`StdGen`), so a seed gives the same memory contents and
//! the same VLIW write order as the original simulator.

const GOLDEN_GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;

#[derive(Clone, Debug)]
pub struct StdGen {
    seed: u64,
    gamma: u64,
}

fn mix64(z: u64) -> u64 {
    let z = (z ^ (z >> 33)).wrapping_mul(0xff51_afd7_ed55_8ccd);
    let z = (z ^ (z >> 33)).wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    z ^ (z >> 33)
}

fn mix64_variant13(z: u64) -> u64 {
    let z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn mix_gamma(z: u64) -> u64 {
    let z = mix64_variant13(z) | 1;
    if (z ^ (z >> 1)).count_ones() >= 24 {
        z
    } else {
        z ^ 0xaaaa_aaaa_aaaa_aaaa
    }
}

impl StdGen {
    /// `mkStdGen`.
    pub fn new(seed: i64) -> Self {
        let s = seed as u64;
        StdGen {
            seed: mix64(s),
            gamma: mix_gamma(s.wrapping_add(GOLDEN_GAMMA)),
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.seed = self.seed.wrapping_add(self.gamma);
        mix64(self.seed)
    }

    /// `split`: the first generator continues this one, the second is new.
    pub fn split(&self) -> (StdGen, StdGen) {
        let seed1 = self.seed.wrapping_add(self.gamma);
        let seed2 = seed1.wrapping_add(self.gamma);
        (
            StdGen {
                seed: seed2,
                gamma: self.gamma,
            },
            StdGen {
                seed: mix64(seed1),
                gamma: mix_gamma(seed2),
            },
        )
    }

    /// `uniformR (lo, hi) :: Int`: bitmask with rejection.
    pub fn uniform_r(&mut self, lo: i64, hi: i64) -> i64 {
        let (lo, hi) = if lo > hi { (hi, lo) } else { (lo, hi) };
        let range = (hi as u64).wrapping_sub(lo as u64);
        let mask = u64::MAX >> (range | 1).leading_zeros();
        loop {
            let x = self.next_u64() & mask;
            if x <= range {
                return (lo as u64).wrapping_add(x) as i64;
            }
        }
    }
}

/// An endless stream of `uniformR (0, maxBound)` values, as handed to an ISA.
#[derive(Clone, Debug)]
pub struct RandomStream(StdGen);

impl RandomStream {
    pub fn new(gen: StdGen) -> Self {
        RandomStream(gen)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> i64 {
        self.0.uniform_r(0, i64::MAX)
    }

    pub fn take(&mut self, n: usize) -> Vec<i64> {
        (0..n).map(|_| self.next()).collect()
    }
}
