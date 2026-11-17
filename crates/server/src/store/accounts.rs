//! User accounts in the Better Auth tables `"user"` and `"account"`, and the
//! `usernames` table.
//!
//! The rows have the Better Auth format of the existing accounts: the e-mail
//! is `<username>@local.spot`, the credential account has
//! `providerId = 'credential'` and `accountId = userId`, and the password is
//! a Better Auth scrypt hash.

use jiff::Timestamp;
use jiff_sqlx::ToSqlx as _;
use sqlx::PgPool;

use crate::db::StoreError;
use crate::store::user_settings::insert_default_settings;

/// One `"user"` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow {
    pub id: String,
    pub name: String,
    pub email: String,
    pub email_verified: bool,
    pub image: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A user and the password hash of the credential account. The hash is
/// `None` when the user has no credential account or no password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialUser {
    pub user: UserRow,
    pub password_hash: Option<String>,
}

/// The values of a new account.
#[derive(Debug, Clone, Copy)]
pub struct NewAccount<'a> {
    pub user_id: &'a str,
    pub account_id: &'a str,
    pub username: &'a str,
    pub email: &'a str,
    pub password_hash: &'a str,
    pub now: Timestamp,
}

/// Why an account was not created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CreateAccountError {
    /// The e-mail, the username, or the id is already in use.
    #[error("the account already exists")]
    Conflict,
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The user id of a normalized username.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn user_id_by_username(
    pool: &PgPool,
    username: &str,
) -> Result<Option<String>, StoreError> {
    let id = sqlx::query_scalar!(
        "SELECT user_id FROM usernames WHERE username = $1",
        username
    )
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

/// The username of a user.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn username_by_user_id(
    pool: &PgPool,
    user_id: &str,
) -> Result<Option<String>, StoreError> {
    let name = sqlx::query_scalar!("SELECT username FROM usernames WHERE user_id = $1", user_id)
        .fetch_optional(pool)
        .await?;
    Ok(name)
}

/// The number of users.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn count_users(pool: &PgPool) -> Result<i64, StoreError> {
    let count = sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM "user""#)
        .fetch_one(pool)
        .await?;
    Ok(count)
}

/// Whether a user with this e-mail exists.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn email_exists(pool: &PgPool, email: &str) -> Result<bool, StoreError> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM "user" WHERE email = $1) AS "exists!""#,
        email
    )
    .fetch_one(pool)
    .await?;
    Ok(exists)
}

/// The user with this e-mail and the password hash of its credential account.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn credential_user_by_email(
    pool: &PgPool,
    email: &str,
) -> Result<Option<CredentialUser>, StoreError> {
    let row = sqlx::query!(
        r#"
        SELECT u.id, u.name, u.email, u."emailVerified" AS email_verified, u.image,
               u."createdAt" AS created_at, u."updatedAt" AS updated_at,
               (SELECT a.password FROM account a
                 WHERE a."userId" = u.id AND a."providerId" = 'credential'
                 ORDER BY a."createdAt" LIMIT 1) AS password_hash
        FROM "user" u
        WHERE u.email = $1
        "#,
        email
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| CredentialUser {
        user: UserRow {
            id: row.id,
            name: row.name,
            email: row.email,
            email_verified: row.email_verified,
            image: row.image,
            created_at: row.created_at.to_jiff(),
            updated_at: row.updated_at.to_jiff(),
        },
        password_hash: row.password_hash,
    }))
}

/// Creates the user, the credential account, the username, the default
/// settings, and their first terms version in one transaction. Nothing is written when a step fails.
///
/// # Errors
///
/// Returns [`CreateAccountError::Conflict`] when a unique value is already
/// in use, and [`CreateAccountError::Store`] when a statement fails.
pub async fn create_account(
    pool: &PgPool,
    new: NewAccount<'_>,
) -> Result<UserRow, CreateAccountError> {
    let now = new.now.to_sqlx();
    let mut transaction = pool.begin().await.map_err(StoreError::from)?;
    sqlx::query!(
        r#"
        INSERT INTO "user" (id, name, email, "emailVerified", image, "createdAt", "updatedAt")
        VALUES ($1, $2, $3, false, NULL, $4, $4)
        "#,
        new.user_id,
        new.username,
        new.email,
        now,
    )
    .execute(&mut *transaction)
    .await
    .map_err(|error| conflict_or_query(&error))?;
    sqlx::query!(
        r#"
        INSERT INTO account (id, "accountId", "providerId", "userId", password, "createdAt", "updatedAt")
        VALUES ($1, $2, 'credential', $2, $3, $4, $4)
        "#,
        new.account_id,
        new.user_id,
        new.password_hash,
        now,
    )
    .execute(&mut *transaction)
    .await
    .map_err(|error| conflict_or_query(&error))?;
    sqlx::query!(
        "INSERT INTO usernames (user_id, username, created_at) VALUES ($1, $2, $3)",
        new.user_id,
        new.username,
        now,
    )
    .execute(&mut *transaction)
    .await
    .map_err(|error| conflict_or_query(&error))?;
    insert_default_settings(&mut *transaction, new.user_id, new.now).await?;
    transaction.commit().await.map_err(StoreError::from)?;
    Ok(UserRow {
        id: new.user_id.to_owned(),
        name: new.username.to_owned(),
        email: new.email.to_owned(),
        email_verified: false,
        image: None,
        created_at: new.now,
        updated_at: new.now,
    })
}

fn conflict_or_query(error: &sqlx::Error) -> CreateAccountError {
    match error {
        sqlx::Error::Database(database) if database.is_unique_violation() => {
            CreateAccountError::Conflict
        }
        _ => CreateAccountError::Store(StoreError::Query),
    }
}
