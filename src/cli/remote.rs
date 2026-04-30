//! `envroll remote {set,show,unset}` — configure the optional sync remote.

use clap::Subcommand;

use crate::cli::Context;
use crate::errors::EnvrollError;

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

pub fn run(_cmd: RemoteCommand, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll remote: not yet implemented (section 14 of tasks.md)")
}
