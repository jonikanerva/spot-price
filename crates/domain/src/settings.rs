//! The electricity contract settings of one user.

use std::fmt;

use crate::areas::Area;
use crate::time::UserTimeZone;

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
