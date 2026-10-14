//! The contract settings of each user in the `user_settings` table.

use jiff::Timestamp;
use jiff_sqlx::ToSqlx as _;
use sqlx::PgPool;

use crate::db::StoreError;

/// One stored settings row. `timezone` and `area` are the stored text; the
/// caller validates them against the domain types.
#[derive(Debug, Clone, PartialEq)]
pub struct UserSettingsRow {
    pub user_id: String,
    pub margin_cents_kwh: f64,
    pub transfer_day_cents_kwh: f64,
    pub transfer_night_cents_kwh: f64,
    pub tax_cents_kwh: f64,
    pub vat_percent: f64,
    pub night_start_hour: i32,
    pub night_end_hour: i32,
    pub timezone: String,
    pub area: String,
}

impl UserSettingsRow {
    /// The default settings of a new user: a Finnish contract.
    #[must_use]
    pub fn defaults(user_id: String) -> Self {
        Self {
            user_id,
            margin_cents_kwh: 0.49,
            transfer_day_cents_kwh: 2.92,
            transfer_night_cents_kwh: 1.37,
            tax_cents_kwh: 2.82752,
            vat_percent: 25.5,
            night_start_hour: 22,
            night_end_hour: 7,
            timezone: "Europe/Helsinki".to_owned(),
            area: "FI".to_owned(),
        }
    }
}

/// The settings of `user_id`, if stored.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn get_user_settings(
    pool: &PgPool,
    user_id: &str,
) -> Result<Option<UserSettingsRow>, StoreError> {
    let row = sqlx::query_as!(
        UserSettingsRow,
        r#"
        SELECT user_id, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh,
               tax_cents_kwh, vat_percent, night_start_hour, night_end_hour, timezone, area
        FROM user_settings
        WHERE user_id = $1
        "#,
        user_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Creates or replaces the settings of `settings.user_id`. `now` becomes
/// `updated_at`.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the statement fails.
pub async fn upsert_user_settings(
    pool: &PgPool,
    settings: &UserSettingsRow,
    now: Timestamp,
) -> Result<(), StoreError> {
    sqlx::query!(
        r#"
        INSERT INTO user_settings
          (user_id, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh,
           tax_cents_kwh, vat_percent, night_start_hour, night_end_hour, timezone, area, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        ON CONFLICT (user_id)
        DO UPDATE SET margin_cents_kwh = EXCLUDED.margin_cents_kwh,
                      transfer_day_cents_kwh = EXCLUDED.transfer_day_cents_kwh,
                      transfer_night_cents_kwh = EXCLUDED.transfer_night_cents_kwh,
                      tax_cents_kwh = EXCLUDED.tax_cents_kwh,
                      vat_percent = EXCLUDED.vat_percent,
                      night_start_hour = EXCLUDED.night_start_hour,
                      night_end_hour = EXCLUDED.night_end_hour,
                      timezone = EXCLUDED.timezone,
                      area = EXCLUDED.area,
                      updated_at = EXCLUDED.updated_at
        "#,
        settings.user_id,
        settings.margin_cents_kwh,
        settings.transfer_day_cents_kwh,
        settings.transfer_night_cents_kwh,
        settings.tax_cents_kwh,
        settings.vat_percent,
        settings.night_start_hour,
        settings.night_end_hour,
        settings.timezone,
        settings.area,
        now.to_sqlx(),
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// The settings of `user_id`. Creates the default settings first when none
/// are stored. A concurrent save of the same user is never overwritten.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when a statement fails.
pub async fn ensure_user_settings(
    pool: &PgPool,
    user_id: &str,
    now: Timestamp,
) -> Result<UserSettingsRow, StoreError> {
    if let Some(existing) = get_user_settings(pool, user_id).await? {
        return Ok(existing);
    }
    let defaults = UserSettingsRow::defaults(user_id.to_owned());
    sqlx::query!(
        r#"
        INSERT INTO user_settings
          (user_id, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh,
           tax_cents_kwh, vat_percent, night_start_hour, night_end_hour, timezone, area, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        ON CONFLICT (user_id) DO NOTHING
        "#,
        defaults.user_id,
        defaults.margin_cents_kwh,
        defaults.transfer_day_cents_kwh,
        defaults.transfer_night_cents_kwh,
        defaults.tax_cents_kwh,
        defaults.vat_percent,
        defaults.night_start_hour,
        defaults.night_end_hour,
        defaults.timezone,
        defaults.area,
        now.to_sqlx(),
    )
    .execute(pool)
    .await?;
    get_user_settings(pool, user_id)
        .await?
        .ok_or(StoreError::InvalidRow)
}
