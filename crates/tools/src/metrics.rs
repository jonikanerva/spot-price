//! Accuracy metrics of the forecast backtest. Rank metrics (Spearman,
//! precision@N) are the primary signal for automations; the absolute errors
//! are for reference. An undefined statistic is `None`, never a silent 0.

use std::cmp::Ordering;

use crate::jsfmt::{count_f64, js_sum};

/// The two series of a paired metric do not fit together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MetricsError {
    #[error("length mismatch: {0} vs {1}")]
    LengthMismatch(usize, usize),
    #[error("empty series")]
    Empty,
    #[error("n must be >= 1")]
    InvalidN,
}

/// Which end of the ranking precision@N scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RankMode {
    Cheap,
    Peak,
}

fn check(pred: &[f64], act: &[f64]) -> Result<(), MetricsError> {
    if pred.len() != act.len() {
        return Err(MetricsError::LengthMismatch(pred.len(), act.len()));
    }
    if pred.is_empty() {
        return Err(MetricsError::Empty);
    }
    Ok(())
}

/// Ascending order like `Array.prototype.sort((a, b) => a - b)`: stable, and
/// `-0` equals `0`.
fn compare(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// Mean absolute error.
///
/// # Errors
/// [`MetricsError`] when the series differ in length or are empty.
pub fn mae(pred: &[f64], act: &[f64]) -> Result<f64, MetricsError> {
    check(pred, act)?;
    let sum = js_sum(pred.iter().zip(act).map(|(p, a)| (p - a).abs()));
    Ok(sum / count_f64(pred.len()))
}

/// Root mean square error.
///
/// # Errors
/// [`MetricsError`] when the series differ in length or are empty.
pub fn rmse(pred: &[f64], act: &[f64]) -> Result<f64, MetricsError> {
    check(pred, act)?;
    let sum = js_sum(pred.iter().zip(act).map(|(p, a)| (p - a) * (p - a)));
    Ok((sum / count_f64(pred.len())).sqrt())
}

/// Mean signed error, `pred - act`.
///
/// # Errors
/// [`MetricsError`] when the series differ in length or are empty.
pub fn bias(pred: &[f64], act: &[f64]) -> Result<f64, MetricsError> {
    check(pred, act)?;
    let sum = js_sum(pred.iter().zip(act).map(|(p, a)| p - a));
    Ok(sum / count_f64(pred.len()))
}

/// 1-based ranks; a group of equal values gets its average rank.
fn average_ranks(values: &[f64]) -> Vec<f64> {
    let n = values.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| compare(values[a], values[b]));
    let mut ranks = vec![0.0; n];
    let mut i = 0;
    while i < n {
        let mut j = i;
        #[expect(clippy::float_cmp, reason = "ties are exact equality, like `===`")]
        while j + 1 < n && values[order[j + 1]] == values[order[i]] {
            j += 1;
        }
        let average_rank = count_f64(i + j) / 2.0 + 1.0;
        for &index in &order[i..=j] {
            ranks[index] = average_rank;
        }
        i = j + 1;
    }
    ranks
}

/// Spearman rank correlation; `None` when either series is constant or has
/// fewer than two values.
///
/// # Errors
/// [`MetricsError`] when the series differ in length or are empty.
pub fn spearman(pred: &[f64], act: &[f64]) -> Result<Option<f64>, MetricsError> {
    check(pred, act)?;
    let n = pred.len();
    if n < 2 {
        return Ok(None);
    }
    let rp = average_ranks(pred);
    let ra = average_ranks(act);
    let mp = js_sum(rp.iter().copied()) / count_f64(n);
    let ma = js_sum(ra.iter().copied()) / count_f64(n);
    let (mut cov, mut vp, mut va) = (0.0, 0.0, 0.0);
    for (p, a) in rp.iter().zip(&ra) {
        let dp = p - mp;
        let da = a - ma;
        cov += dp * da;
        vp += dp * dp;
        va += da * da;
    }
    if vp == 0.0 || va == 0.0 {
        return Ok(None);
    }
    Ok(Some(cov / (vp * va).sqrt()))
}

