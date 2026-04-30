//! `envroll rm <name>` — remove an env. Confirmation gated by `--yes`.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Env name to remove.
    pub name: String,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll rm: not yet implemented (section 9 of tasks.md)")
}
