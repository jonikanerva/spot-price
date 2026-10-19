//! `spot-price-tools regenerate-bands`: backtest a fixture (`--data`, default
//! the committed one) or the database (`--db`), then rewrite
//! `crates/domain/src/forecast/conformal_artifact.rs` (or `--out <path>`) and
//! run `rustfmt` on it. A person reviews and commits the result; never run it
//! as a background job.

use std::path::{Path, PathBuf};
use std::process::Command;

use spot_price_domain::forecast::{BandOffsets, CalibratedBands, iso_millis};
use spot_price_domain::js::number_to_string;

use crate::backtest::{MIN_CALIBRATION_WINDOW_DAYS, derive_bands_from_backtest};
use crate::backtest_cli::{database_url, iso_or_na, parse_window_days};
use crate::cli::{Context, Output, flag_value, has_flag};
use crate::data::{BacktestData, load_fixture, parse_instant};
use crate::db;
use crate::error::ToolError;
use crate::jsfmt::to_fixed;

/// The source of the calibration, for the provenance comment.
#[derive(Debug, Clone, PartialEq)]
pub struct Provenance {
    pub scored_window_start: Option<String>,
    pub scored_window_end: Option<String>,
    pub scored_origins: usize,
    pub scored_window_span_days: f64,
    /// The measured coverage, also when the artifact is dark.
    pub observed_coverage: Option<f64>,
}

fn line(out: &mut String, text: &str) {
    out.push_str(text);
    out.push('\n');
}

/// A finite float as a Rust literal.
fn float(x: f64) -> Result<String, ToolError> {
    if x.is_finite() {
        Ok(format!("{x:?}"))
    } else {
        Err(ToolError::Render(format!("non-finite value {x}")))
    }
}

fn offsets(o: BandOffsets, indent: &str) -> Result<String, ToolError> {
    Ok(format!(
        "Some(BandOffsets {{\n{indent}    low_offset: {},\n{indent}    high_offset: {},\n{indent}}})",
        float(o.low_offset)?,
        float(o.high_offset)?
    ))
}

fn hours_literal(bands: &CalibratedBands) -> Result<String, ToolError> {
    let mut out = String::from("[\n");
    if bands.offsets_by_hour.iter().all(Option::is_none) {
        // The layout of rustfmt for 24 short items.
        out.push_str("        None, None, None, None, None, None, None, None, None, None, None, None, None, None, None,\n");
        out.push_str("        None, None, None, None, None, None, None, None, None,\n");
    } else {
        for hour in &bands.offsets_by_hour {
            match hour {
                Some(o) => {
                    line(&mut out, &format!("        {},", offsets(*o, "        ")?));
                }
                None => out.push_str("        None,\n"),
            }
        }
    }
    out.push_str("    ]");
    Ok(out)
}

/// The artifact as the Rust module `conformal_artifact.rs`.
///
/// # Errors
/// [`ToolError::Render`] for a non-finite value.
pub fn render_artifact(
    bands: &CalibratedBands,
    provenance: &Provenance,
) -> Result<String, ToolError> {
    let span = provenance.scored_window_span_days;
    let mut out = String::new();
    out.push_str("//! The committed band calibration artifact. GENERATED DATA: do not edit by\n");
    out.push_str("//! hand. The offline band regeneration tool rewrites this whole file, and a\n");
    out.push_str("//! person reviews and commits it. The forecast emits bands only when\n");
    out.push_str("//! `calibrated` is true.\n");
    out.push_str("//!\n");
    out.push_str("//! Provenance:\n");
    line(
        &mut out,
        &format!(
            "//!   scored window: {} … {}",
            provenance.scored_window_start.as_deref().unwrap_or("n/a"),
            provenance.scored_window_end.as_deref().unwrap_or("n/a")
        ),
    );
    line(
        &mut out,
        &format!(
            "//!   scored origins: {}   span: {} days",
            provenance.scored_origins,
            to_fixed(span, 1)
        ),
    );
    if span < MIN_CALIBRATION_WINDOW_DAYS {
        line(
            &mut out,
            &format!(
                "//!   SUMMER-ONLY — NOT validated across seasons; shipped DARK by the {}-day window guard.",
                number_to_string(MIN_CALIBRATION_WINDOW_DAYS)
            ),
        );
    }
    line(
        &mut out,
        &format!(
            "//!   measured out-of-sample coverage: {} (the shipped field is None while dark)",
            provenance
                .observed_coverage
                .map_or_else(|| "n/a".to_owned(), |c| to_fixed(c, 3))
        ),
    );
    out.push_str("\nuse std::borrow::Cow;\n\n");
    let has_offsets =
        bands.global_offsets.is_some() || bands.offsets_by_hour.iter().any(Option::is_some);
    out.push_str(if has_offsets {
        "use super::conformal::{BandMethod, BandOffsets, CalibratedBands};\n"
    } else {
        "use super::conformal::{BandMethod, CalibratedBands};\n"
    });
    out.push_str("\n/// The shipped band artifact.\n");
    out.push_str("pub const CALIBRATED_BANDS: CalibratedBands = CalibratedBands {\n");
    out.push_str("    method: BandMethod::EmpiricalResidual,\n");
    line(
        &mut out,
        &format!("    nominal_coverage: {},", float(bands.nominal_coverage)?),
    );
    let observed = match bands.observed_coverage {
        Some(c) => format!("Some({})", float(c)?),
        None => "None".to_owned(),
    };
    line(&mut out, &format!("    observed_coverage: {observed},"));
    line(&mut out, &format!("    calibrated: {},", bands.calibrated));
    line(
        &mut out,
        &format!("    offsets_by_hour: {},", hours_literal(bands)?),
    );
    let global = match bands.global_offsets {
        Some(o) => offsets(o, "    ")?,
        None => "None".to_owned(),
    };
    line(&mut out, &format!("    global_offsets: {global},"));
    line(
        &mut out,
        &format!(
            "    generated_at: Cow::Borrowed({:?}),",
            bands.generated_at.as_ref()
        ),
    );
    out.push_str("};\n");
    Ok(out)
}

