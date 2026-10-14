//! Password hashes in the Better Auth format: `<salt>:<key>`.
//!
//! The salt is 16 random bytes as hexadecimal text, and scrypt uses that
//! TEXT as its salt bytes. The key is scrypt (N = 16384, r = 16, p = 1,
//! 64 bytes) of the NFKC form of the password, as hexadecimal text.

use std::sync::Arc;

use subtle::ConstantTimeEq as _;
use tokio::sync::Semaphore;
use unicode_normalization::UnicodeNormalization as _;

use super::random::{self, RandomError};

/// log2 of the scrypt cost N = 16384.
const LOG_N: u8 = 14;
const BLOCK_SIZE: u32 = 16;
const PARALLELISM: u32 = 1;
const KEY_LENGTH: usize = 64;

/// The most hashes that run at the same time. One hash needs 32 MiB.
pub const MAX_CONCURRENT_HASHES: usize = 2;

/// A password hash could not be computed or read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PasswordError {
    #[error("no random bytes are available")]
    Random,
    #[error("the scrypt computation failed")]
    Scrypt,
    #[error("the stored password hash is malformed")]
    MalformedHash,
    #[error("the hash task stopped")]
    Task,
}

impl From<RandomError> for PasswordError {
    fn from(_: RandomError) -> Self {
        Self::Random
    }
}

/// The scrypt key of `password` with the salt text `salt`.
///
/// # Errors
///
/// Returns [`PasswordError::Scrypt`] when scrypt rejects the parameters.
pub fn derive_key(password: &str, salt: &str) -> Result<[u8; KEY_LENGTH], PasswordError> {
    let normalized: String = password.nfkc().collect();
    let params =
        scrypt::Params::new(LOG_N, BLOCK_SIZE, PARALLELISM).map_err(|_| PasswordError::Scrypt)?;
    let mut key = [0_u8; KEY_LENGTH];
    scrypt::scrypt(normalized.as_bytes(), salt.as_bytes(), &params, &mut key)
        .map_err(|_| PasswordError::Scrypt)?;
    Ok(key)
}

/// The stored hash of `password` with the salt text `salt`.
///
/// # Errors
///
/// Returns [`PasswordError::Scrypt`] when scrypt rejects the parameters.
pub fn hash_with_salt(password: &str, salt: &str) -> Result<String, PasswordError> {
    Ok(format!(
        "{salt}:{}",
        random::hex(&derive_key(password, salt)?)
    ))
}

/// Whether `password` matches the stored hash. The key comparison takes
/// constant time.
///
/// # Errors
///
/// Returns [`PasswordError::MalformedHash`] when the hash has no salt or no
/// key, and [`PasswordError::Scrypt`] when scrypt fails.
pub fn verify_blocking(stored: &str, password: &str) -> Result<bool, PasswordError> {
    let mut parts = stored.split(':');
    let (Some(salt), Some(key)) = (parts.next(), parts.next()) else {
        return Err(PasswordError::MalformedHash);
    };
    if salt.is_empty() || key.is_empty() {
        return Err(PasswordError::MalformedHash);
    }
    let computed = random::hex(&derive_key(password, salt)?);
    Ok(computed.as_bytes().ct_eq(key.as_bytes()).into())
}

/// Runs scrypt on the blocking thread pool, at most
/// [`MAX_CONCURRENT_HASHES`] at a time.
#[derive(Debug, Clone)]
pub struct PasswordHasher {
    permits: Arc<Semaphore>,
}

impl Default for PasswordHasher {
    fn default() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_HASHES)),
        }
    }
}

impl PasswordHasher {
    /// A new stored hash of `password` with a random salt.
    ///
    /// # Errors
    ///
    /// Returns a [`PasswordError`] when no random bytes are available or the
    /// computation fails.
    pub async fn hash(&self, password: String) -> Result<String, PasswordError> {
        let salt = random::hex(&random::bytes::<16>()?);
        self.run(move || hash_with_salt(&password, &salt)).await
    }

    /// Whether `password` matches the stored hash.
    ///
    /// # Errors
    ///
    /// Returns a [`PasswordError`] when the hash is malformed or the
    /// computation fails.
    pub async fn verify(&self, stored: String, password: String) -> Result<bool, PasswordError> {
        self.run(move || verify_blocking(&stored, &password)).await
    }

    async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, PasswordError> + Send + 'static,
    ) -> Result<T, PasswordError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| PasswordError::Task)?;
        tokio::task::spawn_blocking(work)
            .await
            .map_err(|_| PasswordError::Task)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_verifies_its_password_only() {
        let stored = hash_with_salt("correct horse", "00112233445566778899aabbccddeeff").unwrap();
        let (salt, key) = stored.split_once(':').unwrap();
        assert_eq!(salt, "00112233445566778899aabbccddeeff");
        assert_eq!(key.len(), 128);
        assert!(verify_blocking(&stored, "correct horse").unwrap());
        assert!(!verify_blocking(&stored, "correct horsE").unwrap());
    }

    #[test]
    fn the_password_is_nfkc_normalized() {
        let salt = "ffeeddccbbaa99887766554433221100";
        // U+FB01 (LATIN SMALL LIGATURE FI) is "fi" in NFKC.
        assert_eq!(
            hash_with_salt("\u{fb01}ve", salt).unwrap(),
            hash_with_salt("five", salt).unwrap()
        );
    }

    #[test]
    fn a_malformed_hash_is_an_error() {
        assert_eq!(
            verify_blocking("no-separator", "x"),
            Err(PasswordError::MalformedHash)
        );
        assert_eq!(
            verify_blocking(":key", "x"),
            Err(PasswordError::MalformedHash)
        );
    }
}
