//! The pure rules of the price endpoints: which intervals an answer uses, the
//! answer when Nord Pool has not published, and the fixed texts.

use jiff::Timestamp;

use crate::js::number_to_string;
use crate::price::{
    PriceInterval, PriceWindow, TotalPrice, build_price_window, calculate_scheduled_total_prices,
    calculate_total_prices, find_cheapest_window,
};
use crate::query::CheapestQuery;
use crate::settings::{ContractSchedule, UserSettings};
use crate::time::truncate_to_millisecond;

/// The `expectedAt` hint of an unpublished `/price/tomorrow`.
pub const TOMORROW_EXPECTED_AT: &str = "12:00 UTC";
/// The unit of every price in the public and chart responses.
pub const PRICE_UNIT: &str = "c/kWh";
/// The `resolutionMinutes` of the public and chart responses.
pub const RESOLUTION_MINUTES: u32 = 15;

pub const SETTINGS_NOT_FOUND: &str = "User settings not found";
pub const NO_CURRENT_PRICE: &str = "No current price available";

/// The answer of a price-list endpoint.
///
/// On the wire, `Unavailable` is a list with `available: false`, empty text
/// for `start`, `end`, `startLocal`, and `endLocal`, 0 for the three totals,
/// and no prices. It is never an estimate.
#[derive(Debug, Clone, PartialEq)]
pub enum PriceList {
    Available(PriceWindow),
    Unavailable {
        /// Only `/price/tomorrow` gives [`TOMORROW_EXPECTED_AT`].
        expected_at: Option<&'static str>,
    },
}

impl PriceList {
    /// The list of `/price/today`, `/price/history`, and `/price/all`.
    #[must_use]
    pub fn from_totals(totals: Vec<TotalPrice>) -> Self {
        build_price_window(totals).map_or(Self::Unavailable { expected_at: None }, Self::Available)
    }

    /// The list of `/price/tomorrow`. `complete` must be true only when the
    /// totals cover every quarter of the next local day; a partial day is
    /// unavailable.
    #[must_use]
    pub fn tomorrow_from_totals(totals: Vec<TotalPrice>, complete: bool) -> Self {
        let unavailable = Self::Unavailable {
            expected_at: Some(TOMORROW_EXPECTED_AT),
        };
        if !complete {
            return unavailable;
        }
        build_price_window(totals).map_or(unavailable, Self::Available)
    }
}

/// The interval that contains `now`: the first with `start <= now < end`.
#[must_use]
pub fn current_price(prices: &[PriceInterval], now: Timestamp) -> Option<&PriceInterval> {
    let now = truncate_to_millisecond(now);
    prices
        .iter()
        .find(|price| price.period.start() <= now && now < price.period.end())
}

/// The intervals that a cheapest window may use: each starts at or after
/// `start_time` (or `now` without it) and ends at or before `end_time`.
#[must_use]
pub fn cheapest_candidates(
    prices: &[PriceInterval],
    now: Timestamp,
    start_time: Option<Timestamp>,
    end_time: Option<Timestamp>,
) -> Vec<PriceInterval> {
    let earliest = start_time.unwrap_or_else(|| truncate_to_millisecond(now));
    prices
        .iter()
        .filter(|price| price.period.start() >= earliest)
        .filter(|price| end_time.is_none_or(|end| price.period.end() <= end))
        .copied()
        .collect()
}

/// Why `/price/cheapest` has no window. The text is the 404 message.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CheapestError {
    #[error("No price data available for the requested time range")]
    NoPriceData,
    #[error(
        "Not enough contiguous price data{} to form a {duration_minutes}-minute window",
        max_price_text(*.max_price)
    )]
    NotEnoughData {
        duration_minutes: u32,
        max_price: Option<f64>,
    },
}

fn max_price_text(max_price: Option<f64>) -> String {
    max_price.map_or_else(String::new, |max| {
        format!(" at or below {} c/kWh", number_to_string(max))
    })
}