/// Runs `rustfmt` on `path` with the toolchain of the workspace.
fn rustfmt(path: &Path, workspace_root: &Path) -> Result<(), ToolError> {
    let failed = |reason: String| ToolError::Rustfmt {
        path: path.display().to_string(),
        reason,
    };
    let output = Command::new("rustfmt")
        .args(["--edition", "2024"])
        .arg(path)
        .current_dir(workspace_root)
        .output()
        .map_err(|e| failed(e.to_string()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(failed(String::from_utf8_lossy(&output.stderr).into_owned()))
    }
}

fn write_artifact(
    data: &BacktestData,
    generated_at: String,
    out_path: &Path,
    ctx: &Context<'_>,
    out: &mut Output,
) -> Result<(), ToolError> {
    let derived = derive_bands_from_backtest(data, generated_at)?;
    let summary = &derived.summary;
    let provenance = Provenance {
        scored_window_start: summary
            .scored_window_start_ms
            .map(|_| iso_or_na(summary.scored_window_start_ms)),
        scored_window_end: summary
            .scored_window_end_ms
            .map(|_| iso_or_na(summary.scored_window_end_ms)),
        scored_origins: summary.origins,
        scored_window_span_days: summary.scored_window_span_days,
        observed_coverage: derived.observed_coverage,
    };
    let text = render_artifact(&derived.bands, &provenance)?;
    std::fs::write(out_path, text).map_err(|source| ToolError::Write {
        path: out_path.display().to_string(),
        source,
    })?;
    rustfmt(out_path, ctx.workspace_root)?;

    let bands = &derived.bands;
    out.log(&format!(
        "\nConformal band artifact regenerated → {}",
        out_path.display()
    ));
    out.log(&format!(
        "  residuals:           {}",
        summary.residuals.len()
    ));
    out.log(&format!(
        "  scored window:       {} … {} ({}d, {} origins, {} pre-vintage skipped, {} no-grid-data skipped)",
        provenance.scored_window_start.as_deref().unwrap_or("n/a"),
        provenance.scored_window_end.as_deref().unwrap_or("n/a"),
        to_fixed(provenance.scored_window_span_days, 1),
        provenance.scored_origins,
        summary.pre_vintage_origins,
        summary.no_grid_data_origins
    ));
    out.log(&format!("  leakage-free:        {}", summary.leak_free));
    out.log(&format!(
        "  calibrated:          {} (measured coverage {} vs nominal {})",
        bands.calibrated,
        derived
            .observed_coverage
            .map_or_else(|| "n/a".to_owned(), |c| to_fixed(c, 3)),
        number_to_string(bands.nominal_coverage)
    ));
    if !bands.calibrated {
        out.log("  → shipping DARK: coverage below the gate or too few samples. Review before committing.");
    }
    out.log("");
    Ok(())
}

/// The default artifact path below the workspace root.
#[must_use]
pub fn default_artifact_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join("crates/domain/src/forecast/conformal_artifact.rs")
}

