//! The electricity contract settings of one user.

use std::fmt;

use jiff::Timestamp;
use jiff::civil::Date;

use crate::areas::Area;
use crate::time::{UserTimeZone, local_date};

/// The `valid_from` of the first contract terms version of a user, and the
/// earliest date that the API accepts.
pub const EARLIEST_VALID_FROM: Date = jiff::civil::date(1970, 1, 1);

/// The most contract terms versions that one user can hold.
pub const MAX_TERMS_VERSIONS: usize = 100;

/// The opaque identifier of a user account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserId(String);

impl UserId {
    #[must_use]
    pub const fn new(id: String) -> Self {
        Self(id)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An hour of the day, 0..=23.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hour(u8);

/// The value is not a whole hour in 0..=23.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("hour outside 0..=23")]
pub struct InvalidHour;

impl Hour {
    /// # Errors
    /// [`InvalidHour`] when `hour` is above 23.
    pub const fn new(hour: u8) -> Result<Self, InvalidHour> {
        if hour <= 23 {
            Ok(Self(hour))
        } else {
            Err(InvalidHour)
        }
    }

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<i64> for Hour {
    type Error = InvalidHour;

    fn try_from(hour: i64) -> Result<Self, Self::Error> {
        u8::try_from(hour)
            .map_err(|_| InvalidHour)
            .and_then(Self::new)
    }
}

impl TryFrom<i32> for Hour {
    type Error = InvalidHour;

    fn try_from(hour: i32) -> Result<Self, Self::Error> {
        Self::try_from(i64::from(hour))
    }
}

/// The contract terms of one user. Money values are in c/kWh.
///
/// The money fields have no bounds, because stored rows have none; the update
/// path checks the bounds of the API (`validate_settings_update`).
#[derive(Debug, Clone, PartialEq)]
pub struct UserSettings {
    pub user_id: UserId,
    pub margin_cents_kwh: f64,
    pub transfer_day_cents_kwh: f64,
    pub transfer_night_cents_kwh: f64,
    pub tax_cents_kwh: f64,
    pub vat_percent: f64,
    pub night_start_hour: Hour,
    pub night_end_hour: Hour,
    pub timezone: UserTimeZone,
    pub area: Area,
}

/// A checked partial update. A `None` field keeps the current value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsUpdate {
    pub margin_cents_kwh: Option<f64>,
    pub transfer_day_cents_kwh: Option<f64>,
    pub transfer_night_cents_kwh: Option<f64>,
    pub tax_cents_kwh: Option<f64>,
    pub vat_percent: Option<f64>,
    pub night_start_hour: Option<Hour>,
    pub night_end_hour: Option<Hour>,
    pub timezone: Option<UserTimeZone>,
    pub area: Option<Area>,
}

impl UserSettings {
    /// The settings after `update`: each given field replaces the current one.
    #[must_use]
    pub fn with_update(self, update: SettingsUpdate) -> Self {
        Self {
            user_id: self.user_id,
            margin_cents_kwh: update.margin_cents_kwh.unwrap_or(self.margin_cents_kwh),
            transfer_day_cents_kwh: update
                .transfer_day_cents_kwh
                .unwrap_or(self.transfer_day_cents_kwh),
            transfer_night_cents_kwh: update
                .transfer_night_cents_kwh
                .unwrap_or(self.transfer_night_cents_kwh),
            tax_cents_kwh: update.tax_cents_kwh.unwrap_or(self.tax_cents_kwh),
            vat_percent: update.vat_percent.unwrap_or(self.vat_percent),
            night_start_hour: update.night_start_hour.unwrap_or(self.night_start_hour),
            night_end_hour: update.night_end_hour.unwrap_or(self.night_end_hour),
            timezone: update.timezone.unwrap_or(self.timezone),
            area: update.area.unwrap_or(self.area),
        }
    }
}

/// The contract terms that change over time. Money values are in c/kWh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContractTerms {
    pub margin_cents_kwh: f64,
    pub transfer_day_cents_kwh: f64,
    pub transfer_night_cents_kwh: f64,
    pub tax_cents_kwh: f64,
    pub vat_percent: f64,
    pub night_start_hour: Hour,
    pub night_end_hour: Hour,
}

/// One version of the contract terms. It applies from the local calendar
/// date `valid_from` in the user's time zone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TermsVersion {
    pub valid_from: Date,
    pub terms: ContractTerms,
}

