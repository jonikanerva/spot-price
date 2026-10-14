//! The API key of each user in `api_keys`. The key is stored as plaintext,
//! because the setup UI shows it again (VISION.md, Persistence and Privacy
//! Posture). A user has at most one key.

use jiff::Timestamp;
use jiff_sqlx::ToSqlx as _;
use sqlx::PgPool;

use crate::db::StoreError;

/// The current key of a user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyRow {
    pub key: String,
    pub created_at: Timestamp,
}

/// The newest key of a user.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn current_api_key(
    pool: &PgPool,
    user_id: &str,
) -> Result<Option<ApiKeyRow>, StoreError> {
    let row = sqlx::query!(
        r#"
        SELECT key_plaintext, created_at FROM api_keys
        WHERE user_id = $1 ORDER BY created_at DESC LIMIT 1
        "#,
        user_id
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| ApiKeyRow {
        key: row.key_plaintext,
        created_at: row.created_at.to_jiff(),
    }))
}

/// Deletes every key of the user and stores the new key, in one
/// transaction.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when a statement fails.
pub async fn replace_api_key(
    pool: &PgPool,
    user_id: &str,
    id: &str,
    key: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let mut transaction = pool.begin().await?;
    sqlx::query!("DELETE FROM api_keys WHERE user_id = $1", user_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query!(
        "INSERT INTO api_keys (id, user_id, key_plaintext, created_at) VALUES ($1, $2, $3, $4)",
        id,
        user_id,
        key,
        now.to_sqlx(),
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// The user of a key. Sets `last_used_at` of a known key to `now`.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the statement fails.
pub async fn resolve_api_key(
    pool: &PgPool,
    key: &str,
    now: Timestamp,
) -> Result<Option<String>, StoreError> {
    let user_id = sqlx::query_scalar!(
        "UPDATE api_keys SET last_used_at = $2 WHERE key_plaintext = $1 RETURNING user_id",
        key,
        now.to_sqlx(),
    )
    .fetch_optional(pool)
    .await?;
    Ok(user_id)
}
