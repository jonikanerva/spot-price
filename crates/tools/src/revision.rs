//! The Fingrid forecast-revision study of datasets 245 and 165, and a GO /
//! MARGINAL / DEFER recommendation. Pure. The reference of a target is its
//! freshest vintage; a revision at lead L is `value@L − reference`, in UTC
//! epoch milliseconds.

use std::collections::{BTreeMap, HashMap, HashSet};

use jiff::Timestamp;

use crate::data::{
    DATASET_CONSUMPTION_ACTUAL, DATASET_CONSUMPTION_FORECAST, DATASET_WIND_ACTUAL,
    DATASET_WIND_FORECAST, DatasetId, Vintage,
};
use crate::jsfmt::{count_f64, js_sum, to_fixed};
use crate::metrics::{median, percentile, rms, standard_deviation};

const HOUR_MS: i64 = 60 * 60 * 1000;

/// The studied forecast datasets, in report order.
pub const VINTAGE_DATASET_IDS: [DatasetId; 2] =
    [DATASET_WIND_FORECAST, DATASET_CONSUMPTION_FORECAST];

/// A target is admissible only when its freshest issuance is at most this
/// many hours before delivery. `issued_at` has about ±1 h uncertainty, so
/// the cutoff has it too. An issuance after delivery is admissible.
pub const REFERENCE_MAX_LEAD_H: i64 = 2;

/// One lead-time bin `(lo_h, hi_h]` in hours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeadBucket {
    pub label: &'static str,
    pub lo_h: f64,
    pub hi_h: f64,
}

/// The lead-time bins, finer near delivery.
pub const LEAD_BUCKETS: [LeadBucket; 7] = [
    LeadBucket {
        label: "<=6h",
        lo_h: 0.0,
        hi_h: 6.0,
    },
    LeadBucket {
        label: "6-12h",
        lo_h: 6.0,
        hi_h: 12.0,
    },
    LeadBucket {
        label: "12-18h",
        lo_h: 12.0,
        hi_h: 18.0,
    },
    LeadBucket {
        label: "18-24h",
        lo_h: 18.0,
        hi_h: 24.0,
    },
    LeadBucket {
        label: "24-36h",
        lo_h: 24.0,
        hi_h: 36.0,
    },
    LeadBucket {
        label: "36-48h",
        lo_h: 36.0,
        hi_h: 48.0,
    },
    LeadBucket {
        label: ">48h",
        lo_h: 48.0,
        hi_h: f64::INFINITY,
    },
];

/// Below this many observations a bin is insufficient: its statistics are
/// `None`, and the aggregate and the recommendation ignore it.
pub const MIN_SAMPLES_PER_BIN: usize = 100;

/// Minimum observations over the sufficient bins of a dataset before its
/// attenuation can decide the verdict.
pub const MIN_BAND_SAMPLES: usize = 200;

/// At or below this attenuation on any dataset the verdict is GO.
pub const GO_ATTENUATION: f64 = 0.9;

/// At or above this attenuation the verdict is a provisional MARGINAL.
pub const MARGINAL_ATTENUATION: f64 = 0.95;

/// One settled value for the reference-versus-actual check.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActualRecord {
    pub dataset_id: DatasetId,
    pub start: Timestamp,
    pub value: f64,
}

/// The input of the study. An empty actuals map skips the actual check.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RevisionStudyInput {
    pub vintages_by_dataset: BTreeMap<DatasetId, Vec<Vintage>>,
    pub actuals_by_dataset: BTreeMap<DatasetId, Vec<ActualRecord>>,
}

/// Revision statistics of one dataset and lead bin, in MW. The statistics
/// are `None` when the bin is not sufficient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BucketMetrics {
    pub label: &'static str,
    pub lo_h: f64,
    pub hi_h: f64,
    /// Target × issuance observations.
    pub samples: usize,
    pub targets: usize,
    pub sufficient: bool,
    pub mean_abs_revision: Option<f64>,
    pub median_abs_revision: Option<f64>,
    pub p90_abs_revision: Option<f64>,
    pub rms_revision: Option<f64>,
    pub signed_bias_revision: Option<f64>,
    /// `rms_revision / sd(reference)`.
    pub noise_to_signal: Option<f64>,
    /// `1 / (1 + NSR²)`.
    pub attenuation: Option<f64>,
    /// `mean_abs_revision / mean(|reference|)`.
    pub rel_mean_abs: Option<f64>,
}

