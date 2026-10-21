//! The closed-form ridge regression of the FI forecast. `fit` is one
//! synchronous solve and `predict` is a pure scalar function. `VISION.md`
//! limits the forecast to such estimators.

use super::js::count_f64;

/// Minimum aligned samples for a fit; fewer samples give the fallback.
pub const MIN_FIT_SAMPLES: usize = 24;
/// Default ridge penalty λ.
pub const DEFAULT_RIDGE_LAMBDA: f64 = 1.0;
/// The constant prediction (raw c/kWh) of an unusable fit.
pub const FALLBACK_PREDICTION: f64 = 3.0;

/// The target transform. `predict` inverts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PriceTransform {
    /// `asinh`, inverted with `sinh`. It handles negative prices and spikes.
    #[default]
    Arcsinh,
    None,
}

/// Options of one fit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitOptions {
    pub ridge_lambda: f64,
    pub price_transform: PriceTransform,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self {
            ridge_lambda: DEFAULT_RIDGE_LAMBDA,
            price_transform: PriceTransform::Arcsinh,
        }
    }
}

/// Aligned rows and targets: `targets[i]` belongs to `rows[i]`. Each row
/// should have `feature_count` entries; a missing entry reads as 0.
#[derive(Debug, Clone, Copy)]
pub struct TrainingData<'a> {
    pub feature_count: usize,
    pub rows: &'a [Vec<f64>],
    pub targets: &'a [f64],
}

/// Provenance of a fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FitMeta {
    /// True when the fit uses the constant fallback.
    pub used_fallback: bool,
    pub sample_count: usize,
    pub feature_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct Standardizer {
    means: Vec<f64>,
    /// A zero-variance column has scale 1, so it standardizes to 0.
    scales: Vec<f64>,
}

impl Standardizer {
    fn from_rows(rows: &[Vec<f64>], feature_count: usize) -> Self {
        let mut means = vec![0.0; feature_count];
        let mut scales = vec![1.0; feature_count];
        if rows.is_empty() {
            return Self { means, scales };
        }
        let n = count_f64(rows.len());
        for (j, (mean_out, scale_out)) in means.iter_mut().zip(&mut scales).enumerate() {
            let mut sum = 0.0;
            for row in rows {
                sum += cell(row, j);
            }
            let mean = sum / n;
            *mean_out = mean;
            let mut var_sum = 0.0;
            for row in rows {
                let d = cell(row, j) - mean;
                var_sum += d * d;
            }
            let std = (var_sum / n).sqrt();
            *scale_out = if std > 1e-12 { std } else { 1.0 };
        }
        Self { means, scales }
    }

    /// The weighted means and standard deviations of the columns. The
    /// weights must be finite and non-negative, with a positive sum.
    fn from_weighted_rows(rows: &[Vec<f64>], weights: &[f64], feature_count: usize) -> Self {
        let mut means = vec![0.0; feature_count];
        let mut scales = vec![1.0; feature_count];
        let total: f64 = weights.iter().sum();
        if rows.is_empty() || total <= 0.0 {
            return Self { means, scales };
        }
        for (j, (mean_out, scale_out)) in means.iter_mut().zip(&mut scales).enumerate() {
            let mut sum = 0.0;
            for (row, w) in rows.iter().zip(weights) {
                sum += w * cell(row, j);
            }
            let mean = sum / total;
            *mean_out = mean;
            let mut var_sum = 0.0;
            for (row, w) in rows.iter().zip(weights) {
                let d = cell(row, j) - mean;
                var_sum += w * d * d;
            }
            let std = (var_sum / total).sqrt();
            *scale_out = if std > 1e-12 { std } else { 1.0 };
        }
        Self { means, scales }
    }

    fn apply(&self, row: &[f64]) -> Vec<f64> {
        self.means
            .iter()
            .zip(&self.scales)
            .enumerate()
            .map(|(j, (mean, scale))| (cell(row, j) - mean) / scale)
            .collect()
    }
}

fn cell(row: &[f64], j: usize) -> f64 {
    row.get(j).copied().unwrap_or(0.0)
}

#[derive(Debug, Clone, PartialEq)]
enum Fitted {
    Fallback,
    Linear {
        intercept: f64,
        coefficients: Vec<f64>,
        standardizer: Standardizer,
        transform: PriceTransform,
    },
}

/// A fitted model: a pure scalar predictor and its provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct FittedModel {
    pub meta: FitMeta,
    fitted: Fitted,
}

