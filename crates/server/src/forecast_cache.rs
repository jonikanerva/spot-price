//! The precomputed FI forecast (STACK.md §9).
//!
//! The model result does not depend on the user: only the contract terms and
//! the time zone do, and the handler applies them per request. So the
//! service computes the result once after each data change and serves it
//! from memory:
//!
//! - at start-up,
//! - after each price run that stored prices (a new published price moves
//!   the forecast window),
//! - after each Fingrid run that stored data.
//!
//! A new result replaces the old one with one atomic swap of an `Arc`. A
//! reader that holds the old `Arc` keeps a complete result. A failed
//! recompute keeps the previous result and logs a warning. When no result
//! exists yet, the handler computes one per request.
//!
//! The read queries are the live reads of STACK.md §5, unchanged.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use spot_price_domain::forecast::{
    DELIVERY_DAY_TIME_ZONE, FORECAST_AREA, ForecastInput, ForecastWindow, GridRecord,
    NEIGHBOR_AREAS, NeighborPrices, QuarterMap, RouteForecast, forecast_for_route, quarter_map,
};
use spot_price_domain::price::eur_mwh_to_cents_kwh;
use sqlx::PgPool;

use crate::store::fingrid::{
    self, DATASET_CONSUMPTION_ACTUAL, DATASET_CONSUMPTION_FORECAST, DATASET_WIND_ACTUAL,
    DATASET_WIND_FORECAST, FingridRecord,
};
use crate::store::prices::{self, PriceRow};

/// A served result is stale when the newest Fingrid forecast issuance is
/// older than this. The Fingrid job runs every hour, so two missed runs make
/// a result stale.
pub const STALE_AFTER: SignedDuration = SignedDuration::from_hours(2);

/// A forecast build failed. Each variant has a fixed log code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    /// A query failed; the value is the code of the store error.
    #[error("forecast data could not be read")]
    Read(&'static str),
    /// An instant of the window left the supported range.
    #[error("the forecast window is out of range")]
    TimeRange,
    /// The blocking task of the model stopped.
    #[error("the forecast task stopped")]
    Task,
    /// The bundled time zone database has no delivery-day zone.
    #[error("the delivery-day time zone is missing")]
    TimeZone,
}

impl BuildError {
    /// A fixed code for diagnostics.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Read(code) => code,
            Self::TimeRange => "time_range",
            Self::Task => "forecast_task",
            Self::TimeZone => "delivery_time_zone",
        }
    }
}

/// The stored data of one forecast run.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub window: ForecastWindow,
    /// True when no FI price exists, so the window starts at the clock
    /// instant of the build, not after the last published price.
    pub window_from_clock: bool,
    pub spot: QuarterMap,
    pub neighbors: NeighborPrices,
    pub wind_forecast: Vec<GridRecord>,
    pub wind_actual: Vec<GridRecord>,
    pub consumption_forecast: Vec<GridRecord>,
    pub consumption_actual: Vec<GridRecord>,
    /// The newest `fetched_at` of the FI prices in the price window.
    pub prices_fetched_at: Option<Timestamp>,
    /// The newest `issued_at` of the Fingrid forecast vintages (245, 165).
    pub grid_issued_at: Option<Timestamp>,
}

impl Inputs {
    /// The `generatedAt` of a result of these inputs: the data freshness.
    ///
    /// The rule: the later of the newest Fingrid forecast issuance
    /// (`fingrid_forecasts.issued_at` of datasets 245 and 165) and the newest
    /// arrival of an FI price in the price window
    /// (`prices.fetched_at` with `delivery_start` in
    /// `[history_start, series_start)`). When neither exists, `built_at`, the
    /// instant of the computation.
    #[must_use]
    pub fn data_as_of(&self, built_at: Timestamp) -> Timestamp {
        match (self.grid_issued_at, self.prices_fetched_at) {
            (Some(grid), Some(price)) => grid.max(price),
            (Some(one), None) | (None, Some(one)) => one,
            (None, None) => built_at,
        }
    }
}

fn spot_map(rows: &[PriceRow]) -> QuarterMap {
    quarter_map(
        rows.iter()
            .map(|row| (row.delivery_start, eur_mwh_to_cents_kwh(row.price_eur_mwh))),
    )
}

fn grid(records: Vec<FingridRecord>) -> Vec<GridRecord> {
    records
        .into_iter()
        .map(|record| GridRecord {
            start: record.start_time,
            value: record.value,
        })
        .collect()
}

