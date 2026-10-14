//! Read-only database access of the `--db` modes. The queries are runtime
//! queries, so the tools add nothing to `.sqlx`. Instants cross the boundary
//! as UTC epoch milliseconds, floored like a JavaScript `Date`. No migration
//! runs, and the URL is never logged.

use std::collections::BTreeMap;
use std::future::Future;

use jiff::Timestamp;
use spot_price_domain::forecast::iso_millis;
use spot_price_domain::time::truncate_to_millisecond;
use sqlx::{Connection, PgConnection};

use crate::backtest::OutOfRange;
use crate::backtest_cli::{SCORED_AREAS, StoredPrice, assemble_backtest_data};
use crate::data::{
    BacktestData, DATASET_CONSUMPTION_ACTUAL, DATASET_CONSUMPTION_FORECAST, DATASET_WIND_ACTUAL,
    DATASET_WIND_FORECAST, DatasetId, GridActual, Vintage,
};
use crate::error::ToolError;
use crate::revision::{ActualRecord, RevisionStudyInput, VINTAGE_DATASET_IDS};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

const PRICES_SQL: &str = "SELECT area, floor(extract(epoch FROM delivery_start) * 1000)::bigint, \
     price_eur_mwh \
     FROM prices \
     WHERE area = ANY($1) AND delivery_start >= $2::timestamptz AND delivery_start < $3::timestamptz \
     ORDER BY area, delivery_start";

const ACTUALS_SQL: &str = "SELECT dataset_id, floor(extract(epoch FROM start_time) * 1000)::bigint, \
     floor(extract(epoch FROM end_time) * 1000)::bigint, value \
     FROM fingrid_actuals \
     WHERE dataset_id = $1 AND start_time >= $2::timestamptz AND start_time < $3::timestamptz \
     ORDER BY start_time";

const VINTAGES_SQL: &str = "SELECT dataset_id, floor(extract(epoch FROM issued_at) * 1000)::bigint, \
     floor(extract(epoch FROM start_time) * 1000)::bigint, \
     floor(extract(epoch FROM end_time) * 1000)::bigint, value \
     FROM fingrid_forecasts \
     WHERE dataset_id = $1 AND start_time >= $2::timestamptz AND start_time < $3::timestamptz \
     ORDER BY start_time, issued_at";

/// Runs `future` to completion on a current-thread runtime.
///
/// # Errors
/// [`ToolError::Runtime`] when the runtime cannot start.
pub fn block_on<F: Future>(future: F) -> Result<F::Output, ToolError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(ToolError::Runtime)?;
    Ok(runtime.block_on(future))
}

/// `[now − window_days, now)` as ISO text, `now` cut to milliseconds.
fn window(window_days: i64, now: Timestamp) -> Result<(String, String), ToolError> {
    let end_ms = truncate_to_millisecond(now).as_millisecond();
    let start_ms = window_days
        .checked_mul(DAY_MS)
        .and_then(|span| end_ms.checked_sub(span))
        .ok_or(OutOfRange)?;
    Ok((iso_millis(instant(start_ms)?), iso_millis(instant(end_ms)?)))
}

fn instant(ms: i64) -> Result<Timestamp, OutOfRange> {
    Timestamp::from_millisecond(ms).map_err(|_| OutOfRange)
}

/// Stored prices of the scored areas, grouped by area in query order.
async fn read_prices(
    conn: &mut PgConnection,
    start: &str,
    end: &str,
) -> Result<Vec<(String, Vec<StoredPrice>)>, ToolError> {
    let areas: Vec<String> = SCORED_AREAS.iter().map(|a| (*a).to_owned()).collect();
    let rows = sqlx::query_as::<_, (String, i64, f64)>(PRICES_SQL)
        .bind(areas)
        .bind(start)
        .bind(end)
        .fetch_all(&mut *conn)
        .await?;
    let mut grouped: Vec<(String, Vec<StoredPrice>)> = Vec::new();
    for (area, start_ms, price_eur_mwh) in rows {
        let price = StoredPrice {
            area: area.clone(),
            delivery_start: instant(start_ms)?,
            price_eur_mwh,
        };
        match grouped.last_mut() {
            Some((last, prices)) if *last == area => prices.push(price),
            _ => grouped.push((area, vec![price])),
        }
    }
    Ok(grouped)
}

async fn read_actuals(
    conn: &mut PgConnection,
    dataset_id: DatasetId,
    start: &str,
    end: &str,
) -> Result<Vec<GridActual>, ToolError> {
    let rows = sqlx::query_as::<_, (i32, i64, i64, f64)>(ACTUALS_SQL)
        .bind(dataset_id)
        .bind(start)
        .bind(end)
        .fetch_all(&mut *conn)
        .await?;
    rows.into_iter()
        .map(|(id, start_ms, end_ms, value)| {
            Ok(GridActual {
                dataset_id: id,
                start: instant(start_ms)?,
                end: instant(end_ms)?,
                value,
            })
        })
        .collect()
}