/// The reference-versus-actual check.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActualCheck {
    pub targets_compared: usize,
    pub median_abs_ref_minus_actual: Option<f64>,
    pub mean_abs_ref_minus_actual: Option<f64>,
}

/// The study result of one forecast dataset.
#[derive(Debug, Clone, PartialEq)]
pub struct DatasetRevisionSummary {
    pub dataset_id: DatasetId,
    pub admissible_targets: usize,
    pub excluded_targets: usize,
    /// Targets delivered after the last observed issuance.
    pub future_targets: usize,
    pub exclusion_rate: Option<f64>,
    pub references_post_delivery: usize,
    pub references_pre_delivery_within_tol: usize,
    pub empirical_max_lead_h: Option<f64>,
    pub empirical_p90_lead_h: Option<f64>,
    pub sd_reference: Option<f64>,
    pub mean_abs_reference: Option<f64>,
    /// RMS revision over the sufficient bins only.
    pub production_band_rms: Option<f64>,
    pub production_band_nsr: Option<f64>,
    /// The single-regressor attenuation `1 / (1 + NSR²)`: an illustration,
    /// not the multivariate factor of the ridge fit.
    pub attenuation_illustration: Option<f64>,
    pub production_band_samples: usize,
    pub actual_check: Option<ActualCheck>,
    pub buckets: Vec<BucketMetrics>,
}

/// The UTC span of all vintages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevisionWindow {
    pub earliest_issued_at: Timestamp,
    pub latest_issued_at: Timestamp,
    pub earliest_target: Timestamp,
    pub latest_target: Timestamp,
}

/// The whole study.
#[derive(Debug, Clone, PartialEq)]
pub struct RevisionStudyResult {
    pub datasets: Vec<DatasetRevisionSummary>,
    /// `None` when there are no vintages.
    pub window: Option<RevisionWindow>,
}

fn mean_or_null(xs: &[f64]) -> Option<f64> {
    (!xs.is_empty()).then(|| js_sum(xs.iter().copied()) / count_f64(xs.len()))
}

fn max_or_null(xs: &[f64]) -> Option<f64> {
    (!xs.is_empty()).then(|| {
        xs.iter()
            .fold(f64::NEG_INFINITY, |max, &x| if x > max { x } else { max })
    })
}

fn bucket_index_of(lead_h: f64) -> Option<usize> {
    if lead_h <= 0.0 {
        return None;
    }
    LEAD_BUCKETS
        .iter()
        .position(|b| lead_h > b.lo_h && lead_h <= b.hi_h)
}

fn ms(at: Timestamp) -> i64 {
    at.as_millisecond()
}

#[derive(Debug, Default)]
struct TargetOutcome {
    admissible: bool,
    future: bool,
    ref_value: f64,
    ref_lead_ms: i64,
    /// Positive-lead `(lead_ms, revision)` pairs without the reference row.
    observations: Vec<(i64, f64)>,
}

fn classify_target(target_ms: i64, vintages: &[Vintage], now_proxy_ms: i64) -> TargetOutcome {
    let Some(first) = vintages.first() else {
        return TargetOutcome::default();
    };
    let mut ref_index = 0;
    let mut ref_issued_ms = ms(first.issued_at);
    for (i, v) in vintages.iter().enumerate() {
        if ms(v.issued_at) > ref_issued_ms {
            ref_index = i;
            ref_issued_ms = ms(v.issued_at);
        }
    }
    if target_ms > now_proxy_ms {
        return TargetOutcome {
            future: true,
            ..TargetOutcome::default()
        };
    }
    let ref_lead_ms = target_ms - ref_issued_ms;
    if ref_lead_ms > REFERENCE_MAX_LEAD_H * HOUR_MS {
        return TargetOutcome {
            ref_lead_ms,
            ..TargetOutcome::default()
        };
    }
    let ref_value = vintages.get(ref_index).map_or(0.0, |v| v.value);
    let observations = vintages
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != ref_index)
        .map(|(_, v)| (target_ms - ms(v.issued_at), v.value - ref_value))
        .filter(|(lead_ms, _)| *lead_ms > 0)
        .collect();
    TargetOutcome {
        admissible: true,
        future: false,
        ref_value,
        ref_lead_ms,
        observations,
    }
}

