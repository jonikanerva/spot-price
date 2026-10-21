//! The price profiles of the within-day shape: the persistence rule, the
//! previous published day, and the same-weekday mean of the served forecast, on simple known shapes, on
//! missing data, and across the DST changes of Europe/Oslo in 2026.

#[cfg(test)]
mod shape_profile {
    use std::collections::BTreeMap;

    use jiff::Timestamp;
    use jiff::tz::TimeZone;
    use spot_price_domain::forecast::{
        DELIVERY_DAY_TIME_ZONE, ForecastInput, ForecastOptions, ForecastResult, GridRecord,
        NeighborPrices, QuarterMap, QuarterStart, SERVED_SHAPE_WEEKS, ShapeProfile,
        apply_within_day_shape, apply_within_day_shape_with, build_forecast, profile_values,
    };

    const QUARTER_MS: i64 = 15 * 60 * 1000;
    const HOUR_MS: i64 = 60 * 60 * 1000;
    const DAY_MS: i64 = 24 * HOUR_MS;

    fn ms(text: &str) -> i64 {
        text.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn q(text: &str) -> QuarterStart {
        QuarterStart::from_millisecond(ms(text))
    }

    fn oslo() -> TimeZone {
        TimeZone::get("Europe/Oslo").unwrap()
    }

    /// A small integer as a float.
    fn int(value: i64) -> f64 {
        f64::from(i32::try_from(value).unwrap())
    }

    /// Every quarter in `[from, to)` with the price `f(quarter start ms)`.
    fn prices(from: &str, to: &str, f: impl Fn(i64) -> f64) -> QuarterMap {
        let mut out = QuarterMap::new();
        let mut at = ms(from);
        while at < ms(to) {
            out.insert(QuarterStart::from_millisecond(at), f(at));
            at += QUARTER_MS;
        }
        out
    }

    /// The local hour of `at` in Europe/Oslo plus a day tag, so that a test
    /// can read which local day and hour a profile price came from.
    fn tagged(at: i64) -> f64 {
        let zoned = Timestamp::from_millisecond(at).unwrap().to_zoned(oslo());
        int(i64::from(zoned.day())) * 100.0 + int(i64::from(zoned.hour()))
    }

    // Simple known shapes

    #[test]
    fn the_previous_published_day_takes_the_latest_complete_local_day() {
        let zone = oslo();
        // Local days 2026-06-08 and 2026-06-09 (CEST, UTC+2) are complete;
        // 2026-06-10 has only its first hour.
        let spot = prices("2026-06-07T22:00:00Z", "2026-06-09T23:00:00Z", tagged);
        // A target on 2026-06-11 skips the incomplete 2026-06-10.
        let target = q("2026-06-11T08:00:00Z"); // 10:00 local
        let got = profile_values(ShapeProfile::PreviousPublishedDay(&zone), [target], &spot);
        assert_eq!(got.values.get(&target), Some(&910.0));
        assert_eq!((got.missing, got.dst_gap), (0, 0));
        // A target on 2026-06-10 takes 2026-06-09.
        let target = q("2026-06-10T08:00:00Z");
        let got = profile_values(ShapeProfile::PreviousPublishedDay(&zone), [target], &spot);
        assert_eq!(got.values.get(&target), Some(&910.0));
    }

    #[test]
    fn the_previous_published_day_is_missing_without_a_complete_day_in_a_week() {
        let zone = oslo();
        let spot = prices("2026-06-01T00:00:00Z", "2026-06-01T12:00:00Z", tagged);
        let target = q("2026-06-03T08:00:00Z");
        let got = profile_values(ShapeProfile::PreviousPublishedDay(&zone), [target], &spot);
        assert!(got.values.is_empty());
        assert_eq!((got.missing, got.dst_gap), (1, 0));
    }

    #[test]
    fn one_week_takes_the_same_local_time_seven_days_back() {
        let zone = oslo();
        let spot = prices("2026-05-01T00:00:00Z", "2026-06-10T00:00:00Z", tagged);
        let target = q("2026-06-10T08:15:00Z"); // Wednesday 10:15 local
        let profile = ShapeProfile::WeekdayMean {
            zone: &zone,
            weeks: 1,
        };
        let got = profile_values(profile, [target], &spot);
        assert_eq!(got.values.get(&target), Some(&310.0));
    }

    #[test]
    fn the_weekday_mean_averages_the_same_weekday_of_each_week() {
        let zone = oslo();
        let spot = prices("2026-05-01T00:00:00Z", "2026-06-10T00:00:00Z", tagged);
        let target = q("2026-06-10T08:15:00Z");
        let profile = ShapeProfile::WeekdayMean {
            zone: &zone,
            weeks: 3,
        };
        let got = profile_values(profile, [target], &spot);
        // Days 3 June, 27 May, and 20 May at 10 local.
        let want = (310.0 + 2710.0 + 2010.0) / 3.0;
        assert_eq!(got.values.get(&target), Some(&want));
    }

    #[test]
    fn a_weekday_mean_with_a_missing_week_is_missing_not_a_shorter_mean() {
        let zone = oslo();
        let mut spot = prices("2026-05-01T00:00:00Z", "2026-06-10T00:00:00Z", tagged);
        spot.remove(&q("2026-05-27T08:15:00Z"));
        let target = q("2026-06-10T08:15:00Z");
        let profile = |weeks| ShapeProfile::WeekdayMean { zone: &zone, weeks };
        let got = profile_values(profile(3), [target], &spot);
        assert!(got.values.is_empty());
        assert_eq!((got.missing, got.dst_gap), (1, 0));
        // One week does not reach the gap.
        assert_eq!(
            profile_values(profile(1), [target], &spot)
                .values
                .get(&target),
            Some(&310.0)
        );
        assert_eq!(profile_values(profile(0), [target], &spot).missing, 1);
    }

    #[test]
    fn the_persistence_profile_is_the_one_day_lag_else_the_one_week_lag() {
        let spot = prices("2026-06-01T00:00:00Z", "2026-06-10T00:00:00Z", |at| {
            int(at / HOUR_MS % 1000)
        });
        let day_lag = q("2026-06-10T05:00:00Z");
        let week_lag = q("2026-06-11T05:00:00Z");
        let got = profile_values(ShapeProfile::Persistence, [day_lag, week_lag], &spot);
        let price = |text| spot.get(&q(text)).copied();
        assert_eq!(
            got.values.get(&day_lag).copied(),
            price("2026-06-09T05:00:00Z")
        );
        assert_eq!(
            got.values.get(&week_lag).copied(),
            price("2026-06-04T05:00:00Z")
        );
    }

    // The shape step

    /// A ridge-like prediction: a different flat level on each UTC day.
    fn flat_levels(from: &str, to: &str) -> QuarterMap {
        prices(from, to, |at| 5.0 + int(at.div_euclid(DAY_MS) % 3))
    }

    fn utc_day_means(map: &QuarterMap) -> BTreeMap<i64, f64> {
        let mut sums: BTreeMap<i64, (f64, f64)> = BTreeMap::new();
        for (key, value) in map {
            let entry = sums
                .entry(key.as_millisecond().div_euclid(DAY_MS))
                .or_default();
            entry.0 += value;
            entry.1 += 1.0;
        }
        sums.into_iter().map(|(d, (s, n))| (d, s / n)).collect()
    }

    #[test]
    fn every_profile_keeps_the_utc_day_level_of_the_prediction() {
        let zone = oslo();
        let spot = prices("2026-05-01T00:00:00Z", "2026-06-10T00:00:00Z", |at| {
            tagged(at) + (int(at / QUARTER_MS % 7) - 3.0) * 0.25
        });
        let predicted = flat_levels("2026-06-10T00:00:00Z", "2026-06-13T00:00:00Z");
        for profile in [
            ShapeProfile::Persistence,
            ShapeProfile::PreviousPublishedDay(&zone),
            ShapeProfile::WeekdayMean {
                zone: &zone,
                weeks: 1,
            },
            ShapeProfile::WeekdayMean {
                zone: &zone,
                weeks: 4,
            },
        ] {
            let shaped = apply_within_day_shape_with(&predicted, &spot, profile);
            assert_eq!(shaped.shape_fallback_quarters, 0, "{profile:?}");
            let (got, want) = (utc_day_means(&shaped.result), utc_day_means(&predicted));
            for (day, level) in want {
                assert!((got[&day] - level).abs() < 1e-9, "{profile:?} day {day}");
            }
        }
    }

    #[test]
    fn the_shape_follows_the_profile_of_its_family() {
        let zone = oslo();
        // Last week had a peak at 18 local; the latest day had it at 08.
        let spot = prices("2026-05-01T00:00:00Z", "2026-06-10T22:00:00Z", |at| {
            let local = Timestamp::from_millisecond(at).unwrap().to_zoned(oslo());
            let peak_hour = if local.date() == "2026-06-10".parse().unwrap() {
                8
            } else {
                18
            };
            if local.hour() == peak_hour { 20.0 } else { 4.0 }
        });
        let predicted = flat_levels("2026-06-11T00:00:00Z", "2026-06-12T00:00:00Z");
        let peak = |profile| {
            let shaped = apply_within_day_shape_with(&predicted, &spot, profile);
            let (key, _) = shaped
                .result
                .iter()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap();
            key.timestamp().unwrap().to_zoned(oslo()).hour()
        };
        assert_eq!(peak(ShapeProfile::PreviousPublishedDay(&zone)), 8);
        assert_eq!(
            peak(ShapeProfile::WeekdayMean {
                zone: &zone,
                weeks: 1
            }),
            18
        );
    }

    #[test]
    fn the_persistence_shape_is_unchanged_by_the_profile_option() {
        let spot = prices("2026-05-01T00:00:00Z", "2026-06-10T00:00:00Z", |at| {
            tagged(at) * 0.01 + (int(at / QUARTER_MS % 11) - 5.0) * 0.37
        });
        let mut predicted = flat_levels("2026-06-09T12:00:00Z", "2026-06-13T00:00:00Z");
        // A quarter with neither lag keeps the flat level in both paths.
        predicted.insert(q("2026-07-01T00:00:00Z"), 7.0);
        let old = apply_within_day_shape(&predicted, &spot);
        let new = apply_within_day_shape_with(&predicted, &spot, ShapeProfile::Persistence);
        assert_eq!(old.shape_fallback_quarters, 1);
        assert_eq!(old.shape_fallback_quarters, new.shape_fallback_quarters);
        let bits = |m: &QuarterMap| m.iter().map(|(k, v)| (*k, v.to_bits())).collect::<Vec<_>>();
        assert_eq!(bits(&old.result), bits(&new.result));
    }

    // The served shape: the four-week weekday mean

    /// Prices for the served profile of the UTC day `day`. The UTC day covers
    /// parts of the local days `day` and `day + 1`. On those local days minus
    /// 7k days (k = 1 to 4), the price is k × the local hour. Other days have
    /// no price, so the one-day lag of persistence is missing.
    fn weekly_multiples(day: &str) -> QuarterMap {
        let date: jiff::civil::Date = day.parse().unwrap();
        let start = ms(&format!("{day}T00:00:00Z"));
        let mut out = QuarterMap::new();
        let mut at = start - 30 * DAY_MS;
        while at < start {
            let local = Timestamp::from_millisecond(at).unwrap().to_zoned(oslo());
            let days_back = i64::from(local.date().until(date).unwrap().get_days());
            let week = if days_back % 7 == 0 {
                days_back / 7
            } else {
                (days_back + 1) / 7
            };
            if (days_back % 7 == 0 || days_back % 7 == 6) && (1..=4).contains(&week) {
                out.insert(
                    QuarterStart::from_millisecond(at),
                    int(week) * int(i64::from(local.hour())),
                );
            }
            at += QUARTER_MS;
        }
        out
    }

    /// A prediction on the UTC day `day` that varies per quarter around a
    /// day mean of 6.
    fn level_six(day: &str) -> QuarterMap {
        let start = ms(&format!("{day}T00:00:00Z"));
        prices(
            &format!("{day}T00:00:00Z"),
            &Timestamp::from_millisecond(start + DAY_MS)
                .unwrap()
                .to_string(),
            |at| 6.0 + int(at / QUARTER_MS % 4) - 1.5,
        )
    }

    fn local_hour(key: QuarterStart) -> f64 {
        int(i64::from(key.timestamp().unwrap().to_zoned(oslo()).hour()))
    }

    fn assert_close(got: f64, want: f64, what: &str) {
        assert!((got - want).abs() < 1e-9, "{what}: got {got}, want {want}");
    }

    #[test]
    fn the_served_shape_is_the_mean_of_four_weeks_around_the_daily_level() {
        let zone = oslo();
        let served = ForecastOptions::served(&zone).shape_profile;
        // Thursday 10 December 2026, CET: the UTC day is 01:00 local to
        // 01:00 local on the next day, so every local hour occurs once.
        let spot = weekly_multiples("2026-12-10");
        let predicted = level_six("2026-12-10");
        let shaped = apply_within_day_shape_with(&predicted, &spot, served);
        assert_eq!(shaped.shape_fallback_quarters, 0);
        // Profile = (1 + 2 + 3 + 4) / 4 × hour = 2.5 × hour; its day mean is
        // 2.5 × 11.5 = 28.75.
        for (key, value) in &shaped.result {
            assert_close(*value, 6.0 + 2.5 * local_hour(*key) - 28.75, "quarter");
        }
        let at = |text| shaped.result[&q(text)];
        assert_close(at("2026-12-10T00:00:00Z"), -20.25, "01 local");
        assert_close(at("2026-12-10T12:00:00Z"), 9.75, "13 local");
        assert_close(at("2026-12-10T23:45:00Z"), -22.75, "00 local");
        assert_close(utc_day_means(&shaped.result)[&20_797], 6.0, "daily level");
        // Persistence reads only the week before: 6 + 13 − 11.5.
        let persistence = apply_within_day_shape(&predicted, &spot);
        assert_close(
            persistence.result[&q("2026-12-10T12:00:00Z")],
            7.5,
            "persistence",
        );
    }

    #[test]
    fn a_quarter_without_four_weeks_keeps_the_flat_daily_level() {
        let zone = oslo();
        let served = ForecastOptions::served(&zone).shape_profile;
        let mut spot = weekly_multiples("2026-12-10");
        // 13:00 local on 12 November, four weeks back.
        assert_eq!(spot.remove(&q("2026-11-12T12:00:00Z")), Some(52.0));
        let predicted = level_six("2026-12-10");
        let shaped = apply_within_day_shape_with(&predicted, &spot, served);
        assert_eq!(shaped.shape_fallback_quarters, 1);
        let gap = q("2026-12-10T12:00:00Z");
        assert_close(shaped.result[&gap], 6.0, "flat quarter");
        // The other 95 quarters: hour sum 4 × 276 − 13, profile sum
        // 2.5 × 1091 = 2727.5.
        for (key, value) in shaped.result.iter().filter(|(k, _)| **k != gap) {
            assert_close(
                *value,
                6.0 + 2.5 * local_hour(*key) - 2727.5 / 95.0,
                "quarter",
            );
        }
        assert_close(utc_day_means(&shaped.result)[&20_797], 6.0, "daily level");
    }

    #[test]
    fn a_served_source_in_the_skipped_hour_keeps_the_flat_daily_level() {
        let zone = oslo();
        let served = ForecastOptions::served(&zone).shape_profile;
        // Sunday 5 April 2026, CEST: the UTC day is 02:00 local to 02:00 local
        // on the next day. One week back, 29 March has no 02:00.
        let spot = weekly_multiples("2026-04-05");
        let predicted = level_six("2026-04-05");
        let in_gap: Vec<QuarterStart> = (0..4)
            .map(|i| q("2026-04-05T00:00:00Z").offset(i * QUARTER_MS))
            .collect();
        let profile = profile_values(served, predicted.keys().copied(), &spot);
        assert_eq!((profile.missing, profile.dst_gap), (4, 4));
        let shaped = apply_within_day_shape_with(&predicted, &spot, served);
        assert_eq!(shaped.shape_fallback_quarters, 4);
        // The other hours: 03 to 23 on 5 April and 00, 01 on 6 April; hour
        // sum 273 + 0 + 1 = 274 over 23 hours.
        for (key, value) in &shaped.result {
            let want = if in_gap.contains(key) {
                6.0
            } else {
                6.0 + 2.5 * local_hour(*key) - 2.5 * 274.0 / 23.0
            };
            assert_close(*value, want, "quarter");
        }
        assert_close(utc_day_means(&shaped.result)[&20_548], 6.0, "daily level");
    }

    // DST: Europe/Oslo moves to CEST at 01:00 UTC on 29 March 2026 and back
    // to CET at 01:00 UTC on 25 October 2026.

    #[test]
    fn a_source_time_in_the_skipped_hour_is_a_counted_dst_gap() {
        let zone = oslo();
        // No price on 4 April, so the persistence rule falls to its week lag.
        let spot = prices("2026-03-20T00:00:00Z", "2026-04-04T00:00:00Z", tagged);
        // Sunday 5 April 02:15 CEST; one week back, 02:15 does not exist.
        let in_gap = q("2026-04-05T00:15:00Z");
        let after = q("2026-04-05T01:15:00Z"); // 03:15 CEST
        let profile = ShapeProfile::WeekdayMean {
            zone: &zone,
            weeks: 1,
        };
        let got = profile_values(profile, [in_gap, after], &spot);
        assert_eq!((got.missing, got.dst_gap), (1, 1));
        assert_eq!(got.values.get(&after), Some(&2903.0));
        // The persistence rule reads 168 h back: 01:15 CET, one local hour off.
        let persistence = profile_values(ShapeProfile::Persistence, [in_gap], &spot);
        assert_eq!(persistence.values.get(&in_gap), Some(&2901.0));
    }

    #[test]
    fn a_twenty_three_hour_day_is_a_complete_previous_day() {
        let zone = oslo();
        // 29 March 2026 runs from 23:00 UTC on 28 March to 22:00 UTC: 92
        // quarters.
        let spot = prices("2026-03-28T23:00:00Z", "2026-03-29T22:00:00Z", tagged);
        assert_eq!(spot.len(), 92);
        let target = q("2026-03-30T08:00:00Z"); // 10:00 CEST
        let got = profile_values(ShapeProfile::PreviousPublishedDay(&zone), [target], &spot);
        assert_eq!(got.values.get(&target), Some(&2910.0));
    }

    #[test]
    fn the_new_profiles_keep_the_local_time_after_the_change_to_cet() {
        let zone = oslo();
        // No price on 31 October, so the persistence rule falls to its week lag.
        let spot = prices("2026-10-15T00:00:00Z", "2026-10-31T00:00:00Z", tagged);
        // Sunday 1 November 01:00 CET.
        let target = q("2026-11-01T00:00:00Z");
        let profile = ShapeProfile::WeekdayMean {
            zone: &zone,
            weeks: 1,
        };
        assert_eq!(
            profile_values(profile, [target], &spot).values.get(&target),
            Some(&2501.0)
        );
        // 168 h back is 02:00 CEST: one local hour off.
        assert_eq!(
            profile_values(ShapeProfile::Persistence, [target], &spot)
                .values
                .get(&target),
            Some(&2502.0)
        );
    }

    #[test]
    fn the_repeated_hour_takes_the_source_with_the_same_utc_offset() {
        let zone = oslo();
        let spot = prices("2026-10-15T00:00:00Z", "2026-11-02T00:00:00Z", |at| {
            int(at / QUARTER_MS % 100_000)
        });
        let price = |text| spot.get(&q(text)).copied();
        let profile = ShapeProfile::WeekdayMean {
            zone: &zone,
            weeks: 1,
        };
        // Sunday 1 November 02:15 CET (+01): one week back, 02:15 happens
        // twice; the +01 instance is 01:15 UTC.
        let winter = q("2026-11-01T01:15:00Z");
        assert_eq!(
            profile_values(profile, [winter], &spot)
                .values
                .get(&winter)
                .copied(),
            price("2026-10-25T01:15:00Z")
        );
        // The first 02:15 of Sunday 25 October (CEST) reads 02:15 of the
        // week before, which happens once.
        let summer_target_on_fold_day = q("2026-10-25T00:15:00Z");
        assert_eq!(
            profile_values(profile, [summer_target_on_fold_day], &spot)
                .values
                .get(&summer_target_on_fold_day)
                .copied(),
            price("2026-10-18T00:15:00Z")
        );
        // Both 02:15 instances of the 25-hour day read the one 02:15 of the
        // day before.
        let second = q("2026-10-25T01:15:00Z"); // 02:15 CET
        let got = profile_values(
            ShapeProfile::PreviousPublishedDay(&zone),
            [summer_target_on_fold_day, second],
            &spot,
        );
        assert_eq!(
            got.values.get(&summer_target_on_fold_day).copied(),
            price("2026-10-24T00:15:00Z")
        );
        assert_eq!(
            got.values.get(&second).copied(),
            price("2026-10-24T00:15:00Z")
        );
    }

    // The pipeline

    fn rec(at: i64, value: f64) -> GridRecord {
        GridRecord {
            start: Timestamp::from_millisecond(at).unwrap(),
            value,
        }
    }

    struct Month {
        spot: QuarterMap,
        wind: Vec<GridRecord>,
        consumption: Vec<GridRecord>,
        series_start_ms: i64,
    }

    /// Thirty days of history with a daily and a weekly price pattern, then
    /// three days of grid forecasts.
    fn month() -> Month {
        let start = ms("2026-05-01T00:00:00Z");
        let series_start_ms = start + 30 * DAY_MS;
        let mut m = Month {
            spot: QuarterMap::new(),
            wind: Vec::new(),
            consumption: Vec::new(),
            series_start_ms,
        };
        for k in 0..33 * 96 {
            let at = start + k * QUARTER_MS;
            let consumption = 8000.0 + int(k % 96) * 20.0 + int(k / 96 % 7) * 90.0;
            let wind = 2000.0 + int((k * 137) % 1500);
            if at < series_start_ms {
                m.spot.insert(
                    QuarterStart::from_millisecond(at),
                    0.001 * (consumption - wind) + 3.0,
                );
            }
            m.consumption.push(rec(at, consumption));
            m.wind.push(rec(at, wind));
        }
        m
    }

    fn run(m: &Month, opts: &ForecastOptions<'_>) -> ForecastResult {
        let neighbours = NeighborPrices::new();
        let history: Vec<GridRecord> = Vec::new();
        build_forecast(
            &ForecastInput {
                spot_prices: &m.spot,
                neighbor_prices: &neighbours,
                wind_forecast: &m.wind,
                wind_actual: &history,
                consumption_forecast: &m.consumption,
                consumption_actual: &history,
                series_start: Timestamp::from_millisecond(m.series_start_ms).unwrap(),
                series_end: Timestamp::from_millisecond(m.series_start_ms + 3 * DAY_MS).unwrap(),
            },
            opts,
        )
    }

    #[test]
    fn the_default_options_are_the_persistence_profile() {
        let m = month();
        let default = run(&m, &ForecastOptions::default());
        let explicit = run(
            &m,
            &ForecastOptions {
                shape_profile: ShapeProfile::Persistence,
                ..ForecastOptions::default()
            },
        );
        assert_eq!(default, explicit);
        assert!(!default.diagnostics.fit_used_default);
    }

    #[test]
    fn the_served_options_are_the_defaults_with_the_four_week_weekday_mean() {
        let zone = oslo();
        let served = ForecastOptions::served(&zone);
        assert_eq!(SERVED_SHAPE_WEEKS, 4);
        assert_eq!(
            served.shape_profile,
            ShapeProfile::WeekdayMean {
                zone: &zone,
                weeks: 4
            }
        );
        assert_eq!(oslo().iana_name(), Some(DELIVERY_DAY_TIME_ZONE));
        let m = month();
        let explicit = ForecastOptions {
            shape_profile: ShapeProfile::WeekdayMean {
                zone: &zone,
                weeks: 4,
            },
            ..ForecastOptions::default()
        };
        assert_eq!(run(&m, &served), run(&m, &explicit));
    }

    #[test]
    fn a_profile_changes_only_the_shape_of_the_forecast() {
        let m = month();
        let zone = oslo();
        let persistence = run(&m, &ForecastOptions::default());
        let served = run(&m, &ForecastOptions::served(&zone));
        assert_eq!(persistence.diagnostics, served.diagnostics);
        assert_ne!(persistence.series, served.series);
        let level = |r: &ForecastResult| {
            r.series
                .iter()
                .map(|p| (p.start, p.estimated_spot_cents_kwh))
                .collect::<QuarterMap>()
        };
        let (a, b) = (
            utc_day_means(&level(&persistence)),
            utc_day_means(&level(&served)),
        );
        for (day, mean) in a {
            // The output is rounded to 0.001 per quarter.
            assert!((b[&day] - mean).abs() < 1e-3, "day {day}");
        }
        // Thirty days of history hold four weeks for every target.
        assert_eq!(served.diagnostics.shape_fallback_quarters, 0);
    }
}
