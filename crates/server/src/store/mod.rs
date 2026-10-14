//! PostgreSQL stores of prices, Fingrid series, weather forecasts, and user
//! settings. Every function takes the pool and fails with
//! [`crate::db::StoreError`].
//!
//! All instants are UTC [`jiff::Timestamp`] values. A range `[start, end)`
//! includes `start` and excludes `end`.

pub mod fingrid;
pub mod prices;
pub mod user_settings;
pub mod weather;
