//! Accounts, passwords, sessions, and API keys of the setup UI.
//!
//! The `"user"`, `"account"`, and `usernames` rows have the Better Auth
//! format of the TypeScript service, so that service can sign these users in
//! again after a revert. Sessions live in `app_sessions` only.

pub mod password;
pub mod random;
pub mod username;

/// The longest and shortest password of a new account, in UTF-16 code units.
pub const MIN_PASSWORD_LENGTH: usize = 8;
pub const MAX_PASSWORD_LENGTH: usize = 128;

/// Registration closes when this many users exist.
pub const MAX_USERS: i64 = 100_000;

/// The domain of the internal e-mail address `<username>@local.spot`.
pub const INTERNAL_EMAIL_DOMAIN: &str = "local.spot";

/// The internal e-mail address of a normalized username.
#[must_use]
pub fn internal_email(username: &username::Username) -> String {
    format!("{}@{INTERNAL_EMAIL_DOMAIN}", username.as_str())
}

/// The length of `text` as JavaScript `String.prototype.length` counts it.
#[must_use]
pub fn js_length(text: &str) -> usize {
    text.encode_utf16().count()
}
