//! The pure rules of the price endpoints: which intervals an answer uses, the
//! answer when Nord Pool has not published, and the fixed texts.

use jiff::Timestamp;

use crate::js::number_to_string;
use crate::price::{
    PriceInterval, PriceWindow, TotalPrice, build_price_window, calculate_total_prices,
    find_cheapest_window,
};
use crate::query::CheapestQuery;
use crate::settings::UserSettings;
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

    /// The list of `/price/tomorrow`.
    #[must_use]
    pub fn tomorrow_from_totals(totals: Vec<TotalPrice>) -> Self {
        build_price_window(totals).map_or(
            Self::Unavailable {
                expected_at: Some(TOMORROW_EXPECTED_AT),
            },
            Self::Available,
        )
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
    let candidates = cheapest_candidates(prices, now, query.start_time, query.end_time);
    if candidates.is_empty() {
        return Err(CheapestError::NoPriceData);
    }
    let mut totals = calculate_total_prices(&candidates, settings);
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
            PriceList::tomorrow_from_totals(Vec::new()),
            PriceList::Unavailable {
                expected_at: Some("12:00 UTC")
            }
        );
    }
}
