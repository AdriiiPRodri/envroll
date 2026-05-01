//! `envroll log <name>` — commit history for an env, newest-first.
//!
//! Filename is `log_cmd.rs` to avoid colliding with the popular `log` crate
//! if it ever gets pulled in transitively.
//!
//! Walks libgit2 history for commits touching `envs/<name>.age`, then for
//! each adjacent pair decrypts both blobs and computes a `+N -M ~K` summary.
//! Acquires a shared lock per design.md D15.

use std::collections::BTreeSet;

use clap::Args as ClapArgs;
use serde::Serialize;

use crate::cli::common::{
    missing_existing_env_error, open_project, read_pass_and_verify, short_oid_12, LockMode,
};
use crate::cli::Context;
use crate::crypto;
use crate::errors::EnvrollError;
use crate::output::OutputFormat;
use crate::parser;
use crate::vault::sweep_historical_checkouts;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Env name whose history to show. If omitted, envroll lists the envs in
    /// this project and exits non-zero so scripts can detect the missing arg.
    #[arg(value_name = "ENV")]
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
struct Entry {
    hash: String,
    added: usize,
    removed: usize,
    changed: usize,
    message: String,
    timestamp: String,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let prep = open_project(ctx, LockMode::Shared)?;
    let _ = sweep_historical_checkouts(
        &prep.vault,
        &prep.repo,
        prep.project_id(),
        &prep.project_root,
    );

    let name = match args.name {
        Some(n) => n,
        None => return Err(missing_existing_env_error(&prep, "envroll log <ENV>")),
    };

    if !prep.env_blob_path(&name).exists() {
        return Err(EnvrollError::EnvNotFound(format!(
            "env \"{name}\" not found"
        )));
    }

    let scope = prep.repo.project(prep.project_id());
    let history = scope.commit_history(&name)?;
    if history.is_empty() {
        // Should be unreachable given the env-blob-exists check above, but
        // emitting a clean empty result is safer than panicking.
        if matches!(ctx.format, OutputFormat::Json) {
            println!("[]");
        }
        return Ok(());
    }

    let pass = read_pass_and_verify(&prep, ctx)?;

    // Open libgit2 once and reuse so we don't re-discover the repo per commit.
    let repo = git2::Repository::open(prep.vault.root())
        .map_err(|e| EnvrollError::Generic(format!("libgit2: {e}")))?;
    let blob_relpath = format!("projects/{}/envs/{}.age", prep.project_id(), &name);

    let mut entries: Vec<Entry> = Vec::with_capacity(history.len());
    for (i, oid) in history.iter().enumerate() {
        let commit = repo
            .find_commit(*oid)
            .map_err(|e| EnvrollError::Generic(format!("libgit2 find_commit: {e}")))?;
        let cur_bytes = read_blob_at(&repo, &commit, &blob_relpath)?;
        let cur_kv =
            parser::as_key_value_map(&parser::parse_buf(&crypto::decrypt(&cur_bytes, &pass)?)?);

        let (added, removed, changed) = if i + 1 < history.len() {
            let prev_oid = history[i + 1];
            let prev_commit = repo
                .find_commit(prev_oid)
                .map_err(|e| EnvrollError::Generic(format!("libgit2 find_commit: {e}")))?;
            let prev_bytes = read_blob_at(&repo, &prev_commit, &blob_relpath).unwrap_or_default();
            let prev_kv = if prev_bytes.is_empty() {
                std::collections::BTreeMap::new()
            } else {
                parser::as_key_value_map(&parser::parse_buf(&crypto::decrypt(&prev_bytes, &pass)?)?)
            };
            kv_summary(&cur_kv, &prev_kv)
        } else {
            // First-ever commit on this env — every key is "added".
            (cur_kv.len(), 0, 0)
        };

        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(commit.time().seconds(), 0)
            .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_default();

        entries.push(Entry {
            hash: short_oid_12(*oid),
            added,
            removed,
            changed,
            message: commit.message().unwrap_or("").to_string(),
            timestamp,
        });
    }

    match ctx.format {
        OutputFormat::Human => print_human(&entries),
        OutputFormat::Json => {
            let s = serde_json::to_string(&entries)
                .map_err(|e| EnvrollError::Generic(format!("serializing log JSON: {e}")))?;
            println!("{s}");
        }
    }
    Ok(())
}

/// Render the log entries as an aligned table with column headers.
///
/// Layout: `HASH  CHANGES   TIMESTAMP            MESSAGE`. The `CHANGES`
/// column packs `+N -M ~K` into a fixed width so the timestamp and message
/// always start at the same column even when the counts vary by digit count.
fn print_human(entries: &[Entry]) {
    if entries.is_empty() {
        return;
    }
    // Hash is fixed at 12 hex chars; the columns we still need to size are
    // the change-summary (max width across all entries) and the timestamp
    // (constant 20 chars for `YYYY-MM-DDTHH:MM:SSZ`).
    let change_strings: Vec<String> = entries
        .iter()
        .map(|e| format!("+{} -{} ~{}", e.added, e.removed, e.changed))
        .collect();
    let change_w = change_strings.iter().map(String::len).max().unwrap_or(7).max(7);
    let hash_w = 12;
    let ts_w = 20;

    println!(
        "{:<hash_w$}  {:<change_w$}  {:<ts_w$}  MESSAGE",
        "HASH",
        "CHANGES",
        "TIMESTAMP",
        hash_w = hash_w,
        change_w = change_w,
        ts_w = ts_w,
    );
    for (e, changes) in entries.iter().zip(change_strings.iter()) {
        println!(
            "{:<hash_w$}  {:<change_w$}  {:<ts_w$}  {}",
            e.hash,
            changes,
            e.timestamp,
            e.message,
            hash_w = hash_w,
            change_w = change_w,
            ts_w = ts_w,
        );
    }
}

fn read_blob_at(
    repo: &git2::Repository,
    commit: &git2::Commit<'_>,
    relpath: &str,
) -> Result<Vec<u8>, EnvrollError> {
    let tree = commit
        .tree()
        .map_err(|e| EnvrollError::Generic(format!("libgit2 tree: {e}")))?;
    let entry = match tree.get_path(std::path::Path::new(relpath)) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };
    let object = entry
        .to_object(repo)
        .map_err(|e| EnvrollError::Generic(format!("libgit2 to_object: {e}")))?;
    let blob = object
        .as_blob()
        .ok_or_else(|| EnvrollError::Generic("expected blob, got tree".to_string()))?;
    Ok(blob.content().to_vec())
}

fn kv_summary(
    cur: &std::collections::BTreeMap<String, String>,
    prev: &std::collections::BTreeMap<String, String>,
) -> (usize, usize, usize) {
    let cur_keys: BTreeSet<&String> = cur.keys().collect();
    let prev_keys: BTreeSet<&String> = prev.keys().collect();
    let added = cur_keys.difference(&prev_keys).count();
    let removed = prev_keys.difference(&cur_keys).count();
    let changed = cur_keys
        .intersection(&prev_keys)
        .filter(|k| cur[**k] != prev[**k])
        .count();
    (added, removed, changed)
}
