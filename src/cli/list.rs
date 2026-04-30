//! `envroll list` (alias `ls`) — list envs in the current project.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Include envs from every registered project, not just the current one.
    #[arg(long)]
    pub all: bool,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll list: not yet implemented (section 9 of tasks.md)")
}
