//! `envroll sync` — pull-then-push the vault git history.
//!
//! Refuses if the vault working tree is dirty (envroll itself never leaves
//! a dirty tree; a dirty tree means the user manually edited the vault).
//!
//! Uses the libgit2 wrappers in `vault::git` (4.7) to fetch, classify the
//! relationship between local HEAD and `origin/main`, and either fast-forward
//! pull / fast-forward push / refuse on divergence (design.md D10).

use clap::Args as ClapArgs;

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

    prep.repo.fetch()?;

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
                prep.repo.fast_forward_to(r)?;
                println!("pulled {} → {}", short(l), short(r));
                Ok(())
            } else if remote_is_ancestor {
                prep.repo.push_fast_forward()?;
                println!("pushed {} → {}", short(r), short(l));
                Ok(())
            } else {
                // Divergence — emit the verbatim multi-line message from
                // design.md D10 to stderr (via the EnvrollError Display path)
                // and exit EXIT_SYNC_CONFLICT.
                eprintln!("{}", sync_conflict_message());
                Err(EnvrollError::SyncConflict)
            }
        }
        (Some(_), None) => {
            // Remote exists but has no main branch yet — push.
            prep.repo.push_fast_forward()?;
            println!("pushed (initial)");
            Ok(())
        }
        (None, Some(r)) => {
            // Local has no commits but remote does — pull.
            prep.repo.fast_forward_to(r)?;
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

/// Verbatim multi-line conflict message from design.md D10. Printed to stderr
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
