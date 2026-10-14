//! Compares the domain core with the golden fixtures that the TypeScript
//! modules wrote (`tests/golden/README.md`).
//! Each float must be equal bit for bit, and each text must be equal.

#[cfg(test)]
mod golden {
    use jiff::Timestamp;
    use jiff::civil::Date;
    use jiff::tz::TimeZone;
    use serde_json::Value;
    use spot_price_domain::areas::Area;
    use spot_price_domain::js::{math_round, number_to_string, round3, string_to_number};
    use spot_price_domain::price::{
        ContractBreakdown, DeliveryPeriod, PriceInterval, TotalPrice, apply_contract_terms,
        build_price_window, calculate_total_price, calculate_total_prices, eur_mwh_to_cents_kwh,
        find_cheapest_window, is_night_hour,
    };
    use spot_price_domain::query::{
        CheapestQueryInput, SettingsUpdateInput, parse_cheapest_query, parse_history_query,
        validate_settings_update,
    };
    use spot_price_domain::settings::{Hour, UserId, UserSettings};
    use spot_price_domain::time::{
        UserTimeZone, add_days, current_and_next_date, format_date_in_time_zone,
        format_date_time_in_time_zone, format_utc_date, local_hour, utc_range_for_local_date,
        utc_range_for_local_date_span,
    };