/// The committed backtest fixture below the workspace root.
#[must_use]
pub fn default_fixture_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join("crates/tools/data/fixture.json")
}

/// Runs `regenerate-bands` with the arguments after the subcommand.
/// `--generated-at <instant>` fixes the artifact time; the default is now.
///
/// # Errors
/// [`ToolError`] for an unreadable fixture, a bad flag, a database error,
/// or a failed write or `rustfmt` run.
pub fn run(args: &[String], ctx: &Context<'_>, out: &mut Output) -> Result<i32, ToolError> {
    let generated_at = match flag_value(args, "--generated-at") {
        Some(text) => iso_millis(parse_instant(text).map_err(ToolError::Argument)?),
        None => iso_millis(ctx.now),
    };
    let out_path = flag_value(args, "--out")
        .map_or_else(|| default_artifact_path(ctx.workspace_root), PathBuf::from);
    let data = if has_flag(args, "--db") {
        let window_days = parse_window_days(flag_value(args, "--window"))?;
        let url = database_url(args, ctx, "spot-price-tools regenerate-bands --db")?;
        db::block_on(db::fetch_backtest_data(&url, window_days, ctx.now))??
    } else {
        let file = flag_value(args, "--data")
            .map_or_else(|| default_fixture_path(ctx.workspace_root), PathBuf::from);
        load_fixture(&file)?
    };
    write_artifact(&data, generated_at, &out_path, ctx, out)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::io::Write as _;
    use std::process::Stdio;

    use spot_price_domain::forecast::{BandMethod, uncalibrated_bands};

    use super::*;

    fn rustfmt_stdin(text: &str) -> String {
        let mut child = Command::new("rustfmt")
            .args(["--edition", "2024", "--emit", "stdout"])
            .current_dir(crate::cli::build_workspace_root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn renders_the_committed_dark_artifact_byte_for_byte() {
        let provenance = Provenance {
            scored_window_start: Some("2026-06-24T13:00:00.000Z".to_owned()),
            scored_window_end: Some("2026-07-15T13:00:00.000Z".to_owned()),
            scored_origins: 22,
            scored_window_span_days: 21.0,
            observed_coverage: Some(0.8012),
        };
        let text = render_artifact(&uncalibrated_bands(), &provenance).unwrap();
        let committed = include_str!("../../domain/src/forecast/conformal_artifact.rs");
        assert_eq!(text, committed);
    }

    #[test]
    fn a_calibrated_render_is_already_rustfmt_output() {
        let mut offsets_by_hour = [None; 24];
        offsets_by_hour[0] = Some(BandOffsets {
            low_offset: -0.846_262_776_242_272_5,
            high_offset: 0.481_419_350_161_412_6,
        });
        offsets_by_hour[23] = Some(BandOffsets {
            low_offset: -0.0,
            high_offset: 1e-7,
        });
        let bands = CalibratedBands {
            method: BandMethod::EmpiricalResidual,
            nominal_coverage: 0.8,
            observed_coverage: Some(0.799_739_822_467_095_2),
            calibrated: true,
            offsets_by_hour,
            global_offsets: Some(BandOffsets {
                low_offset: -0.4,
                high_offset: 0.35,
            }),
            generated_at: Cow::Borrowed("2026-10-14T00:00:00.000Z"),
        };
        let provenance = Provenance {
            scored_window_start: Some("2026-01-05T13:00:00.000Z".to_owned()),
            scored_window_end: Some("2026-04-06T13:00:00.000Z".to_owned()),
            scored_origins: 92,
            scored_window_span_days: 91.0,
            observed_coverage: Some(0.799_739_822_467_095_2),
        };
        let text = render_artifact(&bands, &provenance).unwrap();
        assert_eq!(rustfmt_stdin(&text), text);
        assert!(!text.contains("SUMMER-ONLY"));
        assert!(text.contains("low_offset: -0.0,"));
        assert!(text.contains("high_offset: 1e-7,"));
    }

    #[test]
    fn a_non_finite_value_is_an_error() {
        let bands = CalibratedBands {
            observed_coverage: Some(f64::NAN),
            ..uncalibrated_bands()
        };
        let provenance = Provenance {
            scored_window_start: None,
            scored_window_end: None,
            scored_origins: 0,
            scored_window_span_days: 0.0,
            observed_coverage: None,
        };
        assert!(matches!(
            render_artifact(&bands, &provenance),
            Err(ToolError::Render(_))
        ));
    }
}
