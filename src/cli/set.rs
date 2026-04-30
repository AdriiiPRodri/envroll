//! `envroll set <KEY=value>` — set or update a single key.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// `KEY=value` literal. Anything that doesn't parse as `K=V` is a
    /// usage error (exit 2).
    pub assignment: String,

    /// Write into this env instead of the active one.
    #[arg(long, value_name = "ENV")]
    pub r#in: Option<String>,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll set: not yet implemented (section 12 of tasks.md)")
}
