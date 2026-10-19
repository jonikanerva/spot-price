//! Local calendar days and local-time text for the response edge.
//!
//! Instants stay UTC [`Timestamp`] values. A delivery day is a [`Date`] in the
//! user's time zone, and its UTC range comes from [`start_of_local_day_utc`].
//! A local day lasts 23, 24, or 25 hours, so no function adds 24 hours to a
//! local day.

use std::fmt;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::{Offset, TimeZone};

use crate::areas::{ALL_AREAS, Area};

/// A time zone that the setup UI offers. Only a supported zone can be held.
#[derive(Clone, PartialEq, Eq)]
pub struct UserTimeZone {
    name: &'static str,
    zone: TimeZone,
}

/// The name is not one of the supported IANA time zones.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unsupported time zone")]
pub struct UnsupportedTimeZone;

impl UserTimeZone {
    /// The supported zone with this exact IANA name.
    ///
    /// # Errors
    /// [`UnsupportedTimeZone`] when the name is not a supported zone.
    pub fn parse(name: &str) -> Result<Self, UnsupportedTimeZone> {
        let name = ALL_AREAS
            .iter()
            .map(|area| area.timezone())
            .find(|zone| *zone == name)
            .ok_or(UnsupportedTimeZone)?;
        let zone = TimeZone::get(name).map_err(|_| UnsupportedTimeZone)?;
        Ok(Self { name, zone })
    }

    /// The default zone of a delivery area.
    ///
    /// # Errors
    /// [`UnsupportedTimeZone`] when the bundled time zone database does not
    /// have the zone of the area.
    pub fn for_area(area: Area) -> Result<Self, UnsupportedTimeZone> {
        Self::parse(area.timezone())
    }

    /// The IANA name, which is also the stored and wire value.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// The Jiff time zone for the functions of this module.
    #[must_use]
    pub const fn time_zone(&self) -> &TimeZone {
        &self.zone
    }
}

impl fmt::Debug for UserTimeZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("UserTimeZone").field(&self.name).finish()
    }
}

impl fmt::Display for UserTimeZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

/// A date or an instant falls outside the range that Jiff supports.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("date outside the supported range")]
pub struct OutOfRange;

/// A half-open UTC range `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcRange {
    pub start: Timestamp,
    pub end: Timestamp,
}

/// Today and tomorrow as local calendar dates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentAndNextDate {
    pub today: Date,
    pub tomorrow: Date,
}

/// `YYYY-MM-DD` of the UTC calendar date of `instant`.
///
/// The year has no zero padding, like `String(date.getUTCFullYear())`.
#[must_use]
pub fn format_utc_date(instant: Timestamp) -> String {
    format_date(instant.to_zoned(TimeZone::UTC).date())
}

/// `YYYY-MM-DD` of the local calendar date of `instant` in `zone`.
#[must_use]
pub fn format_date_in_time_zone(instant: Timestamp, zone: &TimeZone) -> String {
    format_date(local_date(instant, zone))
}

/// The local calendar date of `instant` in `zone`.
#[must_use]
pub fn local_date(instant: Timestamp, zone: &TimeZone) -> Date {
    zone.to_datetime(instant).date()
}

/// The local hour (0..=23) of `instant` in `zone`.
#[must_use]
pub fn local_hour(instant: Timestamp, zone: &TimeZone) -> u8 {
    zone.to_datetime(instant).hour().unsigned_abs()
}

/// Shifts `instant` by whole UTC days. This is instant arithmetic, not a step
/// between local dates: use [`next_calendar_day`] for a local date label.
///
/// # Errors
/// [`OutOfRange`] when the result is outside the Jiff range.
pub fn add_days(instant: Timestamp, days: i32) -> Result<Timestamp, OutOfRange> {
    instant
        .checked_add(jiff::SignedDuration::from_hours(i64::from(days) * 24))
        .map_err(|_| OutOfRange)
}

