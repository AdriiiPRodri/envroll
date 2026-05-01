//! `envroll status` — runtime mode, dirty state, pinned ref of the active env.
//!
//! Shared lock. The reported mode is derived at runtime
//! — never persisted.

use std::collections::BTreeSet;

use clap::Args as ClapArgs;
use serde::Serialize;

use crate::cli::common::{open_project, read_pass_and_verify, LockMode};
use crate::cli::Context;
use crate::crypto;
use crate::errors::EnvrollError;
use crate::output::OutputFormat;
use crate::parser;
use crate::vault::{sweep_historical_checkouts, Mode};

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Mask values with `********` instead of printing them. Off by default —
    /// you're on your own machine looking at your own envs. Enable this when
    /// you're about to paste the output into a screenshot, ticket, or chat.
    #[arg(long)]
    pub mask: bool,

    /// Deprecated alias for the inverse of `--mask`. Kept so the spec's
    /// original `--show-values` invocation still works; new scripts should
    /// just stop passing it (showing values is the default now).
    #[arg(long, hide = true)]
    pub show_values: bool,
}

#[derive(Debug, Serialize)]
struct StatusJson {
    active: String,
    mode: &'static str,
    clean: bool,
    active_ref: Option<String>,
    added: Vec<KeyVal>,
    removed: Vec<String>,
    changed: Vec<KeyVal>,
}

#[derive(Debug, Serialize)]
struct KeyVal {
    key: String,
    value: String,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let prep = open_project(ctx, LockMode::Shared)?;
    // status touches .checkout/ semantics, so it's in the sweep set per D5.
    let _ = sweep_historical_checkouts(
        &prep.vault,
        &prep.repo,
        prep.project_id(),
        &prep.project_root,
        &prep.manifest.target_filename,
    );

    if prep.manifest.active.is_empty() {
        // No active env — nothing to report. Return success with a single
        // human-readable line; JSON callers get an empty active.
        match ctx.format {
            OutputFormat::Human => println!("no active env"),
            OutputFormat::Json => {
                let s = serde_json::to_string(&StatusJson {
                    active: String::new(),
                    mode: "none",
                    clean: true,
                    active_ref: None,
                    added: vec![],
                    removed: vec![],
                    changed: vec![],
                })
                .map_err(|e| EnvrollError::Generic(format!("serializing status JSON: {e}")))?;
                println!("{s}");
            }
        }
        return Ok(());
    }

    let active = prep.manifest.active.clone();
    let mode_str = match prep.mode {
        Mode::Symlink => "symlink",
        Mode::Copy => "copy",
        Mode::None => "unmanaged",
        Mode::StaleOurSymlink => "stale-symlink",
        Mode::ForeignSymlink => "foreign",
    };

    // Dirty detection: parse the working copy (symlink target or ./.env in
    // copy mode) and compare against the env's last vault commit. We compare
    // against the vault — not against `.checkout/<active>` — because in
    // symlink mode the working copy IS the checkout file, so a checkout-vs-
    // working-copy diff would always show clean. The vault blob is the only
    // independent source of truth.
    let env_path = prep.dotenv_path();
    let working = match prep.mode {
        Mode::Symlink | Mode::Copy => Some(std::fs::read(&env_path).map_err(EnvrollError::Io)?),
        _ => None,
    };
    let blob_path = prep.env_blob_path(&active);
    let baseline = if blob_path.exists() {
        let pass = read_pass_and_verify(&prep, ctx)?;
        let bytes = std::fs::read(&blob_path).map_err(EnvrollError::Io)?;
        Some(crypto::decrypt(&bytes, &pass)?)
    } else {
        None
    };

    let (added, removed, changed) = match (working.as_deref(), baseline.as_deref()) {
        (Some(w), Some(b)) => diff_kv(w, b)?,
        _ => (vec![], vec![], vec![]),
    };
    let clean = added.is_empty() && removed.is_empty() && changed.is_empty();

    let active_ref_opt = if prep.manifest.active_ref.is_empty() {
        None
    } else {
        Some(prep.manifest.active_ref.clone())
    };

    match ctx.format {
        OutputFormat::Human => {
            let dirty_tag = if clean { "clean" } else { "dirty" };
            println!("active: {active} ({dirty_tag}) — {mode_str} mode");
            if let Some(ref pinned) = active_ref_opt {
                println!("pinned to historical ref: {pinned} (run `envroll save` will be refused — see envroll save --help)");
            }
            // Default: show values. `--mask` opts into masking; the legacy
            // `--show-values` is honoured to keep old scripts working.
            let show = !args.mask || args.show_values;
            for KeyVal { key, value } in &added {
                println!("+{key} {}", display_value(value, show));
            }
            for k in &removed {
                println!("-{k}");
            }
            for KeyVal { key, value } in &changed {
                println!("~{key} {}", display_value(value, show));
            }
        }
        OutputFormat::Json => {
            let payload = StatusJson {
                active,
                mode: mode_str,
                clean,
                active_ref: active_ref_opt,
                added,
                removed,
                changed,
            };
            let s = serde_json::to_string(&payload)
                .map_err(|e| EnvrollError::Generic(format!("serializing status JSON: {e}")))?;
            println!("{s}");
        }
    }
    Ok(())
}

type DiffTriple = (Vec<KeyVal>, Vec<String>, Vec<KeyVal>);

fn diff_kv(working: &[u8], baseline: &[u8]) -> Result<DiffTriple, EnvrollError> {
    let w = parser::as_key_value_map(&parser::parse_buf(working)?);
    let b = parser::as_key_value_map(&parser::parse_buf(baseline)?);

    let w_keys: BTreeSet<&String> = w.keys().collect();
    let b_keys: BTreeSet<&String> = b.keys().collect();

    let added: Vec<KeyVal> = w_keys
        .difference(&b_keys)
        .map(|k| KeyVal {
            key: (*k).clone(),
            value: w[*k].clone(),
        })
        .collect();
    let removed: Vec<String> = b_keys.difference(&w_keys).map(|k| (*k).clone()).collect();
    let changed: Vec<KeyVal> = w_keys
        .intersection(&b_keys)
        .filter(|k| w[**k] != b[**k])
        .map(|k| KeyVal {
            key: (*k).clone(),
            value: w[*k].clone(),
        })
        .collect();

    Ok((added, removed, changed))
}

fn display_value(value: &str, show: bool) -> String {
    if show {
        value.to_string()
    } else {
        "********".to_string()
    }
}
