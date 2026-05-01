//! `envroll rm <name>` — remove an env. Confirmation gated by `--yes`.

use std::io::{self, BufRead, IsTerminal, Write};

use clap::Args as ClapArgs;

use crate::cli::common::{clear_dotenv, missing_existing_env_error, open_project, LockMode};
use crate::cli::Context;
use crate::errors::EnvrollError;
use crate::vault::fs as vfs;
use crate::vault::sweep_historical_checkouts;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Env name to remove.
    #[arg(value_name = "ENV")]
    pub name: Option<String>,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let mut prep = open_project(ctx, LockMode::Exclusive)?;
    let _ = sweep_historical_checkouts(
        &prep.vault,
        &prep.repo,
        prep.project_id(),
        &prep.project_root,
    );

    let name = match args.name {
        Some(n) => n,
        None => return Err(missing_existing_env_error(&prep, "envroll rm <ENV>")),
    };

    let blob = prep.env_blob_path(&name);
    if !blob.exists() {
        return Err(EnvrollError::EnvNotFound(format!(
            "env \"{name}\" not found"
        )));
    }

    if !ctx.yes && !confirm_rm(&name)? {
        // User declined; don't change anything.
        return Ok(());
    }

    // Delete the encrypted blob.
    std::fs::remove_file(&blob).map_err(EnvrollError::Io)?;

    // Sweep matching checkout files: `<name>` and `<name>@<hash>`.
    let prefix_at = format!("{name}@");
    if let Ok(entries) = std::fs::read_dir(prep.checkout_dir()) {
        for entry in entries.flatten() {
            if let Some(stem) = entry.file_name().to_str() {
                if stem == name || stem.starts_with(&prefix_at) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    let was_active = prep.manifest.active == name;
    if was_active {
        prep.manifest.active = String::new();
        prep.manifest.active_ref = String::new();
        clear_dotenv(&prep.project_root)?;
    }

    let toml = prep.manifest.to_toml()?;
    vfs::atomic_write(&prep.manifest_path, toml.as_bytes(), 0o644)?;

    let msg = format!("rm {name}");
    prep.repo.commit_paths(
        &[&prep.env_blob_relpath(&name), &prep.manifest_relpath()],
        &msg,
    )?;

    println!("removed {name}");
    Ok(())
}

fn confirm_rm(name: &str) -> Result<bool, EnvrollError> {
    let mut stderr = io::stderr();
    write!(stderr, "Remove env \"{name}\"? [y/N]: ").map_err(EnvrollError::Io)?;
    stderr.flush().map_err(EnvrollError::Io)?;

    if !io::stdin().is_terminal() {
        // Non-interactive without --yes: refuse to silently destroy.
        return Ok(false);
    }
    let mut answer = String::new();
    io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(EnvrollError::Io)?;
    let trimmed = answer.trim().to_ascii_lowercase();
    Ok(trimmed == "y" || trimmed == "yes")
}