/// The calendar day after `date`.
///
/// # Errors
/// [`OutOfRange`] after the last date that Jiff supports.
pub fn next_calendar_day(date: Date) -> Result<Date, OutOfRange> {
    date.tomorrow().map_err(|_| OutOfRange)
}

/// The UTC instant at which the local calendar day `date` starts in `zone`.
///
/// # Errors
/// [`OutOfRange`] when the day is outside the Jiff range.
pub fn start_of_local_day_utc(date: Date, zone: &TimeZone) -> Result<Timestamp, OutOfRange> {
    date.to_zoned(zone.clone())
        .and_then(|day| day.start_of_day())
        .map(|start| start.timestamp())
        .map_err(|_| OutOfRange)
}

/// The local dates of `now` and of the next calendar day in `zone`.
///
/// # Errors
/// [`OutOfRange`] when `now` is on the last date that Jiff supports.
pub fn current_and_next_date(
    now: Timestamp,
    zone: &TimeZone,
) -> Result<CurrentAndNextDate, OutOfRange> {
    let today = local_date(now, zone);
    Ok(CurrentAndNextDate {
        today,
        tomorrow: next_calendar_day(today)?,
    })
}

/// The UTC range of exactly the local calendar day `date` in `zone`.
///
/// # Errors
/// [`OutOfRange`] when the day or the next day is outside the Jiff range.
pub fn utc_range_for_local_date(date: Date, zone: &TimeZone) -> Result<UtcRange, OutOfRange> {
    utc_range_for_local_date_span(date, date, zone)
}

/// The UTC range of the local calendar days `from..=to` in `zone`. Each edge
/// uses the UTC offset of its own local midnight.
///
/// # Errors
/// [`OutOfRange`] when a day is outside the Jiff range.
pub fn utc_range_for_local_date_span(
    from: Date,
    to: Date,
    zone: &TimeZone,
) -> Result<UtcRange, OutOfRange> {
    Ok(UtcRange {
        start: start_of_local_day_utc(from, zone)?,
        end: start_of_local_day_utc(next_calendar_day(to)?, zone)?,
    })
}

/// True when the half-open periods `(start, end)` cover all of `range`
/// without a gap. The periods can come in any order and can overlap.
#[must_use]
pub fn covers_range(
    periods: impl IntoIterator<Item = (Timestamp, Timestamp)>,
    range: UtcRange,
) -> bool {
    let mut periods: Vec<(Timestamp, Timestamp)> = periods.into_iter().collect();
    periods.sort_unstable();
    let mut covered_to = range.start;
    for (start, end) in periods {
        if covered_to >= range.end {
            break;
        }
        if start > covered_to {
            return false;
        }
        covered_to = covered_to.max(end);
    }
    covered_to >= range.end
}

/// `YYYY-MM-DDTHH:MM:SS±HH:MM` of `instant` in `zone`.
///
/// The text has no fraction of a second, and UTC gives `+00:00`. The year has
/// no zero padding. An offset with seconds gives `±HH:MM:SS`.
#[must_use]
pub fn format_date_time_in_time_zone(instant: Timestamp, zone: &TimeZone) -> String {
    let offset = zone.to_offset(instant);
    let local = offset.to_datetime(instant);
    format!(
        "{}T{:02}:{:02}:{:02}{}",
        format_date(local.date()),
        local.hour(),
        local.minute(),
        local.second(),
        format_offset(offset),
    )
}

fn format_date(date: Date) -> String {
    format!("{}-{:02}-{:02}", date.year(), date.month(), date.day())
}

/// `±HH:MM`, or `±HH:MM:SS` when the offset has seconds.
fn format_offset(offset: Offset) -> String {
    let total = offset.seconds();
    let sign = if total < 0 { '-' } else { '+' };
    let total = total.unsigned_abs();
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if seconds == 0 {
        format!("{sign}{hours:02}:{minutes:02}")
    } else {
        format!("{sign}{hours:02}:{minutes:02}:{seconds:02}")
    }
}

