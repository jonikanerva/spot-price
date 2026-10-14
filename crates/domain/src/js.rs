//! JavaScript number and string semantics that the API contract depends on.
//!
//! The API rounds with `Math.round`, reads query numbers with `Number(...)`,
//! and writes numbers into error text with `String(...)`. These functions
//! give the same results bit for bit, so the values and the messages stay
//! the same.

use std::cmp::Ordering;

/// `Math.round(x)`: the nearest integer, and a tie goes toward +∞.
///
/// The result keeps the JavaScript sign of zero: an input in `[-0.5, -0]`
/// gives `-0`. `f64::round` differs, because it sends a tie away from zero.
#[must_use]
pub fn math_round(x: f64) -> f64 {
    if !x.is_finite() || x.fract() == 0.0 {
        return x;
    }
    let floor = x.floor();
    // `x - floor` is exact here, because `|x| < 2^52` when `x` has a fraction.
    let rounded = if x - floor >= 0.5 { floor + 1.0 } else { floor };
    if rounded == 0.0 && x < 0.0 {
        -0.0
    } else {
        rounded
    }
}

/// `Math.round(x * 1000) / 1000`: rounds to three decimals like the API.
#[must_use]
pub fn round3(x: f64) -> f64 {
    math_round(x * 1000.0) / 1000.0
}

/// `Math.max(0, x)` for a number that is not NaN: `-0` gives `+0`.
#[must_use]
pub fn max_zero(x: f64) -> f64 {
    if x > 0.0 || x.is_nan() { x } else { 0.0 }
}

/// Whether `c` is `WhiteSpace` or `LineTerminator` in ECMAScript.
///
/// The set is not `char::is_whitespace`: JavaScript trims U+FEFF and keeps
/// U+0085.
#[must_use]
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
#[must_use]
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_whitespace)
}

/// `Number(text)` for a string. An invalid string gives NaN.
#[must_use]
pub fn string_to_number(text: &str) -> f64 {
    let text = js_trim(text);
    if text.is_empty() {
        return 0.0;
    }
    match text {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let bytes = text.as_bytes();
    if let [b'0', prefix, digits @ ..] = bytes {
        let bits_per_digit = match prefix {
            b'x' | b'X' => Some(4),
            b'o' | b'O' => Some(3),
            b'b' | b'B' => Some(1),
            _ => None,
        };
        if let Some(bits_per_digit) = bits_per_digit {
            return radix_to_number(digits, bits_per_digit);
        }
    }
    if is_decimal_literal(bytes) {
        // The grammar check above accepts only forms that Rust parses the same
        // way, and Rust rounds a decimal to the nearest double like ECMAScript.
        text.parse::<f64>().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    }
}

/// `StrDecimalLiteral` without `Infinity`: `[+-]? (d+ (. d*)? | . d+) ([eE] [+-]? d+)?`.
fn is_decimal_literal(bytes: &[u8]) -> bool {
    let rest = without_sign(bytes);
    let integer_digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    let mut rest = &rest[integer_digits..];
    let mut fraction_digits = 0;
    if let [b'.', after @ ..] = rest {
        fraction_digits = after.iter().take_while(|b| b.is_ascii_digit()).count();
        rest = &after[fraction_digits..];
    }
    if integer_digits == 0 && fraction_digits == 0 {
        return false;
    }
    match rest {
        [] => true,
        [b'e' | b'E', exponent @ ..] => {
            let exponent = without_sign(exponent);
            !exponent.is_empty() && exponent.iter().all(u8::is_ascii_digit)
        }
        _ => false,
    }
}

fn without_sign(bytes: &[u8]) -> &[u8] {
    match bytes {
        [b'+' | b'-', rest @ ..] => rest,
        _ => bytes,
    }
}

/// The value of hex, octal, or binary digits, rounded half to even like V8.
fn radix_to_number(digits: &[u8], bits_per_digit: u32) -> f64 {
    let radix = 1_u32 << bits_per_digit;
    let mut bits: Vec<bool> = Vec::with_capacity(digits.len() * 4);
    for &digit in digits {
        let Some(value) = char::from(digit).to_digit(radix) else {
            return f64::NAN;
        };
        for shift in (0..bits_per_digit).rev() {
            bits.push((value >> shift) & 1 == 1);
        }
    }
    if digits.is_empty() {
        return f64::NAN;
    }
    let significant = match bits.iter().position(|&bit| bit) {
        Some(first_one) => &bits[first_one..],
        None => return 0.0,
    };
    let mut mantissa = significant
        .iter()
        .take(53)
        .fold(0_u64, |acc, &bit| (acc << 1) | u64::from(bit));
    let Ok(length) = i32::try_from(significant.len()) else {
        return f64::INFINITY;
    };
    let mut exponent = (length - 53).max(0);
    if let [round_bit, sticky @ ..] = significant.get(53..).unwrap_or_default() {
        let sticky = sticky.iter().any(|&bit| bit);
        if *round_bit && (sticky || mantissa & 1 == 1) {
            mantissa += 1;
            if mantissa == 1 << 53 {
                mantissa >>= 1;
                exponent += 1;
            }
        }
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "the mantissa has at most 53 significant bits"
    )]
    let mantissa = mantissa as f64;
    mantissa * power_of_two(exponent)
}

