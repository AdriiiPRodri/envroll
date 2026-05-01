//! `envroll set <KEY=value>` — set or update a single key.
//!
//! Acquires the exclusive lock. Refuses with the same `active_ref` message as
//! `envroll save` if the target env is the active one AND `active_ref` is
//! pinned (design.md D18 / variable-ops spec).

use clap::Args as ClapArgs;

use crate::cli::common::{
    active_ref_pinned_message, iso_now_local, open_project, parse_active_ref_hash,
    read_pass_and_verify, write_checkout, write_env_blob, LockMode,
};
use crate::cli::Context;
use crate::crypto;
use crate::errors::{generic, EnvrollError};
use crate::parser;
use crate::vault::sweep_historical_checkouts;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// `KEY=value` literal. If omitted, envroll explains which env it would
    /// have written to. Bad shapes (no `=`, empty key) report a usage error.
    #[arg(value_name = "KEY=value")]
    pub assignment: Option<String>,

    /// Write into this env instead of the active one.
    #[arg(long, value_name = "ENV")]
    pub r#in: Option<String>,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let prep = open_project(ctx, LockMode::Exclusive)?;
    let _ = sweep_historical_checkouts(
        &prep.vault,
        &prep.repo,
        prep.project_id(),
        &prep.project_root,
    );

    // Resolve target env first so the missing-assignment error path can name it.
    let env_name = match args.r#in.as_deref() {
        Some(n) => n.to_string(),
        None => {
            if prep.manifest.active.is_empty() {
                return Err(EnvrollError::EnvNotFound(
                    "no active env, and --in <ENV> was not given.\nusage: envroll set <KEY=value> [--in <ENV>]"
                        .to_string(),
                ));
            }
            prep.manifest.active.clone()
        }
    };

    let assignment = match args.assignment {
        Some(a) => a,
        None => {
            let target = if args.r#in.is_some() {
                format!("\"{env_name}\" (from --in)")
            } else {
                format!("active env \"{env_name}\"")
            };
            return Err(generic(format!(
                "no assignment given. Would write to {target}.\n\
                 usage: envroll set <KEY=value> [--in <ENV>]"
            )));
        }
    };

    // Parse the assignment; any malformed input is a usage-class error.
    let (key, value) = match assignment.split_once('=') {
        Some((k, v)) if !k.is_empty() => (k.to_string(), v.to_string()),
        _ => {
            return Err(generic(format!(
                "invalid assignment \"{assignment}\": expected KEY=value (e.g. `envroll set DEBUG=true`)"
            )));
        }
    };

    if !prep.env_blob_path(&env_name).exists() {
        return Err(EnvrollError::EnvNotFound(format!(
            "env \"{env_name}\" not found"
        )));
    }

    // active_ref refuse rule: only triggers when writing INTO the active env.
    if env_name == prep.manifest.active && !prep.manifest.active_ref.is_empty() {
        let hash = parse_active_ref_hash(&prep.manifest.active_ref)
            .unwrap_or(prep.manifest.active_ref.as_str());
        return Err(generic(active_ref_pinned_message(&env_name, hash)));
    }

    let pass = read_pass_and_verify(&prep, ctx)?;

    // Decrypt → parser-merge keeping existing key order → encrypt → commit.
    let blob_bytes = std::fs::read(prep.env_blob_path(&env_name)).map_err(EnvrollError::Io)?;
    let plain = crypto::decrypt(&blob_bytes, &pass)?;
    let parsed = parser::parse_buf(&plain)?;
    let updates = vec![(key.clone(), value)];
    let new_body = parser::serialize(&parsed, &updates);

    write_env_blob(&prep, &env_name, new_body.as_bytes(), &pass)?;
    // Refresh the checkout file when the target IS the active env so ./.env
    // (symlink or copy mirror) reflects the new content right away.
    if env_name == prep.manifest.active {
        write_checkout(&prep, &env_name, new_body.as_bytes())?;
    }

    let msg = format!("set {key} in {env_name} at {}", iso_now_local());
    prep.repo
        .commit_blob(&prep.env_blob_relpath(&env_name), &msg)?;

    println!("set {key} in {env_name}");
    Ok(())
}
