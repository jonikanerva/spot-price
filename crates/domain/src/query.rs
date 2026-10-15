//! Validation of the API inputs: the cheapest-window query, the price-history
//! query, and the settings update.
//!
//! The rules and the issue texts are the API contract. A request with several
//! problems reports every issue in field order, and [`ValidationError`] joins
//! them with `"; "`, the text of the 400 response.

use std::cmp::Ordering;
use std::fmt;

use jiff::Timestamp;
use jiff::civil::{Date, DateTime};
use jiff::tz::Offset;

use crate::areas::{ALL_AREAS, Area};
use crate::js::{compare_utf16, date_utc, js_trim, number_of_utf16_slice, string_to_number};
use crate::settings::{Hour, SettingsUpdate};
use crate::time::UserTimeZone;

/// The longest price-history range, in inclusive calendar days.
pub const MAX_HISTORY_SPAN_DAYS: f64 = 31.0;

/// The bounds of the cheapest-window duration, in minutes.
pub const MIN_WINDOW_MINUTES: u32 = 1;
pub const MAX_WINDOW_MINUTES: u32 = 1440;

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// The input field that an [`Issue`] is about. Its wire name is the path text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Duration,
    StartTime,
    EndTime,
    MaxPrice,
    From,
    To,
    MarginCentsKwh,
    TransferDayCentsKwh,
    TransferNightCentsKwh,
    TaxCentsKwh,
    VatPercent,
    NightStartHour,
    NightEndHour,
    Timezone,
    Area,
}

impl Field {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Duration => "duration",
            Self::StartTime => "startTime",
            Self::EndTime => "endTime",
            Self::MaxPrice => "maxPrice",
            Self::From => "from",
            Self::To => "to",
            Self::MarginCentsKwh => "marginCentsKwh",
            Self::TransferDayCentsKwh => "transferDayCentsKwh",
            Self::TransferNightCentsKwh => "transferNightCentsKwh",
            Self::TaxCentsKwh => "taxCentsKwh",
            Self::VatPercent => "vatPercent",
            Self::NightStartHour => "nightStartHour",
            Self::NightEndHour => "nightEndHour",
            Self::Timezone => "timezone",
            Self::Area => "area",
        }
    }
}

/// What is wrong with a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    Missing,
    NotANumber,
    NotFinite,
    NotAnInteger,
    AboveSafeInteger,
    BelowSafeInteger,
    TooSmall(u32),
    TooBig(u32),
    InvalidDateTime,
    InvalidDate,
    MaxPriceEmpty,
    MaxPriceNotFinite,
    FromAfterTo,
    SpanTooLong,
    UnknownTimezone,
    UnknownArea,
}

/// One problem with one field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Issue {
    pub field: Field,
    pub kind: IssueKind,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.field.name())?;
        match self.kind {
            IssueKind::Missing => f.write_str("Invalid input: expected string, received undefined"),
            IssueKind::NotANumber => f.write_str("Invalid input: expected number, received NaN"),
            IssueKind::NotFinite => f.write_str("Invalid input: expected number, received number"),
            IssueKind::NotAnInteger => f.write_str("Invalid input: expected int, received number"),
            IssueKind::AboveSafeInteger => {
                f.write_str("Too big: expected int to be <=9007199254740991")
            }
            IssueKind::BelowSafeInteger => {
                f.write_str("Too small: expected int to be >=-9007199254740991")
            }
            IssueKind::TooSmall(min) => write!(f, "Too small: expected number to be >={min}"),
            IssueKind::TooBig(max) => write!(f, "Too big: expected number to be <={max}"),
            IssueKind::InvalidDateTime => f.write_str("Invalid ISO datetime"),
            IssueKind::InvalidDate => f.write_str("Invalid ISO date"),
            IssueKind::MaxPriceEmpty => f.write_str("maxPrice is required when provided"),
            IssueKind::MaxPriceNotFinite => f.write_str("maxPrice must be a valid number"),
            IssueKind::FromAfterTo => f.write_str("from must be on or before to"),
            IssueKind::SpanTooLong => f.write_str("date range must not exceed 31 days"),
            IssueKind::UnknownTimezone => {
                let mut zones = crate::areas::supported_timezones().into_iter();
                write_options(f, &mut zones)
            }
            IssueKind::UnknownArea => {
                let mut codes = ALL_AREAS.iter().map(|area| area.code());
                write_options(f, &mut codes)
            }
        }
    }
}

