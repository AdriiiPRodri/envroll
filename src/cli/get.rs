//! `envroll get <KEY>` — print a single key's value to stdout.
//!
//! Script-friendly: never masked, single trailing `\n`. Exits 20 if the key
//! is missing. Acquires a shared lock per design.md D15.
//!
//! Note: `get` deliberately does NOT trigger the historical-checkout TTL
//! sweep — design.md D5 names it as one of the four read-only commands that
//! must NOT touch `.checkout/` cleanup.

use clap::Args as ClapArgs;

use crate::cli::common::{open_project, read_pass_and_verify, LockMode};
use crate::cli::Context;
use crate::crypto;
use crate::errors::{generic, EnvrollError};
use crate::parser;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Key to print. If omitted, envroll explains which env it would have
    /// read from and how to discover the keys.
    #[arg(value_name = "KEY")]
    pub key: Option<String>,

    /// Read from this env instead of the active one.
    #[arg(long, value_name = "ENV")]
    pub from: Option<String>,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let prep = open_project(ctx, LockMode::Shared)?;

    // Resolve which env we'd target FIRST, even on the missing-KEY error
    // path — the user's first question is always "which env am I about to
    // read from?".
    let env_name = match args.from.as_deref() {
        Some(n) => n.to_string(),
        None => {
            if prep.manifest.active.is_empty() {
                return Err(EnvrollError::EnvNotFound(
                    "no active env, and --from <ENV> was not given.\nusage: envroll get <KEY> [--from <ENV>]"
                        .to_string(),
                ));
            }
            prep.manifest.active.clone()
        }
    };

    let key = match args.key {
        Some(k) => k,
        None => {
            let source = if args.from.is_some() {
                format!("\"{env_name}\" (from --from)")
            } else {
                format!("active env \"{env_name}\"")
            };
            return Err(generic(format!(
                "no KEY given. Would read from {source}.\n\
                 To see the keys in this env: envroll status --show-values\n\
                 usage: envroll get <KEY> [--from <ENV>]"
            )));
        }
    };

    if !prep.env_blob_path(&env_name).exists() {
        return Err(EnvrollError::EnvNotFound(format!(
            "env \"{env_name}\" not found"
        )));
    }
    let pass = read_pass_and_verify(&prep, ctx)?;
    let bytes = std::fs::read(prep.env_blob_path(&env_name)).map_err(EnvrollError::Io)?;
    let plaintext = crypto::decrypt(&bytes, &pass)?;
    let kv = parser::as_key_value_map(&parser::parse_buf(&plaintext)?);
    match kv.get(&key) {
        Some(v) => {
            println!("{v}");
            Ok(())
        }
        None => {
            // Hint with the available keys so a typo is one read away from
            // self-correcting. Keys themselves are not secret; values are.
            let mut available: Vec<&String> = kv.keys().collect();
            available.sort();
            let listed = if available.is_empty() {
                "(env is empty)".to_string()
            } else {
                available
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            Err(EnvrollError::EnvNotFound(format!(
                "key \"{key}\" not found in env \"{env_name}\". Available keys: {listed}"
            )))
        }
    }
}