/// `2^exponent` for `exponent >= 0`; infinity above the double range.
fn power_of_two(exponent: i32) -> f64 {
    if exponent > 1023 {
        return f64::INFINITY;
    }
    let biased = u64::from(exponent.unsigned_abs()) + 1023;
    f64::from_bits(biased << 52)
}

/// `String(x)` for a number (`Number.prototype.toString` with radix 10).
#[must_use]
pub fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_owned();
    }
    if x == 0.0 {
        return "0".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if x < 0.0 {
        return format!("-{}", number_to_string(-x));
    }
    // `{:e}` gives the shortest digit count that round-trips. When two digit
    // strings of that length round-trip, ECMAScript takes the one closest to
    // `x`, which is the correctly rounded text with that precision.
    let shortest = format!("{x:e}");
    let precision = shortest
        .split_once('e')
        .map_or(0, |(mantissa, _)| {
            mantissa.chars().filter(char::is_ascii_digit).count()
        })
        .saturating_sub(1);
    let closest = format!("{x:.precision$e}");
    let scientific = if closest
        .parse::<f64>()
        .is_ok_and(|parsed| parsed.to_bits() == x.to_bits())
    {
        closest
    } else {
        shortest
    };
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i64 = exponent.parse().unwrap_or(0);
    let k = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    let n = exponent + 1;
    if k <= n && n <= 21 {
        let zeros = usize::try_from(n - k).unwrap_or(0);
        format!("{digits}{}", "0".repeat(zeros))
    } else if 0 < n && n <= 21 {
        let split = usize::try_from(n).unwrap_or(0);
        format!("{}.{}", &digits[..split], &digits[split..])
    } else if -6 < n && n <= 0 {
        let zeros = usize::try_from(-n).unwrap_or(0);
        format!("0.{}{digits}", "0".repeat(zeros))
    } else {
        let sign = if n - 1 < 0 { '-' } else { '+' };
        let magnitude = (n - 1).unsigned_abs();
        match digits.split_at_checked(1) {
            Some((first, "")) => format!("{first}e{sign}{magnitude}"),
            Some((first, rest)) => format!("{first}.{rest}e{sign}{magnitude}"),
            None => format!("{digits}e{sign}{magnitude}"),
        }
    }
}

