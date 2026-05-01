//! `envroll export <env>` — emit the full env's plaintext content to stdout
//! in one of three formats. Anti-lock-in escape hatch.
//!
//! envroll is a local-first tool with strong opinions about NOT trapping
//! users. `export` is the deliberate, audited path to get plaintext OUT —
//! whether to pipe into a hosted secrets manager (AWS Secrets Manager,
//! Vault, Doppler), to drive a Kubernetes `kubectl create secret`, or just
//! to migrate away from envroll entirely. There are no opinions or rate
//! limits on how you use it; it's your data.
//!
//! Acquires the shared lock. Output is **never** masked — masking would
//! defeat the whole point of the command. If you want a paste-safe summary,
//! use `envroll status --mask` or `envroll diff --mask` instead.

use std::collections::BTreeMap;

use clap::{Args as ClapArgs, ValueEnum};

use crate::cli::common::{open_project, read_pass_and_verify, LockMode};
use crate::cli::Context;
use crate::crypto;
use crate::errors::EnvrollError;
use crate::parser;

/// Output format for `envroll export`.
#[derive(Copy, Clone, Debug, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum ExportFormat {
    /// `KEY="value"` lines (the `.env` convention dotenvy reads back).
    /// Each value is double-quoted with the four `dotenvy` escapes
    /// (`\\`, `\"`, `\$`, `\n`) so the output is a strict subset of the
    /// `.env` syntax that round-trips through `envroll import`.
    Dotenv,
    /// A single JSON object `{"KEY": "value", ...}`. Useful for piping into
    /// `jq`, AWS Secrets Manager (`aws secretsmanager put-secret-value
    /// --secret-string file://-`), or any tool that consumes structured
    /// secrets.
    Json,
    /// `export KEY="value"` lines safe for `eval $(envroll export ...
    /// --format shell)`. Values are POSIX-shell single-quoted with literal
    /// single-quotes escaped via the standard `'\''` trick, so they're safe
    /// even when they contain `$`, backticks, or quotes.
    Shell,
}

/// Print every key/value pair in `<env>` to stdout. Plaintext, never masked.
///
/// Examples:
///
///   # Pipe to a file (most common)
///   envroll export prod > prod.env
///
///   # Push to AWS Secrets Manager
///   envroll export prod --format json | \
///     aws secretsmanager put-secret-value --secret-id myapp/prod --secret-string file:///dev/stdin
///
///   # Create a Kubernetes secret
///   envroll export prod --format dotenv | \
///     kubectl create secret generic prod-env --from-env-file=/dev/stdin
///
///   # Eval directly into your current shell (testing only — leaks to `ps`)
///   eval "$(envroll export dev --format shell)"
///
///   # Migrate AWAY from envroll (back to plain .env files)
///   for e in $(envroll list --format json | jq -r '.[0].envs[]'); do
///     envroll export "$e" > ".env.$e"
///   done
///
/// Since this is the deliberate plaintext-out path, the command requires
/// the vault passphrase but does NOT consult `--mask` / `--show-values`
/// flags. If you find yourself wanting a "masked export", what you want
/// is `envroll status --mask` or `envroll diff --mask`, not export.
#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Env name to export. Bare names only (no `<name>@<hash>` ref forms);
    /// historical exports go through `envroll exec <name>@<hash> -- env`
    /// or by checking out the historical version first.
    #[arg(value_name = "ENV")]
    pub env: String,

    /// Output shape: `dotenv` (default — `KEY="value"` lines that round-trip
    /// through `envroll import`), `json` (a `{KEY: value}` object for piping
    /// into hosted secret managers), or `shell` (`export KEY='value'` lines
    /// safe for `eval $(...)` in your current shell).
    ///
    /// We deliberately use `--output` here instead of the global `--format`
    /// flag (which is for structured human/json output of read commands)
    /// because `envroll export` produces fundamentally different shapes than
    /// the rest of the CLI. The global `--format` is ignored by `export`.
    #[arg(long, value_enum, default_value_t = ExportFormat::Dotenv)]
    pub output: ExportFormat,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let prep = open_project(ctx, LockMode::Shared)?;

    if !prep.env_blob_path(&args.env).exists() {
        return Err(EnvrollError::EnvNotFound(format!(
            "env \"{}\" not found",
            args.env
        )));
    }

    let pass = read_pass_and_verify(&prep, ctx)?;
    let bytes = std::fs::read(prep.env_blob_path(&args.env)).map_err(EnvrollError::Io)?;
    let plaintext = crypto::decrypt(&bytes, &pass)?;
    let kv = parser::as_key_value_map(&parser::parse_buf(&plaintext)?);

    match args.output {
        ExportFormat::Dotenv => print_dotenv(&kv),
        ExportFormat::Json => print_json(&kv)?,
        ExportFormat::Shell => print_shell(&kv),
    }
    Ok(())
}

fn print_dotenv(kv: &BTreeMap<String, String>) {
    // Reuse the parser's serializer so what we emit round-trips through
    // `envroll import`. We pass the kv as both `parsed` and an empty
    // updates slice — the serializer collapses dups and keeps the order.
    let parsed: Vec<(String, String)> = kv.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    print!("{}", parser::serialize(&parsed, &[]));
}

fn print_json(kv: &BTreeMap<String, String>) -> Result<(), EnvrollError> {
    let s = serde_json::to_string(kv)
        .map_err(|e| EnvrollError::Generic(format!("serializing export JSON: {e}")))?;
    println!("{s}");
    Ok(())
}

fn print_shell(kv: &BTreeMap<String, String>) {
    for (k, v) in kv {
        // POSIX single-quote escape: wrap in '...', and replace any literal
        // ' with '\''. This is robust against $, backticks, double-quotes,
        // newlines, everything — single quotes are the only POSIX construct
        // that disables ALL shell expansion.
        let escaped = v.replace('\'', "'\\''");
        println!("export {k}='{escaped}'");
    }
}
