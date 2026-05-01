//! `envroll exec <ref> -- <cmd> [args...]` — run a command with env vars injected.
//!
//! Decrypts to memory only; no plaintext touches disk. The vault's shared
//! lock is released before `cmd` is spawned (the child can run for hours;
//! same reasoning as `edit` per design.md D15).

use std::collections::BTreeMap;

use clap::Args as ClapArgs;

use crate::cli::common::{
    missing_existing_env_error, open_project, parse_ref, read_pass_and_verify, LockMode,
};
use crate::cli::Context;
use crate::crypto;
use crate::errors::{generic, EnvrollError};
use crate::parser;
use crate::vault::git::RefForm;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Ref whose vars to inject.
    #[arg(value_name = "REF")]
    pub reference: Option<String>,

    /// When set, parent-shell env vars override the env's vars on key
    /// collision. Default is the env wins (override-on).
    #[arg(long)]
    pub no_override: bool,

    /// The command to run, plus its arguments. Everything after `--` ends up here.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    pub cmd: Vec<String>,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    // Phase 1: shared lock, decrypt to memory, build the env map. Drop the
    // PreparedProject (and the lock) before spawning the child so a
    // long-running child does not block other vault commands.
    let env_map = {
        let prep = open_project(ctx, LockMode::Shared)?;
        let reference = match args.reference.as_deref() {
            Some(r) => r.to_string(),
            None => {
                return Err(missing_existing_env_error(
                    &prep,
                    "envroll exec <REF> -- <cmd> [args...]",
                ));
            }
        };
        if args.cmd.is_empty() {
            return Err(generic(
                "no command given.\nusage: envroll exec <REF> -- <cmd> [args...]",
            ));
        }
        let (env_name, ref_form) = parse_ref(&reference)?;
        if !prep.env_blob_path(&env_name).exists() {
            return Err(EnvrollError::EnvNotFound(format!(
                "env \"{env_name}\" not found"
            )));
        }
        let pass = read_pass_and_verify(&prep, ctx)?;
        let bytes = match ref_form {
            RefForm::Latest => {
                std::fs::read(prep.env_blob_path(&env_name)).map_err(EnvrollError::Io)?
            }
            _ => {
                let scope = prep.repo.project(prep.project_id());
                let oid = scope.resolve_ref(&env_name, ref_form)?;
                read_blob_at(prep.vault.root(), prep.project_id(), &env_name, oid)?
            }
        };
        let plaintext = crypto::decrypt(&bytes, &pass)?;
        parser::as_key_value_map(&parser::parse_buf(&plaintext)?)
    }; // shared lock released here

    let merged = merge_with_parent_env(&env_map, args.no_override);

    let program = &args.cmd[0];
    let argv = &args.cmd[1..];

    spawn_child(program, argv, &merged)
}

fn merge_with_parent_env(
    env_map: &BTreeMap<String, String>,
    no_override: bool,
) -> Vec<(String, String)> {
    // Default precedence: env wins over parent shell. With --no-override the
    // parent shell wins.
    let mut merged: BTreeMap<String, String> = std::env::vars().collect();
    for (k, v) in env_map {
        if no_override && merged.contains_key(k) {
            continue;
        }
        merged.insert(k.clone(), v.clone());
    }
    merged.into_iter().collect()
}

#[cfg(unix)]
fn spawn_child(
    program: &str,
    argv: &[String],
    env: &[(String, String)],
) -> Result<(), EnvrollError> {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(program);
    cmd.args(argv);
    cmd.env_clear();
    for (k, v) in env {
        cmd.env(k, v);
    }
    // exec replaces this process — only returns if the spawn failed.
    let err = cmd.exec();
    Err(spawn_err(program, err))
}

#[cfg(not(unix))]
fn spawn_child(
    program: &str,
    argv: &[String],
    env: &[(String, String)],
) -> Result<(), EnvrollError> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(argv);
    cmd.env_clear();
    for (k, v) in env {
        cmd.env(k, v);
    }
    let status = cmd.status().map_err(|e| spawn_err(program, e))?;
    let code = status.code().unwrap_or(1);
    std::process::exit(code);
}

fn spawn_err(program: &str, err: std::io::Error) -> EnvrollError {
    // Spec 13.3 calls out the `NotFound` case explicitly. The error message
    // ends up identical in both branches today (the underlying `err`'s
    // Display is descriptive enough), but we keep the predicate so it's
    // obvious where the spec-mandated branch lives if we ever diverge.
    let _ = err.kind() == std::io::ErrorKind::NotFound;
    generic(format!("failed to spawn \"{program}\": {err}"))
}

fn read_blob_at(
    vault_root: &std::path::Path,
    project_id: &str,
    env_name: &str,
    oid: git2::Oid,
) -> Result<Vec<u8>, EnvrollError> {
    let repo = git2::Repository::open(vault_root)
        .map_err(|e| EnvrollError::Generic(format!("libgit2: {e}")))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|e| EnvrollError::Generic(format!("libgit2 find_commit: {e}")))?;
    let tree = commit
        .tree()
        .map_err(|e| EnvrollError::Generic(format!("libgit2 tree: {e}")))?;
    let relpath = format!("projects/{project_id}/envs/{env_name}.age");
    let entry = tree
        .get_path(std::path::Path::new(&relpath))
        .map_err(|e| EnvrollError::Generic(format!("blob missing at commit: {e}")))?;
    let object = entry
        .to_object(&repo)
        .map_err(|e| EnvrollError::Generic(format!("libgit2 to_object: {e}")))?;
    let blob = object
        .as_blob()
        .ok_or_else(|| EnvrollError::Generic("expected blob".to_string()))?;
    Ok(blob.content().to_vec())
}
