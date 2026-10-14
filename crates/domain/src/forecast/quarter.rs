//! UTC 15-minute quarters, the time key of every forecast series.

use std::collections::BTreeMap;

use jiff::Timestamp;

use super::ForecastError;

pub(crate) const QUARTER_MS: i64 = 15 * 60 * 1000;
pub(crate) const HOUR_MS: i64 = 60 * 60 * 1000;
pub(crate) const DAY_MS: i64 = 24 * HOUR_MS;
pub(crate) const WEEK_MS: i64 = 7 * DAY_MS;

/// The UTC start of a 15-minute quarter, in epoch milliseconds. Every
/// constructor floors to the quarter boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QuarterStart(i64);

/// A series of values keyed by quarter. Iteration is chronological.
pub type QuarterMap = BTreeMap<QuarterStart, f64>;

impl QuarterStart {
    /// The quarter that contains the UTC epoch millisecond `ms`.
    #[must_use]
    pub const fn from_millisecond(ms: i64) -> Self {
        Self(ms.saturating_sub(ms.rem_euclid(QUARTER_MS)))
    }

    /// The quarter that contains `at`. Sub-millisecond precision is dropped.
    #[must_use]
    pub fn floor(at: Timestamp) -> Self {
        Self::from_millisecond(at.as_millisecond())
    }

    /// The UTC epoch millisecond of the quarter start.
    #[must_use]
    pub const fn as_millisecond(self) -> i64 {
        self.0
    }

    /// The quarter that contains `self + ms`.
    #[must_use]
    pub const fn offset(self, ms: i64) -> Self {
        Self::from_millisecond(self.0.saturating_add(ms))
    }

    /// The quarter start as a Jiff instant.
    ///
    /// # Errors
    ///
    /// Returns [`ForecastError::OutOfRange`] when the instant is outside the
    /// Jiff range.
    pub fn timestamp(self) -> Result<Timestamp, ForecastError> {
        Timestamp::from_millisecond(self.0).map_err(|_| ForecastError::OutOfRange)
    }

    /// The UTC hour of day, 0–23.
    #[must_use]
    pub const fn utc_hour(self) -> u8 {
        // The value is in 0..24, so the cast is lossless.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "rem_euclid(DAY_MS) / HOUR_MS is in 0..24"
        )]
        let hour = (self.0.rem_euclid(DAY_MS) / HOUR_MS) as u8;
        hour
    }

    /// The UTC day of week, 0 = Sunday … 6 = Saturday.
    #[must_use]
    pub const fn utc_weekday(self) -> u8 {
        // 1970-01-01 was a Thursday (4).
        #[expect(clippy::cast_possible_truncation, reason = "rem_euclid(7) is in 0..7")]
        let weekday = (self.0.div_euclid(DAY_MS) + 4).rem_euclid(7) as u8;
        weekday
    }

    /// The UTC epoch millisecond of the start of the UTC day.
    #[must_use]
    pub(crate) const fn utc_day_start_ms(self) -> i64 {
        self.0.div_euclid(DAY_MS) * DAY_MS
    }

    /// The quarter key in the `Date.prototype.toISOString` format, for example
    /// `2026-03-02T00:15:00.000Z`.
    ///
    /// # Errors
    ///
    /// Returns [`ForecastError::OutOfRange`] when the instant is outside the
    /// Jiff range.
    pub fn key(self) -> Result<String, ForecastError> {
        Ok(iso_millis(self.timestamp()?))
    }
}

/// `Date.prototype.toISOString` for an instant: UTC, millisecond precision.
#[must_use]
pub fn iso_millis(at: Timestamp) -> String {
    format!("{at:.3}")
}

/// A quarter map from `(instant, value)` points. Each instant is floored to
/// its quarter, and a later point replaces an earlier one in the same quarter.
#[must_use]
pub fn quarter_map<I>(points: I) -> QuarterMap
where
    I: IntoIterator<Item = (Timestamp, f64)>,
{
    points
        .into_iter()
        .map(|(at, value)| (QuarterStart::floor(at), value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn floors_to_the_quarter_boundary() {
        let q = QuarterStart::floor(ts("2026-03-01T10:07:33.5Z"));
        assert_eq!(q.key().unwrap(), "2026-03-01T10:00:00.000Z");
        let q = QuarterStart::floor(ts("2026-03-01T10:59:59.999Z"));
        assert_eq!(q.key().unwrap(), "2026-03-01T10:45:00.000Z");
    }

    #[test]
    fn is_idempotent_on_a_boundary() {
        let anchor = ts("2026-03-01T00:00:00Z").as_millisecond();
        assert_eq!(
            QuarterStart::from_millisecond(anchor).as_millisecond(),
            anchor
        );
    }

    #[test]
    fn floors_before_the_epoch() {
        assert_eq!(
            QuarterStart::from_millisecond(-1).as_millisecond(),
            -QUARTER_MS
        );
    }

    #[test]
    fn reads_utc_calendar_fields() {
        let wed_noon = QuarterStart::floor(ts("2026-03-04T12:00:00Z"));
        assert_eq!(wed_noon.utc_hour(), 12);
        assert_eq!(wed_noon.utc_weekday(), 3);
        let sun = QuarterStart::floor(ts("2026-03-08T23:45:00Z"));
        assert_eq!(sun.utc_hour(), 23);
        assert_eq!(sun.utc_weekday(), 0);
        let before_epoch = QuarterStart::floor(ts("1969-12-31T23:45:00Z"));
        assert_eq!(before_epoch.utc_hour(), 23);
        assert_eq!(before_epoch.utc_weekday(), 3);
    }

    #[test]
    fn a_later_point_replaces_an_earlier_one() {
        let map = quarter_map([
            (ts("2026-03-01T00:00:00Z"), 1.0),
            (ts("2026-03-01T00:10:00Z"), 2.0),
        ]);
        assert_eq!(map.len(), 1);
        assert_eq!(map.values().next(), Some(&2.0));
    }
}