/// `instant` without the part below one millisecond, toward the past.
///
/// The API has millisecond precision. A comparison that decides API output
/// must use this value, so a finer part cannot change the result.
#[must_use]
pub fn truncate_to_millisecond(instant: Timestamp) -> Timestamp {
    let millisecond = instant.as_millisecond();
    match Timestamp::from_millisecond(millisecond) {
        Ok(truncated) if truncated <= instant => truncated,
        _ => Timestamp::from_millisecond(millisecond - 1).unwrap_or(instant),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn zone(name: &str) -> TimeZone {
        TimeZone::get(name).unwrap()
    }

    fn range(day: Date, name: &str) -> UtcRange {
        utc_range_for_local_date(day, &zone(name)).unwrap()
    }

    fn hours(range: UtcRange) -> i64 {
        range.end.duration_since(range.start).as_secs() / 3600
    }

    #[test]
    fn helsinki_winter_day() {
        let r = range(date(2026, 2, 25), "Europe/Helsinki");
        assert_eq!(r.start, ts("2026-02-24T22:00:00Z"));
        assert_eq!(r.end, ts("2026-02-25T22:00:00Z"));
    }

    #[test]
    fn berlin_winter_day() {
        let r = range(date(2026, 2, 25), "Europe/Berlin");
        assert_eq!(r.start, ts("2026-02-24T23:00:00Z"));
        assert_eq!(r.end, ts("2026-02-25T23:00:00Z"));
    }

    #[test]
    fn utc_day() {
        let r = utc_range_for_local_date(date(2026, 2, 25), &TimeZone::UTC).unwrap();
        assert_eq!(r.start, ts("2026-02-25T00:00:00Z"));
        assert_eq!(r.end, ts("2026-02-26T00:00:00Z"));
    }

    #[test]
    fn helsinki_summer_day() {
        let r = range(date(2026, 7, 15), "Europe/Helsinki");
        assert_eq!(r.start, ts("2026-07-14T21:00:00Z"));
        assert_eq!(r.end, ts("2026-07-15T21:00:00Z"));
    }

    #[test]
    fn spring_forward_days_have_23_hours() {
        let r = range(date(2026, 3, 29), "Europe/Helsinki");
        assert_eq!(r.start, ts("2026-03-28T22:00:00Z"));
        assert_eq!(r.end, ts("2026-03-29T21:00:00Z"));
        assert_eq!(hours(r), 23);
        let r = range(date(2026, 3, 29), "Europe/Berlin");
        assert_eq!(r.start, ts("2026-03-28T23:00:00Z"));
        assert_eq!(r.end, ts("2026-03-29T22:00:00Z"));
        assert_eq!(hours(r), 23);
    }

    #[test]
    fn fall_back_days_have_25_hours() {
        let r = range(date(2026, 10, 25), "Europe/Helsinki");
        assert_eq!(r.start, ts("2026-10-24T21:00:00Z"));
        assert_eq!(r.end, ts("2026-10-25T22:00:00Z"));
        assert_eq!(hours(r), 25);
        let r = range(date(2026, 10, 25), "Europe/Berlin");
        assert_eq!(r.start, ts("2026-10-24T22:00:00Z"));
        assert_eq!(r.end, ts("2026-10-25T23:00:00Z"));
        assert_eq!(hours(r), 25);
    }

    #[test]
    fn ordinary_days_have_24_hours() {
        for name in ["Europe/Helsinki", "Europe/Berlin", "UTC"] {
            assert_eq!(hours(range(date(2026, 7, 15), name)), 24);
        }
    }

    #[test]
    fn spans_use_the_offset_of_each_edge() {
        let helsinki = zone("Europe/Helsinki");
        let span =
            utc_range_for_local_date_span(date(2026, 2, 1), date(2026, 2, 5), &helsinki).unwrap();
        assert_eq!(span.start, ts("2026-01-31T22:00:00Z"));
        assert_eq!(span.end, ts("2026-02-05T22:00:00Z"));

        let span =
            utc_range_for_local_date_span(date(2026, 3, 20), date(2026, 4, 5), &helsinki).unwrap();
        assert_eq!(
            span.start,
            range(date(2026, 3, 20), "Europe/Helsinki").start
        );
        assert_eq!(span.end, range(date(2026, 4, 6), "Europe/Helsinki").start);
        assert_eq!(span.start, ts("2026-03-19T22:00:00Z"));
        assert_eq!(span.end, ts("2026-04-05T21:00:00Z"));

        let single =
            utc_range_for_local_date_span(date(2026, 2, 25), date(2026, 2, 25), &helsinki).unwrap();
        assert_eq!(single, range(date(2026, 2, 25), "Europe/Helsinki"));

        let oslo =
            utc_range_for_local_date_span(date(2026, 2, 1), date(2026, 2, 3), &zone("Europe/Oslo"))
                .unwrap();
        assert_eq!(oslo.start, ts("2026-01-31T23:00:00Z"));
        assert_eq!(oslo.end, ts("2026-02-03T23:00:00Z"));

        let fall = utc_range_for_local_date_span(date(2026, 10, 24), date(2026, 10, 25), &helsinki)
            .unwrap();
        assert_eq!(fall.start, ts("2026-10-23T21:00:00Z"));
        assert_eq!(fall.end, ts("2026-10-25T22:00:00Z"));
        assert_eq!(hours(fall), 49);

        let fall_day =
            utc_range_for_local_date_span(date(2026, 10, 25), date(2026, 10, 25), &helsinki)
                .unwrap();
        assert_eq!(hours(fall_day), 25);
    }

    fn today_tomorrow(now: &str, name: &str) -> (Date, Date) {
        let r = current_and_next_date(ts(now), &zone(name)).unwrap();
        (r.today, r.tomorrow)
    }

    #[test]
    fn current_and_next_date_steps_calendar_labels() {
        let helsinki = "Europe/Helsinki";
        assert_eq!(
            today_tomorrow("2026-02-25T12:00:00Z", helsinki),
            (date(2026, 2, 25), date(2026, 2, 26))
        );
        assert_eq!(
            today_tomorrow("2026-12-31T22:30:00Z", helsinki),
            (date(2027, 1, 1), date(2027, 1, 2))
        );
        assert_eq!(
            today_tomorrow("2026-10-24T21:30:00Z", helsinki),
            (date(2026, 10, 25), date(2026, 10, 26))
        );
        assert_eq!(
            today_tomorrow("2026-10-25T10:00:00Z", helsinki),
            (date(2026, 10, 25), date(2026, 10, 26))
        );
        assert_eq!(
            today_tomorrow("2026-03-28T21:30:00Z", helsinki),
            (date(2026, 3, 28), date(2026, 3, 29))
        );
        assert_eq!(
            today_tomorrow("2026-03-28T22:30:00Z", helsinki),
            (date(2026, 3, 29), date(2026, 3, 30))
        );
        assert_eq!(
            today_tomorrow("2026-10-24T22:30:00Z", "Europe/Berlin"),
            (date(2026, 10, 25), date(2026, 10, 26))
        );
    }

    #[test]
    fn local_text_has_offset_and_no_fraction() {
        let helsinki = zone("Europe/Helsinki");
        assert_eq!(
            format_date_time_in_time_zone(ts("2026-02-24T12:00:00.999Z"), &helsinki),
            "2026-02-24T14:00:00+02:00"
        );
        assert_eq!(
            format_date_time_in_time_zone(ts("2026-02-24T22:00:00Z"), &helsinki),
            "2026-02-25T00:00:00+02:00"
        );
        assert_eq!(
            format_date_time_in_time_zone(ts("2026-02-24T22:00:00Z"), &TimeZone::UTC),
            "2026-02-24T22:00:00+00:00"
        );
        assert_eq!(local_hour(ts("2026-02-24T22:00:00Z"), &helsinki), 0);
    }

    #[test]
    fn user_time_zone_accepts_only_supported_names() {
        assert_eq!(
            UserTimeZone::parse("Europe/Helsinki").unwrap().name(),
            "Europe/Helsinki"
        );
        assert_eq!(UserTimeZone::parse("UTC"), Err(UnsupportedTimeZone));
        assert_eq!(
            UserTimeZone::parse("europe/helsinki"),
            Err(UnsupportedTimeZone)
        );
        for area in ALL_AREAS {
            assert_eq!(
                UserTimeZone::for_area(area).unwrap().name(),
                area.timezone()
            );
        }
    }

    /// The quarters of `[from, to)` with the quarter starting at `skip`
    /// left out.
    fn quarters(
        from: Timestamp,
        to: Timestamp,
        skip: Option<Timestamp>,
    ) -> Vec<(Timestamp, Timestamp)> {
        let quarter = jiff::SignedDuration::from_mins(15);
        let mut out = Vec::new();
        let mut at = from;
        while at < to {
            let end = at.checked_add(quarter).unwrap();
            if Some(at) != skip {
                out.push((at, end));
            }
            at = end;
        }
        out
    }

    #[test]
    fn a_full_local_day_of_quarters_covers_the_day() {
        for day in [date(2026, 2, 25), date(2026, 3, 29), date(2026, 10, 25)] {
            let r = range(day, "Europe/Helsinki");
            assert!(covers_range(quarters(r.start, r.end, None), r), "{day}");
        }
    }

    #[test]
    fn the_first_hour_of_the_next_helsinki_day_does_not_cover_the_day() {
        // Before the auction, the store has 00:00-01:00 Helsinki of the next
        // day: the last hour of the published CET delivery day.
        let r = range(date(2026, 10, 19), "Europe/Helsinki");
        let first_hour = r
            .start
            .checked_add(jiff::SignedDuration::from_hours(1))
            .unwrap();
        assert!(!covers_range(quarters(r.start, first_hour, None), r));
        assert!(!covers_range([], r));
    }

    #[test]
    fn one_missing_quarter_is_a_gap() {
        let r = range(date(2026, 2, 25), "Europe/Helsinki");
        let first = r.start;
        let middle = ts("2026-02-25T10:00:00Z");
        let last = r
            .end
            .checked_sub(jiff::SignedDuration::from_mins(15))
            .unwrap();
        for skip in [first, middle, last] {
            assert!(
                !covers_range(quarters(r.start, r.end, Some(skip)), r),
                "{skip}"
            );
        }
    }

    #[test]
    fn unordered_overlapping_and_longer_periods_still_cover() {
        let r = range(date(2026, 2, 25), "Europe/Helsinki");
        let mut periods = quarters(r.start, r.end, None);
        periods.reverse();
        periods.push((r.start, ts("2026-02-25T12:00:00Z")));
        assert!(covers_range(periods, r));
        let hourly = ts("2026-02-24T21:00:00Z");
        assert!(covers_range([(hourly, r.end), (r.start, hourly)], r));
        // A period that starts before the range and ends inside it.
        let before = ts("2026-02-24T20:00:00Z");
        assert!(!covers_range([(before, ts("2026-02-25T05:00:00Z"))], r));
    }

    #[test]
    fn truncation_drops_sub_millisecond_parts() {
        let instant = Timestamp::new(10, 1_999_999).unwrap();
        assert_eq!(
            truncate_to_millisecond(instant),
            Timestamp::new(10, 1_000_000).unwrap()
        );
        let before_epoch = Timestamp::new(-1, 999_999).unwrap();
        assert_eq!(
            truncate_to_millisecond(before_epoch),
            Timestamp::new(-1, 0).unwrap()
        );
    }
}
