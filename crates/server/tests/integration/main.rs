//! Integration tests. They need PostgreSQL: `DATABASE_URL` must point to an
//! isolated server where the test user can create databases.
//! `scripts/test-db.sh start` starts one. Each `#[sqlx::test]` gets its own
//! fresh database. No test calls a live upstream: the adapters talk to a
//! local fake server.

#![expect(
    clippy::unwrap_used,
    reason = "the helpers of this test binary fail a check by panicking"
)]

mod common;
mod health;
mod jobs;
mod migrations;
mod scheduler;
mod stores;
mod upstream;