fn read_failed(error: crate::db::StoreError) -> BuildError {
    BuildError::Read(error.code())
}

/// Reads the inputs like the TypeScript route: FI prices and neighbour
/// prices in `[history_start, series_start)`, the latest forecast vintage
/// of each target, and the actuals in `[history_start, series_end)`. Also
/// reads the two freshness instants of [`Inputs::data_as_of`].
///
/// # Errors
///
/// Returns [`BuildError`] when a query fails or the window leaves the
/// supported range.
pub async fn load_inputs(pool: &PgPool, now: Timestamp) -> Result<Inputs, BuildError> {
    let last = prices::get_latest_delivery_start(pool, FORECAST_AREA)
        .await
        .map_err(read_failed)?;
    let window = ForecastWindow::new(last, now).map_err(|_| BuildError::TimeRange)?;
    let (from, series_start, until) =
        (window.history_start, window.series_start, window.series_end);
    let fi = prices::get_prices_by_range(pool, from, series_start, FORECAST_AREA)
        .await
        .map_err(read_failed)?;
    let neighbors = prices::get_prices_by_areas(pool, from, series_start, &NEIGHBOR_AREAS)
        .await
        .map_err(read_failed)?;
    let (wind_forecast, wind_actual, consumption_forecast, consumption_actual) = tokio::try_join!(
        fingrid::get_fingrid_forecast_vintages_latest(pool, DATASET_WIND_FORECAST, from, until),
        fingrid::get_fingrid_records_by_range(pool, DATASET_WIND_ACTUAL, from, until),
        fingrid::get_fingrid_forecast_vintages_latest(
            pool,
            DATASET_CONSUMPTION_FORECAST,
            from,
            until
        ),
        fingrid::get_fingrid_records_by_range(pool, DATASET_CONSUMPTION_ACTUAL, from, until),
    )
    .map_err(read_failed)?;
    let (prices_fetched_at, wind_issued_at, consumption_issued_at) = tokio::try_join!(
        prices::get_latest_fetched_at_by_range(pool, from, series_start, FORECAST_AREA),
        fingrid::get_latest_forecast_issued_at(pool, DATASET_WIND_FORECAST),
        fingrid::get_latest_forecast_issued_at(pool, DATASET_CONSUMPTION_FORECAST),
    )
    .map_err(read_failed)?;
    Ok(Inputs {
        window,
        window_from_clock: last.is_none(),
        spot: spot_map(&fi),
        neighbors: neighbors
            .iter()
            .map(|(area, rows)| (*area, spot_map(rows)))
            .collect::<BTreeMap<_, _>>(),
        wind_forecast: grid(wind_forecast),
        wind_actual: grid(wind_actual),
        consumption_forecast: grid(consumption_forecast),
        consumption_actual: grid(consumption_actual),
        prices_fetched_at,
        grid_issued_at: wind_issued_at.max(consumption_issued_at),
    })
}

/// The user-independent forecast of one build.
#[derive(Debug)]
pub struct Snapshot {
    pub window: ForecastWindow,
    /// See [`Inputs::window_from_clock`]. A cached snapshot with this flag
    /// is not served: its window depends on the build instant.
    pub window_from_clock: bool,
    pub route: RouteForecast,
    /// The `generatedAt` of the response ([`Inputs::data_as_of`]).
    pub generated_at: Timestamp,
    /// The newest Fingrid forecast issuance, for the staleness check.
    pub grid_issued_at: Option<Timestamp>,
    /// The instant of the build.
    pub built_at: Timestamp,
    stale_logged: AtomicBool,
}

impl Snapshot {
    /// The model result of `inputs`. Pure: no I/O and no clock.
    /// `delivery_zone` must be [`DELIVERY_DAY_TIME_ZONE`].
    #[must_use]
    pub fn compute(inputs: &Inputs, delivery_zone: &TimeZone, built_at: Timestamp) -> Self {
        let input = ForecastInput {
            spot_prices: &inputs.spot,
            neighbor_prices: &inputs.neighbors,
            wind_forecast: &inputs.wind_forecast,
            wind_actual: &inputs.wind_actual,
            consumption_forecast: &inputs.consumption_forecast,
            consumption_actual: &inputs.consumption_actual,
            series_start: inputs.window.series_start,
            series_end: inputs.window.series_end,
        };
        Self {
            window: inputs.window,
            window_from_clock: inputs.window_from_clock,
            route: forecast_for_route(&input, delivery_zone),
            generated_at: inputs.data_as_of(built_at),
            grid_issued_at: inputs.grid_issued_at,
            built_at,
            stale_logged: AtomicBool::new(false),
        }
    }

