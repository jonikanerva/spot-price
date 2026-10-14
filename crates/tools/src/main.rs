//! `spot-price-tools`: offline forecast tools. See the library documentation.

use std::io::Write as _;
use std::process::ExitCode;

use spot_price_tools::cli::{Context, Output, Stream, build_workspace_root, run};

/// Writes the lines in order: the report to stdout, warnings to stderr.
fn emit(output: &Output) -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    for (stream, line) in &output.lines {
        match stream {
            Stream::Stdout => writeln!(stdout, "{line}")?,
            Stream::Stderr => {
                stdout.flush()?;
                writeln!(stderr, "{line}")?;
            }
        }
    }
    stdout.flush()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = |name: &str| std::env::var(name).ok();
    let root = build_workspace_root();
    let ctx = Context {
        env: &env,
        now: jiff::Timestamp::now(),
        workspace_root: &root,
    };
    let output = run(&args, &ctx);
    if emit(&output).is_err() {
        return ExitCode::FAILURE;
    }
    ExitCode::from(u8::try_from(output.code).unwrap_or(1))
}
