//! `envroll completions <shell> [--install]` — shell completion scripts.
//!
//! Two modes:
//!
//! - **Default (no `--install`)**: emit the script to stdout. Backwards-
//!   compatible with v0.1.3 — useful in CI, container builds, or when the
//!   user wants to inspect / pipe the script themselves.
//! - **`--install`**: deduce the convention path for the chosen shell,
//!   `mkdir -p` whatever's missing, write the file, and (for shells that
//!   need it) idempotently append a marker block to the user's rc file
//!   so the completion is loaded on next shell start. Never uses sudo;
//!   targets user-local paths only.
//!
//! Idempotent. Re-running `--install` overwrites the completion file
//! (always safe — it's derived from the binary) and skips the rc-file
//! append if the marker block is already there.
//!
//! The command takes NO vault lock and is safe to run from any directory,
//! including outside an envroll project.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Args as ClapArgs, CommandFactory, ValueEnum};
use clap_complete::{generate, Shell};

use crate::cli::{Cli, Context};
use crate::errors::{generic, EnvrollError};

/// Marker comment we wrap around any block we write into a user's rc file
/// so subsequent runs of `--install` can find and skip it. Do NOT change
/// these strings without thinking about backward-compat — they're how we
/// detect "already installed" on existing systems.
const RC_MARKER_BEGIN: &str =
    "# >>> envroll completions (managed — do not edit between markers) >>>";
const RC_MARKER_END: &str = "# <<< envroll completions <<<";

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

/// Generate or install a shell completion script.
///
/// Recommended path — let envroll handle the install:
///
///   envroll completions zsh --install
///   envroll completions bash --install
///   envroll completions fish --install
///   envroll completions powershell --install
///   envroll completions elvish --install
///
/// `--install` writes to the user-local convention path for each shell
/// (no sudo, no system dirs) and, for shells that need it, idempotently
/// adds a small marker block to your shell's rc file so completion is
/// loaded on next shell start. Re-running is safe — the rc append is
/// guarded by a marker comment, and the completion file is always
/// regenerated.
///
/// Without `--install`, the script is printed to stdout (the v0.1.3
/// behavior) so you can pipe it yourself:
///
///   envroll completions zsh > ~/.zsh/completions/_envroll
///   envroll completions powershell | Out-String | Invoke-Expression
///
/// Note that env-name completion (`envroll use <TAB>` listing your envs)
/// is NOT supported in v0.1.x — that needs a session-cache layer planned
/// for v0.3. Subcommands and flags do complete.
#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Which shell to generate completions for.
    #[arg(value_enum)]
    pub shell: CompletionShell,

    /// Install the completion file to the convention path for `<shell>`,
    /// mkdir-ing whatever's missing, and (for shells that need it) appending
    /// a marker block to your rc file so completion loads on next shell
    /// start. No sudo; user-local paths only.
    #[arg(long)]
    pub install: bool,
}

pub fn run(args: Args, _ctx: &Context) -> Result<(), EnvrollError> {
    if args.install {
        let report = install(args.shell)?;
        report.print_summary();
        Ok(())
    } else {
        emit_to_stdout(args.shell)
    }
}

/// Generate the completion script and write it to `out`. Re-derives the clap
/// `Command` from the top-level `Cli` so future subcommands and flags pick
/// up automatically.
fn emit(shell: CompletionShell, out: &mut dyn Write) {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    let shell: Shell = shell.into();
    generate(shell, &mut cmd, bin_name, out);
}

fn emit_to_stdout(shell: CompletionShell) -> Result<(), EnvrollError> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    emit(shell, &mut handle);
    Ok(())
}

/// Outcome of an `--install` run, used to print a clear summary at the end
/// (what file we wrote, what rc we touched, how to activate now).
struct InstallReport {
    /// Where the completion script was written.
    file: PathBuf,
    /// `Some((path, was_added))` if we touched a shell rc file. `was_added`
    /// is false when the marker block was already present (idempotent run).
    rc: Option<(PathBuf, bool)>,
    /// Human-readable hint for activating the completion right now.
    activate_hint: &'static str,
}

