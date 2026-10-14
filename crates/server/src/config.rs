//! Startup configuration from environment variables.
//!
//! The service validates every setting once, before it opens the database
//! or the port. The variable names are the names of the TypeScript service,
//! so the Railway configuration stays the same. An error names the setting
//! and never prints its value.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};

use reqwest::Url;

const DEFAULT_PORT: u16 = 3000;
const DEFAULT_LOCAL_AUTH_URL: &str = "http://localhost:3000";
/// Used only outside production; production requires `BETTER_AUTH_SECRET`.
const DEV_FALLBACK_SECRET: &str = "dev-only-better-auth-secret-must-be-32+";

/// A secret value. `Debug` never shows it.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret([redacted])")
    }
}

/// The runtime mode from `NODE_ENV`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Development,
    Test,
    Production,
}

/// The log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// One JSON object per line (production).
    Json,
    /// Readable text (local development).
    Text,
}

/// The validated service configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub mode: RunMode,
    pub bind: SocketAddr,
    pub database_url: Secret,
    /// `BETTER_AUTH_URL`, or the local default outside production.
    pub auth_base_url: Url,
    /// `BETTER_AUTH_SECRET`, or the development fallback outside production.
    pub auth_secret: Secret,
    /// The Fingrid job runs only when this key is set.
    pub fingrid_api_key: Option<Secret>,
    /// The weather job runs only when this key is set.
    pub openweathermap_api_key: Option<Secret>,
    pub log_format: LogFormat,
}

/// One or more settings are missing or invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// The names of the invalid settings, in a fixed order.
    pub invalid: Vec<&'static str>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid environment variables: {}",
            self.invalid.join(", ")
        )
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Reads the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] with the names of all invalid settings.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| {
            std::env::var_os(name).map(|value| value.into_string().map_err(|_| ()))
        })
    }

    /// Reads the configuration through `lookup`. A value that is not valid
    /// Unicode is `Some(Err(()))`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] with the names of all invalid settings.
    pub fn from_lookup(
        lookup: impl Fn(&str) -> Option<Result<String, ()>>,
    ) -> Result<Self, ConfigError> {
        let mut reader = Reader {
            lookup,
            invalid: Vec::new(),
        };
        let mode = reader.read("NODE_ENV", |v| match v.as_deref() {
            None | Some("development") => Some(RunMode::Development),
            Some("test") => Some(RunMode::Test),
            Some("production") => Some(RunMode::Production),
            Some(_) => None,
        });
        let production = mode == Some(RunMode::Production);
        let port = reader.read("PORT", parse_port);
        // The server cannot serve or migrate without a database, so this
        // setting is required in every mode.
        let database_url = reader.read("DATABASE_URL", |v| required(v).map(Secret::new));
        let auth_base_url = reader.read("BETTER_AUTH_URL", |v| match v {
            None if production => None,
            None => Url::parse(DEFAULT_LOCAL_AUTH_URL).ok(),
            Some(text) if production && text.trim().is_empty() => None,
            Some(text) => Url::parse(&text).ok(),
        });
        let auth_secret = reader.read("BETTER_AUTH_SECRET", |v| match v {
            None if production => None,
            None => Some(Secret::new(DEV_FALLBACK_SECRET.to_owned())),
            Some(text) if production && text.trim().is_empty() => None,
            Some(text) => non_empty(text).map(Secret::new),
        });
        let fingrid_api_key = reader.read("FINGRID_API_KEY", optional_secret);
        let openweathermap_api_key = reader.read("OPENWEATHERMAP_API_KEY", optional_secret);
        let log_format = reader.read("LOG_FORMAT", |v| match v.as_deref() {
            None | Some("json") => Some(LogFormat::Json),
            Some("text") => Some(LogFormat::Text),
            Some(_) => None,
        });

        let (
            Some(mode),
            Some(port),
            Some(database_url),
            Some(auth_base_url),
            Some(auth_secret),
            Some(fingrid_api_key),
            Some(openweathermap_api_key),
            Some(log_format),
        ) = (
            mode,
            port,
            database_url,
            auth_base_url,
            auth_secret,
            fingrid_api_key,
            openweathermap_api_key,
            log_format,
        )
        else {
            return Err(ConfigError {
                invalid: reader.invalid,
            });
        };

        Ok(Self {
            mode,
            bind: SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
            database_url,
            auth_base_url,
            auth_secret,
            fingrid_api_key,
            openweathermap_api_key,
            log_format,
        })
    }
}

/// Reads settings and collects the names of the invalid ones in read order.
struct Reader<L> {
    lookup: L,
    invalid: Vec<&'static str>,
}

impl<L: Fn(&str) -> Option<Result<String, ()>>> Reader<L> {
    /// Reads one setting. `parse` gets `None` when the variable is not set
    /// and returns `None` when the value is invalid.
    fn read<T>(
        &mut self,
        name: &'static str,
        parse: impl FnOnce(Option<String>) -> Option<T>,
    ) -> Option<T> {
        let parsed = match (self.lookup)(name) {
            None => parse(None),
            Some(Ok(value)) => parse(Some(value)),
            Some(Err(())) => None,
        };
        if parsed.is_none() {
            self.invalid.push(name);
        }
        parsed
    }
}

/// A required value that is not empty and not only whitespace.
fn required(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

/// A present value must not be empty.
fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

/// An optional secret: absent is `Some(None)`, empty is invalid.
#[expect(
    clippy::option_option,
    reason = "the outer None marks an invalid value for Reader::read"
)]
fn optional_secret(value: Option<String>) -> Option<Option<Secret>> {
    match value {
        None => Some(None),
        Some(text) => non_empty(text).map(|text| Some(Secret::new(text))),
    }
}