impl FittedModel {
    /// The prediction in raw c/kWh.
    #[must_use]
    pub fn predict(&self, x: &[f64]) -> f64 {
        match &self.fitted {
            Fitted::Fallback => FALLBACK_PREDICTION,
            Fitted::Linear {
                intercept,
                coefficients,
                standardizer,
                transform,
            } => {
                let z = standardizer.apply(x);
                let mut acc = *intercept;
                for (coefficient, value) in coefficients.iter().zip(&z) {
                    acc += coefficient * value;
                }
                match transform {
                    PriceTransform::Arcsinh => acc.sinh(),
                    PriceTransform::None => acc,
                }
            }
        }
    }

    const fn fallback(sample_count: usize, feature_count: usize) -> Self {
        Self {
            meta: FitMeta {
                used_fallback: true,
                sample_count,
                feature_count,
            },
            fitted: Fitted::Fallback,
        }
    }
}

/// Solve the symmetric positive-definite system `a x = b` by Cholesky
/// decomposition. `a` is `n × n` row-major. `None` means `a` is not
/// numerically positive-definite.
pub(crate) fn solve_spd(matrix: &[f64], rhs: &[f64], dim: usize) -> Option<Vec<f64>> {
    let at = |m: &[f64], row: usize, col: usize| m.get(row * dim + col).copied().unwrap_or(0.0);
    let mut lower = vec![0.0; dim * dim];
    for row in 0..dim {
        for col in 0..=row {
            let mut sum = at(matrix, row, col);
            for k in 0..col {
                sum -= at(&lower, row, k) * at(&lower, col, k);
            }
            let value = if row == col {
                if sum <= 1e-12 {
                    return None;
                }
                sum.sqrt()
            } else {
                let diagonal = at(&lower, col, col);
                if diagonal == 0.0 {
                    return None;
                }
                sum / diagonal
            };
            if let Some(slot) = lower.get_mut(row * dim + col) {
                *slot = value;
            }
        }
    }

    // Forward substitution: L y = rhs.
    let mut forward = vec![0.0; dim];
    for row in 0..dim {
        let mut sum = rhs.get(row).copied().unwrap_or(0.0);
        for k in 0..row {
            sum -= at(&lower, row, k) * forward.get(k).copied().unwrap_or(0.0);
        }
        let diagonal = at(&lower, row, row);
        if diagonal == 0.0 {
            return None;
        }
        if let Some(slot) = forward.get_mut(row) {
            *slot = sum / diagonal;
        }
    }

    // Back substitution: Lᵀ x = y.
    let mut solution = vec![0.0; dim];
    for row in (0..dim).rev() {
        let mut sum = forward.get(row).copied().unwrap_or(0.0);
        for k in row + 1..dim {
            sum -= at(&lower, k, row) * solution.get(k).copied().unwrap_or(0.0);
        }
        let diagonal = at(&lower, row, row);
        if diagonal == 0.0 {
            return None;
        }
        if let Some(slot) = solution.get_mut(row) {
            *slot = sum / diagonal;
        }
    }
    Some(solution)
}

/// The ridge normal equations `(XᵀX + λI′) β = Xᵀy` with an unpenalized
/// intercept column first. Returns `(XᵀX + λI′, Xᵀy)`, both of dimension
/// `feature_count + 1`; the matrix is row-major.
fn normal_equations(
    standardized_rows: &[Vec<f64>],
    targets: &[f64],
    feature_count: usize,
    lambda: f64,
) -> (Vec<f64>, Vec<f64>) {
    weighted_normal_equations(standardized_rows, targets, None, feature_count, lambda)
}

/// [`normal_equations`] with a weight per row: `(XᵀWX + λI′) β = XᵀWy`.
/// `None` multiplies by nothing, so the unweighted sums stay bit-identical.
fn weighted_normal_equations(
    standardized_rows: &[Vec<f64>],
    targets: &[f64],
    weights: Option<&[f64]>,
    feature_count: usize,
    lambda: f64,
) -> (Vec<f64>, Vec<f64>) {
    let dim = feature_count + 1;
    let mut ata = vec![0.0; dim * dim];
    let mut aty = vec![0.0; dim];
    let mut xrow = vec![0.0; dim];
    for (n, (z, &target)) in standardized_rows.iter().zip(targets).enumerate() {
        for (j, slot) in xrow.iter_mut().enumerate() {
            *slot = if j == 0 { 1.0 } else { cell(z, j - 1) };
        }
        let weight = weights.map(|w| w.get(n).copied().unwrap_or(0.0));
        for (i, &xi) in xrow.iter().enumerate() {
            let wxi = weight.map_or(xi, |w| w * xi);
            if let Some(row) = ata.get_mut(i * dim..(i + 1) * dim) {
                for (slot, &xk) in row.iter_mut().zip(&xrow) {
                    *slot += wxi * xk;
                }
            }
            if let Some(slot) = aty.get_mut(i) {
                *slot += wxi * target;
            }
        }
    }
    for i in 1..dim {
        if let Some(slot) = ata.get_mut(i * dim + i) {
            *slot += lambda;
        }
    }
    (ata, aty)
}