/// `a < b`, `a == b`, or `a > b` for JavaScript strings, which compare UTF-16
/// code units. Rust's `str` order compares code points, which differs when
/// one side has a character above U+FFFF.
#[must_use]
pub fn compare_utf16(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `Number(text.slice(start, end))`, where the indexes count UTF-16 code units.
///
/// A slice that cuts a surrogate pair, or keeps a whole one, is not numeric,
/// so it gives NaN without a decode.
#[must_use]
pub fn number_of_utf16_slice(text: &str, start: usize, end: usize) -> f64 {
    let units: Vec<u16> = text.encode_utf16().collect();
    let end = end.min(units.len());
    let start = start.min(end);
    let slice = &units[start..end];
    if slice.iter().any(|unit| (0xD800..=0xDFFF).contains(unit)) {
        return f64::NAN;
    }
    String::from_utf16(slice).map_or(f64::NAN, |text| string_to_number(&text))
}

const MS_PER_DAY: f64 = 86_400_000.0;

/// `Date.UTC(year, month, day)` at midnight: milliseconds since the epoch, or
/// NaN. A year in `0..=99` means `1900 + year`, a month or day outside its
/// range rolls into the next unit, and a fraction is truncated.
#[must_use]
pub fn date_utc(year: f64, month: f64, day: f64) -> f64 {
    if !(year.is_finite() && month.is_finite() && day.is_finite()) {
        return f64::NAN;
    }
    let whole_year = year.trunc();
    let year = if (0.0..=99.0).contains(&whole_year) {
        1900.0 + whole_year
    } else {
        year
    };
    let (year, month, day) = (year.trunc(), month.trunc(), day.trunc());
    let year = year + (month / 12.0).floor();
    let month = month.rem_euclid(12.0);
    // A year this far out is past the `Date` range anyway.
    if year.abs() > 1_000_000.0 || day.abs() > 1e9 {
        return f64::NAN;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "year and month are whole numbers checked against the bounds above"
    )]
    let days = days_from_civil(year as i64, month as i64 + 1);
    #[expect(
        clippy::cast_precision_loss,
        reason = "the day count is far below 2^53"
    )]
    let time = (days as f64 + day - 1.0) * MS_PER_DAY;
    if time.abs() > 8.64e15 {
        f64::NAN
    } else {
        time + 0.0
    }
}

/// Days from 1970-01-01 to the first day of `month` (1..=12) in `year`.
fn days_from_civil(year: i64, month: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_round_sends_ties_up_and_keeps_negative_zero() {
        assert_eq!(math_round(0.5).to_bits(), 1.0_f64.to_bits());
        assert_eq!(math_round(-0.5).to_bits(), (-0.0_f64).to_bits());
        assert_eq!(math_round(-1.5).to_bits(), (-1.0_f64).to_bits());
        assert_eq!(math_round(-0.2).to_bits(), (-0.0_f64).to_bits());
        assert_eq!(
            math_round(0.499_999_999_999_999_94).to_bits(),
            0.0_f64.to_bits()
        );
        assert_eq!(math_round(2.5).to_bits(), 3.0_f64.to_bits());
    }

    #[test]
    fn round3_of_known_values() {
        assert_eq!(round3(2.675).to_bits(), 2.675_f64.to_bits());
        assert_eq!(round3(-0.0005).to_bits(), (-0.0_f64).to_bits());
        assert_eq!(round3(1.0005).to_bits(), 1.001_f64.to_bits());
    }

    #[test]
    fn max_zero_returns_positive_zero() {
        assert_eq!(max_zero(-0.0).to_bits(), 0.0_f64.to_bits());
        assert_eq!(max_zero(-3.0).to_bits(), 0.0_f64.to_bits());
        assert_eq!(max_zero(3.0).to_bits(), 3.0_f64.to_bits());
    }

    #[test]
    fn number_parsing_follows_javascript() {
        assert_eq!(string_to_number(" 0x3C ").to_bits(), 60.0_f64.to_bits());
        assert!(string_to_number("-0x3C").is_nan());
        assert!(string_to_number("inf").is_nan());
        assert!(string_to_number("\u{85}60").is_nan());
        assert_eq!(string_to_number("\u{feff}60").to_bits(), 60.0_f64.to_bits());
        assert_eq!(string_to_number("").to_bits(), 0.0_f64.to_bits());
        assert_eq!(string_to_number("5.").to_bits(), 5.0_f64.to_bits());
    }

    #[test]
    fn number_to_string_uses_javascript_layout() {
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(1e-7), "1e-7");
        assert_eq!(number_to_string(-0.0), "0");
        assert_eq!(number_to_string(12.5), "12.5");
        assert_eq!(number_to_string(0.000_001), "0.000001");
    }

    #[test]
    fn date_utc_maps_two_digit_years() {
        assert_eq!(date_utc(0.0, 0.0, 1.0), date_utc(1900.0, 0.0, 1.0));
        assert_eq!(date_utc(1970.0, 0.0, 1.0).to_bits(), 0.0_f64.to_bits());
        assert_eq!(date_utc(2026.0, 1.0, 29.0), date_utc(2026.0, 2.0, 1.0));
        assert!(date_utc(f64::NAN, 0.0, 1.0).is_nan());
    }
}
