//! Feasible continuous schedules on the quarter grid, and the cost basis
//! that prices them. Pure. A schedule is one run of a fixed number of
//! consecutive quarters at constant power inside `[earliest start,
//! deadline)`. Every arm of a comparison gets the same task.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use spot_price_domain::areas::Area;
use spot_price_domain::price::apply_contract_terms;
use spot_price_domain::settings::{Hour, UserId, UserSettings};
use spot_price_domain::time::{UserTimeZone, local_hour};

use crate::jsfmt::{count_f64, js_sum};

/// The length of one delivery quarter.
pub const QUARTER_MS: i64 = 15 * 60 * 1000;

/// The household time zone of the evaluation: the forecast is FI only.
pub const HOUSEHOLD_TIME_ZONE: &str = "Europe/Helsinki";

/// One continuous run at constant power. The run must start on a quarter
/// boundary at or after `earliest_start_ms` and end at or before
/// `deadline_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleTask {
    pub earliest_start_ms: i64,
    pub deadline_ms: i64,
    pub quarters: usize,
}

/// Why a task has no schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleError {
    /// The window is shorter than the run, or its bounds are not quarter
    /// boundaries. The window is never shrunk to fit.
    Infeasible,
    /// The price of this quarter (UTC ms) is not known to the arm.
    MissingQuarter(i64),
}

impl ScheduleTask {
    /// The quarter starts of the window, in order.
    ///
    /// # Errors
    /// [`ScheduleError::Infeasible`] when the window holds fewer quarters
    /// than the run, a bound is not on the quarter grid, or the run is empty.
    pub fn window_quarters(&self) -> Result<Vec<i64>, ScheduleError> {
        let span = self.deadline_ms - self.earliest_start_ms;
        if self.quarters == 0
            || span <= 0
            || self.earliest_start_ms.rem_euclid(QUARTER_MS) != 0
            || span % QUARTER_MS != 0
        {
            return Err(ScheduleError::Infeasible);
        }
        let count = usize::try_from(span / QUARTER_MS).map_err(|_| ScheduleError::Infeasible)?;
        if count < self.quarters {
            return Err(ScheduleError::Infeasible);
        }
        Ok((0..count)
            .map(|i| {
                #[expect(clippy::cast_possible_wrap, reason = "count fits an i64 span")]
                let offset = i as i64 * QUARTER_MS;
                self.earliest_start_ms + offset
            })
            .collect())
    }
}

/// The start (UTC ms) of the run with the lowest mean `price` in the window.
/// An exact tie keeps the earliest start, like the cheapest-window route.
///
/// # Errors
/// [`ScheduleError`] for an infeasible window, or a quarter in the window
/// without a price: a partial window is never chosen.
pub fn cheapest_start<F>(task: &ScheduleTask, price: F) -> Result<i64, ScheduleError>
where
    F: Fn(i64) -> Option<f64>,
{
    let quarters = task.window_quarters()?;
    let prices = quarters
        .iter()
        .map(|&q| price(q).ok_or(ScheduleError::MissingQuarter(q)))
        .collect::<Result<Vec<f64>, _>>()?;
    let mut best: Option<(f64, i64)> = None;
    for (start, window) in quarters.iter().zip(prices.windows(task.quarters)) {
        let total = js_sum(window.iter().copied());
        if best.is_none_or(|(best_total, _)| total < best_total) {
            best = Some((total, *start));
        }
    }
    best.map(|(_, start)| start)
        .ok_or(ScheduleError::Infeasible)
}

/// The mean `price` of the run that starts at `start_ms`.
///
/// # Errors
/// [`ScheduleError::MissingQuarter`] when a quarter of the run has no price.
pub fn run_mean_price<F>(start_ms: i64, quarters: usize, price: F) -> Result<f64, ScheduleError>
where
    F: Fn(i64) -> Option<f64>,
{
    let values = (0..quarters)
        .map(|i| {
            #[expect(clippy::cast_possible_wrap, reason = "a run is a few quarters")]
            let q = start_ms + i as i64 * QUARTER_MS;
            price(q).ok_or(ScheduleError::MissingQuarter(q))
        })
        .collect::<Result<Vec<f64>, _>>()?;
    Ok(js_sum(values.iter().copied()) / count_f64(quarters))
}

