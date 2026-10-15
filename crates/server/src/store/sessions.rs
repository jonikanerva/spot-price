//! Sessions of the setup UI in `app_sessions`. A row holds the SHA-256 hash
//! of the cookie token, never the token.

use jiff::Timestamp;
use jiff_sqlx::ToSqlx as _;
use sqlx::PgPool;

use crate::db::StoreError;
use crate::store::accounts::UserRow;

/// A session that has not expired, with its user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub user: UserRow,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
}

/// Stores a new session. Expired sessions of all users are deleted in the
/// same transaction, so the table does not grow without a limit.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when a statement fails.
pub async fn create_session(
    pool: &PgPool,
    token_hash: &[u8; 32],
    user_id: &str,
    now: Timestamp,
    expires_at: Timestamp,
) -> Result<(), StoreError> {
    let mut transaction = pool.begin().await?;
    sqlx::query!(
        "DELETE FROM app_sessions WHERE expires_at <= $1",
        now.to_sqlx()
    )
    .execute(&mut *transaction)
    .await?;
    sqlx::query!(
        "INSERT INTO app_sessions (token_hash, user_id, created_at, expires_at) VALUES ($1, $2, $3, $4)",
        token_hash.as_slice(),
        user_id,
        now.to_sqlx(),
        expires_at.to_sqlx(),
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// The session of a token hash when it expires after `now`.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn find_session(
    pool: &PgPool,
    token_hash: &[u8; 32],
    now: Timestamp,
) -> Result<Option<SessionRow>, StoreError> {
    let row = sqlx::query!(
        r#"
        SELECT s.created_at, s.expires_at,
               u.id, u.name, u.email, u."emailVerified" AS email_verified, u.image,
               u."createdAt" AS user_created_at, u."updatedAt" AS user_updated_at
        FROM app_sessions s
        JOIN "user" u ON u.id = s.user_id
        WHERE s.token_hash = $1 AND s.expires_at > $2
        "#,
        token_hash.as_slice(),
        now.to_sqlx(),
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| SessionRow {
        user: UserRow {
            id: row.id,
            name: row.name,
            email: row.email,
            email_verified: row.email_verified,
            image: row.image,
            created_at: row.user_created_at.to_jiff(),
            updated_at: row.user_updated_at.to_jiff(),
        },
        created_at: row.created_at.to_jiff(),
        expires_at: row.expires_at.to_jiff(),
    }))
}

/// Moves the expiry of a session.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the statement fails.
pub async fn extend_session(
    pool: &PgPool,
    token_hash: &[u8; 32],
    expires_at: Timestamp,
) -> Result<(), StoreError> {
    sqlx::query!(
        "UPDATE app_sessions SET expires_at = $2 WHERE token_hash = $1",
        token_hash.as_slice(),
        expires_at.to_sqlx(),
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Deletes a session. A missing session is not an error.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the statement fails.
pub async fn delete_session(pool: &PgPool, token_hash: &[u8; 32]) -> Result<(), StoreError> {
    sqlx::query!(
        "DELETE FROM app_sessions WHERE token_hash = $1",
        token_hash.as_slice()
    )
    .execute(pool)
    .await?;
    Ok(())
}