#[derive(Debug, Default)]
struct BucketAcc {
    revisions: Vec<f64>,
    targets: HashSet<i64>,
}

const fn empty_bucket(b: LeadBucket) -> BucketMetrics {
    BucketMetrics {
        label: b.label,
        lo_h: b.lo_h,
        hi_h: b.hi_h,
        samples: 0,
        targets: 0,
        sufficient: false,
        mean_abs_revision: None,
        median_abs_revision: None,
        p90_abs_revision: None,
        rms_revision: None,
        signed_bias_revision: None,
        noise_to_signal: None,
        attenuation: None,
        rel_mean_abs: None,
    }
}

fn attenuation_of(nsr: f64) -> f64 {
    1.0 / (1.0 + nsr * nsr)
}

/// `a / b`; `None` when either is missing or `|b|` is below 1e-12.
fn ratio_or_null(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    let (a, b) = (a?, b?);
    (b.abs() >= 1e-12).then(|| a / b)
}

fn bucket_metrics(
    b: LeadBucket,
    acc: &BucketAcc,
    sd_reference: Option<f64>,
    mean_abs_reference: Option<f64>,
) -> BucketMetrics {
    let samples = acc.revisions.len();
    if samples < MIN_SAMPLES_PER_BIN {
        return BucketMetrics {
            samples,
            targets: acc.targets.len(),
            ..empty_bucket(b)
        };
    }
    let abs: Vec<f64> = acc.revisions.iter().map(|r| r.abs()).collect();
    let rms_revision = rms(&acc.revisions);
    let mean_abs_revision = mean_or_null(&abs);
    let noise_to_signal = ratio_or_null(rms_revision, sd_reference);
    BucketMetrics {
        label: b.label,
        lo_h: b.lo_h,
        hi_h: b.hi_h,
        samples,
        targets: acc.targets.len(),
        sufficient: true,
        mean_abs_revision,
        median_abs_revision: median(&abs),
        p90_abs_revision: percentile(&abs, 90.0),
        rms_revision,
        signed_bias_revision: mean_or_null(&acc.revisions),
        noise_to_signal,
        attenuation: noise_to_signal.map(attenuation_of),
        rel_mean_abs: ratio_or_null(mean_abs_revision, mean_abs_reference),
    }
}

fn actual_check_of(ref_by_target: &[(i64, f64)], actuals: &[ActualRecord]) -> Option<ActualCheck> {
    if actuals.is_empty() || ref_by_target.is_empty() {
        return None;
    }
    let actual_by_target: HashMap<i64, f64> =
        actuals.iter().map(|a| (ms(a.start), a.value)).collect();
    let diffs: Vec<f64> = ref_by_target
        .iter()
        .filter_map(|(target, ref_value)| {
            actual_by_target
                .get(target)
                .map(|actual| (ref_value - actual).abs())
        })
        .collect();
    if diffs.is_empty() {
        return None;
    }
    Some(ActualCheck {
        targets_compared: diffs.len(),
        median_abs_ref_minus_actual: median(&diffs),
        mean_abs_ref_minus_actual: mean_or_null(&diffs),
    })
}

/// Vintages grouped by target in the order of first appearance.
fn group_by_target(vintages: &[Vintage]) -> Vec<(i64, Vec<Vintage>)> {
    let mut groups: Vec<(i64, Vec<Vintage>)> = Vec::new();
    let mut index: HashMap<i64, usize> = HashMap::new();
    for v in vintages {
        let target = ms(v.start);
        if let Some(group) = index.get(&target).and_then(|&i| groups.get_mut(i)) {
            group.1.push(*v);
        } else {
            index.insert(target, groups.len());
            groups.push((target, vec![*v]));
        }
    }
    groups
}

