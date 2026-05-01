//! `envroll save` — save the working copy to the active env.
//!
//! Note: there is NO `save <name>` form. `fork <name>` is the canonical
//! creation verb. A positional name argument is rejected
//! at parse time by clap.

use clap::Args as ClapArgs;

use crate::cli::common::{
    active_ref_pinned_message, iso_now_local, open_project, parse_active_ref_hash,
    read_pass_and_verify, read_working_copy, write_checkout, write_env_blob, LockMode,
};
use crate::cli::Context;
use crate::crypto;
use crate::errors::{generic, EnvrollError};
use crate::parser;
use crate::vault::{sweep_historical_checkouts, Mode};

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Commit message. Defaults to a local-time ISO 8601 timestamp.
    #[arg(short = 'm', long = "message", value_name = "MSG")]
    pub message: Option<String>,

    /// When the active env is pinned to a historical ref (`active_ref` set),
    /// `save` refuses by default. `--force` deliberately rewinds to a new
    /// tip from the historical content.
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

    if prep.manifest.active.is_empty() {
        return Err(generic(
            "no active env (use `envroll fork <name>` to create one)",
        ));
    }

    // active_ref refuse rule: writing to a historically-pinned env would
    // silently rewind it, so we refuse unless --force is passed.
    if !prep.manifest.active_ref.is_empty() && !args.force {
        let hash = parse_active_ref_hash(&prep.manifest.active_ref)
            .unwrap_or(prep.manifest.active_ref.as_str());
        return Err(generic(active_ref_pinned_message(
            &prep.manifest.active,
            hash,
        )));
    }

    if matches!(prep.mode, Mode::ForeignSymlink) {
        return Err(EnvrollError::UnmanagedEnvPresent(
            "./.env is a foreign symlink (not managed by envroll); resolve manually".to_string(),
        ));
    }

    let working = read_working_copy(&prep)?;
    let parsed = parser::parse_buf(&working)?; // ParseError → exit 12
    let working_kv = parser::as_key_value_map(&parsed);

    // Compare against the env's current tip. If active_ref is set + --force,
    // we deliberately rewind so we still write a new commit even if the new
    // content matches the historical baseline.
    let baseline_kv = {
        let blob_path = prep.env_blob_path(&prep.manifest.active);
        if blob_path.exists() {
            let pass_for_baseline = read_pass_and_verify(&prep, ctx)?;
            let baseline_bytes = std::fs::read(&blob_path).map_err(EnvrollError::Io)?;
            let baseline_plain = crypto::decrypt(&baseline_bytes, &pass_for_baseline)?;
            let baseline_parsed = parser::parse_buf(&baseline_plain)?;
            // Stash for re-encryption later — but in the rewind case we want
            // to write regardless of equality.
            let kv = parser::as_key_value_map(&baseline_parsed);
            (Some(pass_for_baseline), kv)
        } else {
            // First save on this env: every fork has already created the blob
            // so this branch should be unreachable under normal flow. Defend
            // against it by treating as "different" so we write.
            (None, std::collections::BTreeMap::new())
        }
    };

    let (cached_pass, baseline) = baseline_kv;

    let is_rewind = !prep.manifest.active_ref.is_empty();
    if !is_rewind && parser::same_kv_set(&working_kv, &baseline) {
        eprintln!("envroll: nothing to save");
        return Ok(());
    }

    // Need a passphrase to encrypt; reuse the one we already prompted for if
    // we decrypted the baseline, otherwise prompt now.
    let pass = match cached_pass {
        Some(p) => p,
        None => read_pass_and_verify(&prep, ctx)?,
    };

    write_env_blob(&prep, &prep.manifest.active.clone(), &working, &pass)?;

    // In copy-mode the .checkout file is just a mirror that backs status/diff;
    // refresh it so the next read sees the new content.
    if matches!(prep.mode, Mode::Copy) {
        write_checkout(&prep, &prep.manifest.active.clone(), &working)?;
    }

    // Build commit message. The rewind case has a special default per D18.
    let active_name = prep.manifest.active.clone();
    let message = if let Some(m) = args.message.as_deref() {
        m.to_string()
    } else if is_rewind {
        let hash = parse_active_ref_hash(&prep.manifest.active_ref)
            .unwrap_or(prep.manifest.active_ref.as_str());
        format!(
            "rewind from {}@{} at {}",
            active_name,
            hash,
            iso_now_local()
        )
    } else {
        iso_now_local()
    };

    // After a successful rewind, clear active_ref so subsequent saves go
    // through the normal path.
    if is_rewind {
        prep.manifest.active_ref = String::new();
        let toml = prep.manifest.to_toml()?;
        crate::vault::fs::atomic_write(&prep.manifest_path, toml.as_bytes(), 0o644)?;
        prep.repo.commit_paths(
            &[
                &prep.env_blob_relpath(&active_name),
                &prep.manifest_relpath(),
            ],
            &message,
        )?;
    } else {
        prep.repo
            .commit_blob(&prep.env_blob_relpath(&active_name), &message)?;
    }

    println!("saved {active_name}");
    Ok(())
}
