//! `envroll rename <old> <new>` — libgit2 file-rename so history follows.

use clap::Args as ClapArgs;

use crate::cli::common::{
    activate_dotenv, missing_existing_env_error, open_project, parse_active_ref_hash, LockMode,
};
use crate::cli::Context;
use crate::errors::{usage, EnvrollError};
use crate::vault::fs as vfs;
use crate::vault::sweep_historical_checkouts;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Existing env name.
    #[arg(value_name = "OLD")]
    pub old: Option<String>,

    /// New env name.
    #[arg(value_name = "NEW")]
    pub new: Option<String>,

    /// Overwrite an existing env at `<new>`. Without this flag, a name
    /// collision exits 30.
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
    );

    let (old, new) = match (args.old, args.new) {
        (Some(o), Some(n)) => (o, n),
        (None, _) => {
            return Err(missing_existing_env_error(
                &prep,
                "envroll rename <OLD> <NEW>",
            ));
        }
        (Some(_), None) => {
            return Err(usage(
                "missing new env name",
                Some("usage: envroll rename <OLD> <NEW>".to_string()),
            ));
        }
    };

    let old_blob = prep.env_blob_path(&old);
    let new_blob = prep.env_blob_path(&new);

    if !old_blob.exists() {
        return Err(EnvrollError::EnvNotFound(format!(
            "env \"{old}\" not found"
        )));
    }
    if new_blob.exists() && !args.force {
        return Err(EnvrollError::NameCollision(format!(
            "env \"{new}\" already exists; pass --force to overwrite"
        )));
    }

    // libgit2's rename detection runs on the diff so a plain rename of the
    // file on disk shows up in `git log --follow` semantics. We use std fs
    // rename (atomic) and let the next commit pick it up.
    std::fs::rename(&old_blob, &new_blob).map_err(EnvrollError::Io)?;

    // Move the latest checkout file too if it exists, so `./.env` stays valid
    // when the renamed env was active.
    let old_checkout = prep.checkout_path(&old);
    let new_checkout = prep.checkout_path(&new);
    if old_checkout.exists() {
        std::fs::rename(&old_checkout, &new_checkout).map_err(EnvrollError::Io)?;
    }

    let was_active = prep.manifest.active == old;
    let pinned_to_old = parse_active_ref_hash(&prep.manifest.active_ref).is_some()
        && prep.manifest.active_ref.starts_with(&format!("{old}@"));

    if was_active {
        prep.manifest.active = new.clone();
        // Retarget ./.env to the new checkout (absolute path).
        if new_checkout.exists() {
            activate_dotenv(&prep.project_root, &new_checkout, false)?;
        }
    }
    if pinned_to_old {
        let hash = prep
            .manifest
            .active_ref
            .split_once('@')
            .unwrap()
            .1
            .to_string();
        prep.manifest.active_ref = format!("{new}@{hash}");
    }

    let toml = prep.manifest.to_toml()?;
    vfs::atomic_write(&prep.manifest_path, toml.as_bytes(), 0o644)?;

    let msg = format!("rename {old} → {new}");
    prep.repo.commit_paths(
        &[
            &prep.env_blob_relpath(&old),
            &prep.env_blob_relpath(&new),
            &prep.manifest_relpath(),
        ],
        &msg,
    )?;

    println!("renamed {old} → {new}");
    Ok(())
}