/// A decimal port in `1..=65535`. Surrounding whitespace is allowed.
fn parse_port(value: Option<String>) -> Option<u16> {
    let Some(text) = value else {
        return Some(DEFAULT_PORT);
    };
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u16>().ok().filter(|port| *port >= 1)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn valid() -> HashMap<&'static str, &'static str> {
        HashMap::from([(
            "DATABASE_URL",
            "postgres://user:secret-password@db.internal:5432/app",
        )])
    }

    fn production() -> HashMap<&'static str, &'static str> {
        let mut vars = valid();
        vars.extend([
            ("NODE_ENV", "production"),
            ("BETTER_AUTH_URL", "https://spot.example"),
            ("BETTER_AUTH_SECRET", "secret-auth-value"),
        ]);
        vars
    }

    fn load(vars: &HashMap<&'static str, &'static str>) -> Result<Config, ConfigError> {
        Config::from_lookup(|name| vars.get(name).map(|v| Ok((*v).to_owned())))
    }

    #[test]
    fn applies_the_development_defaults() {
        let config = load(&valid()).unwrap();
        assert_eq!(config.mode, RunMode::Development);
        assert_eq!(config.bind, "0.0.0.0:3000".parse().unwrap());
        assert_eq!(config.auth_base_url.as_str(), "http://localhost:3000/");
        assert_eq!(config.auth_secret.expose(), DEV_FALLBACK_SECRET);
        assert!(config.fingrid_api_key.is_none());
        assert!(config.openweathermap_api_key.is_none());
        assert_eq!(config.log_format, LogFormat::Json);
    }

    #[test]
    fn reads_every_setting() {
        let mut vars = production();
        vars.extend([
            ("PORT", " 8080 "),
            ("FINGRID_API_KEY", "fingrid-key"),
            ("OPENWEATHERMAP_API_KEY", "owm-key"),
            ("LOG_FORMAT", "text"),
        ]);
        let config = load(&vars).unwrap();
        assert_eq!(config.mode, RunMode::Production);
        assert_eq!(config.bind.port(), 8080);
        assert_eq!(config.auth_base_url.as_str(), "https://spot.example/");
        assert_eq!(config.auth_secret.expose(), "secret-auth-value");
        assert_eq!(
            config.fingrid_api_key.as_ref().map(Secret::expose),
            Some("fingrid-key")
        );
        assert_eq!(
            config.openweathermap_api_key.as_ref().map(Secret::expose),
            Some("owm-key")
        );
        assert_eq!(config.log_format, LogFormat::Text);
    }

    #[test]
    fn production_requires_the_auth_settings() {
        let mut vars = valid();
        vars.insert("NODE_ENV", "production");
        assert_eq!(
            load(&vars).unwrap_err().invalid,
            ["BETTER_AUTH_URL", "BETTER_AUTH_SECRET"]
        );

        let mut vars = production();
        vars.insert("BETTER_AUTH_SECRET", "   ");
        assert_eq!(load(&vars).unwrap_err().invalid, ["BETTER_AUTH_SECRET"]);
    }

    #[test]
    fn database_url_is_required_in_every_mode() {
        for mode in ["development", "test", "production"] {
            let mut vars = production();
            vars.insert("NODE_ENV", mode);
            vars.remove("DATABASE_URL");
            assert_eq!(load(&vars).unwrap_err().invalid, ["DATABASE_URL"], "{mode}");
        }
    }

    #[test]
    fn names_every_invalid_setting_in_a_fixed_order() {
        let vars = HashMap::from([
            ("NODE_ENV", "staging"),
            ("PORT", "0"),
            ("DATABASE_URL", ""),
            ("BETTER_AUTH_URL", "not a url"),
            ("BETTER_AUTH_SECRET", ""),
            ("FINGRID_API_KEY", ""),
            ("OPENWEATHERMAP_API_KEY", ""),
            ("LOG_FORMAT", "xml"),
        ]);
        assert_eq!(
            load(&vars).unwrap_err().invalid,
            [
                "NODE_ENV",
                "PORT",
                "DATABASE_URL",
                "BETTER_AUTH_URL",
                "BETTER_AUTH_SECRET",
                "FINGRID_API_KEY",
                "OPENWEATHERMAP_API_KEY",
                "LOG_FORMAT"
            ]
        );
    }

    #[test]
    fn rejects_ports_that_are_not_decimal_or_out_of_range() {
        for value in ["", "0", "65536", "1e3", "0x10", "-1", "80.5"] {
            let mut vars = valid();
            vars.insert("PORT", value);
            assert_eq!(load(&vars).unwrap_err().invalid, ["PORT"], "{value:?}");
        }
    }

    #[test]
    fn a_non_unicode_value_is_invalid() {
        let vars = valid();
        let error = Config::from_lookup(|name| {
            if name == "FINGRID_API_KEY" {
                Some(Err(()))
            } else {
                vars.get(name).map(|v| Ok((*v).to_owned()))
            }
        })
        .unwrap_err();
        assert_eq!(error.invalid, ["FINGRID_API_KEY"]);
    }

    #[test]
    fn diagnostics_never_show_secret_values() {
        let mut vars = production();
        vars.extend([
            ("FINGRID_API_KEY", "fingrid-key"),
            ("OPENWEATHERMAP_API_KEY", "owm-key"),
        ]);
        let debug = format!("{:?}", load(&vars).unwrap());
        for secret in [
            "secret-password",
            "secret-auth-value",
            "fingrid-key",
            "owm-key",
        ] {
            assert!(!debug.contains(secret), "{secret}");
        }

        vars.insert("FINGRID_API_KEY", "");
        let error = load(&vars).unwrap_err().to_string();
        assert_eq!(error, "invalid environment variables: FINGRID_API_KEY");
    }
}