/// A fixed, declared contract. It is never read from a stored user row.
#[derive(Debug, Clone, PartialEq)]
pub struct ContractScenario {
    pub margin_cents_kwh: f64,
    pub transfer_day_cents_kwh: f64,
    pub transfer_night_cents_kwh: f64,
    pub tax_cents_kwh: f64,
    pub vat_percent: f64,
    pub night_start_hour: Hour,
    pub night_end_hour: Hour,
}

/// How a quarter of a schedule is priced. Every arm and the hindsight
/// schedule use the same basis, both to choose and to score a run.
#[derive(Debug, Clone, PartialEq)]
pub enum CostBasis {
    /// The spot price in c/kWh, also when it is negative.
    Spot,
    /// The total price of the declared contract in [`HOUSEHOLD_TIME_ZONE`],
    /// through the price math of the API. That math floors a negative spot
    /// at 0 before it adds the other terms.
    Contract(ContractScenario),
}

/// A cost basis that is ready to price quarters.
#[derive(Debug, Clone)]
pub struct Pricer {
    settings: Option<UserSettings>,
    zone: TimeZone,
}

/// The household time zone is not in the bundled time zone database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("time zone {HOUSEHOLD_TIME_ZONE} is not supported")]
pub struct HouseholdTimeZone;

impl Pricer {
    /// # Errors
    /// [`HouseholdTimeZone`] when [`HOUSEHOLD_TIME_ZONE`] is not supported.
    pub fn new(basis: &CostBasis) -> Result<Self, HouseholdTimeZone> {
        let timezone = UserTimeZone::parse(HOUSEHOLD_TIME_ZONE).map_err(|_| HouseholdTimeZone)?;
        let zone = timezone.time_zone().clone();
        let settings = match basis {
            CostBasis::Spot => None,
            CostBasis::Contract(c) => Some(UserSettings {
                user_id: UserId::new("evaluation-scenario".to_owned()),
                margin_cents_kwh: c.margin_cents_kwh,
                transfer_day_cents_kwh: c.transfer_day_cents_kwh,
                transfer_night_cents_kwh: c.transfer_night_cents_kwh,
                tax_cents_kwh: c.tax_cents_kwh,
                vat_percent: c.vat_percent,
                night_start_hour: c.night_start_hour,
                night_end_hour: c.night_end_hour,
                timezone,
                area: Area::Fi,
            }),
        };
        Ok(Self { settings, zone })
    }

    /// The cost (c/kWh) of `spot_cents_kwh` in the quarter at `at_ms`.
    #[must_use]
    pub fn price(&self, spot_cents_kwh: f64, at_ms: i64) -> f64 {
        let Some(settings) = &self.settings else {
            return spot_cents_kwh;
        };
        let hour = Timestamp::from_millisecond(at_ms).map_or(0, |at| local_hour(at, &self.zone));
        apply_contract_terms(spot_cents_kwh, settings, hour).total_cents_kwh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: i64 = 1_800_000_000_000 - 1_800_000_000_000 % QUARTER_MS;

    fn task(window_quarters: i64, quarters: usize) -> ScheduleTask {
        ScheduleTask {
            earliest_start_ms: START,
            deadline_ms: START + window_quarters * QUARTER_MS,
            quarters,
        }
    }

    fn series(values: &[f64]) -> impl Fn(i64) -> Option<f64> + '_ {
        move |q| {
            let index = usize::try_from((q - START) / QUARTER_MS).ok()?;
            values.get(index).copied()
        }
    }