fn sorted(xs: &[f64]) -> Vec<f64> {
    let mut out = xs.to_vec();
    out.sort_by(|a, b| compare(*a, *b));
    out
}

/// The median; `None` for an empty slice.
#[must_use]
pub fn median(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let sorted = sorted(xs);
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        return sorted.get(mid).copied();
    }
    Some((sorted.get(mid - 1)? + sorted.get(mid)?) / 2.0)
}

/// The `p`-th percentile (`p` clamped to 0–100) by linear interpolation
/// between the two closest ranks; `None` for an empty slice.
#[must_use]
pub fn percentile(xs: &[f64], p: f64) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let sorted = sorted(xs);
    let clamped = p.clamp(0.0, 100.0);
    let rank = (clamped / 100.0) * count_f64(sorted.len() - 1);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rank is in [0, len - 1]"
    )]
    let (lo, hi) = (rank.floor() as usize, rank.ceil() as usize);
    let lo_value = *sorted.get(lo)?;
    let hi_value = *sorted.get(hi)?;
    if lo == hi {
        return Some(lo_value);
    }
    Some(lo_value + (hi_value - lo_value) * (rank - count_f64(lo)))
}

/// The sample standard deviation (n − 1); `None` below two values.
#[must_use]
pub fn standard_deviation(xs: &[f64]) -> Option<f64> {
    if xs.len() < 2 {
        return None;
    }
    let mean = js_sum(xs.iter().copied()) / count_f64(xs.len());
    let sum_sq = js_sum(xs.iter().map(|x| (x - mean) * (x - mean)));
    Some((sum_sq / count_f64(xs.len() - 1)).sqrt())
}

/// The root mean square; `None` for an empty slice.
#[must_use]
pub fn rms(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let sum_sq = js_sum(xs.iter().map(|x| x * x));
    Some((sum_sq / count_f64(xs.len())).sqrt())
}

/// Whether all values are equal under `SameValueZero` (`new Set(xs).size <= 1`).
fn is_constant(xs: &[f64]) -> bool {
    let Some(&first) = xs.first() else {
        return true;
    };
    #[expect(clippy::float_cmp, reason = "SameValueZero is exact equality")]
    xs.iter()
        .all(|&x| x == first || (x.is_nan() && first.is_nan()))
}

