//! Request input as the TypeScript service read it: Hono query parameters,
//! the JSON body rules of `@hono/zod-openapi`, and the zod issue texts that
//! the domain `query` module does not produce.

use std::cmp::Ordering;

use spot_price_domain::areas::Area;
use spot_price_domain::query::{
    CheapestQuery, CheapestQueryInput, Field, HistoryQuery, Issue, IssueKind, SettingsUpdateInput,
    ValidationError, parse_cheapest_query, parse_history_query, validate_settings_update,
};
use spot_price_domain::settings::SettingsUpdate;

/// The value of one query parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Param<'a> {
    Absent,
    One(&'a str),
    /// The name came more than once; zod sees an array.
    Many,
}

/// The decoded query parameters of a request, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pairs: Vec<(String, String)>,
}

impl Query {
    /// Parses the raw query like Hono `c.req.queries()`: pairs split at `&`,
    /// the name ends at the first `=`, a pair without `=` has the value `""`,
    /// and an empty name is skipped. See [`decode_component`].
    #[must_use]
    pub fn parse(raw: Option<&str>) -> Self {
        let pairs = raw
            .unwrap_or_default()
            .split('&')
            .filter_map(|pair| {
                let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                let name = decode_component(name);
                (!name.is_empty()).then(|| (name, decode_component(value)))
            })
            .collect();
        Self { pairs }
    }

    /// The value of the parameter `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Param<'_> {
        let mut values = self
            .pairs
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.as_str());
        match (values.next(), values.next()) {
            (None, _) => Param::Absent,
            (Some(value), None) => Param::One(value),
            (Some(_), Some(_)) => Param::Many,
        }
    }
}

/// Hono's decoding of one query component: `+` becomes a space, then the
/// text is percent-decoded when it has a `%`. A malformed escape or invalid
/// UTF-8 keeps the text as it is after the `+` step.
#[must_use]
pub fn decode_component(raw: &str) -> String {
    let spaced = raw.replace('+', " ");
    if !spaced.contains('%') {
        return spaced;
    }
    percent_decode(&spaced).unwrap_or(spaced)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let high = char::from(*bytes.get(index + 1)?).to_digit(16)?;
            let low = char::from(*bytes.get(index + 2)?).to_digit(16)?;
            out.push(u8::try_from(high * 16 + low).ok()?);
            index += 3;
        } else {
            out.push(byte);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn issue(field: Field, kind: IssueKind) -> Issue {
    Issue { field, kind }
}

/// The text of one zod issue that the domain cannot express.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Extra {
    field: Field,
    text: String,
}

impl Extra {
    fn new(field: Field, received: &str) -> Self {
        Self {
            field,
            text: format!("{}: Invalid input: expected {received}", field.name()),
        }
    }
}

/// Joins domain issues and extra issues in the field order of `order`, like
/// zod, which reports each field in schema order.
fn join_issues(order: &[Field], domain: &[Issue], extra: &[Extra]) -> String {
    let rank = |field: Field| {
        order
            .iter()
            .position(|f| *f == field)
            .unwrap_or(order.len())
    };
    let mut texts: Vec<(usize, usize, String)> = domain
        .iter()
        .enumerate()
        .map(|(seq, issue)| (rank(issue.field), seq, issue.to_string()))
        .chain(
            extra
                .iter()
                .enumerate()
                .map(|(seq, extra)| (rank(extra.field), seq, extra.text.clone())),
        )
        .collect();
    texts.sort_by(|a, b| match a.0.cmp(&b.0) {
        Ordering::Equal => a.1.cmp(&b.1),
        other => other,
    });
    texts
        .into_iter()
        .map(|(_, _, text)| text)
        .collect::<Vec<_>>()
        .join("; ")
}

/// The `area` query parameter of `/api/public/spot` and the forecast:
/// absent is `FI`; any other value must be an area code.
///
/// # Errors
///
/// Returns the zod issue text when the value is not an area code.
pub fn area_query(query: &Query) -> Result<Area, String> {
    match query.get("area") {
        Param::Absent => Ok(Area::Fi),
        Param::One(code) => code
            .parse()
            .map_err(|_| issue(Field::Area, IssueKind::UnknownArea).to_string()),
        Param::Many => Err(issue(Field::Area, IssueKind::UnknownArea).to_string()),
    }
}

