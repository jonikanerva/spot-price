//! The contract settings of each user: the time zone and the area in
//! `user_settings`, and the versions of the contract terms in
//! `contract_terms`. The term columns of `user_settings` are not used.

use jiff::Timestamp;
use jiff::civil::Date;
use jiff_sqlx::ToSqlx as _;
use spot_price_domain::settings::{
    EARLIEST_VALID_FROM, MAX_TERMS_VERSIONS, SettingsUpdate, applicable_version,
};
use spot_price_domain::time::{UserTimeZone, local_date};
use sqlx::PgPool;

use crate::db::StoreError;

/// One stored version of the contract terms. It applies from the local
/// calendar date `valid_from`. The money values include VAT at
/// `vat_percent`; the columns have the same names without `incl_vat`.
#[derive(Debug, Clone, PartialEq)]
pub struct ContractTermsRow {
    pub valid_from: Date,
    pub margin_incl_vat_cents_kwh: f64,
    pub transfer_day_incl_vat_cents_kwh: f64,
    pub transfer_night_incl_vat_cents_kwh: f64,
    pub tax_incl_vat_cents_kwh: f64,
    pub vat_percent: f64,
    pub night_start_hour: i32,
    pub night_end_hour: i32,
}

impl ContractTermsRow {
    /// The first version of a new user: a Finnish contract.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            valid_from: EARLIEST_VALID_FROM,
            margin_incl_vat_cents_kwh: 0.49,
            transfer_day_incl_vat_cents_kwh: 2.92,
            transfer_night_incl_vat_cents_kwh: 1.37,
            tax_incl_vat_cents_kwh: 2.9179,
            vat_percent: 25.5,
            night_start_hour: 22,
            night_end_hour: 7,
        }
    }

    /// These terms with each given term of `update`, for the same date.
    fn with_update(&self, update: &SettingsUpdate) -> Self {
        Self {
            valid_from: self.valid_from,
            margin_incl_vat_cents_kwh: update
                .margin_incl_vat_cents_kwh
                .unwrap_or(self.margin_incl_vat_cents_kwh),
            transfer_day_incl_vat_cents_kwh: update
                .transfer_day_incl_vat_cents_kwh
                .unwrap_or(self.transfer_day_incl_vat_cents_kwh),
            transfer_night_incl_vat_cents_kwh: update
                .transfer_night_incl_vat_cents_kwh
                .unwrap_or(self.transfer_night_incl_vat_cents_kwh),
            tax_incl_vat_cents_kwh: update
                .tax_incl_vat_cents_kwh
                .unwrap_or(self.tax_incl_vat_cents_kwh),
            vat_percent: update.vat_percent.unwrap_or(self.vat_percent),
            night_start_hour: update
                .night_start_hour
                .map_or(self.night_start_hour, |hour| i32::from(hour.get())),
            night_end_hour: update
                .night_end_hour
                .map_or(self.night_end_hour, |hour| i32::from(hour.get())),
        }
    }
}

/// The stored settings of one user. `timezone` and `area` are the stored
/// text; the caller validates them against the domain types. `versions` is
/// in `valid_from` order and is empty only for a broken store.
#[derive(Debug, Clone, PartialEq)]
pub struct UserSettingsRow {
    pub user_id: String,
    pub timezone: String,
    pub area: String,
    pub versions: Vec<ContractTermsRow>,
}

/// The time zone and the area of a new user.
pub const DEFAULT_TIMEZONE: &str = "Europe/Helsinki";
pub const DEFAULT_AREA: &str = "FI";

/// The settings of `user_id` with every terms version, if stored.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn get_user_settings(
    pool: &PgPool,
    user_id: &str,
) -> Result<Option<UserSettingsRow>, StoreError> {
    let rows = sqlx::query!(
        r#"
        SELECT s.timezone, s.area,
               t.valid_from AS "valid_from?: jiff_sqlx::Date",
               t.margin_cents_kwh AS "margin_cents_kwh?",
               t.transfer_day_cents_kwh AS "transfer_day_cents_kwh?",
               t.transfer_night_cents_kwh AS "transfer_night_cents_kwh?",
               t.tax_cents_kwh AS "tax_cents_kwh?",
               t.vat_percent AS "vat_percent?",
               t.night_start_hour AS "night_start_hour?",
               t.night_end_hour AS "night_end_hour?"
        FROM user_settings s
        LEFT JOIN contract_terms t ON t.user_id = s.user_id
        WHERE s.user_id = $1
        ORDER BY t.valid_from
        "#,
        user_id,
    )
    .fetch_all(pool)
    .await?;
    let Some(first) = rows.first() else {
        return Ok(None);
    };
    let (timezone, area) = (first.timezone.clone(), first.area.clone());
    let versions = rows
        .into_iter()
        .filter_map(|row| {
            Some(ContractTermsRow {
                valid_from: row.valid_from?.to_jiff(),
                margin_incl_vat_cents_kwh: row.margin_cents_kwh?,
                transfer_day_incl_vat_cents_kwh: row.transfer_day_cents_kwh?,
                transfer_night_incl_vat_cents_kwh: row.transfer_night_cents_kwh?,
                tax_incl_vat_cents_kwh: row.tax_cents_kwh?,
                vat_percent: row.vat_percent?,
                night_start_hour: row.night_start_hour?,
                night_end_hour: row.night_end_hour?,
            })
        })
        .collect();
    Ok(Some(UserSettingsRow {
        user_id: user_id.to_owned(),
        timezone,
        area,
        versions,
    }))
}

