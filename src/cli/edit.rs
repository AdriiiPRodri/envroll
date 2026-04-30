//! `envroll edit <name>` — open an env in `$EDITOR` with no lock held.
//!
//! See design.md D19: the vault lock is held only during the
//! decrypt-to-checkout phase; the editor runs unlocked so it does not block
//! other vault commands for hours.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Env name to open.
    pub name: String,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll edit: not yet implemented (section 9 of tasks.md)")
}
