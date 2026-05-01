//! Integration tests for `envroll rename-key OLD NEW [--in <env> | --all] [--force]`.
//!
//! Three target modes (precedence): `--all` > `--in <env>` > active env.
//! The command must:
//! - silently skip envs that don't have OLD (so `--all` is a no-op on
//!   envs that never had the key);
//! - refuse on collisions (`OLD` and `NEW` both present in a target env)
//!   unless `--force`;
//! - reject `OLD == NEW` outright.

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

#[test]
fn rename_key_default_targets_the_active_env() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"DATABASE_URL=postgres://x\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["rename-key", "DATABASE_URL", "DB_URL"])
        .assert()
        .success();

    let val = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["get", "DB_URL"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(val.stdout).unwrap(), "postgres://x\n");

    // Old key gone.
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["get", "DATABASE_URL"])
        .assert()
        .failure()
        .code(20);
}

#[test]
fn rename_key_in_specific_env_only() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"DATABASE_URL=dev\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");
    std::fs::remove_file(cwd.path().join(".env")).unwrap();
    std::fs::write(cwd.path().join(".env"), b"DATABASE_URL=prod\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "prod");

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["rename-key", "DATABASE_URL", "DB_URL", "--in", "prod"])
        .assert()
        .success();

    // prod renamed
    let prod_val = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["get", "DB_URL", "--from", "prod"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(prod_val.stdout).unwrap(), "prod\n");

    // dev unchanged
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["get", "DB_URL", "--from", "dev"])
        .assert()
        .failure()
        .code(20);
    let dev_val = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["get", "DATABASE_URL", "--from", "dev"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(dev_val.stdout).unwrap(), "dev\n");
}

#[test]
fn rename_key_all_renames_across_every_env_with_the_key() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"DATABASE_URL=dev\nOTHER=x\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");
    std::fs::remove_file(cwd.path().join(".env")).unwrap();
    std::fs::write(cwd.path().join(".env"), b"DATABASE_URL=staging\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "staging");
    // An env that does NOT have the key — must be skipped, not error.
    std::fs::remove_file(cwd.path().join(".env")).unwrap();
    std::fs::write(cwd.path().join(".env"), b"OTHER=only\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "without-key");

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["rename-key", "DATABASE_URL", "DB_URL", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 env(s)"))
        .stderr(predicate::str::contains("skipped"));

    for env in ["dev", "staging"] {
        let val = envroll_in(cwd.path(), xdg.path(), "p")
            .args(["get", "DB_URL", "--from", env])
            .output()
            .unwrap();
        assert!(
            !val.stdout.is_empty(),
            "DB_URL missing in {env} after --all"
        );
    }
}

#[test]
fn rename_key_collision_without_force_exits_30() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"OLD=1\nNEW=2\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["rename-key", "OLD", "NEW"])
        .assert()
        .failure()
        .code(30);

    // --force collapses the collision (NEW takes OLD's value).
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["rename-key", "OLD", "NEW", "--force"])
        .assert()
        .success();
    let val = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["get", "NEW"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(val.stdout).unwrap(), "1\n");
}

#[test]
fn rename_key_old_eq_new_refuses() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["rename-key", "SAME", "SAME"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must differ"));
}
