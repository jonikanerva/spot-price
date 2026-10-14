//! JavaScript number semantics that the forecast output depends on.

/// `Math.round(x)`: a tie rounds toward +∞, and a result of zero from a
/// negative input is `-0.0`.
pub(crate) fn js_round(x: f64) -> f64 {
    let floor = x.floor();
    let rounded = if x - floor >= 0.5 { floor + 1.0 } else { floor };
    if rounded == 0.0 && x.is_sign_negative() {
        -0.0
    } else {
        rounded
    }
}

/// `Math.round(x * 1000) / 1000`. The result can be `-0.0`; JSON output must
/// write it as `0`, like `JSON.stringify`.
pub(crate) fn round3(x: f64) -> f64 {
    js_round(x * 1000.0) / 1000.0
}

/// `Math.min(a, b)`: a NaN operand gives NaN, and `-0.0` is less than `0.0`.
pub(crate) fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b {
        a
    } else if b < a {
        b
    } else if a.is_sign_negative() {
        // Equal values; only the sign of a zero can differ.
        a
    } else {
        b
    }
}

/// `Math.max(a, b)`: a NaN operand gives NaN, and `0.0` is greater than `-0.0`.
pub(crate) fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b {
        a
    } else if b > a {
        b
    } else if a.is_sign_positive() {
        // Equal values; only the sign of a zero can differ.
        a
    } else {
        b
    }
}

/// `count` as a float. Counts in this module stay far below 2^53.
#[expect(
    clippy::cast_precision_loss,
    reason = "the counts are row and quarter counts, far below 2^53"
)]
pub(crate) const fn count_f64(count: usize) -> f64 {
    count as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_round_matches_math_round() {
        let cases: [(f64, f64); 11] = [
            (2.5, 3.0),
            (-2.5, -2.0),
            (0.499_999_999_999_999_94, 0.0),
            (-0.7, -1.0),
            (-0.500_000_000_000_000_1, -1.0),
            (1.5, 2.0),
            (-1.5, -1.0),
            (4_503_599_627_370_497.0, 4_503_599_627_370_497.0),
            (f64::INFINITY, f64::INFINITY),
            (f64::NEG_INFINITY, f64::NEG_INFINITY),
            (123.456, 123.0),
        ];
        for (input, expected) in cases {
            assert_eq!(js_round(input).to_bits(), expected.to_bits(), "{input}");
        }
        assert!(js_round(f64::NAN).is_nan());
    }

    #[test]
    fn js_round_keeps_negative_zero() {
        for input in [-0.0, -0.3, -0.5, -1e-20] {
            let out = js_round(input);
            assert!(out == 0.0 && out.is_sign_negative(), "{input}");
        }
        assert!(js_round(0.3).is_sign_positive());
    }

    #[test]
    fn round3_rounds_to_thousandths() {
        assert_eq!(round3(1.234_6), 1.235);
        assert_eq!(round3(-1.234_4), -1.234);
        assert!(round3(-0.000_4).is_sign_negative());
    }

    #[test]
    fn js_min_and_max_propagate_nan() {
        assert!(js_min(1.0, f64::NAN).is_nan());
        assert!(js_max(f64::NAN, 1.0).is_nan());
        assert_eq!(js_min(1.0, 2.0), 1.0);
        assert_eq!(js_max(1.0, 2.0), 2.0);
        assert!(js_min(0.0, -0.0).is_sign_negative());
        assert!(js_max(-0.0, 0.0).is_sign_positive());
    }
}