/// The query of `/api/v1/price/cheapest`.
///
/// # Errors
///
/// Returns the joined zod issue text of every problem.
pub fn cheapest_query(query: &Query) -> Result<CheapestQuery, String> {
    const ORDER: [Field; 4] = [
        Field::Duration,
        Field::StartTime,
        Field::EndTime,
        Field::MaxPrice,
    ];
    let mut extra = Vec::new();
    // `Number(array)` joins the items with commas, which is never a number.
    let duration = match query.get("duration") {
        Param::Absent => None,
        Param::One(text) => Some(text),
        Param::Many => Some(","),
    };
    let mut text = |field: Field| match query.get(field.name()) {
        Param::Absent => None,
        Param::One(text) => Some(text),
        Param::Many => {
            extra.push(Extra::new(field, "string, received array"));
            None
        }
    };
    let input = CheapestQueryInput {
        duration,
        start_time: text(Field::StartTime),
        end_time: text(Field::EndTime),
        max_price: text(Field::MaxPrice),
    };
    match (parse_cheapest_query(input), extra.is_empty()) {
        (Ok(parsed), true) => Ok(parsed),
        (Ok(_), false) => Err(join_issues(&ORDER, &[], &extra)),
        (Err(error), _) => Err(join_issues(&ORDER, &error.issues, &extra)),
    }
}

/// The query of `/api/v1/price/history`.
///
/// # Errors
///
/// Returns the joined zod issue text of every problem.
pub fn history_query(query: &Query) -> Result<HistoryQuery, String> {
    const ORDER: [Field; 2] = [Field::From, Field::To];
    let from = query.get("from");
    let to = query.get("to");
    if from != Param::Many && to != Param::Many {
        let text = |param| match param {
            Param::One(text) => Some(text),
            Param::Absent | Param::Many => None,
        };
        return parse_history_query(text(from), text(to)).map_err(|error| error.to_string());
    }
    // An array is a type error, which stops the order and span checks.
    let mut domain = Vec::new();
    let mut extra = Vec::new();
    for (field, param) in [(Field::From, from), (Field::To, to)] {
        match param {
            Param::Many => extra.push(Extra::new(field, "string, received array")),
            Param::Absent => domain.push(issue(field, IssueKind::Missing)),
            Param::One(text) => {
                let invalid = parse_history_query(Some(text), Some(text)).is_err_and(|error| {
                    error
                        .issues
                        .iter()
                        .any(|found| found.kind == IssueKind::InvalidDate)
                });
                if invalid {
                    domain.push(issue(field, IssueKind::InvalidDate));
                }
            }
        }
    }
    Err(join_issues(&ORDER, &domain, &extra))
}

/// The string that stands for an infinite JSON number after [`parse_json`].
const POSITIVE_INFINITY: &str = "\u{0}+Infinity";
const NEGATIVE_INFINITY: &str = "\u{0}-Infinity";

/// Parses a body like `JSON.parse`. A number outside the double range, such
/// as `1e400`, is infinite in JavaScript; `serde_json` rejects it, so such a
/// number becomes a marker string that [`js_number`] reads as infinite.
#[must_use]
pub fn parse_json(text: &str) -> Option<serde_json::Value> {
    match serde_json::from_str(text) {
        Ok(value) => Some(value),
        Err(error) if error.to_string().starts_with("number out of range") => {
            serde_json::from_str(&mark_infinite_numbers(text)).ok()
        }
        Err(_) => None,
    }
}

/// Replaces each number token outside a string that is infinite as a double
/// with the JSON text of a marker string.
fn mark_infinite_numbers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if in_string {
            out.push(c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }
        if c != '-' && !c.is_ascii_digit() {
            out.push(c);
            continue;
        }
        let mut end = start + c.len_utf8();
        while let Some(&(index, next)) = chars.peek() {
            if !matches!(next, '0'..='9' | '-' | '+' | '.' | 'e' | 'E') {
                break;
            }
            end = index + next.len_utf8();
            chars.next();
        }
        let token = &text[start..end];
        match token.parse::<f64>() {
            Ok(value) if value.is_infinite() => {
                let marker = if value > 0.0 {
                    POSITIVE_INFINITY
                } else {
                    NEGATIVE_INFINITY
                };
                out.push_str(&serde_json::Value::from(marker).to_string());
            }
            _ => out.push_str(token),
        }
    }
    out
}

