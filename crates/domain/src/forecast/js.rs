//! JavaScript number semantics that the forecast output depends on and
//! that `crate::js` does not have.

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
    fn js_min_and_max_propagate_nan() {
        assert!(js_min(1.0, f64::NAN).is_nan());
        assert!(js_max(f64::NAN, 1.0).is_nan());
        assert_eq!(js_min(1.0, 2.0), 1.0);
        assert_eq!(js_max(1.0, 2.0), 2.0);
        assert!(js_min(0.0, -0.0).is_sign_negative());
        assert!(js_max(-0.0, 0.0).is_sign_positive());
    }
}