    fn fixture(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    /// A float token of the fixtures: a shortest round-trip decimal or `-0`.
    fn num(token: &str) -> f64 {
        match token {
            "-0" => -0.0,
            "NaN" => f64::NAN,
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            other => other.parse().unwrap(),
        }
    }

    fn same(got: f64, want: f64, what: &str) {
        assert!(
            got.to_bits() == want.to_bits() || (got.is_nan() && want.is_nan()),
            "{what}: got {got:?} ({:#x}), want {want:?} ({:#x})",
            got.to_bits(),
            want.to_bits()
        );
    }

    fn rows<'a>(value: &'a Value, key: &str) -> Vec<Vec<&'a str>> {
        value[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row.as_str().unwrap().split(' ').collect())
            .collect()
    }

    fn int(token: &str) -> i64 {
        token.parse().unwrap()
    }

    fn instant(ms: i64) -> Timestamp {
        Timestamp::from_millisecond(ms).unwrap()
    }

    fn zone(name: &str) -> TimeZone {
        TimeZone::get(name).unwrap()
    }

    fn date(text: &str) -> Date {
        text.parse().unwrap()
    }

    fn settings(row: &[&str], zone: &str) -> UserSettings {
        UserSettings {
            user_id: UserId::new("golden".to_owned()),
            margin_cents_kwh: num(row[0]),
            transfer_day_cents_kwh: num(row[1]),
            transfer_night_cents_kwh: num(row[2]),
            tax_cents_kwh: num(row[3]),
            vat_percent: num(row[4]),
            night_start_hour: Hour::try_from(int(row[5])).unwrap(),
            night_end_hour: Hour::try_from(int(row[6])).unwrap(),
            timezone: UserTimeZone::parse(zone).unwrap(),
            area: Area::Fi,
        }
    }

    #[test]
    fn round3_and_math_round() {
        let data = fixture(include_str!("golden/core/round3.json"));
        let rows = rows(&data, "rows");
        assert!(rows.len() > 10_000);
        for row in rows {
            let x = num(row[0]);
            same(math_round(x), num(row[1]), &format!("Math.round({x:?})"));
            same(round3(x), num(row[2]), &format!("round3({x:?})"));
            same(
                eur_mwh_to_cents_kwh(x),
                num(row[3]),
                &format!("eurMwhToCentsKwh({x:?})"),
            );
        }
    }

    /// A seeded sweep of `round3` against its definition, with the
    /// floor-based check written in another way.
    #[test]
    fn round3_property_sweep() {
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..200_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let x = f64::from_bits(state);
            if !x.is_finite() {
                continue;
            }
            let y = x * 1000.0;
            let rounded = math_round(y);
            if y.is_finite() {
                let lower = rounded - 0.5;
                assert!(rounded.fract() == 0.0, "{y:?}");
                assert!(lower <= y || y.abs() >= 4_503_599_627_370_496.0, "{y:?}");
                assert!(
                    y < rounded + 0.5 || y.abs() >= 4_503_599_627_370_496.0,
                    "{y:?}"
                );
                if rounded == 0.0 {
                    assert_eq!(
                        rounded.is_sign_negative(),
                        y < 0.0 || y.is_sign_negative(),
                        "{y:?}"
                    );
                }
            }
            same(round3(x), math_round(y) / 1000.0, "round3");
        }
    }

    #[test]
    fn javascript_numbers() {
        let data = fixture(include_str!("golden/core/js_number.json"));
        for case in data["number"].as_array().unwrap() {
            let text = case["text"].as_str().unwrap();
            let want = num(case["value"].as_str().unwrap());
            same(string_to_number(text), want, &format!("Number({text:?})"));
        }
        for case in data["toString"].as_array().unwrap() {
            let row = case.as_str().unwrap();
            let (x, want) = row.split_once(' ').unwrap();
            assert_eq!(number_to_string(num(x)), want, "String({x})");
        }
    }

    #[test]
    fn contract_terms() {
        let data = fixture(include_str!("golden/core/contract_terms.json"));
        let all_settings: Vec<UserSettings> = rows(&data, "settings")
            .iter()
            .map(|row| settings(row, "Europe/Helsinki"))
            .collect();
        let cases = rows(&data, "rows");
        assert!(cases.len() > 3_000);
        for row in cases {
            let s = &all_settings[usize::try_from(int(row[0])).unwrap()];
            let spot = num(row[1]);
            let hour = u8::try_from(int(row[2])).unwrap();
            let b = apply_contract_terms(spot, s, hour);
            let what = format!("{row:?}");
            same(b.spot_cents_kwh, num(row[3]), &what);
            same(b.transfer_cents_kwh, num(row[4]), &what);
            same(b.vat_cents_kwh, num(row[5]), &what);
            same(b.total_cents_kwh, num(row[6]), &what);
            assert_eq!(b.is_night_rate.to_string(), row[7], "{what}");
        }
        for row in rows(&data, "night") {
            let (start, end) = (
                u8::try_from(int(row[0])).unwrap(),
                u8::try_from(int(row[1])).unwrap(),
            );
            let flags: String = (0..24)
                .map(|hour| {
                    if is_night_hour(hour, start, end) {
                        '1'
                    } else {
                        '0'
                    }
                })
                .collect();
            assert_eq!(flags, row[2], "night window {start}..{end}");
        }
    }

    #[test]
    fn local_day_ranges() {
        let data = fixture(include_str!("golden/core/local_days.json"));
        let ranges = rows(&data, "ranges");
        assert!(ranges.len() > 5_000);
        for row in ranges {
            let range = utc_range_for_local_date(date(row[1]), &zone(row[0])).unwrap();
            assert_eq!(range.start.as_millisecond(), int(row[2]), "{row:?} start");
            assert_eq!(range.end.as_millisecond(), int(row[3]), "{row:?} end");
        }
        for row in rows(&data, "spans") {
            let range =
                utc_range_for_local_date_span(date(row[1]), date(row[2]), &zone(row[0])).unwrap();
            assert_eq!(range.start.as_millisecond(), int(row[3]), "{row:?} start");
            assert_eq!(range.end.as_millisecond(), int(row[4]), "{row:?} end");
        }
    }

    #[test]
    fn local_text_and_dates() {
        let data = fixture(include_str!("golden/core/local_format.json"));
        let cases = rows(&data, "rows");
        assert!(cases.len() > 1_000);
        for row in cases {
            let tz = zone(row[0]);
            let at = instant(int(row[1]));
            assert_eq!(format_date_time_in_time_zone(at, &tz), row[2], "{row:?}");
            assert_eq!(i64::from(local_hour(at, &tz)), int(row[3]), "{row:?}");
            assert_eq!(format_date_in_time_zone(at, &tz), row[4], "{row:?}");
        }
        for row in rows(&data, "utc") {
            let at = instant(int(row[0]));
            assert_eq!(format_utc_date(at), row[1], "{row:?}");
            assert_eq!(
                add_days(at, 1).unwrap().as_millisecond(),
                int(row[2]),
                "{row:?}"
            );
            assert_eq!(
                add_days(at, -1).unwrap().as_millisecond(),
                int(row[3]),
                "{row:?}"
            );
            assert_eq!(
                add_days(at, 31).unwrap().as_millisecond(),
                int(row[4]),
                "{row:?}"
            );
        }
        for row in rows(&data, "today") {
            let dates = current_and_next_date(instant(int(row[1])), &zone(row[0])).unwrap();
            assert_eq!(dates.today.to_string(), row[2], "{row:?}");
            assert_eq!(dates.tomorrow.to_string(), row[3], "{row:?}");
        }
    }

    fn interval(start_ms: i64, minutes: i64, eur_mwh: f64) -> PriceInterval {
        PriceInterval {
            period: DeliveryPeriod::new(instant(start_ms), instant(start_ms + minutes * 60_000))
                .unwrap(),
            price_eur_mwh: eur_mwh,
            area: Area::Fi,
        }
    }

    #[test]
    fn total_prices_and_windows() {
        let data = fixture(include_str!("golden/core/total_prices.json"));
        let settings_rows = rows(&data, "settings");
        let cases = rows(&data, "rows");
        assert!(cases.len() > 1_000);
        for row in cases {
            let s = settings(
                &settings_rows[usize::try_from(int(row[1])).unwrap()],
                row[0],
            );
            let (start, end) = (int(row[2]), int(row[3]));
            let price = interval(start, (end - start) / 60_000, num(row[4]));
            let total = calculate_total_price(&price, &s);
            let what = format!("{row:?}");
            assert_eq!(total.local_start, row[5], "{what}");
            assert_eq!(total.local_end, row[6], "{what}");
            same(total.breakdown.spot_cents_kwh, num(row[7]), &what);
            same(total.breakdown.transfer_cents_kwh, num(row[8]), &what);
            same(total.breakdown.vat_cents_kwh, num(row[9]), &what);
            same(total.breakdown.total_cents_kwh, num(row[10]), &what);
            assert_eq!(total.breakdown.is_night_rate.to_string(), row[11], "{what}");
        }
        let windows = rows(&data, "windows");
        assert!(windows.len() > 100);
        for row in windows {
            let s = settings(
                &settings_rows[usize::try_from(int(row[1])).unwrap()],
                row[0],
            );
            let first = row[3].parse::<Timestamp>().unwrap().as_millisecond();
            let minutes = int(row[5]);
            let prices: Vec<PriceInterval> = row[13]
                .split(',')
                .zip(0_i64..)
                .map(|(price, index)| {
                    interval(first + index * minutes * 60_000, minutes, num(price))
                })
                .collect();
            assert_eq!(i64::try_from(prices.len()).unwrap(), int(row[4]));
            let window = build_price_window(calculate_total_prices(&prices, &s)).unwrap();
            let what = format!("{:?}", &row[..6]);
            assert_eq!(window.start().as_millisecond(), int(row[6]), "{what}");
            assert_eq!(window.end().as_millisecond(), int(row[7]), "{what}");
            assert_eq!(window.start_local(), row[8], "{what}");
            assert_eq!(window.end_local(), row[9], "{what}");
            same(window.min_total_cents_kwh(), num(row[10]), &what);
            same(window.max_total_cents_kwh(), num(row[11]), &what);
            same(window.average_total_cents_kwh(), num(row[12]), &what);
        }
        let single: Vec<&str> = data["single"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let s = settings(&settings_rows[1], "Europe/Helsinki");
        let total = calculate_total_price(&interval(1_771_927_200_000, 60, 50.0), &s);
        assert_eq!(
            [total.local_start.as_str(), total.local_end.as_str()],
            [single[0], single[1]]
        );
        same(total.breakdown.total_cents_kwh, num(single[2]), "single");
    }

    fn fake_total(start_ms: i64, minutes: i64, total: f64) -> TotalPrice {
        TotalPrice {
            period: DeliveryPeriod::new(instant(start_ms), instant(start_ms + minutes * 60_000))
                .unwrap(),
            local_start: String::new(),
            local_end: String::new(),
            breakdown: ContractBreakdown {
                spot_cents_kwh: total,
                margin_cents_kwh: 0.0,
                transfer_cents_kwh: 0.0,
                tax_cents_kwh: 0.0,
                vat_cents_kwh: 0.0,
                total_cents_kwh: total,
                is_night_rate: false,
            },
        }
    }

    #[test]
    fn cheapest_windows() {
        let data = fixture(include_str!("golden/core/cheapest.json"));
        let cases = data["cases"].as_array().unwrap();
        assert!(cases.len() > 200);
        let mut found = 0;
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let entries: Vec<TotalPrice> = case["entries"]
                .as_str()
                .unwrap()
                .split(';')
                .filter(|entry| !entry.is_empty())
                .map(|entry| {
                    let cells: Vec<&str> = entry.split(',').collect();
                    fake_total(int(cells[0]), int(cells[1]), num(cells[2]))
                })
                .collect();
            let duration = u32::try_from(case["duration"].as_i64().unwrap()).unwrap();
            let window = find_cheapest_window(&entries, duration);
            let want = &case["result"];
            match window {
                None => assert!(want.is_null(), "{name}: no window, want {want}"),
                Some(window) => {
                    found += 1;
                    assert!(!want.is_null(), "{name}: unexpected window");
                    let first = entries
                        .iter()
                        .position(|entry| entry.period.start() == window.start())
                        .unwrap();
                    assert_eq!(
                        i64::try_from(first).unwrap(),
                        want["first"].as_i64().unwrap(),
                        "{name}"
                    );
                    assert_eq!(
                        i64::try_from(window.prices().len()).unwrap(),
                        want["count"].as_i64().unwrap(),
                        "{name}"
                    );
                    assert_eq!(
                        window.start().as_millisecond(),
                        want["start"].as_i64().unwrap(),
                        "{name}"
                    );
                    assert_eq!(
                        window.end().as_millisecond(),
                        want["end"].as_i64().unwrap(),
                        "{name}"
                    );
                    same(
                        window.min_total_cents_kwh(),
                        num(want["min"].as_str().unwrap()),
                        name,
                    );
                    same(
                        window.max_total_cents_kwh(),
                        num(want["max"].as_str().unwrap()),
                        name,
                    );
                    same(
                        window.average_total_cents_kwh(),
                        num(want["avg"].as_str().unwrap()),
                        name,
                    );
                }
            }
        }
        assert!(found > 100, "too few cases found a window: {found}");
    }

    fn text<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
        input.get(key).map(|value| value.as_str().unwrap())
    }

    #[test]
    fn cheapest_query() {
        let data = fixture(include_str!("golden/core/queries.json"));
        for case in data["cheapest"].as_array().unwrap() {
            let input = &case["input"];
            let parsed = parse_cheapest_query(CheapestQueryInput {
                duration: text(input, "duration"),
                start_time: text(input, "startTime"),
                end_time: text(input, "endTime"),
                max_price: text(input, "maxPrice"),
            });
            match (&parsed, case.get("error")) {
                (Err(error), Some(want)) => {
                    assert_eq!(error.to_string(), want.as_str().unwrap(), "{input}");
                }
                (Ok(query), None) => {
                    let ok = &case["ok"];
                    assert_eq!(
                        i64::from(query.duration_minutes),
                        ok["duration"].as_i64().unwrap(),
                        "{input}"
                    );
                    for (got, key) in [(query.start_time, "startTime"), (query.end_time, "endTime")]
                    {
                        // An instant after the Jiff range saturates; prices are
                        // far before it, so the filter result is the same.
                        let want = ok[key]
                            .as_i64()
                            .map(|ms| ms.min(Timestamp::MAX.as_millisecond()));
                        assert_eq!(got.map(Timestamp::as_millisecond), want, "{input} {key}");
                    }
                    match (query.max_price, ok["maxPrice"].as_str()) {
                        (Some(got), Some(want)) => same(got, num(want), &input.to_string()),
                        (got, want) => assert_eq!(got.is_some(), want.is_some(), "{input}"),
                    }
                }
                (got, want) => panic!("{input}: got {got:?}, want {want:?}"),
            }
        }
    }

    #[test]
    fn history_query() {
        let data = fixture(include_str!("golden/core/queries.json"));
        for case in data["history"].as_array().unwrap() {
            let input = &case["input"];
            let parsed = parse_history_query(text(input, "from"), text(input, "to"));
            match (&parsed, case.get("error")) {
                (Err(error), Some(want)) => {
                    assert_eq!(error.to_string(), want.as_str().unwrap(), "{input}");
                }
                (Ok(query), None) => {
                    assert_eq!(
                        query.from.to_string(),
                        case["ok"]["from"].as_str().unwrap(),
                        "{input}"
                    );
                    assert_eq!(
                        query.to.to_string(),
                        case["ok"]["to"].as_str().unwrap(),
                        "{input}"
                    );
                }
                (got, want) => panic!("{input}: got {got:?}, want {want:?}"),
            }
        }
    }

    #[test]
    fn settings_update() {
        let data = fixture(include_str!("golden/core/queries.json"));
        for case in data["settingsUpdate"].as_array().unwrap() {
            let mut input = SettingsUpdateInput::default();
            let value = case.get("value").and_then(Value::as_str).map(num);
            let label = case.get("text").and_then(Value::as_str);
            match case["field"].as_str().unwrap() {
                "marginCentsKwh" => input.margin_cents_kwh = value,
                "transferDayCentsKwh" => input.transfer_day_cents_kwh = value,
                "transferNightCentsKwh" => input.transfer_night_cents_kwh = value,
                "taxCentsKwh" => input.tax_cents_kwh = value,
                "vatPercent" => input.vat_percent = value,
                "nightStartHour" => input.night_start_hour = value,
                "nightEndHour" => input.night_end_hour = value,
                "timezone" => input.timezone = label,
                "area" => input.area = label,
                "*" => {
                    input = SettingsUpdateInput {
                        margin_cents_kwh: Some(-1.0),
                        vat_percent: Some(101.0),
                        night_start_hour: Some(1.5),
                        night_end_hour: Some(24.0),
                        timezone: Some("UTC"),
                        area: Some("XX"),
                        ..SettingsUpdateInput::default()
                    };
                }
                other => panic!("unknown field {other}"),
            }
            let got = validate_settings_update(input)
                .err()
                .map(|error| error.to_string());
            let want = case["error"].as_str().map(str::to_owned);
            assert_eq!(got, want, "{case}");
        }
    }
}