/// Whether a parsed value is an infinite number of [`parse_json`].
#[must_use]
pub fn is_infinite_number(value: &serde_json::Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| text == POSITIVE_INFINITY || text == NEGATIVE_INFINITY)
}

/// The JavaScript number of a parsed JSON number.
fn js_number(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) if text == POSITIVE_INFINITY => Some(f64::INFINITY),
        serde_json::Value::String(text) if text == NEGATIVE_INFINITY => Some(f64::NEG_INFINITY),
        _ => None,
    }
}

/// The outcome of reading the body of `PUT /api/v1/me/settings`.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsBody {
    Valid(SettingsUpdate),
    /// The zod issue text, answered as `{"error": text}` with 400.
    Invalid(String),
    /// The body is not JSON although the content type says so.
    Malformed,
}

/// `@hono/zod-openapi` validates a JSON body only when the content type
/// starts with `application/json` or `application/<x>+json`, case-sensitive.
fn openapi_json_type(content_type: &str) -> bool {
    let Some(rest) = content_type.strip_prefix("application/") else {
        return false;
    };
    if rest.starts_with("json") {
        return true;
    }
    let suffix = rest.trim_start_matches(|c: char| c.is_ascii_lowercase() || c == '-' || c == '.');
    suffix.len() < rest.len() && suffix.starts_with("+json")
}

/// Hono reads the JSON body only when the whole content type matches
/// `application/[x+]json` and `;name=value` parameters, case-insensitive.
fn hono_json_type(content_type: &str) -> bool {
    let lower = content_type.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("application/") else {
        return false;
    };
    let rest = if let Some(after) = rest.strip_prefix("json") {
        after
    } else {
        let suffix =
            rest.trim_start_matches(|c: char| c.is_ascii_lowercase() || c == '-' || c == '.');
        if suffix.len() == rest.len() {
            return false;
        }
        let Some(after) = suffix.strip_prefix("+json") else {
            return false;
        };
        after
    };
    hono_parameters(rest)
}

/// Matches `(;\s*[a-zA-Z0-9-]+=[^;]+)*` to the end of the text.
fn hono_parameters(mut rest: &str) -> bool {
    while !rest.is_empty() {
        let Some(after) = rest.strip_prefix(';') else {
            return false;
        };
        let after = after.trim_start_matches(char::is_whitespace);
        let name_end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(after.len());
        if name_end == 0 {
            return false;
        }
        let Some(value) = after[name_end..].strip_prefix('=') else {
            return false;
        };
        let value_end = value.find(';').unwrap_or(value.len());
        if value_end == 0 {
            return false;
        }
        rest = &value[value_end..];
    }
    true
}

/// Reads the body of `PUT /api/v1/me/settings`. Without a JSON content type
/// the body is ignored and the update is empty.
#[must_use]
pub fn settings_body(content_type: Option<&str>, bytes: &[u8]) -> SettingsBody {
    let Some(content_type) = content_type else {
        return SettingsBody::Valid(SettingsUpdate::default());
    };
    if !openapi_json_type(content_type) || !hono_json_type(content_type) {
        return SettingsBody::Valid(SettingsUpdate::default());
    }
    // `Request.json()` decodes UTF-8 with replacement characters and drops a
    // byte order mark.
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let text = String::from_utf8_lossy(bytes);
    match parse_json(&text) {
        Some(value) => match settings_update(&value) {
            Ok(update) => SettingsBody::Valid(update),
            Err(text) => SettingsBody::Invalid(text),
        },
        None => SettingsBody::Malformed,
    }
}