/// Fit the closed-form ridge model on standardized features. It falls back
/// to [`FALLBACK_PREDICTION`] with fewer than [`MIN_FIT_SAMPLES`] samples,
/// with no features, or with a singular system.
#[must_use]
pub fn fit_ridge(data: &TrainingData<'_>, opts: &FitOptions) -> FittedModel {
    fit(data, None, opts)
}

/// [`fit_ridge`] with a non-negative weight per row: the closed-form
/// weighted ridge `(XᵀWX + λI′) β = XᵀWy`, standardized with weighted
/// moments. Rows with a non-finite or negative weight, or a weight count
/// that differs from the row count, give the fallback.
#[must_use]
pub fn fit_ridge_weighted(
    data: &TrainingData<'_>,
    weights: &[f64],
    opts: &FitOptions,
) -> FittedModel {
    fit(data, Some(weights), opts)
}

fn fit(data: &TrainingData<'_>, weights: Option<&[f64]>, opts: &FitOptions) -> FittedModel {
    let feature_count = data.feature_count;
    let sample_count = data.rows.len().min(data.targets.len());
    if sample_count < MIN_FIT_SAMPLES || feature_count == 0 {
        return FittedModel::fallback(sample_count, feature_count);
    }
    let rows = data.rows.get(..sample_count).unwrap_or(data.rows);
    let targets = data.targets.get(..sample_count).unwrap_or(data.targets);
    if let Some(w) = weights
        && (w.len() != sample_count
            || w.iter().any(|v| !v.is_finite() || *v < 0.0)
            || w.iter().sum::<f64>() <= 0.0)
    {
        return FittedModel::fallback(sample_count, feature_count);
    }

    let standardizer = match weights {
        None => Standardizer::from_rows(rows, feature_count),
        Some(w) => Standardizer::from_weighted_rows(rows, w, feature_count),
    };
    let z_rows: Vec<Vec<f64>> = rows.iter().map(|row| standardizer.apply(row)).collect();
    let fit_targets: Vec<f64> = match opts.price_transform {
        PriceTransform::Arcsinh => targets.iter().map(|t| t.asinh()).collect(),
        PriceTransform::None => targets.to_vec(),
    };
    let (ata, aty) = match weights {
        None => normal_equations(&z_rows, &fit_targets, feature_count, opts.ridge_lambda),
        Some(w) => weighted_normal_equations(
            &z_rows,
            &fit_targets,
            Some(w),
            feature_count,
            opts.ridge_lambda,
        ),
    };
    let Some(beta) = solve_spd(&ata, &aty, feature_count + 1) else {
        return FittedModel::fallback(sample_count, feature_count);
    };
    let (intercept, coefficients) = match beta.split_first() {
        Some((&intercept, rest)) => (intercept, rest.to_vec()),
        None => (0.0, Vec::new()),
    };
    FittedModel {
        meta: FitMeta {
            used_fallback: false,
            sample_count,
            feature_count,
        },
        fitted: Fitted::Linear {
            intercept,
            coefficients,
            standardizer,
            transform: opts.price_transform,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_a_known_spd_system() {
        // [[4, 2], [2, 3]] x = [10, 8] has the solution x = [1.75, 1.5].
        let x = solve_spd(&[4.0, 2.0, 2.0, 3.0], &[10.0, 8.0], 2).unwrap();
        assert!((x[0] - 1.75).abs() < 1e-12);
        assert!((x[1] - 1.5).abs() < 1e-12);
    }

    #[test]
    fn solves_a_three_by_three_system() {
        // A = Lᵀ L for L = [[2,0,0],[1,3,0],[0.5,1,4]]; x = [1, -2, 0.5].
        let a = [4.0, 2.0, 1.0, 2.0, 10.0, 3.5, 1.0, 3.5, 17.25];
        let expected = [1.0, -2.0, 0.5];
        let b: Vec<f64> = (0..3)
            .map(|i| (0..3).map(|k| a[i * 3 + k] * expected[k]).sum())
            .collect();
        let x = solve_spd(&a, &b, 3).unwrap();
        for (got, want) in x.iter().zip(expected) {
            assert!((got - want).abs() < 1e-12, "{got} vs {want}");
        }
    }

    #[test]
    fn rejects_a_singular_system() {
        assert!(solve_spd(&[1.0, 1.0, 1.0, 1.0], &[1.0, 1.0], 2).is_none());
        assert!(solve_spd(&[0.0], &[1.0], 1).is_none());
    }

    #[test]
    fn rejects_a_matrix_that_is_not_positive_definite() {
        assert!(solve_spd(&[1.0, 2.0, 2.0, 1.0], &[1.0, 1.0], 2).is_none());
    }

    #[test]
    fn a_zero_variance_column_standardizes_to_zero() {
        let rows = vec![vec![1.0, 5.0], vec![3.0, 5.0]];
        let standardizer = Standardizer::from_rows(&rows, 2);
        assert_eq!(standardizer.means, vec![2.0, 5.0]);
        assert_eq!(standardizer.scales, vec![1.0, 1.0]);
        assert_eq!(standardizer.apply(&[3.0, 5.0]), vec![1.0, 0.0]);
    }

    fn toy_rows() -> (Vec<Vec<f64>>, Vec<f64>) {
        let rows: Vec<Vec<f64>> = (0..40_u8)
            .map(|i| {
                let x = f64::from(i);
                vec![x, (x * 0.7).sin(), x % 3.0]
            })
            .collect();
        let targets = rows
            .iter()
            .map(|r| 2.0 + 0.5 * r[0] - 3.0 * r[1] + r[2] * 0.25 + (r[0] * 1.3).cos())
            .collect();
        (rows, targets)
    }

    const NO_TRANSFORM: FitOptions = FitOptions {
        ridge_lambda: 2.0,
        price_transform: PriceTransform::None,
    };

    #[test]
    fn unit_weights_give_the_unweighted_fit() {
        let (rows, targets) = toy_rows();
        let data = TrainingData {
            feature_count: 3,
            rows: &rows,
            targets: &targets,
        };
        let plain = fit_ridge(&data, &NO_TRANSFORM);
        let weighted = fit_ridge_weighted(&data, &vec![1.0; rows.len()], &NO_TRANSFORM);
        for row in &rows {
            assert_eq!(
                plain.predict(row).to_bits(),
                weighted.predict(row).to_bits()
            );
        }
    }

    #[test]
    fn an_integer_weight_equals_a_repeated_row() {
        let (rows, targets) = toy_rows();
        let repeats: Vec<u8> = (0..rows.len())
            .map(|i| if i % 4 == 0 { 3 } else { 1 })
            .collect();
        let weights: Vec<f64> = repeats.iter().map(|r| f64::from(*r)).collect();
        let mut repeated_rows = Vec::new();
        let mut repeated_targets = Vec::new();
        for ((row, target), r) in rows.iter().zip(&targets).zip(&repeats) {
            for _ in 0..*r {
                repeated_rows.push(row.clone());
                repeated_targets.push(*target);
            }
        }
        let weighted = fit_ridge_weighted(
            &TrainingData {
                feature_count: 3,
                rows: &rows,
                targets: &targets,
            },
            &weights,
            &NO_TRANSFORM,
        );
        let repeated = fit_ridge(
            &TrainingData {
                feature_count: 3,
                rows: &repeated_rows,
                targets: &repeated_targets,
            },
            &NO_TRANSFORM,
        );
        for row in &rows {
            assert!((weighted.predict(row) - repeated.predict(row)).abs() < 1e-9);
        }
    }

    #[test]
    fn bad_weights_give_the_fallback() {
        let (rows, targets) = toy_rows();
        let data = TrainingData {
            feature_count: 3,
            rows: &rows,
            targets: &targets,
        };
        let n = rows.len();
        for weights in [
            vec![1.0; n - 1],
            vec![0.0; n],
            {
                let mut w = vec![1.0; n];
                w[3] = -1.0;
                w
            },
            {
                let mut w = vec![1.0; n];
                w[5] = f64::NAN;
                w
            },
        ] {
            assert!(
                fit_ridge_weighted(&data, &weights, &NO_TRANSFORM)
                    .meta
                    .used_fallback
            );
        }
    }

    #[test]
    fn the_intercept_is_not_penalized() {
        let (ata, aty) = normal_equations(&[vec![1.0], vec![-1.0]], &[2.0, 4.0], 1, 10.0);
        assert_eq!(ata, vec![2.0, 0.0, 0.0, 12.0]);
        assert_eq!(aty, vec![6.0, -2.0]);
    }
}
