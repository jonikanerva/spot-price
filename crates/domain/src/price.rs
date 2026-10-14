//! Contract price math: spot price to total price, price windows, and the
//! cheapest contiguous window.

use jiff::Timestamp;

use crate::areas::Area;
use crate::js::{max_zero, round3};
use crate::settings::UserSettings;
use crate::time::{format_date_time_in_time_zone, local_hour, truncate_to_millisecond};

/// A delivery interval `[start, end)` with `start < end`.
///
/// Both edges have millisecond precision: the API text and every comparison
/// use milliseconds, so a finer part must not change a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeliveryPeriod {
    start: Timestamp,
    end: Timestamp,
}

/// The end of a delivery interval is not after its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("delivery interval ends at or before its start")]
pub struct EmptyPeriod;

impl DeliveryPeriod {
    /// # Errors
    /// [`EmptyPeriod`] when `end` is not after `start` at millisecond precision.
    pub fn new(start: Timestamp, end: Timestamp) -> Result<Self, EmptyPeriod> {
        let (start, end) = (truncate_to_millisecond(start), truncate_to_millisecond(end));
        if start < end {
            Ok(Self { start, end })
        } else {
            Err(EmptyPeriod)
        }
    }

    #[must_use]
    pub const fn start(self) -> Timestamp {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> Timestamp {
        self.end
    }

    /// The length in minutes. It is a fraction when the edges are not on a
    /// whole minute.
    #[must_use]
    pub fn minutes(self) -> f64 {
        let milliseconds = self.end.as_millisecond() - self.start.as_millisecond();
        #[expect(
            clippy::cast_precision_loss,
            reason = "an interval of 2^53 ms is longer than the Jiff range"
        )]
        let milliseconds = milliseconds as f64;
        milliseconds / 60_000.0
    }
}

/// One day-ahead spot price of a delivery area, in EUR/MWh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PriceInterval {
    pub period: DeliveryPeriod,
    pub price_eur_mwh: f64,
    pub area: Area,
}

/// The money breakdown of one interval, in c/kWh.
///
/// `spot_cents_kwh` is the raw spot and can be negative. The total and the VAT
/// use the spot floored at 0, so the total is never below the contract terms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContractBreakdown {
    pub spot_cents_kwh: f64,
    pub margin_cents_kwh: f64,
    pub transfer_cents_kwh: f64,
    pub tax_cents_kwh: f64,
    pub vat_cents_kwh: f64,
    pub total_cents_kwh: f64,
    pub is_night_rate: bool,
}

/// The total price of one interval. `local_start` and `local_end` are the
/// edges in the user's time zone, as [`format_date_time_in_time_zone`] text.
#[derive(Debug, Clone, PartialEq)]
pub struct TotalPrice {
    pub period: DeliveryPeriod,
    pub local_start: String,
    pub local_end: String,
    pub breakdown: ContractBreakdown,
}

/// A non-empty, ordered list of total prices with its summary.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceWindow {
    start: Timestamp,
    end: Timestamp,
    start_local: String,
    end_local: String,
    min_total_cents_kwh: f64,
    max_total_cents_kwh: f64,
    average_total_cents_kwh: f64,
    prices: Vec<TotalPrice>,
}

impl PriceWindow {
    /// The start of the first interval.
    #[must_use]
    pub const fn start(&self) -> Timestamp {
        self.start
    }

    /// The end of the last interval.
    #[must_use]
    pub const fn end(&self) -> Timestamp {
        self.end
    }

    #[must_use]
    pub fn start_local(&self) -> &str {
        &self.start_local
    }

    #[must_use]
    pub fn end_local(&self) -> &str {
        &self.end_local
    }

    #[must_use]
    pub const fn min_total_cents_kwh(&self) -> f64 {
        self.min_total_cents_kwh
    }

    #[must_use]
    pub const fn max_total_cents_kwh(&self) -> f64 {
        self.max_total_cents_kwh
    }

    /// The average weighted by interval length, rounded to three decimals.
    #[must_use]
    pub const fn average_total_cents_kwh(&self) -> f64 {
        self.average_total_cents_kwh
    }

    #[must_use]
    pub fn prices(&self) -> &[TotalPrice] {
        &self.prices
    }