fn write_options(
    f: &mut fmt::Formatter<'_>,
    options: &mut dyn Iterator<Item = &'static str>,
) -> fmt::Result {
    f.write_str("Invalid option: expected one of ")?;
    for (index, option) in options.enumerate() {
        if index > 0 {
            f.write_str("|")?;
        }
        write!(f, "\"{option}\"")?;
    }
    Ok(())
}

/// Every issue of one input, in field order.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct ValidationError {
    pub issues: Vec<Issue>,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, issue) in self.issues.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{issue}")?;
        }
        Ok(())
    }
}

fn finish<T>(issues: Vec<Issue>, value: impl FnOnce() -> Option<T>) -> Result<T, ValidationError> {
    if issues.is_empty() {
        // The issue list is empty only when every value parsed.
        value().ok_or(ValidationError { issues })
    } else {
        Err(ValidationError { issues })
    }
}

// --- Numbers ----------------------------------------------------------------

/// The issues of a number that must be finite and in `min..=max`.
fn number_issues(field: Field, value: f64, min: u32, max: Option<u32>) -> Vec<Issue> {
    let issue = |kind| Issue { field, kind };
    if value.is_nan() {
        return vec![issue(IssueKind::NotANumber)];
    }
    if !value.is_finite() {
        return vec![issue(IssueKind::NotFinite)];
    }
    let mut issues = Vec::new();
    if value < f64::from(min) {
        issues.push(issue(IssueKind::TooSmall(min)));
    }
    if let Some(max) = max
        && value > f64::from(max)
    {
        issues.push(issue(IssueKind::TooBig(max)));
    }
    issues
}

/// The issues of a number that must be a whole number in `min..=max`.
fn integer_issues(field: Field, value: f64, min: u32, max: u32) -> Vec<Issue> {
    let issue = |kind| Issue { field, kind };
    if !value.is_finite() {
        return number_issues(field, value, min, Some(max));
    }
    if value.fract() != 0.0 {
        return vec![issue(IssueKind::NotAnInteger)];
    }
    let mut issues = Vec::new();
    if value > MAX_SAFE_INTEGER {
        issues.push(issue(IssueKind::AboveSafeInteger));
    } else if value < -MAX_SAFE_INTEGER {
        issues.push(issue(IssueKind::BelowSafeInteger));
    }
    issues.extend(number_issues(field, value, min, Some(max)));
    issues
}

/// A whole number that [`integer_issues`] accepted for `0..=max`.
fn small_integer(value: f64) -> Option<u32> {
    if value.fract() != 0.0 || !(0.0..=f64::from(u32::MAX)).contains(&value) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is a whole number inside the u32 range"
    )]
    let whole = value as u32;
    Some(whole)
}

// --- Instants ---------------------------------------------------------------

/// Parses `YYYY-MM-DDTHH:MM[:SS[.fraction]]` with `Z` or `±HH:MM`.
///
/// Each space becomes `+` first, because a `+` in a query string decodes to a
/// space. The fraction keeps only milliseconds. An instant after the Jiff
/// range becomes [`Timestamp::MAX`], which is later than every stored price.
fn parse_query_instant(text: &str) -> Option<Timestamp> {
    let text = text.replace(' ', "+");
    let bytes = text.as_bytes();
    let date = parse_iso_date(bytes.get(..10)?)?;
    let rest = bytes.get(10..)?;
    let [b'T', h1, h2, b':', m1, m2, rest @ ..] = rest else {
        return None;
    };
    let hour = two_digits(*h1, *h2).filter(|hour| *hour <= 23)?;
    let minute = two_digits(*m1, *m2).filter(|minute| *minute <= 59)?;
    let (second, millisecond, rest) = match rest {
        [b':', s1, s2, rest @ ..] => {
            let second = two_digits(*s1, *s2).filter(|second| *second <= 59)?;
            match rest {
                [b'.', fraction @ ..] => {
                    let digits = fraction.iter().take_while(|b| b.is_ascii_digit()).count();
                    if digits == 0 {
                        return None;
                    }
                    let millisecond = fraction[..digits]
                        .iter()
                        .chain(std::iter::repeat(&b'0'))
                        .take(3)
                        .fold(0_i32, |acc, digit| acc * 10 + i32::from(digit - b'0'));
                    (second, millisecond, &fraction[digits..])
                }
                rest => (second, 0, rest),
            }
        }
        rest => (0, 0, rest),
    };
    let offset_seconds = match rest {
        [b'Z'] => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let hours = two_digits(*h1, *h2).filter(|hours| *hours <= 23)?;
            let minutes = two_digits(*m1, *m2).filter(|minutes| *minutes <= 59)?;
            let seconds = i32::from(hours) * 3600 + i32::from(minutes) * 60;
            if *sign == b'-' { -seconds } else { seconds }
        }
        _ => return None,
    };
    let local = DateTime::new(
        date.year(),
        date.month(),
        date.day(),
        i8::try_from(hour).ok()?,
        i8::try_from(minute).ok()?,
        i8::try_from(second).ok()?,
        millisecond * 1_000_000,
    )
    .ok()?;
    let offset = Offset::from_seconds(offset_seconds).ok()?;
    Some(offset.to_timestamp(local).unwrap_or(Timestamp::MAX))
}

