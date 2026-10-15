//! The committed band calibration artifact. GENERATED DATA: do not edit by
//! hand. The offline band regeneration tool rewrites this whole file, and a
//! person reviews and commits it. The forecast emits bands only when
//! `calibrated` is true.
//!
//! Provenance:
//!   scored window: 2026-06-24T13:00:00.000Z … 2026-07-15T13:00:00.000Z
//!   scored origins: 22   span: 21.0 days
//!   SUMMER-ONLY — NOT validated across seasons; shipped DARK by the 90-day window guard.
//!   measured out-of-sample coverage: 0.801 (the shipped field is None while dark)

use std::borrow::Cow;

use super::conformal::{BandMethod, CalibratedBands};

/// The shipped band artifact.
pub const CALIBRATED_BANDS: CalibratedBands = CalibratedBands {
    method: BandMethod::EmpiricalResidual,
    nominal_coverage: 0.8,
    observed_coverage: None,
    calibrated: false,
    offsets_by_hour: [
        None, None, None, None, None, None, None, None, None, None, None, None, None, None, None,
        None, None, None, None, None, None, None, None, None,
    ],
    global_offsets: None,
    generated_at: Cow::Borrowed(""),
};
