//! `envroll exec <ref> -- <cmd> [args...]` — run a command with env vars injected.
//!
//! Decrypts to memory only; no plaintext touches disk. The vault's shared
//! lock is released before `cmd` is spawned (the child can run for hours;
//! same reasoning as `edit` per design.md D15).

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Ref whose vars to inject.
    pub reference: String,

    /// When set, parent-shell env vars override the env's vars on key
    /// collision. Default is the env wins (override-on).
    #[arg(long)]
    pub no_override: bool,

    /// The command to run, plus its arguments. Everything after `--` ends up here.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1..)]
    pub cmd: Vec<String>,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll exec: not yet implemented (section 13 of tasks.md)")
}