fn two_digits(tens: u8, ones: u8) -> Option<u8> {
    (tens.is_ascii_digit() && ones.is_ascii_digit()).then(|| (tens - b'0') * 10 + (ones - b'0'))
}

/// Parses exactly `YYYY-MM-DD`, a real Gregorian date with a year 0000..=9999.
fn parse_iso_date(bytes: &[u8]) -> Option<Date> {
    let [y1, y2, y3, y4, b'-', m1, m2, b'-', d1, d2] = bytes else {
        return None;
    };
    let year = i16::from(two_digits(*y1, *y2)?) * 100 + i16::from(two_digits(*y3, *y4)?);
    let month = two_digits(*m1, *m2)?;
    let day = two_digits(*d1, *d2)?;
    Date::new(year, i8::try_from(month).ok()?, i8::try_from(day).ok()?).ok()
}

// --- Cheapest-window query ----------------------------------------------------

/// The raw query values of `GET /api/v1/price/cheapest`. `None` is an absent
/// parameter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CheapestQueryInput<'a> {
    pub duration: Option<&'a str>,
    pub start_time: Option<&'a str>,
    pub end_time: Option<&'a str>,
    pub max_price: Option<&'a str>,
}

/// A valid cheapest-window query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CheapestQuery {
    /// In `MIN_WINDOW_MINUTES..=MAX_WINDOW_MINUTES`.
    pub duration_minutes: u32,
    /// The earliest window start, at millisecond precision.
    pub start_time: Option<Timestamp>,
    /// The latest window end, at millisecond precision.
    pub end_time: Option<Timestamp>,
    /// The highest allowed total price of each interval, in c/kWh.
    pub max_price: Option<f64>,
}

/// Validates the cheapest-window query.
///
/// `duration` and `maxPrice` use JavaScript `Number(...)`, so ` 60 `, `6e1`,
/// and `0x3C` are 60. `maxPrice` must not be blank and must be finite.
///
/// # Errors
/// [`ValidationError`] with every issue in field order.
pub fn parse_cheapest_query(
    input: CheapestQueryInput<'_>,
) -> Result<CheapestQuery, ValidationError> {
    let duration = input.duration.map_or(f64::NAN, string_to_number);
    let mut issues = integer_issues(
        Field::Duration,
        duration,
        MIN_WINDOW_MINUTES,
        MAX_WINDOW_MINUTES,
    );

    let mut instant = |field, text: Option<&str>| match text {
        None => None,
        Some(text) => {
            let parsed = parse_query_instant(text);
            if parsed.is_none() {
                issues.push(Issue {
                    field,
                    kind: IssueKind::InvalidDateTime,
                });
            }
            parsed
        }
    };
    let start_time = instant(Field::StartTime, input.start_time);
    let end_time = instant(Field::EndTime, input.end_time);

    let max_price = input.max_price.map(|text| {
        let text = js_trim(text);
        if text.is_empty() {
            return Err(IssueKind::MaxPriceEmpty);
        }
        let value = string_to_number(text);
        if value.is_finite() {
            Ok(value)
        } else {
            Err(IssueKind::MaxPriceNotFinite)
        }
    });
    if let Some(Err(kind)) = max_price {
        issues.push(Issue {
            field: Field::MaxPrice,
            kind,
        });
    }

    finish(issues, || {
        Some(CheapestQuery {
            duration_minutes: small_integer(duration)?,
            start_time,
            end_time,
            max_price: max_price.transpose().ok()?,
        })
    })
}

