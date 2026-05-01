//! Integration tests for the two anti-lock-in commands:
//! - `envroll import <file> --as <name>` — adopt an existing `.env`-style
//!   file as a new env (onboarding).
//! - `envroll export <env> [--output dotenv|json|shell]` — emit an env's
//!   plaintext to stdout for piping to other tools or migrating away.
//!
//! These two are the inverse of each other — the round-trip test in
//! `export_default_dotenv_format_round_trips_through_import` asserts that
//! invariant directly.

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

fn init(cwd: &Path, xdg: &Path, passphrase: &str) {
    envroll_in(cwd, xdg, passphrase)
        .arg("init")
        .assert()
        .success();
}

fn fork(cwd: &Path, xdg: &Path, passphrase: &str, name: &str) {
    envroll_in(cwd, xdg, passphrase)
        .args(["fork", name])
        .assert()
        .success();
}

// ----------------------------------------------------------------------
// import
// ----------------------------------------------------------------------

#[test]
fn import_adopts_an_existing_dotenv_file_as_a_new_env() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");

    // A file lying around outside any of envroll's machinery.
    let src = cwd.path().join("legacy-staging.env");
    std::fs::write(&src, b"DATABASE_URL=postgres://staging\nDEBUG=true\n").unwrap();

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["import", src.to_str().unwrap(), "--as", "staging"])
        .assert()
        .success()
        .stdout(predicate::str::contains("imported"));

    // Source file must be left untouched on disk.
    assert!(src.exists(), "import deleted its source file");

    // The new env is now active and we can read its keys back.
    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["get", "DATABASE_URL"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "postgres://staging\n"
    );
}

#[test]
fn import_missing_file_refuses() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["import", "does-not-exist.env", "--as", "ghost"])
        .assert()
        .failure();
}

#[test]
fn import_unparseable_file_exits_12() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    let bad = cwd.path().join("bad.env");
    // Unterminated quote — dotenvy rejects.
    std::fs::write(&bad, b"BROKEN=\"unterminated\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["import", bad.to_str().unwrap(), "--as", "broken"])
        .assert()
        .failure()
        .code(12);
}

#[test]
fn import_collision_without_force_exits_30() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let src = cwd.path().join("legacy.env");
    std::fs::write(&src, b"A=2\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["import", src.to_str().unwrap(), "--as", "dev"])
        .assert()
        .failure()
        .code(30);

    // --force overrides
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["import", src.to_str().unwrap(), "--as", "dev", "--force"])
        .assert()
        .success();
}

#[test]
fn import_refuses_when_pointed_at_the_projects_own_dotenv() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev"); // ./.env now symlinks to .checkout/dev

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["import", ".env", "--as", "duplicate"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("`envroll fork"));
}

// ----------------------------------------------------------------------
// export
// ----------------------------------------------------------------------

#[test]
fn export_default_dotenv_format_round_trips_through_import() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(
        cwd.path().join(".env"),
        b"DATABASE_URL=postgres://x\nDEBUG=true\nSTRIPE_KEY=sk_test_xxx\n",
    )
    .unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["export", "dev"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let exported = String::from_utf8(out.stdout).unwrap();
    // Each key must appear; values may be quoted/escaped.
    assert!(exported.contains("DATABASE_URL"));
    assert!(exported.contains("DEBUG"));
    assert!(exported.contains("STRIPE_KEY"));
    assert!(exported.contains("sk_test_xxx"));

    // Round-trip: dump → import → values come out the same.
    let dumped = cwd.path().join("dumped.env");
    std::fs::write(&dumped, exported).unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["import", dumped.to_str().unwrap(), "--as", "round-trip"])
        .assert()
        .success();
    let val = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["get", "DATABASE_URL", "--from", "round-trip"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(val.stdout).unwrap(), "postgres://x\n");
}

#[test]
fn export_json_format_is_valid_object() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\nB=two\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["export", "dev", "--output", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["A"], "1");
    assert_eq!(parsed["B"], "two");
}

#[cfg(unix)]
#[test]
fn export_shell_format_evals_into_real_shell_vars() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    // A value with shell metacharacters that single-quote escaping must defuse.
    std::fs::write(
        cwd.path().join(".env"),
        b"GREETING='hello $WORLD `whoami`'\n",
    )
    .unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["export", "dev", "--output", "shell"])
        .output()
        .unwrap();
    let script = String::from_utf8(out.stdout).unwrap();
    // Pipe through sh, then echo the resolved var to confirm the literal
    // text survived without ANY shell expansion.
    let result = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{script}\necho \"$GREETING\""))
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(result.stdout).unwrap().trim(),
        "hello $WORLD `whoami`"
    );
}

#[test]
fn export_unknown_env_exits_20() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["export", "ghost"])
        .assert()
        .failure()
        .code(20);
}
