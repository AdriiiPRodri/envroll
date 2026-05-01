//! `envroll use <ref>` — atomically activate an env via symlink swap.
//!
//! Filename is `use_cmd.rs` because `use` is a reserved keyword in Rust;
//! the CLI subcommand name is still spelled `use` via `#[command(name = "use")]`
//! on the enum variant.
//!
//! Covers section 10.1 – 10.5:
//! - latest activation (no `@`),
//! - historical activation (`<name>@<short-hash>` / `<name>@~N`),
//! - `--rescue <name>` companion (shares the `create_env_from_path` helper),
//! - foreign-symlink / unmanaged-`./.env` refusal (with `--force` override),
//! - copy-mode (`ENVROLL_USE_COPY=1` and Windows fallback).

use clap::Args as ClapArgs;

use crate::cli::common::{
    activate_dotenv, create_env_from_path, missing_existing_env_error, open_project, parse_ref,
    read_pass_and_verify, short_oid_12, write_checkout, LockMode,
};
use crate::cli::Context;
use crate::crypto;
use crate::errors::EnvrollError;
use crate::paths::project_checkout_at;
use crate::vault::git::RefForm;
use crate::vault::{sweep_historical_checkouts, Mode};

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Ref to activate: `<name>`, `<name>@<short-hash>` (>= 7 hex chars),
    /// or `<name>@~N` (1-based offset). If omitted, envroll lists the envs
    /// in this project so you can pick one.
    #[arg(value_name = "REF")]
    pub reference: Option<String>,

    /// Overwrite a foreign or unmanaged `./.env` without rescuing it.
    /// Loses the foreign content irrevocably — prefer `--rescue` instead.
    #[arg(long)]
    pub force: bool,

    /// Save the existing `./.env` as `<name>` first, then activate the
    /// originally-requested ref. Calls the same code path as `fork`
    ///.
    #[arg(long, value_name = "NAME")]
    pub rescue: Option<String>,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let mut prep = open_project(ctx, LockMode::Exclusive)?;
    let _ = sweep_historical_checkouts(
        &prep.vault,
        &prep.repo,
        prep.project_id(),
        &prep.project_root,
    );

    let reference = match args.reference {
        Some(r) => r,
        None => {
            return Err(missing_existing_env_error(
                &prep,
                "envroll use <REF>  (REF = <name> | <name>@<hash> | <name>@~N)",
            ));
        }
    };
    let (env_name, ref_form) = parse_ref(&reference)?;

    // The blob must exist for the requested env regardless of ref form.
    if !prep.env_blob_path(&env_name).exists() {
        return Err(EnvrollError::EnvNotFound(format!(
            "env \"{env_name}\" not found"
        )));
    }

    // Pre-flight ./.env state / env-switching spec.
    // Stale-our-symlink is recoverable (we'll overwrite via atomic rename).
    // Foreign symlinks and regular files refuse without --force / --rescue.
    let needs_overwrite_consent = matches!(prep.mode, Mode::ForeignSymlink | Mode::Copy);
    if needs_overwrite_consent && !args.force && args.rescue.is_none() {
        return Err(EnvrollError::UnmanagedEnvPresent(refuse_message(prep.mode)));
    }

    let pass = read_pass_and_verify(&prep, ctx)?;

    // --rescue <name>: save existing ./.env first via the shared helper, then
    // proceed with normal activation of the original ref.
    if let Some(rescue_name) = args.rescue.as_deref() {
        let env_path = prep.project_root.join(".env");
        let rescue_bytes = std::fs::read(&env_path).map_err(EnvrollError::Io)?;
        // Sanity-parse to surface bad .env early (matches fork/save behavior).
        crate::parser::parse_buf(&rescue_bytes)?;
        let default_msg = format!("rescue ./.env as {rescue_name}");
        create_env_from_path(
            &mut prep,
            rescue_name,
            &rescue_bytes,
            &pass,
            &default_msg,
            None,
            args.force,
        )?;
        // create_env_from_path retargets ./.env at the rescue env. We're about
        // to retarget again at the requested env, so this is fine — but the
        // intermediate state is correct in case the user CTRL-Cs here.
    }

    // Resolve the ref to a concrete commit OID + decide which checkout file
    // backs it.
    let scope = prep.repo.project(prep.project_id());
    let commit_oid = scope.resolve_ref(&env_name, ref_form.clone())?;

    let (checkout_path, is_historical, short_hash) = match ref_form {
        RefForm::Latest => (prep.checkout_path(&env_name), false, String::new()),
        RefForm::ShortHash(_) | RefForm::Offset(_) => {
            let hash = short_oid_12(commit_oid);
            let p = project_checkout_at(prep.vault.root(), prep.project_id(), &env_name, &hash);
            (p, true, hash)
        }
    };

    // Decrypt: latest activation reads the env's blob; historical activation
    // pulls the blob OID from the resolved commit's tree so old content can
    // be replayed even after the env's tip has moved on.
    let plaintext = if is_historical {
        crypto::decrypt(&blob_bytes_at(&prep, &env_name, commit_oid)?, &pass)?
    } else {
        let blob = std::fs::read(prep.env_blob_path(&env_name)).map_err(EnvrollError::Io)?;
        crypto::decrypt(&blob, &pass)?
    };

    // Write the (possibly historical) checkout file. write_checkout always
    // targets `.checkout/<name>`; we use the lower-level helper for the
    // historical case so we land at `.checkout/<name>@<hash>` instead.
    if is_historical {
        write_checkout_at(&prep, &checkout_path, &plaintext)?;
    } else {
        write_checkout(&prep, &env_name, &plaintext)?;
    }

    // Atomic ./.env retarget. activate_dotenv already honors ENVROLL_USE_COPY
    // and falls back to copy on Windows symlink failure (10.5).
    activate_dotenv(&prep.project_root, &checkout_path, false)?;

    // Update manifest: set active, set/clear active_ref accordingly. Then
    // commit so a synced vault reflects the change.
    prep.manifest.active = env_name.clone();
    prep.manifest.active_ref = if is_historical {
        format!("{env_name}@{short_hash}")
    } else {
        String::new()
    };
    let msg = if is_historical {
        format!("use {env_name}@{short_hash}")
    } else {
        format!("use {env_name}")
    };
    prep.save_and_commit_manifest(&msg)?;

    println!("now using {env_name}");
    Ok(())
}

