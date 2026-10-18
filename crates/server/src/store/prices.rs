//! Nord Pool day-ahead prices in the `prices` table.

use std::collections::BTreeMap;

use jiff::Timestamp;
use jiff_sqlx::ToSqlx as _;
use spot_price_domain::areas::Area;
use sqlx::PgPool;

use crate::db::StoreError;

/// One stored price interval of one delivery area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PriceRow {
    pub delivery_start: Timestamp,
    pub delivery_end: Timestamp,
    pub price_eur_mwh: f64,
    pub area: Area,
}

/// Inserts new prices and overwrites the stored ones in one transaction.
/// Returns the number of prices handed in, not the number of rows written.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when a statement fails. Nothing is written
/// then.
pub async fn store_prices(pool: &PgPool, prices: &[PriceRow]) -> Result<usize, StoreError> {
    if prices.is_empty() {
        return Ok(0);
    }
    let mut transaction = pool.begin().await?;
    for price in prices {
        sqlx::query!(
            r#"
            INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (delivery_start, area)
            DO UPDATE SET delivery_end = EXCLUDED.delivery_end,
                          price_eur_mwh = EXCLUDED.price_eur_mwh,
                          fetched_at = NOW()
            "#,
            price.delivery_start.to_sqlx(),
            price.delivery_end.to_sqlx(),
            price.price_eur_mwh,
            price.area.code(),
        )
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(prices.len())
}

/// The prices of `area` with `delivery_start` in `[start, end)`, oldest
/// first.
///
/// # Errors
///
/// Returns a [`StoreError`] when the query fails or a row has an unknown
/// area.
pub async fn get_prices_by_range(
    pool: &PgPool,
    start: Timestamp,
    end: Timestamp,
    area: Area,
) -> Result<Vec<PriceRow>, StoreError> {
    let rows = sqlx::query!(
        r#"
        SELECT delivery_start, delivery_end, price_eur_mwh, area
        FROM prices
        WHERE area = $1 AND delivery_start >= $2 AND delivery_start < $3
        ORDER BY delivery_start
        "#,
        area.code(),
        start.to_sqlx(),
        end.to_sqlx(),
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            to_price_row(
                row.delivery_start,
                row.delivery_end,
                row.price_eur_mwh,
                &row.area,
            )
        })
        .collect()
}

/// The prices of several areas with `delivery_start` in `[start, end)`, in
/// one query, grouped by area and oldest first. An area without stored rows
/// has no entry in the map.
///
/// # Errors
///
/// Returns a [`StoreError`] when the query fails or a row has an unknown
/// area.
pub async fn get_prices_by_areas(
    pool: &PgPool,
    start: Timestamp,
    end: Timestamp,
    areas: &[Area],
) -> Result<BTreeMap<Area, Vec<PriceRow>>, StoreError> {
    let mut grouped: BTreeMap<Area, Vec<PriceRow>> = BTreeMap::new();
    if areas.is_empty() {
        return Ok(grouped);
    }
    let codes: Vec<String> = areas.iter().map(|area| area.code().to_owned()).collect();
    let rows = sqlx::query!(
        r#"
        SELECT delivery_start, delivery_end, price_eur_mwh, area
        FROM prices
        WHERE area = ANY($1) AND delivery_start >= $2 AND delivery_start < $3
        ORDER BY area, delivery_start
        "#,
        &codes,
        start.to_sqlx(),
        end.to_sqlx(),
    )
    .fetch_all(pool)
    .await?;
    for row in rows {
        let price = to_price_row(
            row.delivery_start,
            row.delivery_end,
            row.price_eur_mwh,
            &row.area,
        )?;
        grouped.entry(price.area).or_default().push(price);
    }
    Ok(grouped)
}

/// The newest stored `delivery_start` of `area`, if any.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn get_latest_delivery_start(
    pool: &PgPool,
    area: Area,
) -> Result<Option<Timestamp>, StoreError> {
    let latest = sqlx::query_scalar!(
        r#"
        SELECT delivery_start FROM prices
        WHERE area = $1
        ORDER BY delivery_start DESC
        LIMIT 1
        "#,
        area.code(),
    )
    .fetch_optional(pool)
    .await?;
    Ok(latest.map(jiff_sqlx::Timestamp::to_jiff))
}

/// The newest `fetched_at` of the prices of `area` with `delivery_start` in
/// `[start, end)`, if any. The upsert sets `fetched_at` on each write of a
/// row, also when the price is unchanged, so this is the newest write.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn get_latest_fetched_at_by_range(
    pool: &PgPool,
    start: Timestamp,
    end: Timestamp,
    area: Area,
) -> Result<Option<Timestamp>, StoreError> {
    let latest = sqlx::query_scalar!(
        r#"
        SELECT MAX(fetched_at) FROM prices
        WHERE area = $1 AND delivery_start >= $2 AND delivery_start < $3
        "#,
        area.code(),
        start.to_sqlx(),
        end.to_sqlx(),
    )
    .fetch_one(pool)
    .await?;
    Ok(latest.map(jiff_sqlx::Timestamp::to_jiff))
}

/// The number of prices of `area` with `delivery_start` in `[start, end)`.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn count_prices_by_range(
    pool: &PgPool,
    start: Timestamp,
    end: Timestamp,
    area: Area,
) -> Result<i64, StoreError> {
    let count = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) AS "count!" FROM prices
        WHERE area = $1 AND delivery_start >= $2 AND delivery_start < $3
        "#,
        area.code(),
        start.to_sqlx(),
        end.to_sqlx(),
    )
    .fetch_one(pool)
    .await?;
    Ok(count)
}

fn to_price_row(
    delivery_start: jiff_sqlx::Timestamp,
    delivery_end: jiff_sqlx::Timestamp,
    price_eur_mwh: f64,
    area: &str,
) -> Result<PriceRow, StoreError> {
    Ok(PriceRow {
        delivery_start: delivery_start.to_jiff(),
        delivery_end: delivery_end.to_jiff(),
        price_eur_mwh,
        area: area.parse().map_err(|_| StoreError::InvalidRow)?,
    })
}
