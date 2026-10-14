//! Nord Pool delivery areas and the IANA time zones that the setup UI offers.

use std::fmt;
use std::str::FromStr;

/// A Nord Pool day-ahead delivery area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Area {
    Fi,
    Se1,
    Se2,
    Se3,
    Se4,
    No1,
    No2,
    No3,
    No4,
    No5,
    Dk1,
    Dk2,
    Ee,
    Lt,
    Lv,
    At,
    Be,
    Fr,
    Ger,
    Nl,
    Pl,
}

/// Every area in the order of the Nord Pool request and the setup UI list.
pub const ALL_AREAS: [Area; 21] = [
    Area::Fi,
    Area::Se1,
    Area::Se2,
    Area::Se3,
    Area::Se4,
    Area::No1,
    Area::No2,
    Area::No3,
    Area::No4,
    Area::No5,
    Area::Dk1,
    Area::Dk2,
    Area::Ee,
    Area::Lt,
    Area::Lv,
    Area::At,
    Area::Be,
    Area::Fr,
    Area::Ger,
    Area::Nl,
    Area::Pl,
];

impl Area {
    /// The Nord Pool delivery area code, which is also the wire value.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Fi => "FI",
            Self::Se1 => "SE1",
            Self::Se2 => "SE2",
            Self::Se3 => "SE3",
            Self::Se4 => "SE4",
            Self::No1 => "NO1",
            Self::No2 => "NO2",
            Self::No3 => "NO3",
            Self::No4 => "NO4",
            Self::No5 => "NO5",
            Self::Dk1 => "DK1",
            Self::Dk2 => "DK2",
            Self::Ee => "EE",
            Self::Lt => "LT",
            Self::Lv => "LV",
            Self::At => "AT",
            Self::Be => "BE",
            Self::Fr => "FR",
            Self::Ger => "GER",
            Self::Nl => "NL",
            Self::Pl => "PL",
        }
    }

    /// The display name of the setup UI.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Fi => "Finland",
            Self::Se1 => "Sweden — Luleå",
            Self::Se2 => "Sweden — Sundsvall",
            Self::Se3 => "Sweden — Stockholm",
            Self::Se4 => "Sweden — Malmö",
            Self::No1 => "Norway — Oslo",
            Self::No2 => "Norway — Kristiansand",
            Self::No3 => "Norway — Trondheim",
            Self::No4 => "Norway — Tromsø",
            Self::No5 => "Norway — Bergen",
            Self::Dk1 => "Denmark — West",
            Self::Dk2 => "Denmark — East",
            Self::Ee => "Estonia",
            Self::Lt => "Lithuania",
            Self::Lv => "Latvia",
            Self::At => "Austria",
            Self::Be => "Belgium",
            Self::Fr => "France",
            Self::Ger => "Germany",
            Self::Nl => "Netherlands",
            Self::Pl => "Poland",
        }
    }

    /// The ISO 3166 country code of the area.
    #[must_use]
    pub const fn country(self) -> &'static str {
        match self {
            Self::Fi => "FI",
            Self::Se1 | Self::Se2 | Self::Se3 | Self::Se4 => "SE",
            Self::No1 | Self::No2 | Self::No3 | Self::No4 | Self::No5 => "NO",
            Self::Dk1 | Self::Dk2 => "DK",
            Self::Ee => "EE",
            Self::Lt => "LT",
            Self::Lv => "LV",
            Self::At => "AT",
            Self::Be => "BE",
            Self::Fr => "FR",
            Self::Ger => "DE",
            Self::Nl => "NL",
            Self::Pl => "PL",
        }
    }

    /// The default IANA time zone of the area.
    #[must_use]
    pub const fn timezone(self) -> &'static str {
        match self {
            Self::Fi => "Europe/Helsinki",
            Self::Se1 | Self::Se2 | Self::Se3 | Self::Se4 => "Europe/Stockholm",
            Self::No1 | Self::No2 | Self::No3 | Self::No4 | Self::No5 => "Europe/Oslo",
            Self::Dk1 | Self::Dk2 => "Europe/Copenhagen",
            Self::Ee => "Europe/Tallinn",
            Self::Lt => "Europe/Vilnius",
            Self::Lv => "Europe/Riga",
            Self::At => "Europe/Vienna",
            Self::Be => "Europe/Brussels",
            Self::Fr => "Europe/Paris",
            Self::Ger => "Europe/Berlin",
            Self::Nl => "Europe/Amsterdam",
            Self::Pl => "Europe/Warsaw",
        }
    }
}

impl fmt::Display for Area {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// The input is not one of the supported area codes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown delivery area code")]
pub struct UnknownArea;

impl FromStr for Area {
    type Err = UnknownArea;

    fn from_str(code: &str) -> Result<Self, Self::Err> {
        ALL_AREAS
            .into_iter()
            .find(|area| area.code() == code)
            .ok_or(UnknownArea)
    }
}

impl serde::Serialize for Area {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

impl<'de> serde::Deserialize<'de> for Area {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = <&str>::deserialize(deserializer)?;
        code.parse()
            .map_err(|_| serde::de::Error::custom("unknown delivery area code"))
    }
}

/// The supported IANA time zones, sorted by name and without duplicates.
#[must_use]
pub fn supported_timezones() -> Vec<&'static str> {
    let mut zones: Vec<&'static str> = ALL_AREAS.iter().map(|area| area.timezone()).collect();
    zones.sort_unstable();
    zones.dedup();
    zones
}

/// Whether `zone` is one of [`supported_timezones`].
#[must_use]
pub fn is_supported_timezone(zone: &str) -> bool {
    ALL_AREAS.iter().any(|area| area.timezone() == zone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_round_trips() {
        for area in ALL_AREAS {
            assert_eq!(area.code().parse::<Area>(), Ok(area));
        }
    }

    #[test]
    fn unknown_and_lowercase_codes_are_rejected() {
        assert_eq!("fi".parse::<Area>(), Err(UnknownArea));
        assert_eq!("DE".parse::<Area>(), Err(UnknownArea));
    }

    #[test]
    fn timezones_are_sorted_and_unique() {
        let zones = supported_timezones();
        assert_eq!(zones.len(), 13);
        assert!(zones.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(is_supported_timezone("Europe/Helsinki"));
        assert!(!is_supported_timezone("UTC"));
    }
}
