//! `envroll import <file> --as <name>` — adopt an existing `.env`-style file
//! as a new env in the current project.
//!
//! Onboarding accelerator. A new contributor typically arrives with a folder
//! full of `.env.dev`, `.env.staging`, `.env.bak.2024`, etc. Without
//! `import` they would have to `mv` each file to `./.env`, run `fork`, then
//! `mv` it back — clunky and error-prone. `import` does the round-trip
//! atomically and never touches the user's source files.
//!
//! Acquires the exclusive lock. Refuses on:
//! - missing source file (Io error)
//! - source file unparseable as `.env` (exit 12)
//! - name collision with an existing env (exit 30, unless `--force`)
//! - source file IS the project's working copy (use `envroll fork` instead)

use std::path::PathBuf;

use clap::Args as ClapArgs;

use crate::cli::common::{create_env_from_path, open_project, read_pass_and_verify, LockMode};
use crate::cli::Context;
use crate::errors::{generic, EnvrollError};
use crate::parser;
use crate::vault::sweep_historical_checkouts;

/// Adopt an existing `.env`-style file as a new env in this project.
///
/// Examples:
///
///   # Adopt one file
///   envroll import .env.dev --as dev
///
///   # Bulk-import a folder full of legacy envs (one shell loop)
///   for f in .env.*; do
///     name=${f#.env.}
///     envroll import "$f" --as "$name"
///   done
///
///   # Import from anywhere on disk, not just the project root
///   envroll import ~/Downloads/prod-secrets.env --as prod
///
/// What gets imported is exactly the parsed key-value content of the file —
/// comments, blank lines, and key ordering are NOT preserved (envroll commits
/// the canonical key-value set, same as `save`). Run `envroll get <KEY>` or
/// `envroll status --show-values` afterwards to verify the import was
/// faithful.
///
/// The source file on disk is left untouched. After importing, you can
/// safely delete it (`rm .env.dev`) — the encrypted copy in the vault is
/// the authoritative one going forward, and `envroll use dev` retargets
/// `./.env` (or your project's configured target) at it.
#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Path to the source `.env`-style file. Can be absolute or relative
    /// to the current directory; lives outside the vault and is left
    /// untouched.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,

    /// Name to give the new env in this project's vault.
    #[arg(long, value_name = "NAME")]
    pub r#as: String,

    /// Overwrite an existing env with the same name. Without this flag,
    /// a name collision exits 30.
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let mut prep = open_project(ctx, LockMode::Exclusive)?;
    let _ = sweep_historical_checkouts(
        &prep.vault,
        &prep.repo,
        prep.project_id(),
        &prep.project_root,
        &prep.manifest.target_filename,
    );

    // Source file must exist on disk before we even ask for the passphrase.
    if !args.file.exists() {
        return Err(generic(format!(
            "import source not found: {}",
            args.file.display()
        )));
    }

    // Refuse if the user is pointing at THEIR OWN working copy. That case
    // is `envroll fork`, not `import` — silently re-importing the working
    // copy as a new env would also lose any unsaved edits, which is exactly
    // the kind of footgun the existing `fork` flow was built to avoid.
    let source_canonical = args.file.canonicalize().map_err(|e| {
        generic(format!(
            "could not canonicalize {}: {e}",
            args.file.display()
        ))
    })?;
    let dotenv_canonical = prep.dotenv_path().canonicalize().ok();
    if dotenv_canonical.as_deref() == Some(source_canonical.as_path()) {
        return Err(generic(format!(
            "{} IS this project's working copy — use `envroll fork {}` instead, \
             which handles the active-env / symlink resolution correctly",
            args.file.display(),
            args.r#as
        )));
    }

    // Read + parse before prompting the user for a passphrase. A bad input
    // file should fail fast with exit 12 (parse error), not after the
    // passphrase prompt.
    let bytes = std::fs::read(&args.file).map_err(EnvrollError::Io)?;
    parser::parse_buf(&bytes)?;

    let pass = read_pass_and_verify(&prep, ctx)?;
    let default_msg = format!(
        "import {} as {}",
        args.file
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| args.file.display().to_string()),
        args.r#as
    );

    create_env_from_path(
        &mut prep,
        &args.r#as,
        &bytes,
        &pass,
        &default_msg,
        None,
        args.force,
    )?;

    println!(
        "imported {} → {} (now active)",
        args.file.display(),
        args.r#as
    );
    Ok(())
}
