//! The spot-price service: the REST API, the setup UI, and the scheduled
//! fetch jobs of Nord Pool, Fingrid, and `OpenWeatherMap` data.
//!
//! `main.rs` wires these modules. The modules are public so that the
//! integration tests in `tests/` can use them.

pub mod auth;
pub mod clock;
pub mod config;
pub mod db;
pub mod http;
pub mod jobs;
pub mod lifecycle;
pub mod observability;
pub mod store;
pub mod upstream;
