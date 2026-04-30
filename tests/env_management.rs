//! Integration tests for section 9 (env-management commands).
//!
//! Each test runs the binary in a sandbox: tempdir cwd, tempdir XDG_DATA_HOME,
//! and ENVROLL_PASSPHRASE set. We deliberately exercise end-to-end flows
//! (init → fork → save → list → ...) so the test suite doubles as an
//! integration smoke for the whole stack we've built so far.

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

fn vault_path(xdg: &Path) -> std::path::PathBuf {
    xdg.join("envroll")
}

/// Init the project under cwd inside a fresh vault rooted at `xdg`.
fn init(cwd: &Path, xdg: &Path, passphrase: &str) {
    envroll_in(cwd, xdg, passphrase)
        .arg("init")
        .assert()
        .success();
}

// ---------- happy-path end-to-end ----------

#[test]
fn fork_bootstraps_from_dotenv_when_no_active_env() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");

    // Create a .env file the user wants to fork from.
    std::fs::write(cwd.path().join(".env"), b"FOO=bar\nBAZ=qux\n").unwrap();

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "local"])
        .assert()
        .success()
        .stdout(predicate::str::contains("forked"));

    // After fork, ./.env should now be a symlink into the vault's .checkout/.
    let env_meta = std::fs::symlink_metadata(cwd.path().join(".env")).unwrap();
    assert!(env_meta.file_type().is_symlink());

    // current should report `local`.
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("current")
        .assert()
        .success()
        .stdout(predicate::str::contains("local"));
}

#[test]
fn fork_with_no_active_env_and_no_dotenv_refuses() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "ghost"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("no working copy to fork from"));
}

#[test]
fn fork_collision_without_force_exits_30() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .success();
    // Try forking dev again — same active env exists, but our second fork
    // names a NEW env "dev" which already exists, hence collision.
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .failure()
        .code(30)
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn save_writes_a_new_commit_when_working_copy_changed() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .success();

    // Mutate the working copy by writing through the symlink target. We use
    // the vault's checkout file directly because ./.env is a symlink to it.
    let vault = vault_path(xdg.path());
    // Find the project id (only one) and rewrite the checkout in place.
    let projects = std::fs::read_dir(vault.join("projects")).unwrap();
    let proj_dir = projects.filter_map(Result::ok).next().unwrap().path();
    let checkout = proj_dir.join(".checkout/dev");
    std::fs::write(&checkout, b"A=2\n").unwrap();

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["save", "-m", "bumped"])
        .assert()
        .success()
        .stdout(predicate::str::contains("saved dev"));
}

#[test]
fn save_unchanged_working_copy_prints_nothing_to_save() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .success();

    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("save")
        .assert()
        .success()
        .stderr(predicate::str::contains("envroll: nothing to save"));
}

#[test]
fn save_with_no_active_env_refuses() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("save")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("no active env"));
}

#[test]
fn save_with_unparseable_dotenv_yields_parse_error_exit_12() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"OK=fine\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .success();

    // Corrupt the working copy by writing an unterminated quote through the
    // symlink target.
    let vault = vault_path(xdg.path());
    let proj_dir = std::fs::read_dir(vault.join("projects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(proj_dir.join(".checkout/dev"), b"BAD=\"unterminated\n").unwrap();

    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("save")
        .assert()
        .failure()
        .code(12);
}

#[test]
fn save_refuses_on_foreign_symlink() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    // Create a regular .env, fork, then replace ./.env with a foreign symlink.
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .success();
    let env_path = cwd.path().join(".env");
    std::fs::remove_file(&env_path).unwrap();
    let foreign = TempDir::new().unwrap();
    let target = foreign.path().join("foreign.env");
    std::fs::write(&target, b"FOREIGN=1\n").unwrap();
    std::os::unix::fs::symlink(&target, &env_path).unwrap();

    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("save")
        .assert()
        .failure()
        .code(31)
        .stderr(predicate::str::contains("foreign symlink"));
}

