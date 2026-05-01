//! `envroll remote {set,show,unset}` — configure the optional sync remote.
//!
//! `set` validates the URL scheme; no network call is made. `show` exits 40
//! if no remote is configured. `unset` is idempotent.

use clap::Subcommand;

use crate::cli::common::{open_project, LockMode};
use crate::cli::Context;
use crate::errors::{generic, EnvrollError};

#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    /// Set the sync remote URL. URL is validated for scheme (`https://`,
    /// `http://`, `ssh://`, `git@host:owner/repo`, `file://`). No network
    /// call is made.
    Set {
        /// Remote URL.
        url: String,
    },

    /// Print the configured remote URL, or exit 40 if none is set.
    Show,

    /// Remove the configured remote.
    Unset,
}

pub fn run(cmd: RemoteCommand, ctx: &Context) -> Result<(), EnvrollError> {
    match cmd {
        RemoteCommand::Set { url } => {
            validate_remote_url(&url)?;
            let prep = open_project(ctx, LockMode::Exclusive)?;
            prep.repo.remote_set(&url)?;
            println!("remote set to {url}");
            Ok(())
        }
        RemoteCommand::Show => {
            let prep = open_project(ctx, LockMode::Shared)?;
            match prep.repo.remote_show()? {
                Some(url) => {
                    println!("{url}");
                    Ok(())
                }
                None => Err(EnvrollError::NoRemote),
            }
        }
        RemoteCommand::Unset => {
            let prep = open_project(ctx, LockMode::Exclusive)?;
            prep.repo.remote_unset()?;
            println!("remote unset");
            Ok(())
        }
    }
}

/// Validate that `url` matches one of the supported scheme shapes. Anything
/// else is a usage-class error so users do not waste a network round-trip on
/// a typo'd URL.
fn validate_remote_url(url: &str) -> Result<(), EnvrollError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(generic("remote URL must not be empty"));
    }
    let ok = trimmed.starts_with("https://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("ssh://")
        || trimmed.starts_with("file://")
        || trimmed.starts_with("git://")
        || is_scp_like(trimmed);
    if !ok {
        return Err(generic(format!(
            "unsupported remote URL: {trimmed}\n\
             expected one of: https://, http://, ssh://, file://, git://, or git@host:owner/repo"
        )));
    }
    Ok(())
}

/// Match the SCP-shorthand form `git@host:owner/repo[.git]`.
fn is_scp_like(url: &str) -> bool {
    if !url.starts_with("git@") {
        return false;
    }
    match url.split_once(':') {
        Some((_, path)) => !path.is_empty(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_supported_schemes() {
        for url in [
            "https://github.com/o/r.git",
            "http://example.com/r",
            "ssh://git@github.com/o/r",
            "git://git.example.com/r",
            "file:///tmp/r.git",
            "git@github.com:o/r.git",
        ] {
            assert!(validate_remote_url(url).is_ok(), "rejected {url}");
        }
    }

    #[test]
    fn rejects_unsupported() {
        assert!(validate_remote_url("ftp://example.com/r").is_err());
        assert!(validate_remote_url("").is_err());
        assert!(validate_remote_url("github.com:o/r").is_err());
    }
}
