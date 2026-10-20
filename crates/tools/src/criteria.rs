//! Success and regression criteria that an experiment declares before its
//! final test; docs/adr/0004 shows the file format. Every success criterion
//! must be met for success. A met regression criterion is a detected
//! regression. The tool reports outcomes; the owner decides.

use std::path::Path;

use serde::Deserialize;

use crate::bootstrap::Estimate;
use crate::data::read_json;
use crate::error::ToolError;
use crate::evaluation::{Statistic, fnv1a_hex};

/// Which value of a statistic a criterion reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Which {
    #[serde(rename = "point")]
    Point,
    /// The lower end of the 95 % bootstrap interval.
    #[serde(rename = "ci_low")]
    CiLow,
    /// The upper end of the 95 % bootstrap interval.
    #[serde(rename = "ci_high")]
    CiHigh,
}

impl Which {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Point => "point",
            Self::CiLow => "ci_low",
            Self::CiHigh => "ci_high",
        }
    }

    const fn of(self, e: &Estimate) -> Option<f64> {
        match self {
            Self::Point => e.point,
            Self::CiLow => e.low,
            Self::CiHigh => e.high,
        }
    }
}

/// A comparison of the statistic with the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Op {
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Ge,
}

impl Op {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }

    fn holds(self, value: f64, threshold: f64) -> bool {
        match self {
            Self::Lt => value < threshold,
            Self::Le => value <= threshold,
            Self::Gt => value > threshold,
            Self::Ge => value >= threshold,
        }
    }
}

/// One declared threshold on one statistic.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub id: String,
    pub statistic: Which,
    pub op: Op,
    pub value: f64,
}

/// A criteria file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Criteria {
    pub experiment: String,
    /// The declaration date, as the experimenter wrote it.
    pub declared: String,
    pub success: Vec<Criterion>,
    pub regression: Vec<Criterion>,
}

/// A loaded criteria file and the digest of its exact text.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredCriteria {
    pub criteria: Criteria,
    pub digest: String,
}

/// Reads a criteria file.
///
/// # Errors
/// [`ToolError`] when the file cannot be read, is not valid criteria JSON,
/// has no criterion, or has a threshold that is not finite.
pub fn load_criteria(path: &Path) -> Result<DeclaredCriteria, ToolError> {
    let text = std::fs::read_to_string(path).map_err(|source| ToolError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let criteria: Criteria = read_json(path)?;
    let invalid = |reason: &str| ToolError::Fixture {
        path: path.display().to_string(),
        reason: reason.to_owned(),
    };
    if criteria.success.is_empty() && criteria.regression.is_empty() {
        return Err(invalid("the criteria file declares no criterion"));
    }
    if criteria
        .success
        .iter()
        .chain(&criteria.regression)
        .any(|c| !c.value.is_finite())
    {
        return Err(invalid("a criterion value is not a finite number"));
    }
    Ok(DeclaredCriteria {
        criteria,
        digest: fnv1a_hex(&text),
    })
}

/// The outcome of one criterion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Met(f64),
    NotMet(f64),
    /// The statistic exists, but the chosen value is undefined (for
    /// example a ratio with a zero baseline error).
    Undefined,
    /// No statistic with this id in the evaluated segment.
    Missing,
}

impl Outcome {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Met(_) => "met",
            Self::NotMet(_) => "not met",
            Self::Undefined => "UNDEFINED",
            Self::Missing => "MISSING",
        }
    }
}

/// The outcome of `criterion` against `statistics`.
#[must_use]
pub fn check(criterion: &Criterion, statistics: &[Statistic]) -> Outcome {
    let Some(stat) = statistics.iter().find(|s| s.id == criterion.id) else {
        return Outcome::Missing;
    };
    match criterion.statistic.of(&stat.estimate) {
        None => Outcome::Undefined,
        Some(v) if criterion.op.holds(v, criterion.value) => Outcome::Met(v),
        Some(v) => Outcome::NotMet(v),
    }
}

