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
///   verbatim multi-line stderr messages from design.md D10/D11; we route
///   those through their dedicated formatters before falling back to the
///   single-line form so the byte-exact spec text reaches the user.
pub fn run() -> Result<u8, anyhow::Error> {
    let cli = cli::Cli::parse();
    let log_level = cli.log;
    match cli::dispatch(cli) {
        Ok(()) => Ok(0),
        Err(e) => {
            let code = e.exit_code();
            print_error(&e, log_level);
            Ok(code as u8)
        }
    }
}

/// Format and print an [`EnvrollError`] to stderr per spec 15.1 and 15.3.
fn print_error(e: &errors::EnvrollError, log_level: cli::LogLevel) {
    use errors::EnvrollError;
    match e {
        EnvrollError::NoPassphraseSource => {
            // Verbatim multi-line message from design.md D11.
            eprint!("{}", prompt::NO_PASSPHRASE_SOURCE_MESSAGE);
            return;
        }
        EnvrollError::SyncConflict => {
            // The sync command itself prints the design.md D10 multi-line
            // block before propagating; we still emit the canonical
            // single-line summary so the structured `envroll:` prefix stays
            // present at the very bottom of the stream.
            eprintln!("envroll: {}: {e}", e.category());
            return;
        }
        _ => {}
    }
    eprintln!("envroll: {}: {e}", e.category());
    if matches!(log_level, cli::LogLevel::Debug)
        || std::env::var("RUST_LOG").ok().as_deref() == Some("debug")
    {
        eprintln!("(debug) {e:?}");
    }
}
