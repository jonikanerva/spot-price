//! A seeded, paired, circular block bootstrap. Pure. A unit is one UTC
//! issuance day, so the issuances of a day stay together; a block holds
//! consecutive days, so overlapping targets stay together. Each statistic
//! is a ratio of two sums over the units, and all statistics of a run draw
//! the same days, so arm differences stay paired.

use crate::jsfmt::js_sum;
use crate::metrics::percentile;

/// The settings of the resampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootstrapOptions {
    pub seed: u64,
    pub resamples: usize,
    /// Consecutive issuance days per block.
    pub block_days: usize,
}

/// The defaults: 2000 resamples of 7-day blocks. One issuance scores targets
/// up to about 4.5 days ahead, so a block shorter than 5 days splits
/// overlapping targets.
pub const DEFAULT_BOOTSTRAP: BootstrapOptions = BootstrapOptions {
    seed: 153,
    resamples: 2000,
    block_days: 7,
};

/// The block length below which overlapping targets cross blocks.
pub const MIN_OVERLAP_BLOCK_DAYS: usize = 5;

/// The two-sided level of the reported interval.
pub const INTERVAL_LEVEL: f64 = 0.95;

/// `SplitMix64`: a small, fixed generator, so a seed gives the same draws on
/// every platform and version.
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform index in `0..n`; `n` must be above 0.
    pub fn below(&mut self, n: usize) -> usize {
        let wide = u128::from(self.next_u64()) * (n as u128);
        #[expect(clippy::cast_possible_truncation, reason = "the result is below n")]
        let index = (wide >> 64) as usize;
        index
    }
}

/// The day indices of each resample: `ceil(n / block)` circular blocks with
/// uniform starts, cut to `n` days.
#[must_use]
pub fn resample_indices(units: usize, options: &BootstrapOptions) -> Vec<Vec<usize>> {
    if units == 0 || options.resamples == 0 {
        return Vec::new();
    }
    let block = options.block_days.clamp(1, units);
    let mut rng = SplitMix64::new(options.seed);
    (0..options.resamples)
        .map(|_| {
            let mut days = Vec::with_capacity(units);
            while days.len() < units {
                let start = rng.below(units);
                for offset in 0..block {
                    if days.len() == units {
                        break;
                    }
                    days.push((start + offset) % units);
                }
            }
            days
        })
        .collect()
}

/// The numerator and denominator sums of one statistic in one unit.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UnitSums {
    pub numerator: f64,
    pub denominator: f64,
}

/// A point value and its percentile interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// `None` when the denominator of the full sample is 0.
    pub point: Option<f64>,
    pub low: Option<f64>,
    pub high: Option<f64>,
    /// Resamples whose denominator is 0; they are left out of the interval.
    pub undefined_resamples: usize,
}

fn ratio(numerator: f64, denominator: f64) -> Option<f64> {
    (denominator != 0.0).then(|| numerator / denominator)
}

/// The ratio of the summed numerators to the summed denominators, and its
/// [`INTERVAL_LEVEL`] percentile interval over `resamples`.
#[must_use]
pub fn estimate(units: &[UnitSums], resamples: &[Vec<usize>]) -> Estimate {
    let point = ratio(
        js_sum(units.iter().map(|u| u.numerator)),
        js_sum(units.iter().map(|u| u.denominator)),
    );
    let mut values = Vec::with_capacity(resamples.len());
    let mut undefined_resamples = 0;
    for days in resamples {
        let pick =
            |f: fn(&UnitSums) -> f64| js_sum(days.iter().filter_map(|&d| units.get(d)).map(f));
        match ratio(pick(|u| u.numerator), pick(|u| u.denominator)) {
            Some(v) => values.push(v),
            None => undefined_resamples += 1,
        }
    }
    let tail = (1.0 - INTERVAL_LEVEL) / 2.0 * 100.0;
    Estimate {
        point,
        low: percentile(&values, tail),
        high: percentile(&values, 100.0 - tail),
        undefined_resamples,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_draws_and_another_seed_differs() {
        let options = BootstrapOptions {
            seed: 7,
            resamples: 20,
            block_days: 3,
        };
        let a = resample_indices(10, &options);
        assert_eq!(a, resample_indices(10, &options));
        let other = BootstrapOptions { seed: 8, ..options };
        assert_ne!(a, resample_indices(10, &other));
    }

    #[test]
    fn a_resample_is_made_of_consecutive_circular_blocks() {
        let options = BootstrapOptions {
            seed: 1,
            resamples: 50,
            block_days: 4,
        };
        for days in resample_indices(10, &options) {
            assert_eq!(days.len(), 10);
            for block in days.chunks(4) {
                for pair in block.windows(2) {
                    assert_eq!(pair[1], (pair[0] + 1) % 10);
                }
            }
        }
    }

    #[test]
    fn the_known_first_value_of_splitmix64() {
        // The reference output of SplitMix64 for seed 0.
        assert_eq!(SplitMix64::new(0).next_u64(), 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn a_constant_ratio_has_a_degenerate_interval() {
        let units = vec![
            UnitSums {
                numerator: 2.0,
                denominator: 4.0
            };
            12
        ];
        let draws = resample_indices(units.len(), &DEFAULT_BOOTSTRAP);
        let e = estimate(&units, &draws);
        assert_eq!(e.point, Some(0.5));
        assert_eq!(e.low, Some(0.5));
        assert_eq!(e.high, Some(0.5));
        assert_eq!(e.undefined_resamples, 0);
    }

    #[test]
    fn a_zero_denominator_is_undefined_not_zero() {
        let units = vec![UnitSums::default(); 5];
        let e = estimate(&units, &resample_indices(5, &DEFAULT_BOOTSTRAP));
        assert_eq!(e.point, None);
        assert_eq!(e.low, None);
        assert_eq!(e.undefined_resamples, DEFAULT_BOOTSTRAP.resamples);
    }

    #[test]
    fn the_interval_contains_the_point_for_a_varying_ratio() {
        let units: Vec<UnitSums> = (0..30)
            .map(|i| UnitSums {
                numerator: f64::from(i % 7),
                denominator: 1.0,
            })
            .collect();
        let e = estimate(&units, &resample_indices(30, &DEFAULT_BOOTSTRAP));
        let (point, low, high) = (e.point.unwrap(), e.low.unwrap(), e.high.unwrap());
        assert!(low < point && point < high, "{low} {point} {high}");
    }
}
