//! `envroll use <ref>` — atomically activate an env via symlink swap.
//!
//! Filename is `use_cmd.rs` because `use` is a reserved keyword in Rust;
//! the CLI subcommand name is still spelled `use` via `#[command(name = "use")]`
//! on the enum variant.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Ref to activate: `<name>`, `<name>@<short-hash>` (>= 7 hex chars),
    /// or `<name>@~N` (1-based offset).
    pub reference: String,

    /// Overwrite a foreign or unmanaged `./.env` without rescuing it.
    /// Loses the foreign content irrevocably — prefer `--rescue` instead.
    #[arg(long)]
    pub force: bool,

    /// Save the existing `./.env` as `<name>` first, then activate the
    /// originally-requested ref. Calls the same code path as `fork`
    /// (design.md D3).
    #[arg(long, value_name = "NAME")]
    pub rescue: Option<String>,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll use: not yet implemented (section 10 of tasks.md)")
}