    #[must_use]
    pub fn into_prices(self) -> Vec<TotalPrice> {
        self.prices
    }
}

/// EUR/MWh to c/kWh, rounded to three decimals.
#[must_use]
pub fn eur_mwh_to_cents_kwh(eur_mwh: f64) -> f64 {
    round3(eur_mwh / 10.0)
}

/// Whether the local `hour` is in the night window `[start, end)`. The window
/// wraps past midnight when `start > end`, and it is empty when they are equal.
#[must_use]
pub const fn is_night_hour(hour: u8, night_start_hour: u8, night_end_hour: u8) -> bool {
    if night_start_hour > night_end_hour {
        hour >= night_start_hour || hour < night_end_hour
    } else {
        hour >= night_start_hour && hour < night_end_hour
    }
}

/// Applies margin, day or night transfer, tax, and VAT to a spot price in
/// c/kWh. `hour` is the local hour (0..=23) that selects the transfer rate.
#[must_use]
pub fn apply_contract_terms(
    spot_cents_kwh: f64,
    settings: &UserSettings,
    hour: u8,
) -> ContractBreakdown {
    let is_night_rate = is_night_hour(
        hour,
        settings.night_start_hour.get(),
        settings.night_end_hour.get(),
    );
    let transfer_cents_kwh = if is_night_rate {
        settings.transfer_night_cents_kwh
    } else {
        settings.transfer_day_cents_kwh
    };
    // Keep this operation order: the rounded results must equal the API bit
    // for bit.
    let before_vat = max_zero(spot_cents_kwh)
        + settings.margin_cents_kwh
        + transfer_cents_kwh
        + settings.tax_cents_kwh;
    let vat_multiplier = 1.0 + settings.vat_percent / 100.0;
    ContractBreakdown {
        spot_cents_kwh,
        margin_cents_kwh: settings.margin_cents_kwh,
        transfer_cents_kwh,
        tax_cents_kwh: settings.tax_cents_kwh,
        vat_cents_kwh: round3(before_vat * (settings.vat_percent / 100.0)),
        total_cents_kwh: round3(before_vat * vat_multiplier),
        is_night_rate,
    }
}

/// The total price of one spot price interval for the user's contract.
#[must_use]
pub fn calculate_total_price(price: &PriceInterval, settings: &UserSettings) -> TotalPrice {
    let zone = settings.timezone.time_zone();
    let spot_cents_kwh = eur_mwh_to_cents_kwh(price.price_eur_mwh);
    let hour = local_hour(price.period.start(), zone);
    TotalPrice {
        period: price.period,
        local_start: format_date_time_in_time_zone(price.period.start(), zone),
        local_end: format_date_time_in_time_zone(price.period.end(), zone),
        breakdown: apply_contract_terms(spot_cents_kwh, settings, hour),
    }
}

/// [`calculate_total_price`] for each interval, in order.
#[must_use]
pub fn calculate_total_prices(
    prices: &[PriceInterval],
    settings: &UserSettings,
) -> Vec<TotalPrice> {
    prices
        .iter()
        .map(|price| calculate_total_price(price, settings))
        .collect()
}

/// The window of `prices` in their given order, or `None` when it is empty.
#[must_use]
pub fn build_price_window(prices: Vec<TotalPrice>) -> Option<PriceWindow> {
    let first = prices.first()?;
    let last = prices.last()?;
    let mut weighted_sum = 0.0;
    let mut total_minutes = 0.0;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for entry in &prices {
        let total = entry.breakdown.total_cents_kwh;
        let minutes = entry.period.minutes();
        weighted_sum += total * minutes;
        total_minutes += minutes;
        if total < min {
            min = total;
        }
        if total > max {
            max = total;
        }
    }
    let average = if total_minutes > 0.0 {
        weighted_sum / total_minutes
    } else {
        0.0
    };
    Some(PriceWindow {
        start: first.period.start(),
        end: last.period.end(),
        start_local: first.local_start.clone(),
        end_local: last.local_end.clone(),
        min_total_cents_kwh: round3(min),
        max_total_cents_kwh: round3(max),
        average_total_cents_kwh: round3(average),
        prices,
    })
}

