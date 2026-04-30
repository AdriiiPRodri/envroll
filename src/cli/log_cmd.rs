//! `envroll log <name>` — commit history for an env, newest-first.
//!
//! Filename is `log_cmd.rs` to avoid colliding with the popular `log` crate
//! if it ever gets pulled in transitively.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Env name whose history to show.
    pub name: String,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll log: not yet implemented (section 11 of tasks.md)")
}
