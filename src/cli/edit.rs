//! `envroll edit <name>` — open an env in `$EDITOR` with no lock held.
//!
//! Per design.md D19, the vault lock is held only during the prepare phase
//! (verify canary + ensure plaintext checkout exists + print save hint). The
//! editor itself runs UNLOCKED — its lifetime is unbounded and holding the
//! exclusive lock for it would block every other vault command.

use std::path::PathBuf;
use std::process::Command;

use clap::Args as ClapArgs;

use crate::cli::common::{open_project, read_pass_and_verify, write_checkout, LockMode};
use crate::cli::Context;
use crate::crypto;
use crate::errors::{generic, EnvrollError};
use crate::vault::Mode;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Env name to open.
    pub name: String,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    // Phase 1: under exclusive lock, verify canary and ensure the plaintext
    // checkout exists. Then drop the lock by letting `prep` go out of scope
    // before spawning the editor.
    let editor_target = {
        let mut prep = open_project(ctx, LockMode::Exclusive)?;

        let blob = prep.env_blob_path(&args.name);
        if !blob.exists() {
            return Err(EnvrollError::EnvNotFound(format!(
                "env \"{}\" not found",
                args.name
            )));
        }
        let pass = read_pass_and_verify(&prep, ctx)?;

        // Decrypt-on-demand: in copy-mode for the active env we open ./.env
        // directly (it IS the working copy). For everything else we ensure
        // the plaintext at .checkout/<name> is up to date.
        let is_active_copy_mode =
            prep.manifest.active == args.name && matches!(prep.mode, Mode::Copy);

        let target: PathBuf = if is_active_copy_mode {
            prep.project_root.join(".env")
        } else {
            let plaintext =
                crypto::decrypt(&std::fs::read(&blob).map_err(EnvrollError::Io)?, &pass)?;
            write_checkout(&prep, &args.name, &plaintext)?;
            // Keep `prep` alive only as long as we need it; capture the path
            // by value before dropping.
            let p = prep.checkout_path(&args.name);
            // Re-bind to a path with no ties to `prep`.
            let _ = &mut prep; // silence "unused mut" if compiler decides so
            p
        };

        eprintln!("(run `envroll save` to commit your changes)");
        target
    }; // <- exclusive lock dropped here, before spawning the editor.

    spawn_editor(&editor_target)
}

fn spawn_editor(path: &std::path::Path) -> Result<(), EnvrollError> {
    let editor = pick_editor()?;
    let status = Command::new(&editor)
        .arg(path)
        .status()
        .map_err(|e| generic(format!("failed to spawn editor `{editor}`: {e}")))?;
    if !status.success() {
        return Err(generic(format!(
            "editor `{editor}` exited with status {status}"
        )));
    }
    Ok(())
}

fn pick_editor() -> Result<String, EnvrollError> {
    if let Ok(v) = std::env::var("EDITOR") {
        if !v.trim().is_empty() {
            return Ok(v);
        }
    }
    if let Ok(v) = std::env::var("VISUAL") {
        if !v.trim().is_empty() {
            return Ok(v);
        }
    }
    #[cfg(windows)]
    {
        return Ok("notepad".to_string());
    }
    #[cfg(not(windows))]
    {
        // vi is universally present on POSIX systems; vim is the common upgrade.
        for candidate in ["vi", "vim"] {
            if which(candidate).is_some() {
                return Ok(candidate.to_string());
            }
        }
        Err(generic(
            "$EDITOR is not set and no fallback (vi/vim) is on PATH",
        ))
    }
}

#[cfg(not(windows))]
fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}
