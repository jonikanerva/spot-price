//! Usernames: 3 to 32 characters of `a-z`, `0-9`, `_`, and `-`, after the
//! input is trimmed and lowercased.

use spot_price_domain::js::js_trim;

/// A valid, normalized username.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Username(String);

impl Username {
    /// The username of the input text, or `None` when the normalized text
    /// breaks the rules.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let normalized = normalize(input);
        let valid = (3..=32).contains(&normalized.len())
            && normalized.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
            });
        valid.then_some(Self(normalized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// JavaScript `value.trim().toLowerCase()`.
#[must_use]
pub fn normalize(input: &str) -> String {
    js_trim(input).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_checks_the_rules() {
        assert_eq!(Username::parse("  Joni_K-9 ").unwrap().as_str(), "joni_k-9");
        assert!(Username::parse("ab").is_none());
        assert!(Username::parse(&"a".repeat(32)).is_some());
        assert!(Username::parse(&"a".repeat(33)).is_none());
        assert!(Username::parse("a b c").is_none());
        assert!(Username::parse("äää").is_none());
        // U+212A KELVIN SIGN lowercases to an ASCII "k", like in JavaScript.
        assert_eq!(Username::parse("\u{212a}ey").unwrap().as_str(), "key");
    }
}