/// Creates the default settings row of `user_id` and its first terms
/// version in one statement. Does nothing when the user has a settings row.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the statement fails.
pub async fn insert_default_settings(
    executor: impl sqlx::PgExecutor<'_>,
    user_id: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let terms = ContractTermsRow::defaults();
    sqlx::query!(
        r#"
        WITH created AS (
          INSERT INTO user_settings (user_id, timezone, area, updated_at)
          VALUES ($1, $2, $3, $4)
          ON CONFLICT (user_id) DO NOTHING
          RETURNING user_id
        )
        INSERT INTO contract_terms
          (user_id, valid_from, margin_cents_kwh, transfer_day_cents_kwh,
           transfer_night_cents_kwh, tax_cents_kwh, vat_percent, night_start_hour, night_end_hour)
        SELECT user_id, $5, $6, $7, $8, $9, $10, $11, $12 FROM created
        "#,
        user_id,
        DEFAULT_TIMEZONE,
        DEFAULT_AREA,
        now.to_sqlx(),
        terms.valid_from.to_sqlx() as jiff_sqlx::Date,
        terms.margin_incl_vat_cents_kwh,
        terms.transfer_day_incl_vat_cents_kwh,
        terms.transfer_night_incl_vat_cents_kwh,
        terms.tax_incl_vat_cents_kwh,
        terms.vat_percent,
        terms.night_start_hour,
        terms.night_end_hour,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// The settings of `user_id`. Creates the default settings and their first
/// terms version first when none are stored. A concurrent save of the same
/// user is never overwritten.
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
    insert_default_settings(pool, user_id, now).await?;
    get_user_settings(pool, user_id)
        .await?
        .ok_or(StoreError::InvalidRow)
}

/// The settings after a save: the time zone, the area, and the terms that
/// apply on the saved date. `applied.valid_from` is the date of the version
/// that holds those terms.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedSettings {
    pub timezone: String,
    pub area: String,
    pub applied: ContractTermsRow,
}

/// Why a save wrote nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SaveError {
    /// The save needs a new version and the user has `MAX_TERMS_VERSIONS`.
    #[error("the user has the maximum number of contract terms versions")]
    TooManyVersions,
    /// The stored time zone is not a supported zone.
    #[error("the stored settings are invalid")]
    InvalidSettings,
    /// The user has a settings row without a terms version.
    #[error("the user has no contract terms version")]
    NoVersion,
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The versions of `user_id` in `valid_from` order.
async fn versions_of(
    executor: impl sqlx::PgExecutor<'_>,
    user_id: &str,
) -> Result<Vec<ContractTermsRow>, StoreError> {
    let rows = sqlx::query!(
        r#"
        SELECT valid_from AS "valid_from: jiff_sqlx::Date", margin_cents_kwh,
               transfer_day_cents_kwh, transfer_night_cents_kwh, tax_cents_kwh, vat_percent,
               night_start_hour, night_end_hour
        FROM contract_terms
        WHERE user_id = $1
        ORDER BY valid_from
        "#,
        user_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ContractTermsRow {
            valid_from: row.valid_from.to_jiff(),
            margin_incl_vat_cents_kwh: row.margin_cents_kwh,
            transfer_day_incl_vat_cents_kwh: row.transfer_day_cents_kwh,
            transfer_night_incl_vat_cents_kwh: row.transfer_night_cents_kwh,
            tax_incl_vat_cents_kwh: row.tax_cents_kwh,
            vat_percent: row.vat_percent,
            night_start_hour: row.night_start_hour,
            night_end_hour: row.night_end_hour,
        })
        .collect())
}

