//! `envroll fork <new-name>` — canonical creation verb.
//!
//! Two modes, selected at runtime:
//! - **Active env exists**: fork its working copy as `<new-name>`.
//!   Default message: `fork from <active> at <ts>`.
//! - **No active env, `./.env` exists** (regular file or our-managed symlink):
//!   bootstrap `./.env` as `<new-name>`. Default message:
//!   `initial save of ./.env as <new-name>`.
//! - **Neither**: refuse.

use clap::Args as ClapArgs;

use crate::cli::common::{
    active_ref_pinned_message, create_env_from_path, iso_now_local, missing_new_env_error,
    open_project, read_pass_and_verify, LockMode,
};
use crate::cli::Context;
use crate::errors::{generic, EnvrollError};
use crate::parser;
use crate::vault::{sweep_historical_checkouts, Mode};

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Name of the new env to create.
    #[arg(value_name = "NAME")]
    pub name: Option<String>,

    /// Commit message. Defaults vary by mode (`fork from <active> at <ts>`
    /// or `initial save of ./.env as <name>`).
    #[arg(short = 'm', long = "message", value_name = "MSG")]
    pub message: Option<String>,

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
    );

    let name = match args.name {
        Some(n) => n,
        None => {
            return Err(missing_new_env_error(
                &prep,
                "envroll fork <NAME> [-m <msg>]",
            ));
        }
    };

    // Surface the active_ref pin pre-emptively for the active-env mode —
    // forking from a pinned env would silently rewind, same hazard as save.
    if !prep.manifest.active.is_empty() && !prep.manifest.active_ref.is_empty() {
        let hash = prep
            .manifest
            .active_ref
            .split_once('@')
            .map(|(_, h)| h)
            .unwrap_or(prep.manifest.active_ref.as_str());
        return Err(generic(active_ref_pinned_message(
            &prep.manifest.active,
            hash,
        )));
    }

    // Decide source bytes per the two modes.
    let (plaintext, default_msg) = if !prep.manifest.active.is_empty() {
        // Active env present: fork its working copy.
        let env_path = prep.project_root.join(".env");
        match prep.mode {
            Mode::Symlink | Mode::Copy => {}
            Mode::ForeignSymlink => {
                return Err(EnvrollError::UnmanagedEnvPresent(
                    "./.env is a foreign symlink (not managed by envroll); resolve manually before forking".to_string(),
                ));
            }
            Mode::StaleOurSymlink | Mode::None => {
                // Active env in manifest but no working copy on disk — read
                // from the checkout file directly (it's the canonical
                // contents per envroll's invariants).
                let checkout = prep.checkout_path(&prep.manifest.active);
                if checkout.exists() {
                    let bytes = std::fs::read(&checkout).map_err(EnvrollError::Io)?;
                    let msg = format!("fork from {} at {}", prep.manifest.active, iso_now_local());
                    let pass = read_pass_and_verify(&prep, ctx)?;
                    parser::parse_buf(&bytes)?;
                    create_env_from_path(
                        &mut prep,
                        &name,
                        &bytes,
                        &pass,
                        &msg,
                        args.message.as_deref(),
                        args.force,
                    )?;
                    print_created(&name);
                    return Ok(());
                }
                return Err(generic(
                    "active env recorded in manifest but no working copy on disk; run `envroll use <name>` first",
                ));
            }
        }
        let bytes = std::fs::read(&env_path).map_err(EnvrollError::Io)?;
        // Ensure it parses as a valid .env so a bad working copy fails fast
        // (matches the env-management spec for save's parse-error case).
        parser::parse_buf(&bytes)?;
        let msg = format!("fork from {} at {}", prep.manifest.active, iso_now_local());
        (bytes, msg)
    } else {
        // No active env. Bootstrap from ./.env if present (regular file or
        // managed symlink). Foreign symlink refuses; absent file refuses.
        match prep.mode {
            Mode::Copy | Mode::Symlink => {
                let env_path = prep.project_root.join(".env");
                let bytes = std::fs::read(&env_path).map_err(EnvrollError::Io)?;
                parser::parse_buf(&bytes)?;
                let msg = format!("initial save of ./.env as {name}");
                (bytes, msg)
            }
            Mode::None => {
                return Err(generic(
                    "no working copy to fork from; create a .env file or activate an existing env first",
                ));
            }
            Mode::StaleOurSymlink => {
                return Err(generic(
                    "./.env points into envroll's checkout dir but the target is gone — and no env is active to bootstrap from",
                ));
            }
            Mode::ForeignSymlink => {
                return Err(EnvrollError::UnmanagedEnvPresent(
                    "./.env is a foreign symlink (not managed by envroll); resolve manually before forking".to_string(),
                ));
            }
        }
    };

    let pass = read_pass_and_verify(&prep, ctx)?;
    create_env_from_path(
        &mut prep,
        &name,
        &plaintext,
        &pass,
        &default_msg,
        args.message.as_deref(),
        args.force,
    )?;
    print_created(&name);
    Ok(())
}

fn print_created(name: &str) {
    println!("forked → {name} (now active)");
}
