//! Integration tests for `envroll init` (task 8.5).
//!
//! Each test runs the binary in a pristine sandbox: a tempdir for `cwd`,
//! a tempdir for `XDG_DATA_HOME` (so the vault is isolated from the host's
//! real `~/.local/share/envroll`), and `ENVROLL_PASSPHRASE` set to a known
//! value so non-TTY runs don't fail at the passphrase-source step.

use std::path::Path;
use std::process::Command;

use age::secrecy::SecretString;
use assert_cmd::prelude::*;
use predicates::prelude::*;
use tempfile::TempDir;

/// Build a `Command` that runs the envroll binary in a sandbox: cwd is set
/// to `cwd`, XDG_DATA_HOME points at `xdg`, and ENVROLL_PASSPHRASE is set so
/// non-TTY invocations succeed (also wipes inherited HOME-style vars that
/// could leak the developer's vault into the test).
fn envroll_in(cwd: &Path, xdg: &Path, passphrase: &str) -> Command {
    let mut cmd = Command::cargo_bin("envroll").unwrap();
    cmd.current_dir(cwd)
        .env_clear()
        // PATH is needed for libgit2 to find anything it shells out to (it
        // doesn't, in our usage, but keeping a sane PATH avoids surprises
        // on macOS where the dyld loader resolves crate-bundled libs).
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        )
        .env("XDG_DATA_HOME", xdg)
        .env("HOME", xdg) // some libs look at HOME directly
        .env("ENVROLL_PASSPHRASE", passphrase);
    cmd
}

/// Sandbox = (cwd, xdg). Both are kept alive for the duration of a test
/// via the returned `(TempDir, TempDir)` tuple — drop them and the dirs go.
fn sandbox() -> (TempDir, TempDir) {
    (TempDir::new().unwrap(), TempDir::new().unwrap())
}

/// Path to the vault root inside the test sandbox (mirrors what the real
/// `paths::resolve_vault_root` produces given `XDG_DATA_HOME=<xdg>`).
fn vault_path(xdg: &Path) -> std::path::PathBuf {
    xdg.join("envroll")
}

#[test]
fn init_on_fresh_machine_creates_vault_and_registers_project() {
    let (cwd, xdg) = sandbox();
    let vault = vault_path(xdg.path());

    envroll_in(cwd.path(), xdg.path(), "test-pass-1")
        .arg("init")
        .assert()
        .success();

    // Vault layout exists.
    assert!(vault.is_dir(), "vault root not created");
    assert!(vault.join(".git").is_dir(), "libgit2 repo not initialized");
    assert!(vault.join(".envroll-version").is_file());
    assert!(vault.join(".gitignore").is_file());
    assert!(vault.join(".canary.age").is_file(), "canary not written");

    // Canary decrypts with the passphrase we used.
    let canary = std::fs::read(vault.join(".canary.age")).unwrap();
    let pass = SecretString::from("test-pass-1".to_string());
    let plaintext = envroll::crypto::decrypt(&canary, &pass).expect("canary should decrypt");
    assert_eq!(plaintext, b"envroll-canary-v1\n");

    // Exactly one project registered, with a path-derived ID (no .git in cwd).
    let projects = std::fs::read_dir(vault.join("projects")).unwrap();
    let project_dirs: Vec<_> = projects.filter_map(Result::ok).map(|e| e.path()).collect();
    assert_eq!(project_dirs.len(), 1);
    let proj = &project_dirs[0];
    assert!(proj
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("path-"));
    assert!(proj.join("envs").is_dir());
    assert!(proj.join(".checkout").is_dir());

    // Manifest fields: includes id/id_source/id_input/active/active_ref/created_at,
    // does NOT include path/mode/copy_hash.
    let manifest_raw = std::fs::read_to_string(proj.join("manifest.toml")).unwrap();
    assert!(manifest_raw.contains("schema_version = 1"));
    assert!(manifest_raw.contains("id_source = \"path\""));
    assert!(manifest_raw.contains("id_input = \"\""));
    assert!(manifest_raw.contains("active = \"\""));
    assert!(manifest_raw.contains("active_ref = \"\""));
    assert!(manifest_raw.contains("created_at = "));
    // The forbidden machine-local fields (design.md D2/D9):
    assert!(!manifest_raw.contains("path = "));
    assert!(!manifest_raw.contains("mode = "));
    assert!(!manifest_raw.contains("copy_hash = "));
}

#[test]
fn init_is_idempotent_in_an_already_registered_directory() {
    let (cwd, xdg) = sandbox();

    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("init")
        .assert()
        .success();

    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("project already registered"));
}

#[test]
fn init_with_id_override_uses_manual_source() {
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("init")
        .arg("--id")
        .arg("my-monorepo-frontend")
        .assert()
        .success();

    let manifest = vault_path(xdg.path()).join("projects/my-monorepo-frontend/manifest.toml");
    let raw = std::fs::read_to_string(&manifest).unwrap();
    assert!(raw.contains("id_source = \"manual\""));
    assert!(raw.contains("id_input = \"\""));
    assert!(raw.contains("id = \"my-monorepo-frontend\""));
}

#[test]
fn verify_passphrase_succeeds_with_correct_passphrase() {
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "right")
        .arg("init")
        .assert()
        .success();

    envroll_in(cwd.path(), xdg.path(), "right")
        .arg("init")
        .arg("--verify-passphrase")
        .assert()
        .success()
        .stdout(predicate::str::contains("passphrase verified"));
}

#[test]
fn verify_passphrase_fails_with_wrong_passphrase_exit_10() {
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "right")
        .arg("init")
        .assert()
        .success();

    envroll_in(cwd.path(), xdg.path(), "wrong")
        .arg("init")
        .arg("--verify-passphrase")
        .assert()
        .failure()
        .code(10);
}

#[test]
fn projects_lists_registered_projects() {
    let (cwd_a, xdg) = sandbox();
    let cwd_b = TempDir::new().unwrap();

    envroll_in(cwd_a.path(), xdg.path(), "p")
        .arg("init")
        .assert()
        .success();
    envroll_in(cwd_b.path(), xdg.path(), "p")
        .arg("init")
        .assert()
        .success();

    let json = envroll_in(cwd_a.path(), xdg.path(), "p")
        .args(["projects", "--format", "json"])
        .output()
        .unwrap();
    assert!(json.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let arr = parsed.as_array().expect("projects JSON must be an array");
    assert_eq!(arr.len(), 2, "two projects expected, got {arr:?}");
    for elem in arr {
        // Schema fields the spec requires (8.4 + project-lifecycle spec).
        for key in [
            "id",
            "envs",
            "active",
            "id_source",
            "id_input",
            "created_at",
        ] {
            assert!(elem.get(key).is_some(), "missing field `{key}` in {elem}");
        }
    }
}

#[test]
fn projects_on_empty_vault_prints_no_projects_registered() {
    let (cwd, xdg) = sandbox();
    // Don't init anything — the projects/ dir doesn't exist yet.
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["projects"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no projects registered"));
}

#[test]
fn projects_json_on_empty_vault_is_empty_array() {
    let (cwd, xdg) = sandbox();
    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["projects", "--format", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let trimmed = stdout.trim();
    assert_eq!(trimmed, "[]", "expected empty JSON array, got {trimmed:?}");
}
