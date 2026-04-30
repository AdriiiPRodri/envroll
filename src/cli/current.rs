//! `envroll current` — print the active env name (lock-free, manifest-only).

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll current: not yet implemented (section 9 of tasks.md)")
}