/// The version that applies on `date`: the latest with `valid_from <= date`,
/// or the earliest when every version starts after `date`. `ascending` must
/// be in `valid_from` order. `None` only for an empty list.
pub fn applicable_version<'a, T>(
    ascending: impl DoubleEndedIterator<Item = &'a T>,
    valid_from: impl Fn(&T) -> Date,
    date: Date,
) -> Option<&'a T> {
    let mut earliest = None;
    for version in ascending.rev() {
        if valid_from(version) <= date {
            return Some(version);
        }
        earliest = Some(version);
    }
    earliest
}

/// The settings that one version of a [`ContractSchedule`] gives.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledSettings {
    pub valid_from: Date,
    pub settings: UserSettings,
}

/// Why a [`ContractSchedule`] cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    #[error("a contract schedule needs at least one version")]
    NoVersion,
    #[error("two contract terms versions have the same date")]
    DuplicateDate,
}

/// The contract of one user over time: one time zone, one area, and at
/// least one terms version. Every date and every instant has settings.
#[derive(Debug, Clone, PartialEq)]
pub struct ContractSchedule {
    first: ScheduledSettings,
    /// In `valid_from` order, each after `first`.
    later: Vec<ScheduledSettings>,
}

impl ContractSchedule {
    /// # Errors
    /// [`ScheduleError`] when `versions` is empty or two versions have the
    /// same `valid_from`.
    pub fn new(
        user_id: &UserId,
        timezone: &UserTimeZone,
        area: Area,
        mut versions: Vec<TermsVersion>,
    ) -> Result<Self, ScheduleError> {
        versions.sort_by_key(|version| version.valid_from);
        if versions
            .windows(2)
            .any(|pair| pair.first().map(|v| v.valid_from) == pair.last().map(|v| v.valid_from))
        {
            return Err(ScheduleError::DuplicateDate);
        }
        let mut scheduled = versions.into_iter().map(|version| ScheduledSettings {
            valid_from: version.valid_from,
            settings: UserSettings {
                user_id: user_id.clone(),
                margin_cents_kwh: version.terms.margin_cents_kwh,
                transfer_day_cents_kwh: version.terms.transfer_day_cents_kwh,
                transfer_night_cents_kwh: version.terms.transfer_night_cents_kwh,
                tax_cents_kwh: version.terms.tax_cents_kwh,
                vat_percent: version.terms.vat_percent,
                night_start_hour: version.terms.night_start_hour,
                night_end_hour: version.terms.night_end_hour,
                timezone: timezone.clone(),
                area,
            },
        });
        let first = scheduled.next().ok_or(ScheduleError::NoVersion)?;
        Ok(Self {
            first,
            later: scheduled.collect(),
        })
    }

    /// The schedule of one version that applies to every date.
    #[must_use]
    pub const fn single(settings: UserSettings) -> Self {
        Self {
            first: ScheduledSettings {
                valid_from: EARLIEST_VALID_FROM,
                settings,
            },
            later: Vec::new(),
        }
    }

    #[must_use]
    pub const fn timezone(&self) -> &UserTimeZone {
        &self.first.settings.timezone
    }

    #[must_use]
    pub const fn area(&self) -> Area {
        self.first.settings.area
    }

    /// The versions in `valid_from` order.
    pub fn versions(&self) -> impl DoubleEndedIterator<Item = &ScheduledSettings> {
        std::iter::once(&self.first).chain(&self.later)
    }

    /// The version that applies on the local calendar date `date`.
    #[must_use]
    pub fn version_on(&self, date: Date) -> &ScheduledSettings {
        applicable_version(self.versions(), |version| version.valid_from, date)
            .unwrap_or(&self.first)
    }

