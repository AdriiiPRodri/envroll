//! `envroll completions <shell>` — emit shell completion scripts to stdout.
//!
//! Powered by `clap_complete`. The subcommand prints to stdout so users can
//! redirect into the appropriate file for their shell:
//!
//! ```text
//!   bash       → ~/.local/share/bash-completion/completions/envroll
//!                /usr/local/etc/bash_completion.d/envroll          (Homebrew)
//!   zsh        → ~/.zsh/completions/_envroll                       (custom $fpath)
//!                /usr/local/share/zsh/site-functions/_envroll      (Homebrew)
//!   fish       → ~/.config/fish/completions/envroll.fish
//!   powershell → $PROFILE                                          (eval'd inline)
//!   elvish     → ~/.config/elvish/lib/envroll-completions.elv
//! ```
//!
//! The command takes a vault lock of NONE — it does not touch the vault and
//! is safe to run from any directory, including outside an envroll project.

use std::io;

use clap::{Args as ClapArgs, CommandFactory, ValueEnum};
use clap_complete::{generate, Shell};

use crate::cli::{Cli, Context};
use crate::errors::EnvrollError;

/// Supported shells. Mirrors `clap_complete::Shell` so we can pass through
/// directly — but kept as our own enum so the CLI's value parser stays in
/// envroll's control (e.g., we can deprecate or rename without coupling to
/// clap_complete's exact set).
#[derive(Copy, Clone, Debug, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Elvish,
}

impl From<CompletionShell> for Shell {
    fn from(s: CompletionShell) -> Self {
        match s {
            CompletionShell::Bash => Shell::Bash,
            CompletionShell::Zsh => Shell::Zsh,
            CompletionShell::Fish => Shell::Fish,
            CompletionShell::PowerShell => Shell::PowerShell,
            CompletionShell::Elvish => Shell::Elvish,
        }
    }
}

/// Generate shell completion script for `<shell>` and print it to stdout.
///
/// Install one-liners (paste into your shell init):
///
///   # bash
///   envroll completions bash > ~/.local/share/bash-completion/completions/envroll
///
///   # zsh (with a `~/.zsh/completions` dir on $fpath)
///   envroll completions zsh > ~/.zsh/completions/_envroll
///
///   # fish
///   envroll completions fish > ~/.config/fish/completions/envroll.fish
///
///   # powershell — paste this into $PROFILE
///   envroll completions powershell | Out-String | Invoke-Expression
///
///   # elvish
///   envroll completions elvish > ~/.config/elvish/lib/envroll-completions.elv
///
/// After installing, restart your shell (or `source` the file) and you'll get
/// `<TAB>` completion for every subcommand and flag — including subcommand
/// help. Note that env-name completion (`envroll use <TAB>` listing your
/// envs) is NOT supported in v0.1.x because it would require running
/// `envroll list` synchronously inside every TAB press; that needs a
/// daemon/cache layer planned for v0.3.
#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Which shell to generate completions for.
    #[arg(value_enum)]
    pub shell: CompletionShell,
}

pub fn run(args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    // Reconstruct the clap Command from the top-level Cli derive so the
    // generator emits completions for every subcommand and flag, including
    // anything we add in the future without having to update this code.
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    let shell: Shell = args.shell.into();
    generate(shell, &mut cmd, bin_name, &mut io::stdout());
    Ok(())
}