/// The overall result of a criteria file.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub success: Vec<Outcome>,
    pub regression: Vec<Outcome>,
}

/// A three-way answer: `None` when an outcome is undefined or missing.
pub type Decided = Option<bool>;

impl Verdict {
    #[must_use]
    pub fn of(criteria: &Criteria, statistics: &[Statistic]) -> Self {
        let outcomes = |list: &[Criterion]| list.iter().map(|c| check(c, statistics)).collect();
        Self {
            success: outcomes(&criteria.success),
            regression: outcomes(&criteria.regression),
        }
    }

    /// Whether every success criterion is met. `None` when one is
    /// undefined or missing and none is clearly not met.
    #[must_use]
    pub fn success_met(&self) -> Decided {
        if self.success.iter().any(|o| matches!(o, Outcome::NotMet(_))) {
            return Some(false);
        }
        if self.success.iter().all(|o| matches!(o, Outcome::Met(_))) {
            return Some(true);
        }
        None
    }

    /// Whether a regression criterion is met. `None` when none is met and
    /// one is undefined or missing.
    #[must_use]
    pub fn regression_detected(&self) -> Decided {
        if self.regression.iter().any(|o| matches!(o, Outcome::Met(_))) {
            return Some(true);
        }
        if self
            .regression
            .iter()
            .all(|o| matches!(o, Outcome::NotMet(_)))
        {
            return Some(false);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(id: &str, point: Option<f64>, low: Option<f64>, high: Option<f64>) -> Statistic {
        Statistic {
            id: id.to_owned(),
            n: 10,
            estimate: Estimate {
                point,
                low,
                high,
                undefined_resamples: 0,
            },
        }
    }

    fn criterion(id: &str, statistic: Which, op: Op, value: f64) -> Criterion {
        Criterion {
            id: id.to_owned(),
            statistic,
            op,
            value,
        }
    }

    #[test]
    fn reads_the_chosen_value_and_compares_it() {
        let stats = [stat("x", Some(0.9), Some(0.8), Some(1.0))];
        assert_eq!(
            check(&criterion("x", Which::CiHigh, Op::Le, 1.0), &stats),
            Outcome::Met(1.0)
        );
        assert_eq!(
            check(&criterion("x", Which::CiHigh, Op::Lt, 1.0), &stats),
            Outcome::NotMet(1.0)
        );
        assert_eq!(
            check(&criterion("x", Which::CiLow, Op::Ge, 0.8), &stats),
            Outcome::Met(0.8)
        );
    }

    #[test]
    fn an_undefined_or_missing_statistic_is_never_met() {
        let stats = [stat("ratio", None, None, None)];
        assert_eq!(
            check(&criterion("ratio", Which::Point, Op::Lt, 1.0), &stats),
            Outcome::Undefined
        );
        assert_eq!(
            check(&criterion("other", Which::Point, Op::Lt, 1.0), &stats),
            Outcome::Missing
        );
        let verdict = Verdict {
            success: vec![Outcome::Met(0.5), Outcome::Undefined],
            regression: vec![Outcome::NotMet(0.0), Outcome::Missing],
        };
        assert_eq!(verdict.success_met(), None);
        assert_eq!(verdict.regression_detected(), None);
    }

    #[test]
    fn one_unmet_success_or_one_met_regression_decides() {
        let verdict = Verdict {
            success: vec![Outcome::NotMet(2.0), Outcome::Undefined],
            regression: vec![Outcome::Met(0.2), Outcome::Missing],
        };
        assert_eq!(verdict.success_met(), Some(false));
        assert_eq!(verdict.regression_detected(), Some(true));
        let clean = Verdict {
            success: vec![Outcome::Met(0.5)],
            regression: vec![Outcome::NotMet(0.0)],
        };
        assert_eq!(clean.success_met(), Some(true));
        assert_eq!(clean.regression_detected(), Some(false));
    }
}