    /// The settings of an interval that starts at `start`: the version of
    /// the local date of `start` in the time zone of the schedule.
    #[must_use]
    pub fn settings_at(&self, start: Timestamp) -> &UserSettings {
        let date = local_date(start, self.timezone().time_zone());
        &self.version_on(date).settings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> UserSettings {
        UserSettings {
            user_id: UserId::new("u".to_owned()),
            margin_cents_kwh: 0.49,
            transfer_day_cents_kwh: 2.92,
            transfer_night_cents_kwh: 1.37,
            tax_cents_kwh: 2.827_52,
            vat_percent: 25.5,
            night_start_hour: Hour::new(22).unwrap(),
            night_end_hour: Hour::new(7).unwrap(),
            timezone: UserTimeZone::parse("Europe/Helsinki").unwrap(),
            area: Area::Fi,
        }
    }

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    /// A schedule in `zone` whose margins are the `valid_from` values in
    /// `(year, month, day, margin)` form.
    fn schedule(zone: &str, versions: &[(i16, i8, i8, f64)]) -> ContractSchedule {
        let base = settings();
        let terms = |margin| ContractTerms {
            margin_cents_kwh: margin,
            transfer_day_cents_kwh: base.transfer_day_cents_kwh,
            transfer_night_cents_kwh: base.transfer_night_cents_kwh,
            tax_cents_kwh: base.tax_cents_kwh,
            vat_percent: base.vat_percent,
            night_start_hour: base.night_start_hour,
            night_end_hour: base.night_end_hour,
        };
        ContractSchedule::new(
            &base.user_id,
            &UserTimeZone::parse(zone).unwrap(),
            Area::Fi,
            versions
                .iter()
                .map(|(year, month, day, margin)| TermsVersion {
                    valid_from: jiff::civil::date(*year, *month, *day),
                    terms: terms(*margin),
                })
                .collect(),
        )
        .unwrap()
    }

    fn margin_at(schedule: &ContractSchedule, instant: &str) -> f64 {
        schedule.settings_at(ts(instant)).margin_cents_kwh
    }

    #[test]
    fn a_version_changes_exactly_at_local_midnight() {
        // Helsinki is UTC+2 in December: the local day starts at 22:00Z.
        let s = schedule("Europe/Helsinki", &[(1970, 1, 1, 0.5), (2026, 12, 1, 0.9)]);
        assert_eq!(
            margin_at(&s, "2026-11-30T21:45:00Z").to_bits(),
            0.5_f64.to_bits()
        );
        assert_eq!(
            margin_at(&s, "2026-11-30T22:00:00Z").to_bits(),
            0.9_f64.to_bits()
        );
        // The same instants are one local day apart in another zone.
        let s = schedule("Europe/Berlin", &[(1970, 1, 1, 0.5), (2026, 12, 1, 0.9)]);
        assert_eq!(
            margin_at(&s, "2026-11-30T22:45:00Z").to_bits(),
            0.5_f64.to_bits()
        );
        assert_eq!(
            margin_at(&s, "2026-11-30T23:00:00Z").to_bits(),
            0.9_f64.to_bits()
        );
    }

    #[test]
    fn a_version_changes_at_the_edges_of_a_23_hour_day() {
        // 2026-03-29 in Helsinki: 22:00Z to 21:00Z, 23 hours.
        let s = schedule(
            "Europe/Helsinki",
            &[(1970, 1, 1, 1.0), (2026, 3, 29, 2.0), (2026, 3, 30, 3.0)],
        );
        assert_eq!(
            margin_at(&s, "2026-03-28T21:45:00Z").to_bits(),
            1.0_f64.to_bits()
        );
        assert_eq!(
            margin_at(&s, "2026-03-28T22:00:00Z").to_bits(),
            2.0_f64.to_bits()
        );
        assert_eq!(
            margin_at(&s, "2026-03-29T20:45:00Z").to_bits(),
            2.0_f64.to_bits()
        );
        assert_eq!(
            margin_at(&s, "2026-03-29T21:00:00Z").to_bits(),
            3.0_f64.to_bits()
        );
    }

    #[test]
    fn a_version_changes_at_the_edges_of_a_25_hour_day() {
        // 2026-10-25 in Helsinki: 21:00Z to 22:00Z, 25 hours.
        let s = schedule(
            "Europe/Helsinki",
            &[(1970, 1, 1, 1.0), (2026, 10, 25, 2.0), (2026, 10, 26, 3.0)],
        );
        assert_eq!(
            margin_at(&s, "2026-10-24T20:45:00Z").to_bits(),
            1.0_f64.to_bits()
        );
        assert_eq!(
            margin_at(&s, "2026-10-24T21:00:00Z").to_bits(),
            2.0_f64.to_bits()
        );
        // Both passes of the repeated local hour 03:00 are on the same date.
        assert_eq!(
            margin_at(&s, "2026-10-25T00:30:00Z").to_bits(),
            2.0_f64.to_bits()
        );
        assert_eq!(
            margin_at(&s, "2026-10-25T01:30:00Z").to_bits(),
            2.0_f64.to_bits()
        );
        assert_eq!(
            margin_at(&s, "2026-10-25T21:45:00Z").to_bits(),
            2.0_f64.to_bits()
        );
        assert_eq!(
            margin_at(&s, "2026-10-25T22:00:00Z").to_bits(),
            3.0_f64.to_bits()
        );
    }

    #[test]
    fn a_date_before_the_earliest_version_uses_the_earliest_version() {
        let s = schedule("Europe/Helsinki", &[(2026, 6, 1, 1.0), (2026, 12, 1, 2.0)]);
        assert_eq!(
            margin_at(&s, "2020-01-01T12:00:00Z").to_bits(),
            1.0_f64.to_bits()
        );
        let version = s.version_on(jiff::civil::date(2026, 5, 31));
        assert_eq!(version.valid_from, jiff::civil::date(2026, 6, 1));
    }

    #[test]
    fn three_versions_resolve_in_date_order_for_any_input_order() {
        let s = schedule(
            "Europe/Helsinki",
            &[(2026, 12, 1, 3.0), (1970, 1, 1, 1.0), (2026, 6, 1, 2.0)],
        );
        let dates: Vec<Date> = s.versions().map(|version| version.valid_from).collect();
        assert_eq!(
            dates,
            [
                jiff::civil::date(1970, 1, 1),
                jiff::civil::date(2026, 6, 1),
                jiff::civil::date(2026, 12, 1)
            ]
        );
        for (day, margin) in [
            (jiff::civil::date(2026, 5, 31), 1.0_f64),
            (jiff::civil::date(2026, 6, 1), 2.0),
            (jiff::civil::date(2026, 11, 30), 2.0),
            (jiff::civil::date(2026, 12, 1), 3.0),
            (jiff::civil::date(2030, 1, 1), 3.0),
        ] {
            let version = s.version_on(day);
            assert_eq!(
                version.settings.margin_cents_kwh.to_bits(),
                margin.to_bits(),
                "{day}"
            );
        }
    }

    #[test]
    fn a_schedule_needs_one_version_and_distinct_dates() {
        let base = settings();
        let build =
            |versions| ContractSchedule::new(&base.user_id, &base.timezone, base.area, versions);
        assert_eq!(build(Vec::new()), Err(ScheduleError::NoVersion));
        let version = TermsVersion {
            valid_from: jiff::civil::date(2026, 1, 1),
            terms: ContractTerms {
                margin_cents_kwh: 1.0,
                transfer_day_cents_kwh: 0.0,
                transfer_night_cents_kwh: 0.0,
                tax_cents_kwh: 0.0,
                vat_percent: 0.0,
                night_start_hour: base.night_start_hour,
                night_end_hour: base.night_end_hour,
            },
        };
        let twice = vec![version, version];
        assert_eq!(build(twice), Err(ScheduleError::DuplicateDate));
    }

    #[test]
    fn a_single_schedule_gives_the_same_settings_for_every_instant() {
        let s = ContractSchedule::single(settings());
        assert_eq!(s.settings_at(ts("1960-01-01T00:00:00Z")), &settings());
        assert_eq!(s.settings_at(ts("2099-01-01T00:00:00Z")), &settings());
        assert_eq!(s.area(), Area::Fi);
    }

    #[test]
    fn hours_must_be_in_range() {
        assert_eq!(Hour::new(23).map(Hour::get), Ok(23));
        assert_eq!(Hour::new(24), Err(InvalidHour));
        assert_eq!(Hour::try_from(-1_i64), Err(InvalidHour));
    }

    #[test]
    fn an_empty_update_keeps_every_field() {
        assert_eq!(
            settings().with_update(SettingsUpdate::default()),
            settings()
        );
    }

    #[test]
    fn an_update_replaces_only_given_fields() {
        let update = SettingsUpdate {
            margin_cents_kwh: Some(0.0),
            night_end_hour: Some(Hour::new(6).unwrap()),
            area: Some(Area::Se3),
            ..SettingsUpdate::default()
        };
        let next = settings().with_update(update);
        assert_eq!(next.margin_cents_kwh.to_bits(), 0.0_f64.to_bits());
        assert_eq!(next.night_end_hour.get(), 6);
        assert_eq!(next.area, Area::Se3);
        assert_eq!(next.timezone.name(), "Europe/Helsinki");
        assert_eq!(next.vat_percent.to_bits(), 25.5_f64.to_bits());
    }
}
