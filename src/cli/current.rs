//! `envroll current` — print the active env name (lock-free, manifest-only).
//!
//! Per design.md D15, this command takes NO vault lock — it only reads
//! `manifest.toml`, which is always written via tempfile+rename so a torn
//! read is impossible.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;
use crate::manifest::find_project_for_cwd;
use crate::paths::resolve_vault_root;
use crate::vault::Vault;

#[derive(Debug, ClapArgs)]
pub struct Args {}

pub fn run(_args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let vault_root = resolve_vault_root(ctx.vault.as_deref())?;
    let vault = Vault::open(&vault_root)?;
    let cwd = std::env::current_dir().map_err(EnvrollError::Io)?;
    let manifest = find_project_for_cwd(&vault, &cwd)?;

    if manifest.active.is_empty() {
        // env-management spec: "No active env set" exits 31.
        return Err(EnvrollError::UnmanagedEnvPresent(
            "no active env".to_string(),
        ));
    }
    println!("{}", manifest.active);
    Ok(())
}