/// Saves `update` for the version date `valid_from` in one transaction.
///
/// Without `valid_from`, the date is the local date of `now` in the time
/// zone after the update. A term that `update` does not give keeps the value
/// that applies on the date. A version of the same date is replaced. No
/// version is created when the terms equal the terms that apply on the date.
/// The settings row is locked first, so saves and deletes of one user run
/// one at a time.
///
/// # Errors
///
/// Returns [`SaveError`]; nothing is written then.
pub async fn save_settings(
    pool: &PgPool,
    user_id: &str,
    update: &SettingsUpdate,
    valid_from: Option<Date>,
    now: Timestamp,
) -> Result<SavedSettings, SaveError> {
    let mut transaction = pool.begin().await.map_err(StoreError::from)?;
    let current = sqlx::query!(
        "SELECT timezone, area FROM user_settings WHERE user_id = $1 FOR UPDATE",
        user_id,
    )
    .fetch_optional(&mut *transaction)
    .await
    .map_err(StoreError::from)?
    .ok_or(StoreError::InvalidRow)?;
    let timezone = match &update.timezone {
        Some(zone) => zone.clone(),
        None => UserTimeZone::parse(&current.timezone).map_err(|_| SaveError::InvalidSettings)?,
    };
    let area = update
        .area
        .map_or(current.area, |area| area.code().to_owned());
    let valid_from = valid_from.unwrap_or_else(|| local_date(now, timezone.time_zone()));

    let versions = versions_of(&mut *transaction, user_id).await?;
    let applying = applicable_version(versions.iter(), |version| version.valid_from, valid_from)
        .ok_or(SaveError::NoVersion)?;
    let merged = applying.with_update(update);
    let replaces = applying.valid_from == valid_from;
    let applied = if !replaces && merged == *applying {
        merged
    } else {
        if !replaces && versions.len() >= MAX_TERMS_VERSIONS {
            return Err(SaveError::TooManyVersions);
        }
        let version = ContractTermsRow {
            valid_from,
            ..merged
        };
        sqlx::query!(
            r#"
            INSERT INTO contract_terms
              (user_id, valid_from, margin_cents_kwh, transfer_day_cents_kwh,
               transfer_night_cents_kwh, tax_cents_kwh, vat_percent, night_start_hour,
               night_end_hour)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (user_id, valid_from)
            DO UPDATE SET margin_cents_kwh = EXCLUDED.margin_cents_kwh,
                          transfer_day_cents_kwh = EXCLUDED.transfer_day_cents_kwh,
                          transfer_night_cents_kwh = EXCLUDED.transfer_night_cents_kwh,
                          tax_cents_kwh = EXCLUDED.tax_cents_kwh,
                          vat_percent = EXCLUDED.vat_percent,
                          night_start_hour = EXCLUDED.night_start_hour,
                          night_end_hour = EXCLUDED.night_end_hour
            "#,
            user_id,
            version.valid_from.to_sqlx() as jiff_sqlx::Date,
            version.margin_incl_vat_cents_kwh,
            version.transfer_day_incl_vat_cents_kwh,
            version.transfer_night_incl_vat_cents_kwh,
            version.tax_incl_vat_cents_kwh,
            version.vat_percent,
            version.night_start_hour,
            version.night_end_hour,
        )
        .execute(&mut *transaction)
        .await
        .map_err(StoreError::from)?;
        version
    };
    sqlx::query!(
        "UPDATE user_settings SET timezone = $2, area = $3, updated_at = $4 WHERE user_id = $1",
        user_id,
        timezone.name(),
        area,
        now.to_sqlx(),
    )
    .execute(&mut *transaction)
    .await
    .map_err(StoreError::from)?;
    transaction.commit().await.map_err(StoreError::from)?;
    Ok(SavedSettings {
        timezone: timezone.name().to_owned(),
        area,
        applied,
    })
}

/// The result of a version delete.
#[derive(Debug, Clone, PartialEq)]
pub enum DeleteOutcome {
    /// The version is gone; these versions remain, in `valid_from` order.
    Deleted(Vec<ContractTermsRow>),
    /// The user has no version with this date.
    NotFound,
    /// The version is the only version of the user. It stays.
    LastVersion,
}

/// Deletes the version of `user_id` with the date `valid_from`, but never
/// the only version. The settings row is locked first, like in
/// [`save_settings`].
///
/// # Errors
///
/// Returns [`StoreError::Query`] when a statement fails.
pub async fn delete_version(
    pool: &PgPool,
    user_id: &str,
    valid_from: Date,
) -> Result<DeleteOutcome, StoreError> {
    let mut transaction = pool.begin().await?;
    sqlx::query!(
        "SELECT user_id FROM user_settings WHERE user_id = $1 FOR UPDATE",
        user_id,
    )
    .fetch_optional(&mut *transaction)
    .await?;
    let mut versions = versions_of(&mut *transaction, user_id).await?;
    let Some(index) = versions
        .iter()
        .position(|version| version.valid_from == valid_from)
    else {
        return Ok(DeleteOutcome::NotFound);
    };
    if versions.len() == 1 {
        return Ok(DeleteOutcome::LastVersion);
    }
    sqlx::query!(
        "DELETE FROM contract_terms WHERE user_id = $1 AND valid_from = $2",
        user_id,
        valid_from.to_sqlx() as jiff_sqlx::Date,
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    versions.remove(index);
    Ok(DeleteOutcome::Deleted(versions))
}
