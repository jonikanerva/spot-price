//! Random values from the operating system: ids, tokens, keys, and salts.

/// The operating system gave no random bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no random bytes are available")]
pub struct RandomError;

/// `N` random bytes.
///
/// # Errors
///
/// Returns [`RandomError`] when the operating system gives no random bytes.
pub fn bytes<const N: usize>() -> Result<[u8; N], RandomError> {
    let mut buffer = [0_u8; N];
    getrandom::fill(&mut buffer).map_err(|_| RandomError)?;
    Ok(buffer)
}

/// Lowercase hexadecimal text of `bytes`, like Node `toString("hex")`.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// The bytes of lowercase or uppercase hexadecimal text of exactly `N` bytes.
#[must_use]
pub fn from_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let digits = text.as_bytes();
    if digits.len() != N * 2 {
        return None;
    }
    let mut out = [0_u8; N];
    for (slot, pair) in out.iter_mut().zip(digits.as_chunks::<2>().0) {
        let high = char::from(pair[0]).to_digit(16)?;
        let low = char::from(pair[1]).to_digit(16)?;
        *slot = u8::try_from(high * 16 + low).ok()?;
    }
    Some(out)
}

/// The alphabet of Better Auth `generateId`, in its order.
const ID_ALPHABET: &[u8; 62] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
/// The length of a Better Auth id.
pub const ID_LENGTH: usize = 32;

/// A new id in the Better Auth format: 32 characters of `a-zA-Z0-9`. A
/// random byte at or above 248 is skipped, so each character is uniform.
///
/// # Errors
///
/// Returns [`RandomError`] when the operating system gives no random bytes.
pub fn better_auth_id() -> Result<String, RandomError> {
    const ACCEPT_BELOW: u8 = 248;
    let mut id = String::with_capacity(ID_LENGTH);
    while id.len() < ID_LENGTH {
        for byte in bytes::<64>()? {
            if byte < ACCEPT_BELOW && id.len() < ID_LENGTH {
                id.push(char::from(ID_ALPHABET[usize::from(byte % 62)]));
            }
        }
    }
    Ok(id)
}

/// A new API key: `sp_` and 32 random bytes as hexadecimal text.
///
/// # Errors
///
/// Returns [`RandomError`] when the operating system gives no random bytes.
pub fn api_key() -> Result<String, RandomError> {
    Ok(format!("sp_{}", hex(&bytes::<32>()?)))
}

/// The id of a new API key row: 16 random bytes as hexadecimal text.
///
/// # Errors
///
/// Returns [`RandomError`] when the operating system gives no random bytes.
pub fn api_key_id() -> Result<String, RandomError> {
    Ok(hex(&bytes::<16>()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(from_hex::<4>("000FA5ff"), Some([0x00, 0x0f, 0xa5, 0xff]));
        assert_eq!(from_hex::<4>("000fa5f"), None);
        assert_eq!(from_hex::<2>("zz00"), None);
        assert_eq!(from_hex::<1>("+f"), None);
    }

    #[test]
    fn ids_have_the_better_auth_format() {
        let id = better_auth_id().unwrap();
        assert_eq!(id.len(), ID_LENGTH);
        assert!(id.bytes().all(|byte| byte.is_ascii_alphanumeric()));
        assert_ne!(id, better_auth_id().unwrap());
    }

    #[test]
    fn api_keys_have_the_prefix_and_64_hex_digits() {
        let key = api_key().unwrap();
        let digits = key.strip_prefix("sp_").unwrap();
        assert_eq!(digits.len(), 64);
        assert!(from_hex::<32>(digits).is_some());
        assert_eq!(api_key_id().unwrap().len(), 32);
    }
}