fn empty_summary(dataset_id: DatasetId) -> DatasetRevisionSummary {
    DatasetRevisionSummary {
        dataset_id,
        admissible_targets: 0,
        excluded_targets: 0,
        future_targets: 0,
        exclusion_rate: None,
        references_post_delivery: 0,
        references_pre_delivery_within_tol: 0,
        empirical_max_lead_h: None,
        empirical_p90_lead_h: None,
        sd_reference: None,
        mean_abs_reference: None,
        production_band_rms: None,
        production_band_nsr: None,
        attenuation_illustration: None,
        production_band_samples: 0,
        actual_check: None,
        buckets: LEAD_BUCKETS.iter().map(|b| empty_bucket(*b)).collect(),
    }
}

fn summarise_dataset(
    dataset_id: DatasetId,
    vintages: &[Vintage],
    actuals: &[ActualRecord],
) -> DatasetRevisionSummary {
    let Some(now_proxy_ms) = vintages.iter().map(|v| ms(v.issued_at)).max() else {
        return empty_summary(dataset_id);
    };
    let mut bucket_acc: [BucketAcc; 7] = Default::default();
    let mut ref_values = Vec::new();
    let mut all_positive_leads_h = Vec::new();
    let mut ref_by_target: Vec<(i64, f64)> = Vec::new();
    let mut s = empty_summary(dataset_id);

    for (target_ms, group) in group_by_target(vintages) {
        let outcome = classify_target(target_ms, &group, now_proxy_ms);
        if outcome.future {
            s.future_targets += 1;
            continue;
        }
        if !outcome.admissible {
            s.excluded_targets += 1;
            continue;
        }
        s.admissible_targets += 1;
        ref_values.push(outcome.ref_value);
        ref_by_target.push((target_ms, outcome.ref_value));
        if outcome.ref_lead_ms <= 0 {
            s.references_post_delivery += 1;
        } else {
            s.references_pre_delivery_within_tol += 1;
        }
        #[expect(clippy::cast_precision_loss, reason = "a lead in ms is far below 2^53")]
        for (lead_ms, revision) in outcome.observations {
            let lead_h = lead_ms as f64 / HOUR_MS as f64;
            all_positive_leads_h.push(lead_h);
            if let Some(acc) = bucket_index_of(lead_h).and_then(|i| bucket_acc.get_mut(i)) {
                acc.revisions.push(revision);
                acc.targets.insert(target_ms);
            }
        }
    }

    s.sd_reference = standard_deviation(&ref_values);
    let abs_refs: Vec<f64> = ref_values.iter().map(|v| v.abs()).collect();
    s.mean_abs_reference = mean_or_null(&abs_refs);
    s.buckets = LEAD_BUCKETS
        .iter()
        .zip(&bucket_acc)
        .map(|(b, acc)| bucket_metrics(*b, acc, s.sd_reference, s.mean_abs_reference))
        .collect();

    let band_revisions: Vec<f64> = bucket_acc
        .iter()
        .filter(|acc| acc.revisions.len() >= MIN_SAMPLES_PER_BIN)
        .flat_map(|acc| acc.revisions.iter().copied())
        .collect();
    s.production_band_rms = rms(&band_revisions);
    s.production_band_nsr = ratio_or_null(s.production_band_rms, s.sd_reference);
    s.attenuation_illustration = s.production_band_nsr.map(attenuation_of);
    s.production_band_samples = band_revisions.len();
    let considered = s.admissible_targets + s.excluded_targets;
    s.exclusion_rate =
        (considered > 0).then(|| count_f64(s.excluded_targets) / count_f64(considered));
    s.empirical_max_lead_h = max_or_null(&all_positive_leads_h);
    s.empirical_p90_lead_h = percentile(&all_positive_leads_h, 90.0);
    s.actual_check = actual_check_of(&ref_by_target, actuals);
    s
}

/// The actual dataset paired with a forecast dataset.
#[must_use]
pub const fn actual_id_for(forecast_id: DatasetId) -> DatasetId {
    match forecast_id {
        DATASET_WIND_FORECAST => DATASET_WIND_ACTUAL,
        DATASET_CONSUMPTION_FORECAST => DATASET_CONSUMPTION_ACTUAL,
        other => other,
    }
}

