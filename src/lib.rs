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
pub fn run() -> Result<u8, anyhow::Error> {
    let cli = cli::Cli::parse();
    match cli::dispatch(cli) {
        Ok(()) => Ok(0),
        Err(e) => {
            let code = e.exit_code();
            // The Display message is the user-visible spec text; the
            // `category()` tag is reserved for structured (JSON) output and
            // is deliberately omitted here so messages stay byte-identical
            // to the spec scenarios.
            eprintln!("envroll: {e}");
            Ok(code as u8)
        }
    }
}
