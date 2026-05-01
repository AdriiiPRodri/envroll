//! `envroll rename-key OLD NEW [--in <env>] [--all]` — rename a key across
//! one env or every env in the project.
//!
//! Refactoring helper. Without this, renaming `DATABASE_URL` to `DB_URL`
//! across `dev`, `staging`, `prod`, and `feature-x` requires eight separate
//! `envroll set --in <env>` invocations plus remembering to delete the old
//! key in each env — easy to leave dangling references.
//!
//! Three target modes, in precedence order:
//!
//! 1. `--all`            → every env in the project that contains `OLD`
//! 2. `--in <env>`       → exactly that env
//! 3. (no flag)          → the active env
//!
//! For each target env we:
//! - decrypt
//! - skip silently if `OLD` is not present (so `--all` is a no-op on envs
//!   that never had the key)
//! - refuse if `NEW` is already present and `--force` was not passed
//!   (would silently overwrite the existing value)
//! - rewrite the env content with `NEW = old_value`, `OLD` removed
//! - encrypt + commit (one commit per affected env, message
//!   `rename-key OLD → NEW in <env>`)
//!
//! Refuses on the same `active_ref` rule as `envroll save`/`set`/`copy` —
//! writing into a historically-pinned env would silently rewind it.

use clap::Args as ClapArgs;

use crate::cli::common::{
    active_ref_pinned_message, iso_now_local, list_env_names, open_project, parse_active_ref_hash,
    read_pass_and_verify, write_checkout, write_env_blob, LockMode,
};
use crate::cli::Context;
use crate::crypto;
use crate::errors::{generic, EnvrollError};
use crate::parser;
use crate::vault::sweep_historical_checkouts;

/// Rename a key across one or every env in the project.
///
/// Examples:
///
///   # Rename DATABASE_URL to DB_URL in the active env
///   envroll rename-key DATABASE_URL DB_URL
///
///   # Rename in a specific env
///   envroll rename-key STRIPE_SK STRIPE_SECRET --in prod
///
///   # Rename across every env that has the key (skips silently those that don't)
///   envroll rename-key DATABASE_URL DB_URL --all
///
///   # Force overwrite if NEW already exists in a target env
///   envroll rename-key DATABASE_URL DB_URL --all --force
///
/// One commit is created per affected env, with the message
/// `rename-key OLD → NEW in <env> at <ts>`. Envs that don't contain OLD
/// are skipped — they don't get an empty commit and they don't count
/// toward the "n envs renamed" summary printed at the end.
#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Existing key name to rename.
    #[arg(value_name = "OLD")]
    pub old: String,

    /// New key name.
    #[arg(value_name = "NEW")]
    pub new: String,

    /// Apply to this env only. Mutually exclusive with `--all`.
    #[arg(long, value_name = "ENV", conflicts_with = "all")]
    pub r#in: Option<String>,

    /// Apply to every env in the project that contains the old key.
    #[arg(long)]
    pub all: bool,

    /// Overwrite NEW if it already exists in a target env. Without this
    /// flag, a target env that already has both OLD and NEW is refused
    /// to avoid silently dropping NEW's existing value.
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    if args.old == args.new {
        return Err(generic("OLD and NEW must differ"));
    }

    let prep = open_project(ctx, LockMode::Exclusive)?;
    let _ = sweep_historical_checkouts(
        &prep.vault,
        &prep.repo,
        prep.project_id(),
        &prep.project_root,
        &prep.manifest.target_filename,
    );

    // Resolve the target set of env names per the precedence above.
    let targets: Vec<String> = if args.all {
        list_env_names(&prep)
    } else if let Some(name) = args.r#in.as_deref() {
        vec![name.to_string()]
    } else {
        if prep.manifest.active.is_empty() {
            return Err(EnvrollError::EnvNotFound(
                "no active env, and neither --in <ENV> nor --all was given".to_string(),
            ));
        }
        vec![prep.manifest.active.clone()]
    };

    if targets.is_empty() {
        println!("rename-key: no envs in this project — nothing to do");
        return Ok(());
    }

    // Active_ref refuse rule: if any target IS the pinned active env, refuse
    // BEFORE doing any decryption work. Same hazard as `save`/`set`/`copy`.
    if !prep.manifest.active_ref.is_empty() && targets.iter().any(|t| t == &prep.manifest.active) {
        let hash = parse_active_ref_hash(&prep.manifest.active_ref)
            .unwrap_or(prep.manifest.active_ref.as_str());
        return Err(generic(active_ref_pinned_message(
            &prep.manifest.active,
            hash,
        )));
    }

    // Verify each target env actually exists in the project (catches typos
    // in --in fast). For --all we already filtered to the registered set.
    for name in &targets {
        if !prep.env_blob_path(name).exists() {
            return Err(EnvrollError::EnvNotFound(format!(
                "env \"{name}\" not found"
            )));
        }
    }

    let pass = read_pass_and_verify(&prep, ctx)?;

    let mut renamed = 0usize;
    let mut skipped_no_key: Vec<String> = Vec::new();
    for env_name in &targets {
        let blob = std::fs::read(prep.env_blob_path(env_name)).map_err(EnvrollError::Io)?;
        let plain = crypto::decrypt(&blob, &pass)?;
        let parsed = parser::parse_buf(&plain)?;
        let kv = parser::as_key_value_map(&parsed);

        // Silently skip envs that never had OLD. This is what makes --all
        // useful: a key that lives in some envs and not others doesn't
        // require the user to enumerate them.
        let old_value = match kv.get(&args.old) {
            Some(v) => v.clone(),
            None => {
                skipped_no_key.push(env_name.clone());
                continue;
            }
        };

        if kv.contains_key(&args.new) && !args.force {
            return Err(EnvrollError::NameCollision(format!(
                "env \"{env_name}\" already has key \"{}\"; pass --force to overwrite",
                args.new
            )));
        }

        // Rewrite: drop OLD from the parsed sequence, then merge NEW=old_value
        // through the standard serializer. Order is preserved for everything
        // we DIDN'T touch.
        let kept: Vec<(String, String)> = parsed
            .iter()
            .filter(|(k, _)| k != &args.old)
            .cloned()
            .collect();
        let updates = vec![(args.new.clone(), old_value)];
        let new_body = parser::serialize(&kept, &updates);

        write_env_blob(&prep, env_name, new_body.as_bytes(), &pass)?;
        if env_name == &prep.manifest.active {
            write_checkout(&prep, env_name, new_body.as_bytes())?;
        }
        let msg = format!(
            "rename-key {} → {} in {env_name} at {}",
            args.old,
            args.new,
            iso_now_local()
        );
        prep.repo
            .commit_blob(&prep.env_blob_relpath(env_name), &msg)?;
        renamed += 1;
    }

    // Summary. Print to stdout so it's pipeable; the skipped list goes to
    // stderr as informational so scripts don't have to grep it out.
    println!(
        "rename-key: {} → {} in {renamed} env(s)",
        args.old, args.new
    );
    if !skipped_no_key.is_empty() {
        eprintln!(
            "envroll: skipped (no \"{}\" key): {}",
            args.old,
            skipped_no_key.join(", ")
        );
    }
    Ok(())
}
