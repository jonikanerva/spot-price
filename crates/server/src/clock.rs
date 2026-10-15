//! The wall clock, behind a trait so that tests can fix the time.

use jiff::Timestamp;

/// A source of the current instant.
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// A clock that always returns the same instant.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub Timestamp);

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

/// The start of the UTC hour of `instant`.
#[must_use]
pub fn truncate_to_utc_hour(instant: Timestamp) -> Timestamp {
    let second = instant.as_second().div_euclid(3600) * 3600;
    Timestamp::from_second(second).unwrap_or(instant)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_to_the_start_of_the_utc_hour() {
        let instant: Timestamp = "2026-06-13T13:37:42.5Z".parse().unwrap();
        assert_eq!(
            truncate_to_utc_hour(instant).to_string(),
            "2026-06-13T13:00:00Z"
        );
    }
}
