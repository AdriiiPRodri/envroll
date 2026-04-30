//! `envroll diff <a> <b>` — key-level diff between two refs.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Left-hand ref (a `<name>`, `<name>@<short-hash>`, or `<name>@~N`).
    pub a: String,

    /// Right-hand ref.
    pub b: String,

    /// Print actual values instead of `***` masks.
    #[arg(long)]
    pub show_values: bool,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll diff: not yet implemented (section 11 of tasks.md)")
}