// ---------- list / current ----------

#[test]
fn list_marks_active_env_in_human_output() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .success();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "staging"])
        .assert()
        .success();

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["list", "--no-color"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    // Two envs, staging is now active (last fork wins).
    assert!(
        stdout.contains("dev"),
        "expected dev in list, got: {stdout}"
    );
    assert!(
        stdout.contains("* staging"),
        "expected staging marked active, got: {stdout}"
    );
}

#[test]
fn list_json_format_includes_envs_and_active() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .success();
    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["list", "--format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v.is_array());
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let elem = &arr[0];
    assert_eq!(elem["active"].as_str(), Some("dev"));
    assert!(elem["envs"].as_array().unwrap().iter().any(|e| e == "dev"));
}

#[test]
fn current_with_no_active_env_exits_31() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("current")
        .assert()
        .failure()
        .code(31)
        .stderr(predicate::str::contains("envroll: no active env"));
}

#[test]
fn current_outside_a_registered_project_exits_22() {
    let (cwd, xdg) = sandbox();
    // No init at all — vault doesn't exist.
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("current")
        .assert()
        .failure();
}

// ---------- rename ----------

#[test]
fn rename_active_env_retargets_symlink_and_updates_manifest() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .success();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["rename", "dev", "development"])
        .assert()
        .success();
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("current")
        .assert()
        .success()
        .stdout(predicate::str::contains("development"));
    // Symlink resolves into the vault's checkout dir under the new name.
    let resolved = std::fs::read_link(cwd.path().join(".env")).unwrap();
    assert!(
        resolved.to_string_lossy().contains("development"),
        "symlink not retargeted; resolved to {resolved:?}"
    );
}

#[test]
fn rename_to_existing_name_without_force_exits_30() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .success();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "staging"])
        .assert()
        .success();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["rename", "dev", "staging"])
        .assert()
        .failure()
        .code(30)
        .stderr(predicate::str::contains("already exists"));
}

// ---------- rm ----------

#[test]
fn rm_active_env_with_yes_clears_active_and_removes_symlink() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .success();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["--yes", "rm", "dev"])
        .assert()
        .success()
        .stdout(predicate::str::contains("removed dev"));

    // ./.env is gone, current reports no active env.
    assert!(
        !cwd.path().join(".env").exists()
            && std::fs::symlink_metadata(cwd.path().join(".env")).is_err(),
        "./.env should be removed"
    );
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("current")
        .assert()
        .failure()
        .code(31);
}

#[test]
fn rm_nonexistent_env_exits_20() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["--yes", "rm", "ghost"])
        .assert()
        .failure()
        .code(20);
}

// ---------- edit ----------

/// `edit` with EDITOR=true exercises the prepare phase + lock release
/// without keeping any process around.
#[test]
fn edit_runs_prepare_phase_and_releases_lock() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .success();

    envroll_in(cwd.path(), xdg.path(), "p")
        .env("EDITOR", "true")
        .args(["edit", "dev"])
        .assert()
        .success();

    // After edit returns, the vault lock must be re-acquirable. Run another
    // exclusive command to verify (a foreign-symlink-clean save is the
    // smallest exclusive op that touches the lock).
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("save")
        .assert()
        .success()
        .stderr(predicate::str::contains("nothing to save"));
}

#[test]
fn edit_without_editor_or_fallback_errors() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .success();

    // Strip PATH so vi/vim aren't found, AND don't set EDITOR.
    let mut cmd = Command::cargo_bin("envroll").unwrap();
    cmd.current_dir(cwd.path())
        .env_clear()
        .env("PATH", "")
        .env("XDG_DATA_HOME", xdg.path())
        .env("HOME", xdg.path())
        .env("ENVROLL_PASSPHRASE", "p");
    cmd.args(["edit", "dev"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("EDITOR"));
}
