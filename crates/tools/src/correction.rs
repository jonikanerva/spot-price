//! The quarter-correction arms of the evaluation: their settings and names,
//! the predeclared grid, the selection on the selection segment, and the
//! alias of the selected arm that the criteria refer to. Pure.

use spot_price_domain::forecast::Decomposition;

use crate::evaluation::{Evaluation, Segment, Split, Statistic};
use crate::profile::{ShapeFamily, pooled_mae};

/// The shape profile of every correction arm: the served profile, so arm A
/// is the baseline arm.
pub const CORRECTION_PROFILE: ShapeFamily = ShapeFamily::SERVED;

/// The zone of the local date of the holiday column.
pub const HOLIDAY_TIME_ZONE: &str = "Europe/Helsinki";

/// The alias of the selected correction arm.
pub const SELECTED: &str = "corr_selected";
/// The alias of the holiday twin of the selected correction arm.
pub const SELECTED_HOLIDAY: &str = "corr_selected_hol";

/// The longest training window in days: the issuance-training window.
pub const MAX_WINDOW_DAYS: u8 = 30;
/// The longest recency half-life in days.
pub const MAX_HALF_LIFE_DAYS: u8 = 60;

/// The settings of one correction arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrectionSpec {
    pub decomposition: Decomposition,
    /// The ridge penalty λ on standardized features.
    pub ridge_lambda: u32,
    /// Training targets from this many days before the series start.
    pub window_days: u8,
    /// The recency half-life in days; 0 weights every example the same.
    pub half_life_days: u8,
    /// Adds the Finnish public-holiday column.
    pub holiday: bool,
}

const fn decomposition_token(d: Decomposition) -> &'static str {
    match d {
        Decomposition::Joint => "joint",
        Decomposition::DayDeviation => "dev",
    }
}

impl CorrectionSpec {
    /// The name in flags, arm names, and the report, for example
    /// `joint_l1000_w30_h7` or `dev_l100_w14_h0_hol`.
    #[must_use]
    pub fn token(self) -> String {
        format!(
            "{}_l{}_w{}_h{}{}",
            decomposition_token(self.decomposition),
            self.ridge_lambda,
            self.window_days,
            self.half_life_days,
            if self.holiday { "_hol" } else { "" }
        )
    }

    /// Reads a [`CorrectionSpec::token`].
    ///
    /// # Errors
    /// A message for another value or a setting outside its range.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let invalid = || {
            format!(
                "a correction must be <joint|dev>_l<lambda>_w<1-{MAX_WINDOW_DAYS}>_h<0-{MAX_HALF_LIFE_DAYS}>[_hol], got \"{raw}\""
            )
        };
        let (body, holiday) = match raw.strip_suffix("_hol") {
            Some(body) => (body, true),
            None => (raw, false),
        };
        let parts: Vec<&str> = body.split('_').collect();
        let [decomposition, lambda, window, half_life] = parts.as_slice() else {
            return Err(invalid());
        };
        let decomposition = match *decomposition {
            "joint" => Decomposition::Joint,
            "dev" => Decomposition::DayDeviation,
            _ => return Err(invalid()),
        };
        let number = |part: &str, prefix: char| -> Option<u32> {
            let digits = part.strip_prefix(prefix)?;
            // One canonical spelling per value, so a token names one arm.
            if digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
                return None;
            }
            digits.parse().ok()
        };
        let ridge_lambda = number(lambda, 'l').ok_or_else(invalid)?;
        let window_days = number(window, 'w')
            .and_then(|v| u8::try_from(v).ok())
            .filter(|v| (1..=MAX_WINDOW_DAYS).contains(v))
            .ok_or_else(invalid)?;
        let half_life_days = number(half_life, 'h')
            .and_then(|v| u8::try_from(v).ok())
            .filter(|v| *v <= MAX_HALF_LIFE_DAYS)
            .ok_or_else(invalid)?;
        Ok(Self {
            decomposition,
            ridge_lambda,
            window_days,
            half_life_days,
            holiday,
        })
    }

    /// The same settings without the holiday column.
    #[must_use]
    pub const fn without_holiday(self) -> Self {
        Self {
            holiday: false,
            ..self
        }
    }

    /// The same settings with the holiday column.
    #[must_use]
    pub const fn with_holiday(self) -> Self {
        Self {
            holiday: true,
            ..self
        }
    }
}

/// The predeclared grid of `--corrections grid`, in the order that breaks a
/// selection tie: the earlier arm wins.
#[must_use]
pub fn grid() -> Vec<CorrectionSpec> {
    let mut out = Vec::new();
    for decomposition in [Decomposition::Joint, Decomposition::DayDeviation] {
        for window_days in [30, 14] {
            for half_life_days in [0, 7] {
                for ridge_lambda in [10_000, 1000, 100, 1] {
                    out.push(CorrectionSpec {
                        decomposition,
                        ridge_lambda,
                        window_days,
                        half_life_days,
                        holiday: false,
                    });
                }
            }
        }
    }
    out
}