/// The fraction of the predicted `n` cheapest (or most expensive) entries
/// that are also in the actual set. `None` for a constant prediction.
///
/// # Errors
/// [`MetricsError`] when the series differ in length or are empty, or when
/// `n` is 0.
pub fn precision_at_n(
    pred: &[f64],
    act: &[f64],
    n: usize,
    mode: RankMode,
) -> Result<Option<f64>, MetricsError> {
    check(pred, act)?;
    if n == 0 {
        return Err(MetricsError::InvalidN);
    }
    if is_constant(pred) {
        return Ok(None);
    }
    let take = n.min(pred.len());
    let index_by = |values: &[f64]| -> Vec<usize> {
        let mut order: Vec<usize> = (0..values.len()).collect();
        order.sort_by(|&a, &b| {
            let ordered = match mode {
                RankMode::Cheap => compare(values[a], values[b]),
                RankMode::Peak => compare(values[b], values[a]),
            };
            ordered.then(a.cmp(&b))
        });
        order.truncate(take);
        order
    };
    let predicted = index_by(pred);
    let actual = index_by(act);
    let hits = predicted.iter().filter(|i| actual.contains(i)).count();
    Ok(Some(count_f64(hits) / count_f64(take)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(got: f64, want: f64) {
        assert!((got - want).abs() < 1e-9, "got {got}, want {want}");
    }

    #[test]
    fn mae_rmse_and_bias_on_a_known_series() {
        let (pred, act) = ([1.0, 2.0, 3.0], [2.0, 2.0, 5.0]);
        close(mae(&pred, &act).unwrap(), 1.0);
        close(rmse(&pred, &act).unwrap(), (5.0_f64 / 3.0).sqrt());
        close(bias(&pred, &act).unwrap(), -1.0);
    }

    #[test]
    fn rejects_a_length_mismatch_and_an_empty_series() {
        assert_eq!(
            mae(&[1.0], &[1.0, 2.0]),
            Err(MetricsError::LengthMismatch(1, 2))
        );
        assert_eq!(mae(&[], &[]), Err(MetricsError::Empty));
    }

    #[test]
    fn spearman_is_one_for_a_monotone_increase() {
        let got = spearman(&[1.0, 2.0, 3.0, 4.0, 5.0], &[10.0, 20.0, 30.0, 40.0, 50.0]);
        close(got.unwrap().unwrap(), 1.0);
    }

    #[test]
    fn spearman_is_minus_one_for_a_reversed_ranking() {
        let got = spearman(&[1.0, 2.0, 3.0, 4.0, 5.0], &[50.0, 40.0, 30.0, 20.0, 10.0]);
        close(got.unwrap().unwrap(), -1.0);
    }

    #[test]
    fn spearman_is_none_for_a_constant_series() {
        assert_eq!(spearman(&[3.0, 3.0, 3.0], &[1.0, 2.0, 3.0]), Ok(None));
    }

    #[test]
    fn median_handles_odd_even_and_empty() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn percentile_interpolates_and_clamps() {
        assert_eq!(percentile(&[0.0, 10.0], 50.0), Some(5.0));
        close(percentile(&[0.0, 10.0, 20.0, 30.0], 90.0).unwrap(), 27.0);
        assert_eq!(percentile(&[5.0], 90.0), Some(5.0));
        assert_eq!(percentile(&[1.0, 2.0, 3.0], 200.0), Some(3.0));
        assert_eq!(percentile(&[], 90.0), None);
    }

    #[test]
    fn standard_deviation_is_the_sample_value() {
        close(standard_deviation(&[2.0, 4.0, 6.0]).unwrap(), 2.0);
        assert_eq!(standard_deviation(&[7.0]), None);
        assert_eq!(standard_deviation(&[]), None);
    }

    #[test]
    fn rms_is_the_quadratic_mean() {
        close(rms(&[3.0, 4.0]).unwrap(), 12.5_f64.sqrt());
        assert_eq!(rms(&[0.0, 0.0, 0.0]), Some(0.0));
        assert_eq!(rms(&[]), None);
    }

    #[test]
    fn precision_scores_the_cheapest_overlap() {
        let got = precision_at_n(
            &[4.0, 1.0, 8.0, 3.0],
            &[5.0, 1.0, 9.0, 2.0],
            2,
            RankMode::Cheap,
        );
        close(got.unwrap().unwrap(), 1.0);
    }

    #[test]
    fn precision_scores_the_peak_overlap() {
        let got = precision_at_n(
            &[4.0, 1.0, 8.0, 3.0],
            &[5.0, 1.0, 9.0, 2.0],
            2,
            RankMode::Peak,
        );
        close(got.unwrap().unwrap(), 1.0);
    }

    #[test]
    fn precision_detects_a_wrong_pick() {
        let got = precision_at_n(
            &[1.0, 9.0, 2.0, 8.0],
            &[5.0, 1.0, 9.0, 2.0],
            1,
            RankMode::Cheap,
        );
        assert_eq!(got, Ok(Some(0.0)));
    }

    #[test]
    fn precision_is_none_for_a_constant_prediction_and_rejects_zero_n() {
        assert_eq!(
            precision_at_n(&[2.0, 2.0, 2.0], &[1.0, 2.0, 3.0], 1, RankMode::Cheap),
            Ok(None)
        );
        assert_eq!(
            precision_at_n(&[1.0, 2.0], &[1.0, 2.0], 0, RankMode::Cheap),
            Err(MetricsError::InvalidN)
        );
        assert_eq!(
            precision_at_n(&[0.0, -0.0], &[1.0, 2.0], 1, RankMode::Cheap),
            Ok(None)
        );
    }
}
