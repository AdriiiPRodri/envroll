//! `envroll projects` — list every envroll project on this machine (lock-free).

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll projects: not yet implemented (section 8 of tasks.md)")
}