/// Message printed when `./.env` is foreign or unmanaged and `--force` /
/// `--rescue` was not passed. Matches the env-switching spec scenarios.
fn refuse_message(mode: Mode) -> String {
    match mode {
        Mode::ForeignSymlink => "./.env is a foreign symlink (target outside envroll's vault); \
             pass --force to overwrite or --rescue <name>"
            .to_string(),
        Mode::Copy => "./.env exists and is not managed by envroll; \
             pass --force to overwrite or --rescue <name> to save it first"
            .to_string(),
        // The other modes are handled before this is reached.
        _ => "./.env is in an unsupported state".to_string(),
    }
}

/// Read the encrypted blob for `<env_name>` as it existed at `commit_oid`.
/// Used by historical activation so we replay the old ciphertext rather than
/// the current tip.
fn blob_bytes_at(
    prep: &crate::cli::common::PreparedProject,
    env_name: &str,
    commit_oid: git2::Oid,
) -> Result<Vec<u8>, EnvrollError> {
    let blob_relpath = format!("projects/{}/envs/{}.age", prep.project_id(), env_name);
    let repo = git2::Repository::open(prep.vault.root())
        .map_err(|e| EnvrollError::Generic(format!("libgit2: {e}")))?;
    let commit = repo
        .find_commit(commit_oid)
        .map_err(|e| EnvrollError::Generic(format!("libgit2 find_commit: {e}")))?;
    let tree = commit
        .tree()
        .map_err(|e| EnvrollError::Generic(format!("libgit2 tree: {e}")))?;
    let entry = tree
        .get_path(std::path::Path::new(&blob_relpath))
        .map_err(|e| {
            EnvrollError::Generic(format!(
                "no blob for {env_name} at commit {commit_oid}: {e}"
            ))
        })?;
    let object = entry
        .to_object(&repo)
        .map_err(|e| EnvrollError::Generic(format!("libgit2 to_object: {e}")))?;
    let blob = object
        .as_blob()
        .ok_or_else(|| EnvrollError::Generic("expected blob, got tree".to_string()))?;
    Ok(blob.content().to_vec())
}

/// Write `plaintext` to an explicit `.checkout/<name>@<hash>` path (mode 0600).
/// Used by historical activation; the latest path goes through `write_checkout`.
fn write_checkout_at(
    prep: &crate::cli::common::PreparedProject,
    target: &std::path::Path,
    plaintext: &[u8],
) -> Result<(), EnvrollError> {
    crate::vault::fs::ensure_dir(&prep.checkout_dir(), 0o700)?;
    crate::vault::fs::atomic_write(target, plaintext, 0o600)
}
