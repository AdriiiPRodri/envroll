//! `envroll copy <KEY> --from <a> --to <b>` — copy a single key between envs.
//!
//! Refuses if `a == b` (exit 2 / generic message), if the key is absent in
//! `<a>` (exit 20), and (when `<b>` is the active env AND `active_ref` is
//! pinned) with the same message as `envroll save`.

use clap::Args as ClapArgs;

use crate::cli::common::{
    active_ref_pinned_message, iso_now_local, list_env_names, open_project, parse_active_ref_hash,
    read_pass_and_verify, write_checkout, write_env_blob, LockMode,
};
use crate::cli::Context;
use crate::crypto;
use crate::errors::{generic, usage, EnvrollError};
use crate::parser;
use crate::vault::sweep_historical_checkouts;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Key to copy.
    #[arg(value_name = "KEY")]
    pub key: Option<String>,

    /// Source env.
    #[arg(long, value_name = "ENV")]
    pub from: Option<String>,

    /// Destination env. `--from` and `--to` must differ.
    #[arg(long, value_name = "ENV")]
    pub to: Option<String>,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let prep = open_project(ctx, LockMode::Exclusive)?;
    let _ = sweep_historical_checkouts(
        &prep.vault,
        &prep.repo,
        prep.project_id(),
        &prep.project_root,
        &prep.manifest.target_filename,
    );

    // Validate args one by one so the error tells the user exactly which
    // piece is missing instead of clap's "the following required arguments
    // were not provided: <KEY> <--from> <--to>" wall.
    let key = args.key.ok_or_else(|| usage_error(&prep, "no KEY given"))?;
    let from = args
        .from
        .ok_or_else(|| usage_error(&prep, "missing --from <ENV>"))?;
    let to = args
        .to
        .ok_or_else(|| usage_error(&prep, "missing --to <ENV>"))?;

    if from == to {
        return Err(generic(format!(
            "--from and --to must differ (both were \"{from}\")"
        )));
    }

    if !prep.env_blob_path(&from).exists() {
        return Err(EnvrollError::EnvNotFound(format!(
            "env \"{from}\" not found"
        )));
    }
    if !prep.env_blob_path(&to).exists() {
        return Err(EnvrollError::EnvNotFound(format!("env \"{to}\" not found")));
    }

    if to == prep.manifest.active && !prep.manifest.active_ref.is_empty() {
        let hash = parse_active_ref_hash(&prep.manifest.active_ref)
            .unwrap_or(prep.manifest.active_ref.as_str());
        return Err(generic(active_ref_pinned_message(&to, hash)));
    }

    let pass = read_pass_and_verify(&prep, ctx)?;

    let from_bytes = std::fs::read(prep.env_blob_path(&from)).map_err(EnvrollError::Io)?;
    let from_plain = crypto::decrypt(&from_bytes, &pass)?;
    let from_kv = parser::as_key_value_map(&parser::parse_buf(&from_plain)?);

    let value = match from_kv.get(&key) {
        Some(v) => v.clone(),
        None => {
            let mut available: Vec<&String> = from_kv.keys().collect();
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
            return Err(EnvrollError::EnvNotFound(format!(
                "key \"{key}\" not found in env \"{from}\". Available keys: {listed}"
            )));
        }
    };

    let to_bytes = std::fs::read(prep.env_blob_path(&to)).map_err(EnvrollError::Io)?;
    let to_plain = crypto::decrypt(&to_bytes, &pass)?;
    let to_parsed = parser::parse_buf(&to_plain)?;
    let updates = vec![(key.clone(), value)];
    let new_body = parser::serialize(&to_parsed, &updates);

    write_env_blob(&prep, &to, new_body.as_bytes(), &pass)?;
    if to == prep.manifest.active {
        write_checkout(&prep, &to, new_body.as_bytes())?;
    }
    let msg = format!("copy {key} from {from} to {to} at {}", iso_now_local());
    prep.repo.commit_blob(&prep.env_blob_relpath(&to), &msg)?;

    println!("copied {key} from {from} to {to}");
    Ok(())
}

/// Build a usage error for `copy` that lists the available envs alongside the
/// canonical invocation, so the user knows what to pass to `--from` / `--to`
/// without leaving the shell. Returned as [`EnvrollError::Usage`] so miette
/// renders the lead and the help body separately.
fn usage_error(prep: &crate::cli::common::PreparedProject, lead: &str) -> EnvrollError {
    let names = list_env_names(prep);
    let envs_line = if names.is_empty() {
        "(no envs in this project yet — create one with `envroll fork <name>`)".to_string()
    } else {
        format!("Envs in this project: {}", names.join(", "))
    };
    usage(
        lead,
        Some(format!(
            "usage: envroll copy <KEY> --from <ENV> --to <ENV>\n{envs_line}"
        )),
    )
}
