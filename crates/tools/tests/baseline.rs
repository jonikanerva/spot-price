//! The naive baselines of the backtest: the last-published-day baseline
//! maps each target to the same local quarter in Europe/Helsinki on the one
//! local day that ends at the series start, also across both DST changes.

#[cfg(test)]
mod baseline {
    use std::collections::BTreeMap;

    use jiff::Timestamp;
    use jiff::tz::TimeZone;
    use spot_price_domain::forecast::{QuarterMap, QuarterStart};
    use spot_price_tools::backtest::{Baseline, IssueTimeInputs, baseline_for, baseline_zone};

    const QUARTER_MS: i64 = 15 * 60 * 1000;
    const HOUR_MS: i64 = 60 * 60 * 1000;
    const DAY_MS: i64 = 24 * HOUR_MS;

    fn ms(text: &str) -> i64 {
        text.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn helsinki() -> TimeZone {
        baseline_zone().unwrap()
    }

    /// Issue-time inputs whose series starts at `series_start`. Each FI
    /// price in the 8 days before it is its own delivery start in ms, so a
    /// baseline value names its source quarter.
    #[expect(clippy::cast_precision_loss, reason = "ms values fit exactly in f64")]
    fn inputs(series_start: &str) -> IssueTimeInputs {
        let origin = ms(series_start);
        let mut fi_prices = QuarterMap::new();
        let mut q = origin - 8 * DAY_MS;
        while q < origin {
            fi_prices.insert(QuarterStart::from_millisecond(q), q as f64);
            q += QUARTER_MS;
        }
        IssueTimeInputs {
            t_ms: origin - 10 * HOUR_MS,
            last_published_ms: Some(origin - QUARTER_MS),
            origin_ms: origin,
            fi_prices,
            neighbor_prices: BTreeMap::new(),
            forecast: BTreeMap::new(),
            forecast_ladder: BTreeMap::new(),
            actual: BTreeMap::new(),
            older_fi_prices: QuarterMap::new(),
            older_neighbor_prices: BTreeMap::new(),
            older_actual: BTreeMap::new(),
        }
    }

    /// The source quarter of the last-published-day baseline, as UTC text.
    #[expect(clippy::cast_possible_truncation, reason = "values are ms")]
    fn source(inputs: &IssueTimeInputs, target: &str) -> Option<String> {
        baseline_for(Baseline::LastPublishedDay, ms(target), inputs, &helsinki())
            .map(|v| Timestamp::from_millisecond(v as i64).unwrap().to_string())
    }

    /// The fixed 24-hour rule that the baseline used before.
    fn fixed_day_key(target_ms: i64, series_start_ms: i64) -> i64 {
        series_start_ms - DAY_MS + (target_ms - series_start_ms).rem_euclid(DAY_MS)
    }

    #[test]
    fn a_target_reads_the_same_local_quarter_of_the_last_published_day() {
        // The CET day 2026-02-11 is 01:00–01:00 in Helsinki (UTC+2).
        let day = inputs("2026-02-11T23:00:00Z");
        // 12:15 local on 2026-02-13 reads 12:15 local on 2026-02-11.
        assert_eq!(
            source(&day, "2026-02-13T10:15:00Z").as_deref(),
            Some("2026-02-11T10:15:00Z")
        );
        // 00:30 local is in the last published hour, on 2026-02-12.
        assert_eq!(
            source(&day, "2026-02-13T22:30:00Z").as_deref(),
            Some("2026-02-11T22:30:00Z")
        );
        // 01:00 local is the first quarter of the window.
        assert_eq!(
            source(&day, "2026-02-12T23:00:00Z").as_deref(),
            Some("2026-02-10T23:00:00Z")
        );
    }

    #[test]
    fn without_a_dst_change_the_baseline_equals_the_fixed_24_hour_rule() {
        let zone = helsinki();
        let offset = |at: i64| {
            Timestamp::from_millisecond(at)
                .unwrap()
                .to_zoned(zone.clone())
                .offset()
        };
        let mut series_start = ms("2025-12-31T23:00:00Z");
        let mut compared = 0;
        while series_start < ms("2027-01-01T00:00:00Z") {
            let day = inputs(
                &Timestamp::from_millisecond(series_start)
                    .unwrap()
                    .to_string(),
            );
            let window_start = series_start - DAY_MS;
            let mut target = series_start;
            while target < series_start + 3 * DAY_MS {
                if offset(target) == offset(window_start) && offset(target) == offset(series_start)
                {
                    let got = baseline_for(Baseline::LastPublishedDay, target, &day, &zone);
                    #[expect(clippy::cast_precision_loss, reason = "ms values fit exactly in f64")]
                    let want = fixed_day_key(target, series_start) as f64;
                    assert_eq!(got, Some(want), "target {target}");
                    compared += 1;
                }
                target += QUARTER_MS;
            }
            series_start += DAY_MS;
        }
        assert!(compared > 350 * 288, "compared {compared}");
    }

    // Spring: Helsinki skips 03:00–04:00 local on 2026-03-29 (01:00 UTC).

    #[test]
    fn the_skipped_spring_hour_of_the_last_published_day_has_no_value() {
        // The window is the 23-hour CET day 2026-03-29.
        let day = inputs("2026-03-29T22:00:00Z");
        for target in [
            "2026-03-30T00:00:00Z",
            "2026-03-30T00:45:00Z",
            "2026-03-31T00:15:00Z",
        ] {
            assert_eq!(source(&day, target), None, "{target}");
        }
        // 02:45 local reads the winter quarter of the short day, 04:00
        // local its summer quarter.
        assert_eq!(
            source(&day, "2026-03-30T23:45:00Z").as_deref(),
            Some("2026-03-29T00:45:00Z")
        );
        assert_ne!(
            fixed_day_key(ms("2026-03-30T23:45:00Z"), ms("2026-03-29T22:00:00Z")),
            ms("2026-03-29T00:45:00Z")
        );
        assert_eq!(
            source(&day, "2026-03-31T01:00:00Z").as_deref(),
            Some("2026-03-29T01:00:00Z")
        );
        // 12:00 local keeps its local quarter.
        assert_eq!(
            source(&day, "2026-03-31T09:00:00Z").as_deref(),
            Some("2026-03-29T09:00:00Z")
        );
    }

    #[test]
    fn a_target_after_the_spring_change_reads_the_winter_local_quarter() {
        // A 24-hour window in winter time; the target day is the short day.
        let day = inputs("2026-03-28T23:00:00Z");
        // 04:15 local (UTC+3) reads 04:15 local (UTC+2) on 2026-03-28.
        assert_eq!(
            source(&day, "2026-03-29T01:15:00Z").as_deref(),
            Some("2026-03-28T02:15:00Z")
        );
        assert_ne!(
            fixed_day_key(ms("2026-03-29T01:15:00Z"), ms("2026-03-28T23:00:00Z")),
            ms("2026-03-28T02:15:00Z")
        );
        // 02:15 local (UTC+2), before the change, keeps its local quarter.
        assert_eq!(
            source(&day, "2026-03-29T00:15:00Z").as_deref(),
            Some("2026-03-28T00:15:00Z")
        );
    }

    // Autumn: Helsinki repeats 03:00–04:00 local on 2026-10-25: first UTC+3
    // (00:00–01:00 UTC), then UTC+2 (01:00–02:00 UTC).

    #[test]
    fn the_repeated_autumn_hour_takes_the_utc_offset_of_the_target() {
        // The window is the 25-hour CET day 2026-10-25.
        let day = inputs("2026-10-25T23:00:00Z");
        // 03:15 local (UTC+2) on 2026-10-26 reads the UTC+2 copy.
        assert_eq!(
            source(&day, "2026-10-26T01:15:00Z").as_deref(),
            Some("2026-10-25T01:15:00Z")
        );
        // 02:15 local reads 02:15 local (UTC+3), not the first 03:15.
        assert_eq!(
            source(&day, "2026-10-26T00:15:00Z").as_deref(),
            Some("2026-10-24T23:15:00Z")
        );
        assert_eq!(
            fixed_day_key(ms("2026-10-26T00:15:00Z"), ms("2026-10-25T23:00:00Z")),
            ms("2026-10-25T00:15:00Z")
        );
        // 01:00 local (UTC+3) is the first quarter of the 25-hour window.
        assert_eq!(
            source(&day, "2026-10-26T23:00:00Z").as_deref(),
            Some("2026-10-24T22:00:00Z")
        );
    }

    #[test]
    fn both_copies_of_a_repeated_target_hour_read_one_source_quarter() {
        // A 24-hour window in summer time; the target day is the long day.
        let day = inputs("2026-10-24T22:00:00Z");
        for target in ["2026-10-25T00:15:00Z", "2026-10-25T01:15:00Z"] {
            assert_eq!(
                source(&day, target).as_deref(),
                Some("2026-10-24T00:15:00Z"),
                "{target}"
            );
        }
    }

    #[test]
    fn the_last_week_baseline_keeps_its_fixed_offset() {
        let day = inputs("2026-03-29T22:00:00Z");
        let got = baseline_for(
            Baseline::LastWeek,
            ms("2026-03-30T09:00:00Z"),
            &day,
            &helsinki(),
        );
        #[expect(clippy::cast_precision_loss, reason = "ms values fit exactly in f64")]
        let want = (ms("2026-03-30T09:00:00Z") - 7 * DAY_MS) as f64;
        assert_eq!(got, Some(want));
    }

    #[test]
    fn no_last_published_price_gives_no_baseline() {
        let mut day = inputs("2026-02-11T23:00:00Z");
        day.last_published_ms = None;
        assert_eq!(source(&day, "2026-02-13T10:15:00Z"), None);
    }
}
