//! `envroll rename <old> <new>` — libgit2 file-rename so history follows.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Existing env name.
    pub old: String,

    /// New env name.
    pub new: String,

    /// Overwrite an existing env at `<new>`. Without this flag, a name
    /// collision exits 30.
    #[arg(long)]
    pub force: bool,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll rename: not yet implemented (section 9 of tasks.md)")
}