impl InstallReport {
    fn print_summary(&self) {
        println!("✓ wrote completion file: {}", self.file.display());
        if let Some((path, was_added)) = &self.rc {
            if *was_added {
                println!("✓ added marker block to {}", path.display());
            } else {
                println!(
                    "✓ {} already has the marker block (no changes)",
                    path.display()
                );
            }
        }
        println!("\nTo activate now: {}", self.activate_hint);
    }
}

fn install(shell: CompletionShell) -> Result<InstallReport, EnvrollError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| generic("$HOME is not set; cannot determine install path"))?;
    match shell {
        CompletionShell::Bash => install_bash(&home),
        CompletionShell::Zsh => install_zsh(&home),
        CompletionShell::Fish => install_fish(&home),
        CompletionShell::PowerShell => install_powershell(&home),
        CompletionShell::Elvish => install_elvish(&home),
    }
}

// ---------------------------------------------------------------------------
// bash
//
// `~/.local/share/bash-completion/completions/<bin>` is the user-local path
// the [bash-completion project](https://github.com/scop/bash-completion)
// already scans by default. No `.bashrc` edit needed.
// ---------------------------------------------------------------------------

fn install_bash(home: &Path) -> Result<InstallReport, EnvrollError> {
    let dir = home.join(".local/share/bash-completion/completions");
    fs::create_dir_all(&dir).map_err(EnvrollError::Io)?;
    let file = dir.join("envroll");
    write_completion_file(CompletionShell::Bash, &file)?;
    Ok(InstallReport {
        file,
        rc: None,
        activate_hint: "exec bash  (or open a new shell)",
    })
}

// ---------------------------------------------------------------------------
// zsh
//
// The system convention path (`/usr/local/share/zsh/site-functions/`) needs
// sudo and may not exist at all. For `--install` we use a user-local path
// (`~/.zsh/completions/_envroll`) and idempotently teach `~/.zshrc` to load
// from there via a small fpath + compinit block guarded by markers.
// ---------------------------------------------------------------------------

fn install_zsh(home: &Path) -> Result<InstallReport, EnvrollError> {
    let dir = home.join(".zsh/completions");
    fs::create_dir_all(&dir).map_err(EnvrollError::Io)?;
    let file = dir.join("_envroll");
    write_completion_file(CompletionShell::Zsh, &file)?;

    let rc = home.join(".zshrc");
    let block = format!(
        "{RC_MARKER_BEGIN}\n\
         fpath=(~/.zsh/completions $fpath)\n\
         autoload -U compinit && compinit\n\
         {RC_MARKER_END}\n"
    );
    let was_added = ensure_rc_block(&rc, &block)?;
    Ok(InstallReport {
        file,
        rc: Some((rc, was_added)),
        activate_hint: "rm -f ~/.zcompdump* && exec zsh",
    })
}

// ---------------------------------------------------------------------------
// fish
//
// `~/.config/fish/completions/` is auto-loaded by fish — no rc edit needed.
// ---------------------------------------------------------------------------

fn install_fish(home: &Path) -> Result<InstallReport, EnvrollError> {
    let dir = home.join(".config/fish/completions");
    fs::create_dir_all(&dir).map_err(EnvrollError::Io)?;
    let file = dir.join("envroll.fish");
    write_completion_file(CompletionShell::Fish, &file)?;
    Ok(InstallReport {
        file,
        rc: None,
        activate_hint: "(none — fish auto-loads completion files immediately)",
    })
}

// ---------------------------------------------------------------------------
// powershell
//
// $PROFILE points at the user's PowerShell init file — but `envroll` runs
// from a Unix-y shell here, not from PowerShell, so we can't query
// `$PROFILE` directly. We use the documented Windows path
// (`%USERPROFILE%/Documents/PowerShell/Microsoft.PowerShell_profile.ps1`)
// when running on Windows; on Unix-y systems with PowerShell installed it
// lives at `~/.config/powershell/Microsoft.PowerShell_profile.ps1`.
//
// We write the actual completion script to a separate `.ps1` next to
// $PROFILE and add a single dot-source line, marker-guarded, to $PROFILE
// itself. This keeps PowerShell init fast (no `Invoke-Expression` of
// `envroll completions powershell` on every shell start).
// ---------------------------------------------------------------------------

