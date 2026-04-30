//! `envroll status` — runtime mode, dirty state, pinned ref of the active env.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Print actual values instead of `***` masks. Off by default — `status`
    /// output is meant to be paste-safe in screenshots and tickets.
    #[arg(long)]
    pub show_values: bool,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll status: not yet implemented (section 10 of tasks.md)")
}