// --- Price-history query --------------------------------------------------------

/// A valid inclusive range of local calendar days.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryQuery {
    pub from: Date,
    pub to: Date,
}

/// Validates the price-history query: two `YYYY-MM-DD` dates, `from` not
/// after `to`, and at most 31 inclusive days.
///
/// The order and span checks run on the raw text whenever both values are
/// present, also when a value is not a date, and they can add issues then.
///
/// # Errors
/// [`ValidationError`] with every issue in field order.
pub fn parse_history_query(
    from: Option<&str>,
    to: Option<&str>,
) -> Result<HistoryQuery, ValidationError> {
    let mut issues = Vec::new();
    let mut date = |field, text: Option<&str>| {
        let kind = match text {
            None => IssueKind::Missing,
            Some(text) => match parse_iso_date(text.as_bytes()) {
                Some(date) => return Some(date),
                None => IssueKind::InvalidDate,
            },
        };
        issues.push(Issue { field, kind });
        None
    };
    let from_date = date(Field::From, from);
    let to_date = date(Field::To, to);

    if let (Some(from), Some(to)) = (from, to) {
        if compare_utf16(from, to) == Ordering::Greater {
            issues.push(Issue {
                field: Field::From,
                kind: IssueKind::FromAfterTo,
            });
        }
        // A NaN span fails this check, like the API.
        let span = inclusive_day_span(from, to).partial_cmp(&MAX_HISTORY_SPAN_DAYS);
        if !matches!(span, Some(Ordering::Less | Ordering::Equal)) {
            issues.push(Issue {
                field: Field::To,
                kind: IssueKind::SpanTooLong,
            });
        }
    }

    finish(issues, || {
        Some(HistoryQuery {
            from: from_date?,
            to: to_date?,
        })
    })
}

/// The inclusive day count of the API: the `YYYY`, `MM`, and `DD` positions of
/// each text go through `Number(...)` and `Date.UTC`, so a two-digit year is
/// in the 1900s and a text that is not a date can still give a number.
fn inclusive_day_span(from: &str, to: &str) -> f64 {
    let day = |text: &str| {
        date_utc(
            number_of_utf16_slice(text, 0, 4),
            number_of_utf16_slice(text, 5, 7) - 1.0,
            number_of_utf16_slice(text, 8, 10),
        )
    };
    (day(to) - day(from)) / 86_400_000.0 + 1.0
}

// --- Settings update ----------------------------------------------------------------

/// The values of a settings update body. `None` is an absent field.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SettingsUpdateInput<'a> {
    pub margin_cents_kwh: Option<f64>,
    pub transfer_day_cents_kwh: Option<f64>,
    pub transfer_night_cents_kwh: Option<f64>,
    pub tax_cents_kwh: Option<f64>,
    pub vat_percent: Option<f64>,
    pub night_start_hour: Option<f64>,
    pub night_end_hour: Option<f64>,
    pub timezone: Option<&'a str>,
    pub area: Option<&'a str>,
}

