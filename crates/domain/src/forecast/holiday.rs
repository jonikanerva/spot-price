//! Finnish public holidays for the optional holiday feature of the quarter
//! correction. The list is the fixed and Easter-based days off in Finland,
//! plus Midsummer Eve and Christmas Eve, which are common days off but not
//! statutory holidays. Pure: the caller gives the local date.

use jiff::civil::{Date, Weekday, date};

/// Easter Sunday of the Gregorian `year` (anonymous Gregorian algorithm).
/// `None` outside the Jiff date range.
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "the names of the published algorithm"
)]
pub fn easter_sunday(year: i16) -> Option<Date> {
    let y = i32::from(year);
    let a = y.rem_euclid(19);
    let b = y.div_euclid(100);
    let c = y.rem_euclid(100);
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15).rem_euclid(30);
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k).rem_euclid(7);
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    Date::new(year, i8::try_from(month).ok()?, i8::try_from(day).ok()?).ok()
}

/// Whether `day` falls on `weekday` within `[first, first + 6 days]` of the
/// same month.
fn weekday_between(day: Date, weekday: Weekday, month: i8, first: i8) -> bool {
    day.weekday() == weekday && day.month() == month && (first..=first + 6).contains(&day.day())
}

/// Whether the local date `day` is a Finnish public holiday or one of the
/// two common eves (see the module text).
#[must_use]
pub fn is_finnish_public_holiday(day: Date) -> bool {
    let fixed = [
        (1, 1),
        (1, 6),
        (5, 1),
        (12, 6),
        (12, 24),
        (12, 25),
        (12, 26),
    ];
    if fixed.contains(&(day.month(), day.day())) {
        return true;
    }
    // Midsummer Eve: the Friday from 19 to 25 June; Midsummer Day: the
    // Saturday from 20 to 26 June; All Saints' Day: the Saturday from
    // 31 October to 6 November.
    if weekday_between(day, Weekday::Friday, 6, 19)
        || weekday_between(day, Weekday::Saturday, 6, 20)
        || (day.weekday() == Weekday::Saturday
            && (day == date(day.year(), 10, 31) || (day.month() == 11 && day.day() <= 6)))
    {
        return true;
    }
    let Some(easter) = easter_sunday(day.year()) else {
        return false;
    };
    // Good Friday, Easter Sunday, Easter Monday, Ascension Day, Pentecost.
    [-2_i32, 0, 1, 39, 49]
        .iter()
        .filter_map(|offset| easter.checked_add(jiff::Span::new().days(*offset)).ok())
        .any(|holiday| holiday == day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easter_matches_published_dates() {
        for (year, month, day) in [
            (2000, 4, 23),
            (2019, 4, 21),
            (2024, 3, 31),
            (2025, 4, 20),
            (2026, 4, 5),
            (2027, 3, 28),
            (2038, 4, 25),
        ] {
            assert_eq!(easter_sunday(year), Some(date(year, month, day)), "{year}");
        }
    }

    #[test]
    fn the_2026_calendar_has_its_holidays_and_no_others() {
        let expected = [
            date(2026, 1, 1),
            date(2026, 1, 6),
            date(2026, 4, 3),
            date(2026, 4, 5),
            date(2026, 4, 6),
            date(2026, 5, 1),
            date(2026, 5, 14),
            date(2026, 5, 24),
            date(2026, 6, 19),
            date(2026, 6, 20),
            date(2026, 10, 31),
            date(2026, 12, 6),
            date(2026, 12, 24),
            date(2026, 12, 25),
            date(2026, 12, 26),
        ];
        let mut got = Vec::new();
        let mut day = date(2026, 1, 1);
        while day.year() == 2026 {
            if is_finnish_public_holiday(day) {
                got.push(day);
            }
            day = day.tomorrow().unwrap();
        }
        assert_eq!(got, expected);
    }

    #[test]
    fn all_saints_day_moves_into_november() {
        assert!(is_finnish_public_holiday(date(2025, 11, 1)));
        assert!(!is_finnish_public_holiday(date(2025, 10, 31)));
        assert!(is_finnish_public_holiday(date(2027, 11, 6)));
    }
}
