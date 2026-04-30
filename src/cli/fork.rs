//! `envroll fork <new-name>` — canonical creation verb (design.md D6).
//!
//! Two modes, selected at runtime:
//! - Active env exists → fork its working copy as `<new-name>`.
//! - No active env, `./.env` exists → bootstrap `./.env` as `<new-name>`.
//! - Neither → refuse with a clear message.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Name of the new env to create.
    pub name: String,

    /// Commit message. Defaults vary by mode (`fork from <active> at <ts>`
    /// or `initial save of ./.env as <name>`).
    #[arg(short = 'm', long = "message", value_name = "MSG")]
    pub message: Option<String>,

    /// Overwrite an existing env with the same name. Without this flag,
    /// a name collision exits 30.
    #[arg(long)]
    pub force: bool,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll fork: not yet implemented (section 9 of tasks.md)")
}