/// The contiguous run of `prices` with the lowest weighted average that lasts
/// at least `duration_minutes`.
///
/// Each start index takes intervals until the run is long enough, so the
/// window can be longer than asked (280 min of 15-min data gives 285 min). A
/// gap between intervals ends a run. The earliest window wins a tie. The scan
/// is O(n²): pass at most two days of intervals.
#[must_use]
pub fn find_cheapest_window(prices: &[TotalPrice], duration_minutes: u32) -> Option<PriceWindow> {
    if prices.is_empty() || duration_minutes == 0 {
        return None;
    }
    let duration = f64::from(duration_minutes);
    let mut best_average = f64::INFINITY;
    let mut best: Option<(usize, usize)> = None;
    for start_index in 0..prices.len() {
        let mut accumulated_minutes = 0.0;
        let mut weighted_sum = 0.0;
        let mut previous_end: Option<Timestamp> = None;
        for (index, entry) in prices.iter().enumerate().skip(start_index) {
            if previous_end.is_some_and(|end| entry.period.start() != end) {
                break;
            }
            let minutes = entry.period.minutes();
            accumulated_minutes += minutes;
            weighted_sum += entry.breakdown.total_cents_kwh * minutes;
            previous_end = Some(entry.period.end());
            if accumulated_minutes >= duration {
                let average = weighted_sum / accumulated_minutes;
                if average < best_average {
                    best_average = average;
                    best = Some((start_index, index + 1));
                }
                break;
            }
        }
    }
    let (start, end) = best?;
    build_price_window(prices.get(start..end)?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Hour, UserId};
    use crate::time::UserTimeZone;

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn settings() -> UserSettings {
        UserSettings {
            user_id: UserId::new("test-user".to_owned()),
            margin_cents_kwh: 0.45,
            transfer_day_cents_kwh: 3.02,
            transfer_night_cents_kwh: 1.55,
            tax_cents_kwh: 2.793_72,
            vat_percent: 25.5,
            night_start_hour: Hour::new(22).unwrap(),
            night_end_hour: Hour::new(7).unwrap(),
            timezone: UserTimeZone::parse("Europe/Helsinki").unwrap(),
            area: Area::Fi,
        }
    }

    /// An hour of 2026-02-24 in Helsinki winter time (+02:00).
    fn price_at(hour: i64, eur_mwh: f64) -> PriceInterval {
        let start = ts("2026-02-23T22:00:00Z")
            .checked_add(jiff::SignedDuration::from_hours(hour))
            .unwrap();
        let end = start
            .checked_add(jiff::SignedDuration::from_hours(1))
            .unwrap();
        PriceInterval {
            period: DeliveryPeriod::new(start, end).unwrap(),
            price_eur_mwh: eur_mwh,
            area: Area::Fi,
        }
    }

    fn floor() -> f64 {
        round3((0.0 + 0.45 + 3.02 + 2.793_72) * 1.255)
    }

    fn close(a: f64, b: f64, digits: i32) {
        assert!(
            (a - b).abs() < 10_f64.powi(-digits) / 2.0,
            "{a} is not close to {b}"
        );
    }

    #[test]
    fn night_window_wraps_past_midnight() {
        for hour in [22, 23, 0, 3, 6] {
            assert!(is_night_hour(hour, 22, 7));
        }
        for hour in [7, 12, 21, 14] {
            assert!(!is_night_hour(hour, 22, 7));
        }
    }

    #[test]
    fn night_window_inside_one_day() {
        for hour in [1, 3, 5] {
            assert!(is_night_hour(hour, 1, 6));
        }
        for hour in [0, 6, 12] {
            assert!(!is_night_hour(hour, 1, 6));
        }
    }

    #[test]
    fn day_hour_total() {
        let total = calculate_total_price(&price_at(12, 50.0), &settings());
        let b = total.breakdown;
        assert_eq!(b.spot_cents_kwh.to_bits(), 5.0_f64.to_bits());
        assert_eq!(b.margin_cents_kwh.to_bits(), 0.45_f64.to_bits());
        assert_eq!(b.transfer_cents_kwh.to_bits(), 3.02_f64.to_bits());
        assert_eq!(b.tax_cents_kwh.to_bits(), 2.793_72_f64.to_bits());
        assert!(!b.is_night_rate);
        assert_eq!(total.local_start, "2026-02-24T12:00:00+02:00");
        assert_eq!(total.local_end, "2026-02-24T13:00:00+02:00");
        close(b.total_cents_kwh, 14.136, 2);
        close(b.vat_cents_kwh, 2.872, 2);
    }

    #[test]
    fn night_hour_total() {
        let total = calculate_total_price(&price_at(2, 30.0), &settings());
        let b = total.breakdown;
        assert_eq!(b.transfer_cents_kwh.to_bits(), 1.55_f64.to_bits());
        assert!(b.is_night_rate);
        assert_eq!(total.local_start, "2026-02-24T02:00:00+02:00");
        close(b.total_cents_kwh, 9.781, 2);
    }

    #[test]
    fn negative_spot_is_floored_only_in_the_total() {
        let b = calculate_total_price(&price_at(14, -10.0), &settings()).breakdown;
        assert_eq!(b.spot_cents_kwh.to_bits(), (-1.0_f64).to_bits());
        close(b.total_cents_kwh, 7.861, 2);
        close(b.vat_cents_kwh, 1.597, 2);

        let b = calculate_total_price(&price_at(14, -200.0), &settings()).breakdown;
        assert_eq!(b.spot_cents_kwh.to_bits(), (-20.0_f64).to_bits());
        assert_eq!(b.total_cents_kwh.to_bits(), floor().to_bits());
        assert!(b.vat_cents_kwh >= 0.0);

        let b = calculate_total_price(&price_at(14, 0.0), &settings()).breakdown;
        assert_eq!(b.total_cents_kwh.to_bits(), floor().to_bits());
    }

    #[test]
    fn zero_margin_and_configured_zone() {
        let mut zero_margin = settings();
        zero_margin.margin_cents_kwh = 0.0;
        let b = calculate_total_price(&price_at(12, 50.0), &zero_margin).breakdown;
        close(b.total_cents_kwh, 13.571, 2);

        let price = PriceInterval {
            period: DeliveryPeriod::new(ts("2026-02-24T20:00:00Z"), ts("2026-02-24T21:00:00Z"))
                .unwrap(),
            price_eur_mwh: 50.0,
            area: Area::Fi,
        };
        let total = calculate_total_price(&price, &settings());
        assert_eq!(total.local_start, "2026-02-24T22:00:00+02:00");
        assert_eq!(total.local_end, "2026-02-24T23:00:00+02:00");
        assert!(total.breakdown.is_night_rate);
    }

    #[test]
    fn band_bounds_keep_their_order() {
        let low = apply_contract_terms(-2.0, &settings(), 14).total_cents_kwh;
        let point = apply_contract_terms(0.5, &settings(), 14).total_cents_kwh;
        let high = apply_contract_terms(4.0, &settings(), 14).total_cents_kwh;
        assert_eq!(low.to_bits(), floor().to_bits());
        assert!(low <= point && point <= high);
        let raw = apply_contract_terms(-3.0, &settings(), 14);
        assert_eq!(raw.spot_cents_kwh.to_bits(), (-3.0_f64).to_bits());
    }

    #[test]
    fn total_prices_keep_order() {
        let totals = calculate_total_prices(&[price_at(0, 20.0), price_at(12, 80.0)], &settings());
        assert_eq!(totals.len(), 2);
        assert!(totals[0].breakdown.is_night_rate);
        assert!(!totals[1].breakdown.is_night_rate);
    }

    fn total(start: Timestamp, minutes: i64, value: f64) -> TotalPrice {
        let end = start
            .checked_add(jiff::SignedDuration::from_mins(minutes))
            .unwrap();
        TotalPrice {
            period: DeliveryPeriod::new(start, end).unwrap(),
            local_start: String::new(),
            local_end: String::new(),
            breakdown: ContractBreakdown {
                spot_cents_kwh: value,
                margin_cents_kwh: 0.0,
                transfer_cents_kwh: 0.0,
                tax_cents_kwh: 0.0,
                vat_cents_kwh: 0.0,
                total_cents_kwh: value,
                is_night_rate: false,
            },
        }
    }

    fn series(minutes: i64, values: &[f64]) -> Vec<TotalPrice> {
        let base = ts("2026-02-24T00:00:00Z");
        values
            .iter()
            .zip(0_i64..)
            .map(|(value, index)| {
                let start = base
                    .checked_add(jiff::SignedDuration::from_mins(index * minutes))
                    .unwrap();
                total(start, minutes, *value)
            })
            .collect()
    }

    #[test]
    fn cheapest_three_hours() {
        let w = find_cheapest_window(&series(60, &[10.0, 2.0, 1.0, 3.0, 15.0]), 180).unwrap();
        assert_eq!(w.prices().len(), 3);
        assert_eq!(w.start(), ts("2026-02-24T01:00:00Z"));
        assert_eq!(w.end(), ts("2026-02-24T04:00:00Z"));
        assert_eq!(w.average_total_cents_kwh().to_bits(), 2.0_f64.to_bits());
        assert_eq!(w.min_total_cents_kwh().to_bits(), 1.0_f64.to_bits());
        assert_eq!(w.max_total_cents_kwh().to_bits(), 3.0_f64.to_bits());
    }

    #[test]
    fn cheapest_without_enough_data() {
        assert!(find_cheapest_window(&series(60, &[5.0, 3.0]), 180).is_none());
        assert!(find_cheapest_window(&[], 60).is_none());
        assert!(find_cheapest_window(&series(60, &[5.0]), 0).is_none());
    }

    #[test]
    fn cheapest_single_hour_and_whole_list() {
        let w = find_cheapest_window(&series(60, &[10.0, 3.0, 8.0]), 60).unwrap();
        assert_eq!(w.prices().len(), 1);
        assert_eq!(w.average_total_cents_kwh().to_bits(), 3.0_f64.to_bits());
        assert_eq!(w.start(), ts("2026-02-24T01:00:00Z"));

        let w = find_cheapest_window(&series(60, &[5.0, 3.0, 7.0]), 180).unwrap();
        assert_eq!(w.prices().len(), 3);
        assert_eq!(w.average_total_cents_kwh().to_bits(), 5.0_f64.to_bits());
    }

    fn brute_force(values: &[f64], size: usize) -> f64 {
        values
            .windows(size)
            .map(|window| window.iter().sum::<f64>() / f64::from(u32::try_from(size).unwrap()))
            .fold(f64::INFINITY, f64::min)
    }

    #[test]
    fn cheapest_matches_brute_force() {
        let values: Vec<f64> = (0..24).map(|i| f64::from(i).sin() * 5.0 + 8.0).collect();
        let w = find_cheapest_window(&series(60, &values), 180).unwrap();
        assert_eq!(
            w.average_total_cents_kwh().to_bits(),
            round3(brute_force(&values, 3)).to_bits()
        );

        let negative = [5.0, -2.0, -3.0, 1.0, 10.0, -1.0, -4.0, 2.0];
        let w = find_cheapest_window(&series(60, &negative), 120).unwrap();
        let best = negative
            .windows(2)
            .map(|pair| f64::midpoint(pair[0], pair[1]))
            .fold(f64::INFINITY, f64::min);
        assert_eq!(
            w.average_total_cents_kwh().to_bits(),
            round3(best).to_bits()
        );
    }

    #[test]
    fn cheapest_rounds_up_to_whole_intervals() {
        let values = [
            10.0, 10.0, 10.0, 10.0, 2.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0, 9.0, 9.0, 9.0, 9.0,
        ];
        let w = find_cheapest_window(&series(15, &values), 180).unwrap();
        assert_eq!(w.prices().len(), 12);
        close(w.average_total_cents_kwh(), 4.0, 3);

        let w = find_cheapest_window(&series(15, &[5.0, 1.0, 2.0, 8.0]), 50).unwrap();
        assert_eq!(w.prices().len(), 4);

        let mut values = vec![10.0; 5];
        values.extend([2.0; 17]);
        values.extend([10.0; 2]);
        let w = find_cheapest_window(&series(15, &values), 280).unwrap();
        assert_eq!(w.prices().len(), 19);
    }

    #[test]
    fn delivery_period_rejects_empty_intervals() {
        let at = ts("2026-02-24T00:00:00Z");
        assert_eq!(DeliveryPeriod::new(at, at), Err(EmptyPeriod));
        let sub_ms = Timestamp::new(at.as_second(), 500_000).unwrap();
        assert_eq!(DeliveryPeriod::new(at, sub_ms), Err(EmptyPeriod));
    }
}