fn install_powershell(home: &Path) -> Result<InstallReport, EnvrollError> {
    // Best-effort cross-platform $PROFILE location. Windows users can
    // override by passing the file path manually with the non-`--install`
    // form if this default doesn't fit their setup.
    let profile_dir = if cfg!(windows) {
        home.join("Documents/PowerShell")
    } else {
        home.join(".config/powershell")
    };
    fs::create_dir_all(&profile_dir).map_err(EnvrollError::Io)?;
    let file = profile_dir.join("envroll-completions.ps1");
    write_completion_file(CompletionShell::PowerShell, &file)?;

    let profile = profile_dir.join("Microsoft.PowerShell_profile.ps1");
    let block = format!(
        "{RC_MARKER_BEGIN}\n\
         . \"$PSScriptRoot/envroll-completions.ps1\"\n\
         {RC_MARKER_END}\n"
    );
    let was_added = ensure_rc_block(&profile, &block)?;
    Ok(InstallReport {
        file,
        rc: Some((profile, was_added)),
        activate_hint: ". $PROFILE  (or open a new PowerShell session)",
    })
}

// ---------------------------------------------------------------------------
// elvish
//
// `~/.config/elvish/lib/<name>.elv` can be `use`-d. We write the file there
// and add a `use` line, marker-guarded, to `~/.config/elvish/rc.elv`.
// ---------------------------------------------------------------------------

fn install_elvish(home: &Path) -> Result<InstallReport, EnvrollError> {
    let lib_dir = home.join(".config/elvish/lib");
    fs::create_dir_all(&lib_dir).map_err(EnvrollError::Io)?;
    let file = lib_dir.join("envroll-completions.elv");
    write_completion_file(CompletionShell::Elvish, &file)?;

    let rc = home.join(".config/elvish/rc.elv");
    let block = format!(
        "{RC_MARKER_BEGIN}\n\
         use envroll-completions\n\
         {RC_MARKER_END}\n"
    );
    let was_added = ensure_rc_block(&rc, &block)?;
    Ok(InstallReport {
        file,
        rc: Some((rc, was_added)),
        activate_hint: "exec elvish  (or open a new shell)",
    })
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

/// Write the completion script for `shell` to `dest` (overwriting if it
/// already exists). The completion file is derived from the binary, so an
/// overwrite is always safe — that's why we don't bother with a backup.
fn write_completion_file(shell: CompletionShell, dest: &Path) -> Result<(), EnvrollError> {
    let mut buf: Vec<u8> = Vec::new();
    emit(shell, &mut buf);
    fs::write(dest, buf).map_err(EnvrollError::Io)?;
    Ok(())
}

/// Append `block` to `rc` if (and only if) the file does not already
/// contain `RC_MARKER_BEGIN`. Returns `true` if we appended, `false` if
/// the marker block was already present (idempotent run). Creates the
/// file if missing.
fn ensure_rc_block(rc: &Path, block: &str) -> Result<bool, EnvrollError> {
    let existing = match fs::read_to_string(rc) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(EnvrollError::Io(e)),
    };
    if existing.contains(RC_MARKER_BEGIN) {
        return Ok(false);
    }
    if let Some(parent) = rc.parent() {
        fs::create_dir_all(parent).map_err(EnvrollError::Io)?;
    }
    let mut new_contents = existing;
    if !new_contents.is_empty() && !new_contents.ends_with('\n') {
        new_contents.push('\n');
    }
    if !new_contents.is_empty() {
        new_contents.push('\n');
    }
    new_contents.push_str(block);
    fs::write(rc, new_contents).map_err(EnvrollError::Io)?;
    Ok(true)
}