/// The zod type name of a JSON value.
fn type_name(value: &serde_json::Value) -> &'static str {
    if is_infinite_number(value) {
        return "number";
    }
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Validates a parsed settings body like `UserSettingsUpdateSchema`.
/// Unknown keys are ignored.
///
/// # Errors
///
/// Returns the joined zod issue text of every problem.
pub fn settings_update(value: &serde_json::Value) -> Result<SettingsUpdate, String> {
    const ORDER: [Field; 9] = [
        Field::MarginCentsKwh,
        Field::TransferDayCentsKwh,
        Field::TransferNightCentsKwh,
        Field::TaxCentsKwh,
        Field::VatPercent,
        Field::NightStartHour,
        Field::NightEndHour,
        Field::Timezone,
        Field::Area,
    ];
    let serde_json::Value::Object(object) = value else {
        return Err(format!(
            "Invalid input: expected object, received {}",
            type_name(value)
        ));
    };
    let mut extra = Vec::new();
    let mut domain = Vec::new();
    let mut number = |field: Field| match object.get(field.name()) {
        None => None,
        Some(value) if js_number(value).is_some() => js_number(value),
        Some(other) => {
            extra.push(Extra::new(
                field,
                &format!("number, received {}", type_name(other)),
            ));
            None
        }
    };
    let margin_cents_kwh = number(Field::MarginCentsKwh);
    let transfer_day_cents_kwh = number(Field::TransferDayCentsKwh);
    let transfer_night_cents_kwh = number(Field::TransferNightCentsKwh);
    let tax_cents_kwh = number(Field::TaxCentsKwh);
    let vat_percent = number(Field::VatPercent);
    let night_start_hour = number(Field::NightStartHour);
    let night_end_hour = number(Field::NightEndHour);
    let mut option = |field: Field, kind: IssueKind| match object.get(field.name()) {
        None => None,
        Some(serde_json::Value::String(text)) => Some(text.as_str()),
        Some(_) => {
            domain.push(issue(field, kind));
            None
        }
    };
    let timezone = option(Field::Timezone, IssueKind::UnknownTimezone);
    let area = option(Field::Area, IssueKind::UnknownArea);
    let input = SettingsUpdateInput {
        margin_cents_kwh,
        transfer_day_cents_kwh,
        transfer_night_cents_kwh,
        tax_cents_kwh,
        vat_percent,
        night_start_hour,
        night_end_hour,
        timezone,
        area,
    };
    match validate_settings_update(input) {
        Ok(update) if extra.is_empty() && domain.is_empty() => Ok(update),
        Ok(_) => Err(join_issues(&ORDER, &domain, &extra)),
        Err(ValidationError { issues }) => {
            domain.extend(issues);
            Err(join_issues(&ORDER, &domain, &extra))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_queries_like_hono() {
        let query = Query::parse(Some("a=1&b&=x&c=%41+b&d=%ZZ+1&a=2&e=%C3%A4&f=%FF&g=x=y"));
        assert_eq!(query.get("a"), Param::Many);
        assert_eq!(query.get("b"), Param::One(""));
        assert_eq!(query.get("c"), Param::One("A b"));
        assert_eq!(query.get("d"), Param::One("%ZZ 1"));
        assert_eq!(query.get("e"), Param::One("ä"));
        assert_eq!(query.get("f"), Param::One("%FF"));
        assert_eq!(query.get("g"), Param::One("x=y"));
        assert_eq!(query.get("zz"), Param::Absent);
        assert_eq!(Query::parse(None).get("a"), Param::Absent);
    }

    #[test]
    fn area_defaults_to_fi_and_rejects_empty_and_arrays() {
        assert_eq!(area_query(&Query::parse(None)), Ok(Area::Fi));
        assert_eq!(area_query(&Query::parse(Some("area=SE3"))), Ok(Area::Se3));
        assert!(area_query(&Query::parse(Some("area="))).is_err());
        let error = area_query(&Query::parse(Some("area=FI&area=FI"))).unwrap_err();
        assert!(error.starts_with("area: Invalid option: expected one of \"FI\"|\"SE1\""));
    }

    #[test]
    fn cheapest_reports_arrays_in_field_order() {
        let error = cheapest_query(&Query::parse(Some(
            "duration=1&duration=2&startTime=a&startTime=b&maxPrice=1&maxPrice=2&endTime=x",
        )))
        .unwrap_err();
        assert_eq!(
            error,
            "duration: Invalid input: expected number, received NaN; startTime: Invalid input: expected string, received array; endTime: Invalid ISO datetime; maxPrice: Invalid input: expected string, received array"
        );
    }

    #[test]
    fn history_reports_an_array_without_the_order_checks() {
        assert_eq!(
            history_query(&Query::parse(Some("from=2026-01-01&from=x&to=2027-01-02"))).unwrap_err(),
            "from: Invalid input: expected string, received array"
        );
        assert_eq!(
            history_query(&Query::parse(Some("from=a&from=b&to=bad"))).unwrap_err(),
            "from: Invalid input: expected string, received array; to: Invalid ISO date"
        );
        assert_eq!(
            history_query(&Query::parse(Some("to=2026-01-01&to=2026-01-02"))).unwrap_err(),
            "from: Invalid input: expected string, received undefined; to: Invalid input: expected string, received array"
        );
    }

    #[test]
    fn settings_body_follows_the_content_type_rules() {
        let empty = SettingsBody::Valid(SettingsUpdate::default());
        assert_eq!(settings_body(None, b"{bad"), empty);
        assert_eq!(settings_body(Some("text/plain"), b"{bad"), empty);
        assert_eq!(settings_body(Some("Application/JSON"), b"{bad"), empty);
        assert_eq!(settings_body(Some("application/jsonx"), b"{bad"), empty);
        assert_eq!(
            settings_body(Some("application/json"), b"{bad"),
            SettingsBody::Malformed
        );
        assert_eq!(
            settings_body(Some("application/json"), b""),
            SettingsBody::Malformed
        );
        assert_eq!(
            settings_body(Some("application/json; charset=utf-8"), b"{bad"),
            SettingsBody::Malformed
        );
        assert_eq!(
            settings_body(Some("application/vnd.api+json"), b"{\"vatPercent\":10}"),
            SettingsBody::Valid(SettingsUpdate {
                vat_percent: Some(10.0),
                ..SettingsUpdate::default()
            })
        );
        assert_eq!(settings_body(Some("application/json;"), b"{bad"), empty);
    }

    #[test]
    fn numbers_outside_the_double_range_are_infinite() {
        assert_eq!(
            settings_body(Some("application/json"), b"1e400"),
            SettingsBody::Invalid("Invalid input: expected object, received number".to_owned())
        );
        assert_eq!(
            settings_body(
                Some("application/json"),
                br#"{"marginCentsKwh":-1e400,"area":"1e999"}"#
            ),
            SettingsBody::Invalid("marginCentsKwh: Invalid input: expected number, received number; area: Invalid option: expected one of \"FI\"|\"SE1\"|\"SE2\"|\"SE3\"|\"SE4\"|\"NO1\"|\"NO2\"|\"NO3\"|\"NO4\"|\"NO5\"|\"DK1\"|\"DK2\"|\"EE\"|\"LT\"|\"LV\"|\"AT\"|\"BE\"|\"FR\"|\"GER\"|\"NL\"|\"PL\"".to_owned())
        );
        assert_eq!(
            parse_json("[1e400, \"a\\\"1e400\"]").unwrap()[1],
            "a\"1e400"
        );
        assert!(parse_json("{bad").is_none());
    }

    #[test]
    fn settings_body_reports_zod_texts() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{"nightStartHour":"1","nightEndHour":[],"taxCentsKwh":{},"transferDayCentsKwh":true,"area":null,"marginCentsKwh":-1}"#,
        )
        .unwrap();
        let error = settings_update(&value).unwrap_err();
        assert_eq!(
            error,
            "marginCentsKwh: Too small: expected number to be >=0; transferDayCentsKwh: Invalid input: expected number, received boolean; taxCentsKwh: Invalid input: expected number, received object; nightStartHour: Invalid input: expected number, received string; nightEndHour: Invalid input: expected number, received array; area: Invalid option: expected one of \"FI\"|\"SE1\"|\"SE2\"|\"SE3\"|\"SE4\"|\"NO1\"|\"NO2\"|\"NO3\"|\"NO4\"|\"NO5\"|\"DK1\"|\"DK2\"|\"EE\"|\"LT\"|\"LV\"|\"AT\"|\"BE\"|\"FR\"|\"GER\"|\"NL\"|\"PL\""
        );
        for (body, expected) in [
            ("[]", "array"),
            ("null", "null"),
            ("\"x\"", "string"),
            ("5", "number"),
            ("true", "boolean"),
        ] {
            let value: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!(
                settings_update(&value).unwrap_err(),
                format!("Invalid input: expected object, received {expected}")
            );
        }
    }
}