    /// Whether the newest Fingrid forecast issuance is older than
    /// [`STALE_AFTER`] at `now`. Without any issuance, the result is not
    /// stale: the forecast then has no grid forecast at all, and the
    /// response says so.
    #[must_use]
    pub fn is_stale(&self, now: Timestamp) -> bool {
        self.grid_issued_at
            .is_some_and(|issued| now.duration_since(issued) > STALE_AFTER)
    }

    /// True once per snapshot: the first time that a stale result is served.
    fn first_stale_serve(&self, now: Timestamp) -> bool {
        self.is_stale(now) && !self.stale_logged.swap(true, Ordering::Relaxed)
    }

    /// Logs a warning the first time that this stale result is served.
    pub fn warn_if_stale(&self, now: Timestamp) {
        if self.first_stale_serve(now) {
            tracing::warn!(
                code = "forecast_stale",
                grid_issued_at = self.grid_issued_at.map(|t| t.to_string()),
                "the served forecast uses Fingrid data older than 2 h"
            );
        }
    }
}

/// Loads the inputs and runs the model on the blocking thread pool.
///
/// # Errors
///
/// Returns [`BuildError`] when a read fails or the task stops.
pub async fn build(pool: &PgPool, now: Timestamp) -> Result<Snapshot, BuildError> {
    let now = spot_price_domain::time::truncate_to_millisecond(now);
    let zone = TimeZone::get(DELIVERY_DAY_TIME_ZONE).map_err(|_| BuildError::TimeZone)?;
    let inputs = load_inputs(pool, now).await?;
    tokio::task::spawn_blocking(move || Snapshot::compute(&inputs, &zone, now))
        .await
        .map_err(|_| BuildError::Task)
}

/// The shared forecast result. Cloning shares the same result.
#[derive(Debug, Clone, Default)]
pub struct ForecastCache {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    current: RwLock<Option<Arc<Snapshot>>>,
    /// Serialises the recomputes, so a slow older build cannot replace a
    /// newer result.
    refresh: tokio::sync::Mutex<()>,
}