/// The arm name of a correction.
#[must_use]
pub fn arm_name(spec: CorrectionSpec) -> String {
    format!("corr_{}", spec.token())
}

/// The selection of the correction arms; see [`select`].
#[derive(Debug, Clone, PartialEq)]
pub struct CorrectionSelection {
    /// The arm with the lowest pooled MAE among the corrections without the
    /// holiday column, if any has a value.
    pub selected: Option<String>,
    /// The holiday twin of the selected arm, if the run has it.
    pub holiday_twin: Option<String>,
}

/// The declared selection rule: the correction arm without the holiday
/// column with the lowest pooled MAE on `segment`; a tie keeps the earlier
/// arm of `specs`.
#[must_use]
pub fn select(
    evaluation: &Evaluation,
    specs: &[CorrectionSpec],
    segment: Segment,
    split: Option<Split>,
) -> CorrectionSelection {
    let rows = pooled_mae(evaluation, segment, split);
    let mut best: Option<(String, f64)> = None;
    for spec in specs.iter().filter(|s| !s.holiday) {
        let name = arm_name(*spec);
        let Some(mae) = rows.iter().find(|r| r.arm == name).and_then(|r| r.mae) else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, b)| mae < *b) {
            best = Some((name, mae));
        }
    }
    let selected = best.map(|(name, _)| name);
    let holiday_twin = selected.as_ref().and_then(|name| {
        specs
            .iter()
            .find(|s| s.holiday && arm_name(s.without_holiday()) == *name)
            .map(|s| arm_name(*s))
    });
    CorrectionSelection {
        selected,
        holiday_twin,
    }
}

fn rename(part: &str, from: &str, to: &str) -> String {
    part.split('-')
        .map(|piece| if piece == from { to } else { piece })
        .collect::<Vec<_>>()
        .join("-")
}

/// The statistics of the selected arm and its holiday twin again, under
/// [`SELECTED`] and [`SELECTED_HOLIDAY`]. A criteria file can then name
/// the selected arm before the selection is known.
#[must_use]
pub fn aliases(stats: &[Statistic], selection: &CorrectionSelection) -> Vec<Statistic> {
    let Some(selected) = &selection.selected else {
        return Vec::new();
    };
    let mut renames = vec![(selected.as_str(), SELECTED)];
    if let Some(twin) = &selection.holiday_twin {
        renames.push((twin.as_str(), SELECTED_HOLIDAY));
    }
    stats
        .iter()
        .filter_map(|s| {
            let parts: Vec<String> =
                s.id.split('/')
                    .map(|part| {
                        renames
                            .iter()
                            .fold(part.to_owned(), |p, (from, to)| rename(&p, from, to))
                    })
                    .collect();
            let id = parts.join("/");
            (id != s.id).then(|| Statistic { id, ..s.clone() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_read_back() {
        for spec in grid() {
            assert_eq!(CorrectionSpec::parse(&spec.token()), Ok(spec));
            let holiday = spec.with_holiday();
            assert_eq!(CorrectionSpec::parse(&holiday.token()), Ok(holiday));
        }
        assert_eq!(grid().len(), 32);
        for bad in [
            "",
            "joint",
            "joint_l1_w30",
            "joint_l1_w0_h0",
            "joint_l1_w31_h0",
            "joint_l01_w30_h0",
            "joint_l1_w30_h61",
            "Joint_l1_w30_h0",
            "joint_l1_w30_h0_hol_hol",
            "joint_l-1_w30_h0",
            "mix_l1_w30_h0",
        ] {
            assert!(CorrectionSpec::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_alias_renames_every_part_of_an_id() {
        let stat = |id: &str| Statistic {
            id: id.to_owned(),
            n: 3,
            estimate: crate::bootstrap::Estimate {
                point: Some(1.0),
                low: None,
                high: None,
                undefined_resamples: 0,
            },
        };
        let stats = vec![
            stat("mae/corr_a/06/D+1"),
            stat("mae_delta/corr_a-A/06/D+1"),
            stat("mae_delta/corr_a_hol-corr_a/13/D+2"),
            stat("cost_delta/corr_a-A/within_day/4h/06"),
            stat("mae/corr_ab/06/D+1"),
            stat("mae/B/06/D+1"),
        ];
        let selection = CorrectionSelection {
            selected: Some("corr_a".to_owned()),
            holiday_twin: Some("corr_a_hol".to_owned()),
        };
        let ids: Vec<String> = aliases(&stats, &selection)
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(
            ids,
            [
                "mae/corr_selected/06/D+1",
                "mae_delta/corr_selected-A/06/D+1",
                "mae_delta/corr_selected_hol-corr_selected/13/D+2",
                "cost_delta/corr_selected-A/within_day/4h/06",
            ]
        );
    }
}
