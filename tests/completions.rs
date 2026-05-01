//! Integration tests for `envroll completions <shell> [--install]`.
//!
//! Two surfaces:
//! - **Stdout emission**: `envroll completions <shell>` prints the script
//!   for each of the five supported shells. We check each shell's expected
//!   header signature (`#compdef envroll`, `_envroll()`, etc.) so a
//!   regression in clap_complete is caught.
//! - **`--install`**: writes to the convention path under `$HOME` and
//!   (where applicable) idempotently appends a marker block to the user's
//!   rc file. We use the sandbox's `xdg` tempdir as `$HOME` so each test
//!   is fully isolated.

use std::path::Path;
use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;
use tempfile::TempDir;

fn envroll_in(cwd: &Path, xdg: &Path, passphrase: &str) -> Command {
    let mut cmd = Command::cargo_bin("envroll").unwrap();
    cmd.current_dir(cwd)
        .env_clear()
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        )
        .env("XDG_DATA_HOME", xdg)
        .env("HOME", xdg)
        .env("ENVROLL_PASSPHRASE", passphrase);
    cmd
}

fn sandbox() -> (TempDir, TempDir) {
    (TempDir::new().unwrap(), TempDir::new().unwrap())
}

// ----------------------------------------------------------------------
// completions <shell> — stdout emission
// ----------------------------------------------------------------------

#[test]
fn completions_bash_emits_a_completion_script() {
    let (cwd, xdg) = sandbox();
    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "bash"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    // bash completion uses `_envroll()` and `complete -F`. Both must be present.
    assert!(stdout.contains("_envroll()"), "missing function: {stdout}");
    assert!(
        stdout.contains("complete"),
        "missing complete cmd: {stdout}"
    );
}

#[test]
fn completions_zsh_emits_compdef_header() {
    let (cwd, xdg) = sandbox();
    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "zsh"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .starts_with("#compdef envroll"));
}

#[test]
fn completions_fish_emits_complete_lines() {
    let (cwd, xdg) = sandbox();
    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "fish"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .contains("complete -c envroll"));
}

#[test]
fn completions_powershell_emits_register_block() {
    let (cwd, xdg) = sandbox();
    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "powershell"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .contains("Register-ArgumentCompleter"));
}

#[test]
fn completions_runs_outside_a_project() {
    // No init called — completions must not require a vault or project.
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "bash"])
        .assert()
        .success();
}

#[test]
fn completions_unknown_shell_is_a_clap_usage_error() {
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "tcsh"])
        .assert()
        .failure()
        .code(2);
}

// ----------------------------------------------------------------------
// completions <shell> --install
// ----------------------------------------------------------------------

#[test]
fn install_zsh_writes_completion_file_and_marker_block() {
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "zsh", "--install"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ wrote completion file"))
        .stdout(predicate::str::contains("rm -f ~/.zcompdump*"));

    // The completion file lands under HOME (which the sandbox sets to xdg).
    let completion = xdg.path().join(".zsh/completions/_envroll");
    assert!(completion.exists());
    let body = std::fs::read_to_string(&completion).unwrap();
    assert!(body.contains("#compdef envroll"));

    // .zshrc gets the marker block + fpath + compinit.
    let zshrc = xdg.path().join(".zshrc");
    let rc = std::fs::read_to_string(&zshrc).unwrap();
    assert!(rc.contains("envroll completions (managed"));
    assert!(rc.contains("fpath=(~/.zsh/completions $fpath)"));
    assert!(rc.contains("autoload -U compinit"));
}

#[test]
fn install_zsh_is_idempotent() {
    let (cwd, xdg) = sandbox();
    // First install adds the marker block.
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "zsh", "--install"])
        .assert()
        .success();
    // Second install reports "already has the marker" instead of duplicating.
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "zsh", "--install"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already has the marker"));

    // The .zshrc must contain exactly ONE marker-begin line.
    let rc = std::fs::read_to_string(xdg.path().join(".zshrc")).unwrap();
    let count = rc
        .lines()
        .filter(|l| l.contains("envroll completions (managed"))
        .count();
    assert_eq!(count, 1, "marker block was duplicated on rerun:\n{rc}");
}

#[test]
fn install_zsh_preserves_existing_zshrc_contents() {
    let (cwd, xdg) = sandbox();
    let zshrc = xdg.path().join(".zshrc");
    std::fs::write(&zshrc, "alias ll='ls -la'\nexport EDITOR=vim\n").unwrap();

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "zsh", "--install"])
        .assert()
        .success();

    let rc = std::fs::read_to_string(&zshrc).unwrap();
    // Original lines must still be there.
    assert!(rc.contains("alias ll='ls -la'"));
    assert!(rc.contains("export EDITOR=vim"));
    // Plus the new marker block.
    assert!(rc.contains("envroll completions (managed"));
}

#[test]
fn install_bash_writes_to_bash_completion_dir_no_rc_edit() {
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "bash", "--install"])
        .assert()
        .success();

    let completion = xdg
        .path()
        .join(".local/share/bash-completion/completions/envroll");
    assert!(completion.exists());

    // bash install must NOT touch .bashrc (the bash-completion project
    // already scans this dir).
    assert!(!xdg.path().join(".bashrc").exists());
}

#[test]
fn install_fish_writes_to_completions_dir_no_rc_edit() {
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "fish", "--install"])
        .assert()
        .success();
    assert!(xdg
        .path()
        .join(".config/fish/completions/envroll.fish")
        .exists());
}

#[test]
fn install_elvish_writes_lib_file_and_use_line() {
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "elvish", "--install"])
        .assert()
        .success();
    assert!(xdg
        .path()
        .join(".config/elvish/lib/envroll-completions.elv")
        .exists());
    let rc = std::fs::read_to_string(xdg.path().join(".config/elvish/rc.elv")).unwrap();
    assert!(rc.contains("use envroll-completions"));
}

#[test]
fn install_powershell_writes_profile_dotsource() {
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["completions", "powershell", "--install"])
        .assert()
        .success();
    // Unix-y systems use ~/.config/powershell.
    let dir = xdg.path().join(".config/powershell");
    assert!(dir.join("envroll-completions.ps1").exists());
    let profile = std::fs::read_to_string(dir.join("Microsoft.PowerShell_profile.ps1")).unwrap();
    assert!(profile.contains(". \"$PSScriptRoot/envroll-completions.ps1\""));
}
