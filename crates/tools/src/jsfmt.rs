//! JavaScript text formats that the tool output keeps: `Number.prototype.toFixed`
//! and `JSON.stringify(value, null, 2)`.

use spot_price_domain::js::number_to_string;

/// `count` as a float. The counts of these tools stay far below 2^53.
#[expect(
    clippy::cast_precision_loss,
    reason = "row, target, and origin counts stay far below 2^53"
)]
#[must_use]
pub const fn count_f64(count: usize) -> f64 {
    count as f64
}

/// A left-to-right sum from `+0`, like `xs.reduce((a, b) => a + b, 0)`.
pub fn js_sum<I: IntoIterator<Item = f64>>(xs: I) -> f64 {
    xs.into_iter().fold(0.0, |acc, x| acc + x)
}

/// More fraction digits than the exact decimal form of any `f64` has.
const EXACT_FRACTION_DIGITS: usize = 1100;

/// `x.toFixed(digits)`. An exact tie rounds the magnitude up, `-0` gives no
/// sign, and a magnitude of at least 1e21 gives `String(x)`.
#[must_use]
pub fn to_fixed(x: f64, digits: usize) -> String {
    if x.is_nan() {
        return "NaN".to_owned();
    }
    if x.abs() >= 1e21 {
        return number_to_string(x);
    }
    let exact = format!("{:.*}", EXACT_FRACTION_DIGITS, x.abs());
    let (int_part, fraction) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut kept: Vec<u8> = int_part.bytes().collect();
    kept.extend(fraction.bytes().take(digits));
    if fraction.as_bytes().get(digits).is_some_and(|d| *d >= b'5') {
        round_up(&mut kept);
    }
    let (int_digits, fraction_digits) = kept.split_at(kept.len() - digits);
    let mut out = String::with_capacity(kept.len() + 2);
    if x < 0.0 {
        out.push('-');
    }
    out.push_str(&String::from_utf8_lossy(int_digits));
    if digits > 0 {
        out.push('.');
        out.push_str(&String::from_utf8_lossy(fraction_digits));
    }
    out
}

/// Adds one unit in the last place of the ASCII decimal digits.
fn round_up(digits: &mut Vec<u8>) {
    for digit in digits.iter_mut().rev() {
        if *digit == b'9' {
            *digit = b'0';
        } else {
            *digit += 1;
            return;
        }
    }
    digits.insert(0, b'1');
}

/// A JSON value with JavaScript number text.
#[derive(Debug, Clone, PartialEq)]
pub enum JsJson {
    Number(f64),
    Text(String),
    Array(Vec<JsJson>),
    /// Members in output order.
    Object(Vec<(String, JsJson)>),
}

impl JsJson {
    /// `JSON.stringify(self, null, 2)`.
    #[must_use]
    pub fn stringify_pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        match self {
            Self::Number(x) if x.is_finite() => out.push_str(&number_to_string(*x)),
            Self::Number(_) => out.push_str("null"),
            Self::Text(text) => write_string(out, text),
            Self::Array(items) if items.is_empty() => out.push_str("[]"),
            Self::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    item.write(out, depth + 1);
                }
                newline(out, depth);
                out.push(']');
            }
            Self::Object(members) if members.is_empty() => out.push_str("{}"),
            Self::Object(members) => {
                out.push('{');
                for (i, (key, value)) in members.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    write_string(out, key);
                    out.push_str(": ");
                    value.write(out, depth + 1);
                }
                newline(out, depth);
                out.push('}');
            }
        }
    }
}

fn newline(out: &mut String, depth: usize) {
    out.push('\n');
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// A JSON string literal with the escapes of `JSON.stringify`.
fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let code = u32::from(c);
                out.push_str("\\u00");
                for nibble in [code >> 4, code & 0xf] {
                    out.extend(char::from_digit(nibble, 16));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_matches_javascript() {
        let cases: [(f64, usize, &str); 14] = [
            (0.0625, 3, "0.063"),
            (0.125, 2, "0.13"),
            (-0.125, 2, "-0.13"),
            (-0.0, 3, "0.000"),
            (-0.0001, 3, "-0.000"),
            (1.005, 2, "1.00"),
            (2.5, 0, "3"),
            (9.9996, 3, "10.000"),
            (0.801_234, 3, "0.801"),
            (21.0, 1, "21.0"),
            (1e21, 3, "1e+21"),
            (f64::INFINITY, 3, "Infinity"),
            (f64::NEG_INFINITY, 1, "-Infinity"),
            (123.456, 1, "123.5"),
        ];
        for (x, digits, want) in cases {
            assert_eq!(to_fixed(x, digits), want, "{x}");
        }
        assert_eq!(to_fixed(f64::NAN, 3), "NaN");
    }

    #[test]
    fn stringify_pretty_matches_json_stringify() {
        let value = JsJson::Object(vec![
            ("a".to_owned(), JsJson::Number(3000.0)),
            ("b".to_owned(), JsJson::Array(vec![])),
            (
                "c".to_owned(),
                JsJson::Array(vec![
                    JsJson::Number(-0.0),
                    JsJson::Text("x\"\u{1}".to_owned()),
                ]),
            ),
            ("d".to_owned(), JsJson::Object(vec![])),
            ("e".to_owned(), JsJson::Number(f64::NAN)),
        ]);
        assert_eq!(
            value.stringify_pretty(),
            "{\n  \"a\": 3000,\n  \"b\": [],\n  \"c\": [\n    0,\n    \"x\\\"\\u0001\"\n  ],\n  \"d\": {},\n  \"e\": null\n}"
        );
    }
}
