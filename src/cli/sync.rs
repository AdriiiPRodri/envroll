//! `envroll sync` — pull-then-push the vault git history.
//!
//! Refuses if the vault working tree is dirty (envroll itself never leaves
//! a dirty tree; a dirty tree means the user manually edited the vault).

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll sync: not yet implemented (section 14 of tasks.md)")
}
