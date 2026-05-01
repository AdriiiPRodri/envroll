//! `envroll sync` — pull-then-push the vault git history.
//!
//! Refuses if the vault working tree is dirty (envroll itself never leaves
//! a dirty tree; a dirty tree means the user manually edited the vault).
//!
//! Uses the libgit2 wrappers in `vault::git` (4.7) to fetch, classify the
//! relationship between local HEAD and `origin/main`, and either fast-forward
//! pull / fast-forward push / refuse on divergence.

use std::io::IsTerminal;
use std::time::Duration;

use clap::Args as ClapArgs;
use indicatif::{ProgressBar, ProgressStyle};

use crate::cli::common::{open_project, LockMode};
use crate::cli::Context;
use crate::errors::{generic, EnvrollError};

#[derive(Debug, ClapArgs)]
pub struct Args {}

pub fn run(_args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let prep = open_project(ctx, LockMode::Exclusive)?;

    // Pre-flight: dirty working tree refuses BEFORE any network call.
    if prep.repo.is_working_tree_dirty()? {
        return Err(generic(format!(
            "vault working tree is dirty; resolve in {} using regular git tools before syncing",
            prep.vault.root().display()
        )));
    }

    // Confirm a remote is configured before paying for the lock contention
    // path that is `git fetch`.
    if prep.repo.remote_show()?.is_none() {
        return Err(EnvrollError::NoRemote);
    }

    let fetch_spinner = spinner("fetching from origin…");
    let fetch_result = prep.repo.fetch();
    finish_spinner(&fetch_spinner, fetch_result.is_ok());
    fetch_result?;

    let local = prep.repo.local_head()?;
    let remote = prep.repo.remote_head()?;

    match (local, remote) {
        (Some(l), Some(r)) if l == r => {
            println!("already in sync");
            Ok(())
        }
        (Some(l), Some(r)) => {
            // Classify: ff-pull, ff-push, or divergence.
            let local_is_ancestor = prep.repo.is_ancestor(l, r)?;
            let remote_is_ancestor = prep.repo.is_ancestor(r, l)?;
            if local_is_ancestor {
                let s = spinner("fast-forwarding local to remote tip…");
                let res = prep.repo.fast_forward_to(r);
                finish_spinner(&s, res.is_ok());
                res?;
                println!("pulled {} → {}", short(l), short(r));
                Ok(())
            } else if remote_is_ancestor {
                let s = spinner("pushing local to origin…");
                let res = prep.repo.push_fast_forward();
                finish_spinner(&s, res.is_ok());
                res?;
                println!("pushed {} → {}", short(r), short(l));
                Ok(())
            } else {
                // Divergence — emit the verbatim multi-line conflict
                // message to stderr and exit EXIT_SYNC_CONFLICT.
                eprintln!("{}", sync_conflict_message());
                Err(EnvrollError::SyncConflict)
            }
        }
        (Some(_), None) => {
            // Remote exists but has no main branch yet — push.
            let s = spinner("pushing local to origin (initial)…");
            let res = prep.repo.push_fast_forward();
            finish_spinner(&s, res.is_ok());
            res?;
            println!("pushed (initial)");
            Ok(())
        }
        (None, Some(r)) => {
            // Local has no commits but remote does — pull.
            let s = spinner("fast-forwarding local to remote (initial)…");
            let res = prep.repo.fast_forward_to(r);
            finish_spinner(&s, res.is_ok());
            res?;
            println!("pulled (initial) → {}", short(r));
            Ok(())
        }
        (None, None) => {
            println!("nothing to sync (no local or remote commits)");
            Ok(())
        }
    }
}

/// First 12 hex chars of an OID. Mirrors short_oid_12 in cli::common.
fn short(oid: git2::Oid) -> String {
    let s = oid.to_string();
    s[..12.min(s.len())].to_string()
}

/// Build an indicatif spinner with envroll's standard tick template, or a
/// no-op hidden spinner when stderr is not a TTY (so JSON / scripted callers
/// see clean output without the animation bytes).
fn spinner(message: &str) -> ProgressBar {
    if !std::io::stderr().is_terminal() {
        return ProgressBar::hidden();
    }
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );
    pb.set_message(message.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

/// Replace the spinner with a `✓` (success) or `✗` (failure) glyph and the
/// final message, matching modern CLI conventions (cargo, deno, gh).
fn finish_spinner(pb: &ProgressBar, ok: bool) {
    let suffix = if ok { "done" } else { "failed" };
    let glyph = if ok { "✓" } else { "✗" };
    let original = pb.message();
    pb.set_style(
        ProgressStyle::with_template(if ok {
            "{prefix:.green.bold} {msg}"
        } else {
            "{prefix:.red.bold} {msg}"
        })
        .unwrap(),
    );
    pb.set_prefix(glyph.to_string());
    pb.finish_with_message(format!("{original} {suffix}"));
}

/// Verbatim multi-line conflict message. Printed to stderr
/// before the EnvrollError::SyncConflict propagates to main; main's own
/// formatter then prints the short single-line form so the user sees both
/// the structured message and the actionable instructions.
pub fn sync_conflict_message() -> String {
    "envroll: sync conflict — local and remote vault histories have diverged.

To resolve:
  1. cd ~/.local/share/envroll/
  2. Use regular git tools (git log, git merge, git rebase, git mergetool) to reconcile.
     The encrypted .age files are git-managed; you can resolve conflicts on them
     after decrypting both sides if necessary.
  3. When the working tree is clean and the conflict is resolved:
     envroll sync

For help: envroll sync --help"
        .to_string()
}