/// Every archived vintage of a forecast dataset with a target in the window.
async fn read_vintages(
    conn: &mut PgConnection,
    dataset_id: DatasetId,
    start: &str,
    end: &str,
) -> Result<Vec<Vintage>, ToolError> {
    let rows = sqlx::query_as::<_, (i32, i64, i64, i64, f64)>(VINTAGES_SQL)
        .bind(dataset_id)
        .bind(start)
        .bind(end)
        .fetch_all(&mut *conn)
        .await?;
    rows.into_iter()
        .map(|(id, issued_ms, start_ms, end_ms, value)| {
            Ok(Vintage {
                dataset_id: id,
                issued_at: instant(issued_ms)?,
                start: instant(start_ms)?,
                end: instant(end_ms)?,
                value,
            })
        })
        .collect()
}

/// Opens one connection, runs `read`, and closes the connection also when
/// `read` fails.
async fn with_connection<T, F>(url: &str, read: F) -> Result<T, ToolError>
where
    F: AsyncFnOnce(&mut PgConnection) -> Result<T, ToolError>,
{
    let mut conn = PgConnection::connect(url).await?;
    let result = read(&mut conn).await;
    let closed = conn.close().await;
    let value = result?;
    closed?;
    Ok(value)
}

/// The backtest data of `[now − window_days, now)`: FI and the neighbour
/// prices, the actuals (75/124), and all forecast vintages (245/165).
///
/// # Errors
/// [`ToolError`] for a window out of range or a database error.
pub async fn read_backtest_data(
    conn: &mut PgConnection,
    window_days: i64,
    now: Timestamp,
) -> Result<BacktestData, ToolError> {
    let (start, end) = window(window_days, now)?;
    let prices = read_prices(conn, &start, &end).await?;
    let mut actuals = BTreeMap::new();
    for id in [DATASET_WIND_ACTUAL, DATASET_CONSUMPTION_ACTUAL] {
        actuals.insert(id, read_actuals(conn, id, &start, &end).await?);
    }
    let mut vintages = BTreeMap::new();
    for id in [DATASET_WIND_FORECAST, DATASET_CONSUMPTION_FORECAST] {
        vintages.insert(id, read_vintages(conn, id, &start, &end).await?);
    }
    Ok(assemble_backtest_data(&prices, actuals, vintages))
}

/// [`read_backtest_data`] over a new connection to `url`.
///
/// # Errors
/// [`ToolError`] for a window out of range or a database error.
pub async fn fetch_backtest_data(
    url: &str,
    window_days: i64,
    now: Timestamp,
) -> Result<BacktestData, ToolError> {
    with_connection(url, async |conn: &mut PgConnection| {
        read_backtest_data(conn, window_days, now).await
    })
    .await
}

/// The revision-study input of `[now − window_days, now)`: all vintages of
/// the forecast datasets and the paired actuals.
///
/// # Errors
/// [`ToolError`] for a window out of range or a database error.
pub async fn fetch_study_input(
    url: &str,
    window_days: i64,
    now: Timestamp,
) -> Result<RevisionStudyInput, ToolError> {
    with_connection(url, async |conn: &mut PgConnection| {
        read_study_input(conn, window_days, now).await
    })
    .await
}

/// [`fetch_study_input`] over an open connection.
///
/// # Errors
/// [`ToolError`] for a window out of range or a database error.
pub async fn read_study_input(
    conn: &mut PgConnection,
    window_days: i64,
    now: Timestamp,
) -> Result<RevisionStudyInput, ToolError> {
    let (start, end) = window(window_days, now)?;
    let mut vintages = BTreeMap::new();
    for id in VINTAGE_DATASET_IDS {
        vintages.insert(id, read_vintages(conn, id, &start, &end).await?);
    }
    let mut actuals = BTreeMap::new();
    for id in VINTAGE_DATASET_IDS {
        let actual_id = crate::revision::actual_id_for(id);
        let records = read_actuals(conn, actual_id, &start, &end)
            .await?
            .into_iter()
            .map(|r| ActualRecord {
                dataset_id: r.dataset_id,
                start: r.start,
                value: r.value,
            })
            .collect();
        actuals.insert(actual_id, records);
    }
    Ok(RevisionStudyInput {
        vintages_by_dataset: vintages,
        actuals_by_dataset: actuals,
    })
}