/// Validates a settings update: money values at least 0, VAT in 0..=100,
/// hours whole numbers in 0..=23, a supported time zone, and an area code.
///
/// # Errors
/// [`ValidationError`] with every issue in field order.
pub fn validate_settings_update(
    input: SettingsUpdateInput<'_>,
) -> Result<SettingsUpdate, ValidationError> {
    let mut issues = Vec::new();
    let mut money = |field, value: Option<f64>, max| {
        if let Some(value) = value {
            issues.extend(number_issues(field, value, 0, max));
        }
        value
    };
    let margin_cents_kwh = money(Field::MarginCentsKwh, input.margin_cents_kwh, None);
    let transfer_day_cents_kwh = money(
        Field::TransferDayCentsKwh,
        input.transfer_day_cents_kwh,
        None,
    );
    let transfer_night_cents_kwh = money(
        Field::TransferNightCentsKwh,
        input.transfer_night_cents_kwh,
        None,
    );
    let tax_cents_kwh = money(Field::TaxCentsKwh, input.tax_cents_kwh, None);
    let vat_percent = money(Field::VatPercent, input.vat_percent, Some(100));

    let mut hour = |field, value: Option<f64>| {
        let value = value?;
        let found = integer_issues(field, value, 0, 23);
        if !found.is_empty() {
            issues.extend(found);
            return Some(Err(()));
        }
        let hour = small_integer(value)
            .and_then(|hour| u8::try_from(hour).ok())
            .and_then(|hour| Hour::new(hour).ok());
        Some(hour.ok_or(()))
    };
    let night_start_hour = hour(Field::NightStartHour, input.night_start_hour);
    let night_end_hour = hour(Field::NightEndHour, input.night_end_hour);

    let timezone = input.timezone.map(|name| {
        UserTimeZone::parse(name).map_err(|_| {
            issues.push(Issue {
                field: Field::Timezone,
                kind: IssueKind::UnknownTimezone,
            });
        })
    });
    let area = input.area.map(|code| {
        code.parse::<Area>().map_err(|_| {
            issues.push(Issue {
                field: Field::Area,
                kind: IssueKind::UnknownArea,
            });
        })
    });

    finish(issues, || {
        Some(SettingsUpdate {
            margin_cents_kwh,
            transfer_day_cents_kwh,
            transfer_night_cents_kwh,
            tax_cents_kwh,
            vat_percent,
            night_start_hour: night_start_hour.transpose().ok()?,
            night_end_hour: night_end_hour.transpose().ok()?,
            timezone: timezone.transpose().ok()?,
            area: area.transpose().ok()?,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn cheapest(duration: &str) -> CheapestQueryInput<'_> {
        CheapestQueryInput {
            duration: Some(duration),
            ..CheapestQueryInput::default()
        }
    }

    #[test]
    fn duration_uses_javascript_numbers() {
        assert_eq!(
            parse_cheapest_query(cheapest(" 0x3C "))
                .unwrap()
                .duration_minutes,
            60
        );
        assert_eq!(
            parse_cheapest_query(cheapest("6e1"))
                .unwrap()
                .duration_minutes,
            60
        );
        assert_eq!(
            parse_cheapest_query(cheapest("0.5"))
                .unwrap_err()
                .to_string(),
            "duration: Invalid input: expected int, received number"
        );
        assert_eq!(
            parse_cheapest_query(CheapestQueryInput::default())
                .unwrap_err()
                .to_string(),
            "duration: Invalid input: expected number, received NaN"
        );
    }

    #[test]
    fn query_instants_accept_a_space_for_plus() {
        let query = parse_cheapest_query(CheapestQueryInput {
            start_time: Some("2026-02-27T14:00:00 02:00"),
            end_time: Some("2026-02-27T14:00:00.9999Z"),
            ..cheapest("60")
        })
        .unwrap();
        assert_eq!(
            query.start_time,
            Some("2026-02-27T12:00:00Z".parse().unwrap())
        );
        assert_eq!(
            query.end_time,
            Some("2026-02-27T14:00:00.999Z".parse().unwrap())
        );
    }

    #[test]
    fn history_checks_order_and_span() {
        assert_eq!(
            parse_history_query(Some("2026-04-01"), Some("2026-05-01")).unwrap(),
            HistoryQuery {
                from: date(2026, 4, 1),
                to: date(2026, 5, 1)
            }
        );
        assert_eq!(
            parse_history_query(Some("2026-04-01"), Some("2026-05-02"))
                .unwrap_err()
                .to_string(),
            "to: date range must not exceed 31 days"
        );
        assert_eq!(
            parse_history_query(Some("2026-05-02"), Some("2026-05-01"))
                .unwrap_err()
                .to_string(),
            "from: from must be on or before to"
        );
    }

    #[test]
    fn settings_update_reports_each_field() {
        let error = validate_settings_update(SettingsUpdateInput {
            margin_cents_kwh: Some(-1.0),
            night_start_hour: Some(1.5),
            area: Some("XX"),
            ..SettingsUpdateInput::default()
        })
        .unwrap_err();
        assert_eq!(error.issues.len(), 3);
        assert!(error.to_string().starts_with(
            "marginCentsKwh: Too small: expected number to be >=0; nightStartHour: Invalid input: expected int, received number; area: Invalid option: expected one of \"FI\"|"
        ));
        let update = validate_settings_update(SettingsUpdateInput {
            night_end_hour: Some(-0.0),
            timezone: Some("Europe/Oslo"),
            ..SettingsUpdateInput::default()
        })
        .unwrap();
        assert_eq!(update.night_end_hour.map(Hour::get), Some(0));
        assert_eq!(update.timezone.map(|zone| zone.name()), Some("Europe/Oslo"));
    }
}
