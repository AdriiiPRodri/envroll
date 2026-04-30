//! `envroll get <KEY>` — print a single key's value to stdout.
//!
//! Script-friendly: never masked, single trailing `\n`. Exits 20 if the key
//! is missing.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Key to print.
    pub key: String,

    /// Read from this env instead of the active one.
    #[arg(long, value_name = "ENV")]
    pub from: Option<String>,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll get: not yet implemented (section 12 of tasks.md)")
}
