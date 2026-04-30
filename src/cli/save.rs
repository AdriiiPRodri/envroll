//! `envroll save` — save the working copy to the active env.
//!
//! Note: there is NO `save <name>` form. `fork <name>` is the canonical
//! creation verb (design.md D6). A positional name argument is rejected
//! at parse time by clap.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Commit message. Defaults to an ISO 8601 timestamp.
    #[arg(short = 'm', long = "message", value_name = "MSG")]
    pub message: Option<String>,

    /// When the active env is pinned to a historical ref (`active_ref` set),
    /// `save` refuses by default. `--force` deliberately rewinds to a new
    /// tip from the historical content (design.md D18).
    #[arg(long)]
    pub force: bool,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll save: not yet implemented (section 9 of tasks.md)")
}
