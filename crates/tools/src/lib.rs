//! Offline tools of the FI price forecast: the issue-time backtest and its
//! evaluation report, the band artifact regeneration, and the Fingrid
//! revision study. Never part of the server build (STACK.md §0).
//! `data/fixture.json` is a fixed synthetic input (2026-03-02, 96 quarters,
//! vintage leads 20 h and 1 h, LCG seed 1234567); no tool regenerates it.

pub mod backtest;
pub mod backtest_cli;
pub mod bands;
pub mod bootstrap;
pub mod cli;
pub mod correction;
pub mod criteria;
pub mod data;
pub mod db;
pub mod error;
pub mod eval_cli;
pub mod evaluation;
pub mod jsfmt;
pub mod metrics;
pub mod profile;
pub mod revision;
pub mod revision_cli;
pub mod schedule;
