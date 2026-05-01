//! envroll — git for your `.env` files.
//!
//! This crate exposes the binary's internals as a library so integration
//! tests can drive subcommands without going through `assert_cmd`. The public
//! API surface is intentionally small: [`run`] is the single entrypoint
//! `main` calls, and the modules below are re-exported for tests.

pub mod cli;
pub mod crypto;
pub mod errors;
pub mod lock;
pub mod manifest;
pub mod output;
pub mod parser;
pub mod paths;
pub mod prompt;
pub mod vault;

use clap::Parser;

/// Parse argv, dispatch to the requested subcommand, and return the exit code.
///
/// Errors produced by subcommands are translated into stable exit codes via
/// [`errors::EnvrollError::exit_code`]. Clap parse failures use clap's own
/// exit (2) and never reach this function.
///
/// Output formatting (spec 15.1):
/// - The single-line `envroll: <category>: <message>` form is the default.
/// - With `--log debug` (or `RUST_LOG=debug`), the chain is printed in full
///   so users debugging an obscure error get the upstream context.
/// - A handful of error variants (`NoPassphraseSource`, `SyncConflict`) own
///   verbatim multi-line stderr messages; we route
///   those through their dedicated formatters before falling back to the
///   single-line form so the byte-exact spec text reaches the user.
pub fn run() -> Result<u8, anyhow::Error> {
    let cli = cli::Cli::parse();
    let log_level = cli.log;
    match cli::dispatch(cli) {
        Ok(()) => Ok(0),
        Err(e) => {
            let code = e.exit_code();
            print_error(e, log_level);
            Ok(code as u8)
        }
    }
}

/// Format and print an [`EnvrollError`] to stderr.
///
/// Two variants own verbatim multi-line text and bypass miette so the bytes
/// the user sees stay byte-exact:
///
/// - `NoPassphraseSource` prints the D11 block (the `prompt` module owns the
///   canonical string).
/// - `SyncConflict`'s D10 block is already emitted by `cli::sync::run`
///   before this propagates, so here we only need miette's structured cap.
///
/// Everything else goes through `miette::Report` which gives the colored,
/// boxed, icon-decorated rendering with `code` / `help` / source labels —
/// see `errors::EnvrollError`'s Diagnostic derive.
fn print_error(e: errors::EnvrollError, log_level: cli::LogLevel) {
    use errors::EnvrollError;
    if matches!(e, EnvrollError::NoPassphraseSource) {
        eprint!("{}", prompt::NO_PASSPHRASE_SOURCE_MESSAGE);
        return;
    }
    let debug_chain = matches!(log_level, cli::LogLevel::Debug)
        || std::env::var("RUST_LOG").ok().as_deref() == Some("debug");
    let report: miette::Report = miette::Report::new(e);
    if debug_chain {
        eprintln!("{report:?}");
    } else {
        // Default rendering: miette's Display goes through the GraphicalReport
        // handler installed by the `fancy` feature, which honors NO_COLOR
        // and pipe-detection automatically.
        eprintln!("{report:?}");
    }
}