/// The answer of `/price/cheapest` for the stored intervals of today and
/// tomorrow. An interval above `max_price` is left out, which can split a run.
///
/// # Errors
/// [`CheapestError`] when no interval is in range or no window is long enough.
pub fn cheapest_window(
    prices: &[PriceInterval],
    settings: &UserSettings,
    query: &CheapestQuery,
    now: Timestamp,
) -> Result<PriceWindow, CheapestError> {
    cheapest_window_of(prices, query, now, |candidates| {
        calculate_total_prices(candidates, settings)
    })
}

/// [`cheapest_window`] where each interval uses the settings that
/// `schedule` gives for its start. The `max_price` filter uses those totals.
///
/// # Errors
/// [`CheapestError`] when no interval is in range or no window is long enough.
pub fn cheapest_scheduled_window(
    prices: &[PriceInterval],
    schedule: &ContractSchedule,
    query: &CheapestQuery,
    now: Timestamp,
) -> Result<PriceWindow, CheapestError> {
    cheapest_window_of(prices, query, now, |candidates| {
        calculate_scheduled_total_prices(candidates, schedule)
    })
}

fn cheapest_window_of(
    prices: &[PriceInterval],
    query: &CheapestQuery,
    now: Timestamp,
    totals_of: impl FnOnce(&[PriceInterval]) -> Vec<TotalPrice>,
) -> Result<PriceWindow, CheapestError> {
    let candidates = cheapest_candidates(prices, now, query.start_time, query.end_time);
    if candidates.is_empty() {
        return Err(CheapestError::NoPriceData);
    }
    let mut totals = totals_of(&candidates);
    if let Some(max) = query.max_price {
        totals.retain(|total| total.breakdown.total_cents_kwh <= max);
    }
    find_cheapest_window(&totals, query.duration_minutes).ok_or(CheapestError::NotEnoughData {
        duration_minutes: query.duration_minutes,
        max_price: query.max_price,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::areas::Area;
    use crate::price::DeliveryPeriod;
    use crate::settings::{Hour, UserId};
    use crate::time::UserTimeZone;

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn settings() -> UserSettings {
        UserSettings {
            user_id: UserId::new("u".to_owned()),
            margin_cents_kwh: 0.0,
            transfer_day_cents_kwh: 0.0,
            transfer_night_cents_kwh: 0.0,
            tax_cents_kwh: 0.0,
            vat_percent: 0.0,
            night_start_hour: Hour::new(22).unwrap(),
            night_end_hour: Hour::new(7).unwrap(),
            timezone: UserTimeZone::parse("Europe/Helsinki").unwrap(),
            area: Area::Fi,
        }
    }

    /// Quarter-hours from 2026-02-24T00:00Z with prices in EUR/MWh.
    fn quarters(eur_mwh: &[f64]) -> Vec<PriceInterval> {
        let base = ts("2026-02-24T00:00:00Z");
        eur_mwh
            .iter()
            .zip(0_i64..)
            .map(|(price, index)| {
                let start = base
                    .checked_add(jiff::SignedDuration::from_mins(index * 15))
                    .unwrap();
                let end = start
                    .checked_add(jiff::SignedDuration::from_mins(15))
                    .unwrap();
                PriceInterval {
                    period: DeliveryPeriod::new(start, end).unwrap(),
                    price_eur_mwh: *price,
                    area: Area::Fi,
                }
            })
            .collect()
    }

    fn query(duration_minutes: u32) -> CheapestQuery {
        CheapestQuery {
            duration_minutes,
            start_time: None,
            end_time: None,
            max_price: None,
        }
    }

    #[test]
    fn current_price_uses_a_half_open_interval() {
        let prices = quarters(&[10.0, 20.0]);
        let at_edge = current_price(&prices, ts("2026-02-24T00:15:00Z")).unwrap();
        assert_eq!(at_edge.price_eur_mwh.to_bits(), 20.0_f64.to_bits());
        assert!(current_price(&prices, ts("2026-02-24T00:30:00Z")).is_none());
        let sub_ms =
            Timestamp::new(ts("2026-02-24T00:30:00Z").as_second() - 1, 999_999_999).unwrap();
        assert!(current_price(&prices, sub_ms).is_some());
    }

    #[test]
    fn candidates_respect_now_and_bounds() {
        let prices = quarters(&[1.0, 2.0, 3.0, 4.0]);
        let now = ts("2026-02-24T00:10:00Z");
        assert_eq!(cheapest_candidates(&prices, now, None, None).len(), 3);
        let start = Some(ts("2026-02-24T00:00:00Z"));
        let end = Some(ts("2026-02-24T00:45:00Z"));
        assert_eq!(cheapest_candidates(&prices, now, start, end).len(), 3);
        assert_eq!(
            cheapest_candidates(&prices, now, start, Some(ts("2026-02-24T00:44:00Z"))).len(),
            2
        );
    }

    #[test]
    fn max_price_splits_runs_and_names_the_limit() {
        let prices = quarters(&[10.0, 90.0, 10.0, 10.0]);
        let now = ts("2026-02-24T00:00:00Z");
        let mut limited = query(45);
        limited.max_price = Some(5.0);
        let error = cheapest_window(&prices, &settings(), &limited, now).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Not enough contiguous price data at or below 5 c/kWh to form a 45-minute window"
        );
        let mut limited = query(30);
        limited.max_price = Some(1e21);
        let window = cheapest_window(&prices, &settings(), &limited, now).unwrap();
        assert_eq!(window.start(), ts("2026-02-24T00:30:00Z"));
        assert_eq!(
            CheapestError::NotEnoughData {
                duration_minutes: 60,
                max_price: Some(1e21)
            }
            .to_string(),
            "Not enough contiguous price data at or below 1e+21 c/kWh to form a 60-minute window"
        );
    }

    /// Helsinki versions: zero terms until 2026-02-24, then a margin of
    /// `margin` c/kWh. 2026-02-24 starts at 2026-02-23T22:00Z.
    fn two_versions(margin: f64) -> ContractSchedule {
        use crate::settings::{ContractTerms, TermsVersion};
        let base = settings();
        let terms = |margin| ContractTerms {
            margin_cents_kwh: margin,
            transfer_day_cents_kwh: 0.0,
            transfer_night_cents_kwh: 0.0,
            tax_cents_kwh: 0.0,
            vat_percent: 0.0,
            night_start_hour: base.night_start_hour,
            night_end_hour: base.night_end_hour,
        };
        ContractSchedule::new(
            &base.user_id,
            &base.timezone,
            base.area,
            vec![
                TermsVersion {
                    valid_from: jiff::civil::date(1970, 1, 1),
                    terms: terms(0.0),
                },
                TermsVersion {
                    valid_from: jiff::civil::date(2026, 2, 24),
                    terms: terms(margin),
                },
            ],
        )
        .unwrap()
    }

    /// Quarter-hours from 2026-02-23T21:00Z: four on the local date
    /// 2026-02-23 and then the local date 2026-02-24.
    fn quarters_across_midnight(eur_mwh: &[f64]) -> Vec<PriceInterval> {
        let shift = jiff::SignedDuration::from_hours(-3);
        quarters(eur_mwh)
            .into_iter()
            .map(|price| PriceInterval {
                period: DeliveryPeriod::new(
                    price.period.start().checked_add(shift).unwrap(),
                    price.period.end().checked_add(shift).unwrap(),
                )
                .unwrap(),
                ..price
            })
            .collect()
    }

    #[test]
    fn a_scheduled_window_prices_each_interval_with_its_own_version() {
        // Spot: 3 c/kWh before midnight, 1 c/kWh after it.
        let prices = quarters_across_midnight(&[30.0, 30.0, 30.0, 30.0, 10.0, 10.0, 10.0, 10.0]);
        let now = ts("2026-02-23T21:00:00Z");
        // A margin of 1 keeps the next day cheaper: 2 against 3.
        let window =
            cheapest_scheduled_window(&prices, &two_versions(1.0), &query(30), now).unwrap();
        assert_eq!(window.start(), ts("2026-02-23T22:00:00Z"));
        assert_eq!(
            window.average_total_cents_kwh().to_bits(),
            2.0_f64.to_bits()
        );
        // A margin of 5 makes the next day dearer: 6 against 3.
        let window =
            cheapest_scheduled_window(&prices, &two_versions(5.0), &query(30), now).unwrap();
        assert_eq!(window.start(), ts("2026-02-23T21:00:00Z"));
        assert_eq!(
            window.average_total_cents_kwh().to_bits(),
            3.0_f64.to_bits()
        );
        let margins: Vec<f64> = calculate_scheduled_total_prices(&prices, &two_versions(5.0))
            .iter()
            .map(|total| total.breakdown.margin_cents_kwh)
            .collect();
        assert_eq!(margins, [0.0, 0.0, 0.0, 0.0, 5.0, 5.0, 5.0, 5.0]);
    }

    #[test]
    fn max_price_uses_the_total_of_the_version_of_each_interval() {
        let prices = quarters_across_midnight(&[30.0, 30.0, 30.0, 30.0, 10.0, 10.0, 10.0, 10.0]);
        let now = ts("2026-02-23T21:00:00Z");
        let mut limited = query(30);
        // The next day costs 1 + 5 = 6, above the limit; the old terms give 3.
        limited.max_price = Some(4.0);
        let window = cheapest_scheduled_window(&prices, &two_versions(5.0), &limited, now).unwrap();
        assert_eq!(window.start(), ts("2026-02-23T21:00:00Z"));
        assert_eq!(window.end(), ts("2026-02-23T21:30:00Z"));
        limited.max_price = Some(2.5);
        let error =
            cheapest_scheduled_window(&prices, &two_versions(5.0), &limited, now).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Not enough contiguous price data at or below 2.5 c/kWh to form a 30-minute window"
        );
    }

    #[test]
    fn a_single_version_gives_the_window_of_the_settings() {
        let prices = quarters(&[10.0, 90.0, 10.0, 10.0]);
        let now = ts("2026-02-24T00:00:00Z");
        let schedule = ContractSchedule::single(settings());
        assert_eq!(
            cheapest_scheduled_window(&prices, &schedule, &query(30), now),
            cheapest_window(&prices, &settings(), &query(30), now)
        );
        assert_eq!(
            calculate_scheduled_total_prices(&prices, &schedule),
            calculate_total_prices(&prices, &settings())
        );
    }

    #[test]
    fn cheapest_without_candidates() {
        let prices = quarters(&[10.0]);
        let error = cheapest_window(&prices, &settings(), &query(15), ts("2026-02-25T00:00:00Z"))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "No price data available for the requested time range"
        );
        let error = cheapest_window(&prices, &settings(), &query(30), ts("2026-02-24T00:00:00Z"))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Not enough contiguous price data to form a 30-minute window"
        );
    }

    #[test]
    fn empty_lists_are_unavailable() {
        assert_eq!(
            PriceList::from_totals(Vec::new()),
            PriceList::Unavailable { expected_at: None }
        );
        assert_eq!(
            PriceList::tomorrow_from_totals(Vec::new(), true),
            PriceList::Unavailable {
                expected_at: Some("12:00 UTC")
            }
        );
    }

    #[test]
    fn a_partial_tomorrow_is_unavailable() {
        let totals = calculate_total_prices(&quarters(&[10.0, 20.0, 30.0, 40.0]), &settings());
        assert_eq!(
            PriceList::tomorrow_from_totals(totals.clone(), false),
            PriceList::Unavailable {
                expected_at: Some("12:00 UTC")
            }
        );
        assert!(matches!(
            PriceList::tomorrow_from_totals(totals, true),
            PriceList::Available(_)
        ));
    }
}
