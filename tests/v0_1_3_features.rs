//! Integration tests for the v0.1.3 feature set:
//! - `envroll completions <shell>`
//! - `envroll import <file> --as <name>`
//! - `envroll export <env> [--format dotenv|json|shell]`
//! - `envroll rename-key OLD NEW [--in <env>] [--all]`
//!
//! Same sandbox shape as the rest of the integration tests: tempdir cwd,
//! tempdir XDG_DATA_HOME, ENVROLL_PASSPHRASE set.

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
// completions
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

// ----------------------------------------------------------------------
// rename-key
// ----------------------------------------------------------------------

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
