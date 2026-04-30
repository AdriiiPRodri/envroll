//! `envroll copy <KEY> --from <a> --to <b>` — copy a single key between envs.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Key to copy.
    pub key: String,

    /// Source env.
    #[arg(long, value_name = "ENV")]
    pub from: String,

    /// Destination env. `--from` and `--to` must differ (exit 2 otherwise).
    #[arg(long, value_name = "ENV")]
    pub to: String,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll copy: not yet implemented (section 12 of tasks.md)")
}