    #[test]
    fn picks_the_cheapest_continuous_run_inside_the_window() {
        let prices = [5.0, 4.0, 1.0, 1.0, 6.0, 0.5, 9.0];
        let start = cheapest_start(&task(7, 2), series(&prices)).unwrap();
        assert_eq!(start, START + 2 * QUARTER_MS);
    }

    #[test]
    fn an_exact_tie_keeps_the_earliest_start() {
        let prices = [3.0, 1.0, 2.0, 3.0, 1.0, 2.0];
        let start = cheapest_start(&task(6, 2), series(&prices)).unwrap();
        assert_eq!(start, START + QUARTER_MS);
        let flat = [2.0; 8];
        assert_eq!(cheapest_start(&task(8, 4), series(&flat)).unwrap(), START);
    }

    #[test]
    fn a_window_shorter_than_the_run_is_infeasible_and_never_shrunk() {
        let prices = [1.0; 3];
        assert_eq!(
            cheapest_start(&task(3, 4), series(&prices)),
            Err(ScheduleError::Infeasible)
        );
        assert_eq!(
            cheapest_start(&task(3, 0), series(&prices)),
            Err(ScheduleError::Infeasible)
        );
        let off_grid = ScheduleTask {
            earliest_start_ms: START + 1,
            deadline_ms: START + 1 + 4 * QUARTER_MS,
            quarters: 2,
        };
        assert_eq!(off_grid.window_quarters(), Err(ScheduleError::Infeasible));
    }

    #[test]
    fn a_window_of_exactly_the_run_length_has_one_start() {
        let prices = [7.0, 8.0];
        assert_eq!(cheapest_start(&task(2, 2), series(&prices)).unwrap(), START);
    }

    #[test]
    fn a_missing_price_anywhere_in_the_window_stops_the_choice() {
        let prices = [1.0, 1.0, 1.0];
        assert_eq!(
            cheapest_start(&task(4, 2), series(&prices)),
            Err(ScheduleError::MissingQuarter(START + 3 * QUARTER_MS))
        );
    }

    #[test]
    fn the_run_mean_is_the_mean_of_its_quarters() {
        let prices = [1.0, 2.0, 3.0, 6.0];
        let mean = run_mean_price(START + QUARTER_MS, 3, series(&prices)).unwrap();
        assert!((mean - 11.0 / 3.0).abs() < 1e-12);
        assert_eq!(
            run_mean_price(START + 2 * QUARTER_MS, 3, series(&prices)),
            Err(ScheduleError::MissingQuarter(START + 4 * QUARTER_MS))
        );
    }

    #[test]
    fn the_contract_basis_uses_the_api_price_math_and_the_local_night_hours() {
        let contract = CostBasis::Contract(ContractScenario {
            margin_cents_kwh: 0.5,
            transfer_day_cents_kwh: 4.0,
            transfer_night_cents_kwh: 2.0,
            tax_cents_kwh: 2.0,
            vat_percent: 25.5,
            night_start_hour: Hour::new(22).unwrap(),
            night_end_hour: Hour::new(7).unwrap(),
        });
        let pricer = Pricer::new(&contract).unwrap();
        // 2026-01-15T10:00Z is 12:00 in Helsinki (day rate); 22:00Z is 00:00
        // (night rate).
        let day: i64 = "2026-01-15T10:00:00Z"
            .parse::<Timestamp>()
            .unwrap()
            .as_millisecond();
        let night = day + 12 * 60 * 60 * 1000;
        assert!((pricer.price(10.0, day) - 16.5 * 1.255).abs() < 1e-3);
        assert!((pricer.price(10.0, night) - 14.5 * 1.255).abs() < 1e-3);
        // A negative spot is floored at 0 by the API math.
        assert!((pricer.price(-5.0, night) - 4.5 * 1.255).abs() < 1e-3);
        let spot = Pricer::new(&CostBasis::Spot).unwrap();
        assert_eq!(spot.price(-5.0, night), -5.0);
    }
}
