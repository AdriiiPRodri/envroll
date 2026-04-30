//! `envroll init` — initialize the vault (first run) and/or register this directory.

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::errors::EnvrollError;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Override project ID derivation. Use when the auto-derived ID would
    /// collide (monorepo subdirs sharing an origin) or when reattaching a
    /// renamed project (design.md D1).
    #[arg(long, value_name = "ID")]
    pub id: Option<String>,

    /// Optional override for the persisted `id_input` field (e.g., the
    /// origin URL string when `id_source = "manual"`). Rare.
    #[arg(long, value_name = "STRING")]
    pub id_input: Option<String>,

    /// Verify the vault passphrase by decrypting the canary, then exit.
    /// Does not register a project. Exits 10 on a wrong passphrase
    /// (design.md D20).
    #[arg(long)]
    pub verify_passphrase: bool,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    unimplemented!("envroll init: not yet implemented (section 8 of tasks.md)")
}