impl ForecastCache {
    /// The current result, if any.
    #[must_use]
    pub fn current(&self) -> Option<Arc<Snapshot>> {
        self.inner
            .current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replaces the current result in one swap.
    pub fn replace(&self, snapshot: Snapshot) {
        let next = Arc::new(snapshot);
        *self
            .inner
            .current
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(next);
    }

    /// Recomputes the result and swaps it in. On failure the previous result
    /// stays, and a warning names the trigger and the error code. A stale
    /// new result also logs a warning.
    ///
    /// # Errors
    ///
    /// Returns the [`BuildError`] of a failed build.
    pub async fn refresh(
        &self,
        pool: &PgPool,
        now: Timestamp,
        trigger: &'static str,
    ) -> Result<(), BuildError> {
        let _serial = self.inner.refresh.lock().await;
        let started = tokio::time::Instant::now();
        match build(pool, now).await {
            Ok(snapshot) => {
                let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                let generated_at = snapshot.generated_at.to_string();
                if snapshot.is_stale(now) {
                    tracing::warn!(
                        job = "forecast",
                        stage = trigger,
                        code = "forecast_stale",
                        grid_issued_at = snapshot.grid_issued_at.map(|t| t.to_string()),
                        "the recomputed forecast uses Fingrid data older than 2 h"
                    );
                }
                self.replace(snapshot);
                tracing::info!(
                    job = "forecast",
                    stage = trigger,
                    elapsed_ms,
                    generated_at,
                    "forecast recomputed"
                );
                Ok(())
            }
            Err(error) => {
                tracing::warn!(
                    job = "forecast",
                    stage = trigger,
                    code = error.code(),
                    "forecast recompute failed; the previous result stays"
                );
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(iso: &str) -> Timestamp {
        iso.parse().unwrap()
    }

    fn compute(inputs: &Inputs, built_at: Timestamp) -> Snapshot {
        Snapshot::compute(
            inputs,
            &TimeZone::get(DELIVERY_DAY_TIME_ZONE).unwrap(),
            built_at,
        )
    }

    fn inputs(grid: Option<&str>, price: Option<&str>) -> Inputs {
        let now = ts("2026-10-24T09:07:30Z");
        Inputs {
            window: ForecastWindow::new(Some(ts("2026-10-25T21:45:00Z")), now).unwrap(),
            window_from_clock: false,
            spot: QuarterMap::new(),
            neighbors: NeighborPrices::new(),
            wind_forecast: Vec::new(),
            wind_actual: Vec::new(),
            consumption_forecast: Vec::new(),
            consumption_actual: Vec::new(),
            prices_fetched_at: price.map(ts),
            grid_issued_at: grid.map(ts),
        }
    }

    #[test]
    fn data_as_of_is_the_later_of_the_grid_issuance_and_the_price_arrival() {
        let built = ts("2026-10-24T09:07:30Z");
        let both = inputs(Some("2026-10-24T08:00:00Z"), Some("2026-10-23T11:02:00Z"));
        assert_eq!(both.data_as_of(built), ts("2026-10-24T08:00:00Z"));
        let price_later = inputs(Some("2026-10-23T08:00:00Z"), Some("2026-10-23T11:02:00Z"));
        assert_eq!(price_later.data_as_of(built), ts("2026-10-23T11:02:00Z"));
        let grid_only = inputs(Some("2026-10-24T08:00:00Z"), None);
        assert_eq!(grid_only.data_as_of(built), ts("2026-10-24T08:00:00Z"));
        let price_only = inputs(None, Some("2026-10-23T11:02:00Z"));
        assert_eq!(price_only.data_as_of(built), ts("2026-10-23T11:02:00Z"));
        assert_eq!(inputs(None, None).data_as_of(built), built);
    }

    #[test]
    fn a_result_is_stale_only_after_two_hours_without_a_grid_issuance() {
        let snapshot = compute(
            &inputs(Some("2026-10-24T08:00:00Z"), None),
            ts("2026-10-24T08:00:35Z"),
        );
        assert!(!snapshot.is_stale(ts("2026-10-24T10:00:00Z")));
        assert!(snapshot.is_stale(ts("2026-10-24T10:00:01Z")));
        let no_grid = compute(&inputs(None, None), ts("2026-10-24T08:00:35Z"));
        assert!(!no_grid.is_stale(ts("2026-10-30T00:00:00Z")));
    }

    #[test]
    fn a_stale_result_warns_once() {
        let snapshot = compute(
            &inputs(Some("2026-10-24T08:00:00Z"), None),
            ts("2026-10-24T08:00:35Z"),
        );
        assert!(!snapshot.first_stale_serve(ts("2026-10-24T09:00:00Z")));
        assert!(snapshot.first_stale_serve(ts("2026-10-24T11:00:00Z")));
        assert!(!snapshot.first_stale_serve(ts("2026-10-24T12:00:00Z")));
    }

    #[test]
    fn a_new_result_replaces_the_old_one_and_a_reader_keeps_its_copy() {
        let cache = ForecastCache::default();
        assert!(cache.current().is_none());
        cache.replace(compute(
            &inputs(Some("2026-10-24T07:00:00Z"), None),
            ts("2026-10-24T07:00:35Z"),
        ));
        let old = cache.current().unwrap();
        let shared = cache.clone();
        shared.replace(compute(
            &inputs(Some("2026-10-24T08:00:00Z"), None),
            ts("2026-10-24T08:00:35Z"),
        ));
        let new = cache.current().unwrap();
        assert!(!Arc::ptr_eq(&old, &new));
        assert_eq!(old.generated_at, ts("2026-10-24T07:00:00Z"));
        assert_eq!(new.generated_at, ts("2026-10-24T08:00:00Z"));
    }

    #[test]
    fn readers_see_only_complete_results_during_swaps() {
        let cache = ForecastCache::default();
        let writer = {
            let cache = cache.clone();
            std::thread::spawn(move || {
                for hour in 0..200_i64 {
                    let issued = ts("2026-10-24T00:00:00Z") + SignedDuration::from_hours(hour);
                    let mut next = inputs(None, None);
                    next.grid_issued_at = Some(issued);
                    cache.replace(compute(&next, issued));
                }
            })
        };
        let mut seen = 0;
        while !writer.is_finished() || seen == 0 {
            if let Some(snapshot) = cache.current() {
                // A complete result: its fields belong to one build.
                assert_eq!(Some(snapshot.generated_at), snapshot.grid_issued_at);
                assert_eq!(snapshot.generated_at, snapshot.built_at);
                seen += 1;
            }
        }
        writer.join().unwrap();
        assert_eq!(
            cache.current().unwrap().generated_at,
            ts("2026-10-24T00:00:00Z") + SignedDuration::from_hours(199)
        );
    }
}