/// Runs the study.
#[must_use]
pub fn run_revision_study(input: &RevisionStudyInput) -> RevisionStudyResult {
    let datasets = VINTAGE_DATASET_IDS
        .iter()
        .map(|&id| {
            summarise_dataset(
                id,
                input
                    .vintages_by_dataset
                    .get(&id)
                    .map_or(&[][..], Vec::as_slice),
                input
                    .actuals_by_dataset
                    .get(&actual_id_for(id))
                    .map_or(&[][..], Vec::as_slice),
            )
        })
        .collect();

    let all = input.vintages_by_dataset.values().flatten();
    let window = all.clone().next().map(|first| {
        all.fold(
            RevisionWindow {
                earliest_issued_at: first.issued_at,
                latest_issued_at: first.issued_at,
                earliest_target: first.start,
                latest_target: first.start,
            },
            |w, v| RevisionWindow {
                earliest_issued_at: w.earliest_issued_at.min(v.issued_at),
                latest_issued_at: w.latest_issued_at.max(v.issued_at),
                earliest_target: w.earliest_target.min(v.start),
                latest_target: w.latest_target.max(v.start),
            },
        )
    });
    RevisionStudyResult { datasets, window }
}

/// The verdict. The study never closes the lead-time-matched training
/// question; it only says GO, MARGINAL, or DEFER.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Go,
    Marginal,
    Defer,
}

impl Verdict {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Go => "GO",
            Self::Marginal => "MARGINAL",
            Self::Defer => "DEFER",
        }
    }
}

/// A verdict and its reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recommendation {
    pub verdict: Verdict,
    pub reason: String,
}

fn fmt(v: Option<f64>, digits: usize) -> String {
    v.map_or_else(|| "n/a".to_owned(), |v| to_fixed(v, digits))
}

/// The recommendation from the dataset with the lowest attenuation among
/// those with at least [`MIN_BAND_SAMPLES`] sufficient-bin samples.
#[must_use]
pub fn recommendation(result: &RevisionStudyResult) -> Recommendation {
    let mut strongest: Option<(&DatasetRevisionSummary, f64)> = None;
    for d in &result.datasets {
        let Some(att) = d.attenuation_illustration else {
            continue;
        };
        if d.production_band_samples < MIN_BAND_SAMPLES {
            continue;
        }
        if strongest.is_none_or(|(_, best)| att < best) {
            strongest = Some((d, att));
        }
    }
    let Some((d, att)) = strongest else {
        return Recommendation {
            verdict: Verdict::Defer,
            reason: format!(
                "No dataset reached {MIN_BAND_SAMPLES} revision samples across its sufficient bins with a computable attenuation — insufficient data to decide; re-measure once more (ideally winter) vintages have accrued."
            ),
        };
    };
    let detail = format!(
        "dataset {}: attenuation {} (NSR {}, {} samples)",
        d.dataset_id,
        to_fixed(att, 3),
        fmt(d.production_band_nsr, 3),
        d.production_band_samples
    );
    let go = to_fixed(GO_ATTENUATION, 2);
    let marginal = to_fixed(MARGINAL_ATTENUATION, 2);
    if att <= GO_ATTENUATION {
        return Recommendation {
            verdict: Verdict::Go,
            reason: format!(
                "{detail} ≤ {go} — the leaky fit over-trusts the forecast enough that lead-time-matched training (#81) should pay off."
            ),
        };
    }
    if att >= MARGINAL_ATTENUATION {
        return Recommendation {
            verdict: Verdict::Marginal,
            reason: format!(
                "{detail} ≥ {marginal} — the revision effect barely moves the fit; PROVISIONAL (summer). Confirm via #80's vintage-correct backtest; do not close #81 on summer-only data."
            ),
        };
    }
    Recommendation {
        verdict: Verdict::Marginal,
        reason: format!(
            "{detail} in ({go}, {marginal}) — a modest effect; confirm the real gain via #80's vintage-correct backtest before investing in #81 (do not close #81 on summer-only data)."
        ),
    }
}
