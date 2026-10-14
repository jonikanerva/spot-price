//! Subcommand dispatch, flag lookup, and the collected output of one run.
//! Flags are looked up like the TypeScript CLIs did: a flag can be anywhere,
//! and its value is the next argument, whatever it is.

use std::path::PathBuf;

use jiff::Timestamp;

use crate::{backtest_cli, bands, revision_cli};

const USAGE: &str =
    "usage: spot-price-tools <backtest|regenerate-bands|revision-magnitude> [flags]";

/// The stream of one output line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// The lines and exit code of one run, in the order of writing. The binary
/// writes them after the run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    pub lines: Vec<(Stream, String)>,
    pub code: i32,
}

impl Output {
    /// `console.log(line)`.
    pub fn log(&mut self, line: &str) {
        self.lines.push((Stream::Stdout, line.to_owned()));
    }

    /// `console.warn(line)`.
    pub fn warn(&mut self, line: &str) {
        self.lines.push((Stream::Stderr, line.to_owned()));
    }

    /// `console.error(line)`.
    pub fn error(&mut self, line: &str) {
        self.warn(line);
    }

    /// The text of one stream, one line per newline.
    #[must_use]
    pub fn text(&self, stream: Stream) -> String {
        self.lines.iter().filter(|(s, _)| *s == stream).fold(
            String::new(),
            |mut text, (_, line)| {
                text.push_str(line);
                text.push('\n');
                text
            },
        )
    }
}

/// The process inputs of a run, injected so tests control them.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    /// Reads one process environment variable.
    pub env: &'a dyn Fn(&str) -> Option<String>,
    pub now: Timestamp,
    /// The workspace root; the default fixture and artifact paths are below it.
    pub workspace_root: &'a std::path::Path,
}

impl std::fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("now", &self.now)
            .field("workspace_root", &self.workspace_root)
            .finish_non_exhaustive()
    }
}

/// The workspace root of this build.
#[must_use]
pub fn build_workspace_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(std::path::Path::parent)
        .map_or(manifest.clone(), std::path::Path::to_path_buf)
}

/// The argument after the first `flag`, if any.
#[must_use]
pub fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let index = args.iter().position(|a| a == flag)?;
    args.get(index + 1).map(String::as_str)
}

/// Whether `flag` is among the arguments.
#[must_use]
pub fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

/// Runs a subcommand. `args` excludes the program name. An error ends the
/// run with exit code 1 and its message on stderr.
#[must_use]
pub fn run(args: &[String], ctx: &Context<'_>) -> Output {
    let mut out = Output::default();
    let (command, rest) = match args.split_first() {
        Some((command, rest)) => (command.as_str(), rest),
        None => ("", &[][..]),
    };
    let result = match command {
        "backtest" => backtest_cli::run(rest, ctx, &mut out),
        "regenerate-bands" => bands::run(rest, ctx, &mut out),
        "revision-magnitude" => revision_cli::run(rest, ctx, &mut out),
        _ => {
            out.error(USAGE);
            Ok(2)
        }
    };
    out.code = match result {
        Ok(code) => code,
        Err(error) => {
            out.error(&format!("error: {error}"));
            1
        }
    };
    out
}
