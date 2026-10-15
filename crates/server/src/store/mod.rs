//! PostgreSQL stores of prices, Fingrid series, weather forecasts, user
//! settings, accounts, sessions, and API keys. Every function takes the
//! pool and fails with [`crate::db::StoreError`].
//!
//! All instants are UTC [`jiff::Timestamp`] values. A range `[start, end)`
//! includes `start` and excludes `end`.

pub mod accounts;
pub mod api_keys;
pub mod fingrid;
pub mod prices;
pub mod sessions;
pub mod user_settings;
pub mod weather;
