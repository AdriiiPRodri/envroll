//! Top-level clap parser and subcommand dispatch.
//!
//! Each subcommand lives in its own file under `src/cli/`. The pattern is:
//!
//! ```ignore
//! pub fn run(args: CmdArgs, ctx: &Context) -> Result<(), EnvrollError>
//! ```
//!
//! `Context` carries the resolved global flags (output format, color flag,
//! passphrase sources, vault override) so subcommands don't each re-derive
//! them from clap.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::errors::EnvrollError;
use crate::output::OutputFormat;

pub mod common;
pub mod completions;
pub mod copy;
pub mod current;
pub mod diff;
pub mod edit;
pub mod exec;
pub mod export;
pub mod fork;
pub mod get;
pub mod import;
pub mod init;
pub mod list;
pub mod log_cmd;
pub mod projects;
pub mod remote;
pub mod rename;
pub mod rename_key;
pub mod rm;
pub mod save;
pub mod set;
pub mod status;
pub mod sync;
pub mod use_cmd;

/// Verbosity for diagnostic logging. Mirrors `RUST_LOG` levels.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, clap::ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum LogLevel {
    #[default]
    Off,
    Error,
    Warn,
    Info,
    Debug,
}

/// Top-level CLI: global flags + a required subcommand.
///
/// `--vault <path>` is intentionally undocumented in user-facing help
/// (it is a testing escape hatch) but lives here because every subcommand
/// needs it.
#[derive(Debug, Parser)]
#[command(
    name = "envroll",
    version,
    about = "git for your .env files — local-first, encrypted, single-binary",
    long_about = None,
    propagate_version = true,
    arg_required_else_help = true,
)]
pub struct Cli {
    /// Output format for read commands. JSON output uses a stable schema
    /// documented in `docs/json-schemas/`.
    #[arg(long, value_enum, default_value_t = OutputFormat::Human, global = true)]
    pub format: OutputFormat,

    /// Auto-confirm destructive prompts. Required for `rm`, `--force`
    /// overrides, and other actions that would otherwise pause for input.
    #[arg(long, global = true)]
    pub yes: bool,

    /// Diagnostic verbosity. `debug` includes full anyhow chains.
    #[arg(long, value_enum, default_value_t = LogLevel::Off, global = true)]
    pub log: LogLevel,

    /// Disable ANSI colors in human output. Also honored: `NO_COLOR` env var.
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Read the passphrase from stdin to EOF instead of prompting.
    /// Mutually exclusive with a TTY stdin.
    #[arg(long, global = true)]
    pub passphrase_stdin: bool,

    /// Override the env-var name used as the passphrase fallback.
    /// Default: `ENVROLL_PASSPHRASE`.
    #[arg(long, value_name = "NAME", global = true)]
    pub passphrase_env: Option<String>,

    /// Override the vault root directory. Testing escape hatch — not
    /// documented in the user-facing README. Multi-vault topologies are
    /// explicitly unsupported.
    #[arg(long, value_name = "PATH", global = true, hide = true)]
    pub vault: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize the vault and register this directory as an envroll project.
    Init(init::Args),

    /// List every envroll project on this machine (lock-free).
    Projects(projects::Args),

    /// List envs in this project.
    #[command(alias = "ls")]
    List(list::Args),

    /// Print the active env name (lock-free).
    Current(current::Args),

    /// Save the current working copy to the active env.
    Save(save::Args),

    /// Create a new env from the active env (or from `./.env` when none active).
    Fork(fork::Args),

    /// Activate an env: swap `./.env` to point at the (decrypted) checkout.
    #[command(name = "use")]
    Use(use_cmd::Args),

    /// Show the active env's mode, dirty state, and pinned ref (if any).
    Status(status::Args),

    /// Rename an env in place; libgit2 file-rename so history follows.
    Rename(rename::Args),

    /// Remove an env from this project.
    Rm(rm::Args),

    /// Open an env in `$EDITOR` (or fallbacks). Lock is released while the
    /// editor runs.
    Edit(edit::Args),

    /// Show commit history for an env, with key-level summaries.
    Log(log_cmd::Args),

    /// Show key-level differences between two refs.
    Diff(diff::Args),

    /// Print a single key's value to stdout (script-friendly, never masked).
    Get(get::Args),

    /// Set or update a single key in an env.
    Set(set::Args),

    /// Copy a single key from one env to another.
    Copy(copy::Args),

    /// Run a command with an env's variables injected (no symlink change).
    Exec(exec::Args),

    /// Configure the optional sync remote.
    #[command(subcommand)]
    Remote(remote::RemoteCommand),

    /// Pull-then-push the vault git history against the configured remote.
    Sync(sync::Args),

    /// Print a shell completion script (bash/zsh/fish/powershell/elvish) to stdout.
    Completions(completions::Args),

    /// Adopt an existing `.env`-style file as a new env. Onboarding shortcut.
    Import(import::Args),

    /// Print an env's plaintext content to stdout (dotenv / json / shell).
    /// Anti-lock-in escape hatch.
    Export(export::Args),

    /// Rename a key (e.g. DATABASE_URL → DB_URL) across one or every env.
    #[command(name = "rename-key")]
    RenameKey(rename_key::Args),
}

/// Resolved global context passed into every subcommand. Subcommands read
/// these instead of re-parsing the global flags themselves.
pub struct Context {
    pub format: OutputFormat,
    pub yes: bool,
    pub log_level: LogLevel,
    pub no_color: bool,
    pub passphrase_stdin: bool,
    pub passphrase_env: Option<String>,
    pub vault: Option<PathBuf>,
}

impl Context {
    fn from_cli(cli: &Cli) -> Self {
        Self {
            format: cli.format,
            yes: cli.yes,
            log_level: cli.log,
            no_color: cli.no_color,
            passphrase_stdin: cli.passphrase_stdin,
            passphrase_env: cli.passphrase_env.clone(),
            vault: cli.vault.clone(),
        }
    }
}

/// Dispatch a parsed [`Cli`] to the right subcommand handler.
pub fn dispatch(cli: Cli) -> Result<(), EnvrollError> {
    let ctx = Context::from_cli(&cli);
    match cli.command {
        Command::Init(a) => init::run(a, &ctx),
        Command::Projects(a) => projects::run(a, &ctx),
        Command::List(a) => list::run(a, &ctx),
        Command::Current(a) => current::run(a, &ctx),
        Command::Save(a) => save::run(a, &ctx),
        Command::Fork(a) => fork::run(a, &ctx),
        Command::Use(a) => use_cmd::run(a, &ctx),
        Command::Status(a) => status::run(a, &ctx),
        Command::Rename(a) => rename::run(a, &ctx),
        Command::Rm(a) => rm::run(a, &ctx),
        Command::Edit(a) => edit::run(a, &ctx),
        Command::Log(a) => log_cmd::run(a, &ctx),
        Command::Diff(a) => diff::run(a, &ctx),
        Command::Get(a) => get::run(a, &ctx),
        Command::Set(a) => set::run(a, &ctx),
        Command::Copy(a) => copy::run(a, &ctx),
        Command::Exec(a) => exec::run(a, &ctx),
        Command::Remote(c) => remote::run(c, &ctx),
        Command::Sync(a) => sync::run(a, &ctx),
        Command::Completions(a) => completions::run(a, &ctx),
        Command::Import(a) => import::run(a, &ctx),
        Command::Export(a) => export::run(a, &ctx),
        Command::RenameKey(a) => rename_key::run(a, &ctx),
    }
}
