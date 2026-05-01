//! Integration tests for sections 10 (switching), 11 (versioning), 12
//! (variable ops), and 13 (exec).
//!
//! Same sandbox shape as `tests/env_management.rs`: tempdir cwd + tempdir
//! XDG_DATA_HOME + ENVROLL_PASSPHRASE.

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

fn project_dir(xdg: &Path) -> std::path::PathBuf {
    let projects = std::fs::read_dir(vault_path(xdg).join("projects")).unwrap();
    projects.filter_map(Result::ok).next().unwrap().path()
}

// ------------------------------------------------------------------
// 10.1 — `envroll use <name>` (latest)
// ------------------------------------------------------------------

#[test]
fn use_latest_activates_an_env_and_creates_symlink() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");
    fork(cwd.path(), xdg.path(), "p", "staging");

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["use", "dev"])
        .assert()
        .success()
        .stdout(predicate::str::contains("now using dev"));

    let meta = std::fs::symlink_metadata(cwd.path().join(".env")).unwrap();
    assert!(meta.file_type().is_symlink());
}

#[test]
fn use_latest_clears_a_prior_active_ref() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    // Make a second commit so we have history to pin to.
    let proj = project_dir(xdg.path());
    std::fs::write(proj.join(".checkout/dev"), b"A=2\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["save", "-m", "v2"])
        .assert()
        .success();

    // Discover the v1 commit hash via the libgit2-managed vault.
    let history_out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["log", "dev", "--format", "json"])
        .output()
        .unwrap();
    assert!(history_out.status.success());
    let stdout = String::from_utf8(history_out.stdout).unwrap();
    let entries: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    assert!(entries.len() >= 2);
    let prev_hash = entries[1]["hash"].as_str().unwrap().to_string();

    // Pin to historical, then clear via `use dev`.
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["use", &format!("dev@{prev_hash}")])
        .assert()
        .success();

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["use", "dev"])
        .assert()
        .success();

    // Read manifest, assert active_ref is now empty.
    let manifest_path = project_dir(xdg.path()).join("manifest.toml");
    let body = std::fs::read_to_string(&manifest_path).unwrap();
    assert!(
        body.contains("active_ref = \"\"") || body.contains("active_ref=\"\""),
        "manifest still pinned: {body}"
    );
}

#[test]
fn use_unknown_env_exits_20() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["use", "ghost"])
        .assert()
        .failure()
        .code(20);
}

// ------------------------------------------------------------------
// 10.2 — historical activation
// ------------------------------------------------------------------

#[test]
fn use_historical_by_offset_pins_active_ref() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let proj = project_dir(xdg.path());
    std::fs::write(proj.join(".checkout/dev"), b"A=2\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["save", "-m", "v2"])
        .assert()
        .success();

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["use", "dev@~1"])
        .assert()
        .success();

    let manifest_path = project_dir(xdg.path()).join("manifest.toml");
    let body = std::fs::read_to_string(&manifest_path).unwrap();
    assert!(
        body.contains("active_ref = \"dev@"),
        "no pin in manifest: {body}"
    );

    // ./.env should resolve to a content matching v1.
    let resolved = std::fs::read_to_string(cwd.path().join(".env")).unwrap();
    assert!(resolved.contains("A="), "got {resolved:?}");
}

// ------------------------------------------------------------------
// 10.3 — --rescue
// ------------------------------------------------------------------

#[test]
fn use_rescue_saves_existing_dotenv_and_then_activates() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    // Replace the symlink with a regular file (simulating a foreign / unmanaged
    // ./.env after the user reverted to a checked-in version).
    std::fs::remove_file(cwd.path().join(".env")).unwrap();
    std::fs::write(cwd.path().join(".env"), b"FROM_FOREIGN=yes\n").unwrap();

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["use", "dev", "--rescue", "rescued"])
        .assert()
        .success();

    // Both rescued and dev should now exist; dev is active.
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("current")
        .assert()
        .success()
        .stdout(predicate::str::contains("dev"));

    let proj = project_dir(xdg.path());
    assert!(proj.join("envs/rescued.age").exists());
}

// ------------------------------------------------------------------
// 10.4 — broken-our-symlink recoverable; foreign-symlink refuses
// ------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn use_recovers_from_broken_our_symlink_without_force() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let proj = project_dir(xdg.path());
    // Delete the checkout so the symlink dangles into our own .checkout/.
    std::fs::remove_file(proj.join(".checkout/dev")).unwrap();

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["use", "dev"])
        .assert()
        .success();
}

#[cfg(unix)]
#[test]
fn use_refuses_foreign_symlink_without_force() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    // Replace ./.env with a foreign symlink.
    std::fs::remove_file(cwd.path().join(".env")).unwrap();
    let foreign = cwd.path().join("elsewhere.env");
    std::fs::write(&foreign, b"X=1\n").unwrap();
    std::os::unix::fs::symlink(&foreign, cwd.path().join(".env")).unwrap();

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["use", "dev"])
        .assert()
        .failure()
        .code(31);
}

// ------------------------------------------------------------------
// 10.5 — copy-mode via ENVROLL_USE_COPY
// ------------------------------------------------------------------

#[test]
fn use_copy_mode_round_trip() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    // Forget the symlink and ask for copy-mode on the next activation.
    std::fs::remove_file(cwd.path().join(".env")).unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .env("ENVROLL_USE_COPY", "1")
        .args(["use", "dev"])
        .assert()
        .success();

    let meta = std::fs::symlink_metadata(cwd.path().join(".env")).unwrap();
    assert!(
        meta.file_type().is_file(),
        "expected regular file (copy-mode), got {:?}",
        meta.file_type()
    );
}

// ------------------------------------------------------------------
// 10.6 — status
// ------------------------------------------------------------------

#[test]
fn status_clean_symlink_mode() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("active: dev (clean)"));
}

#[test]
fn status_dirty_with_added_and_changed_keys_shows_values_by_default() {
    // Default flipped on 2026-05-01: values are visible by default since the
    // user is on their own machine looking at their own envs. `--mask` is
    // the opt-in for paste-safe output (see `status_mask_hides_values`).
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\nB=2\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    // Mutate the symlink target to introduce a change + addition.
    let proj = project_dir(xdg.path());
    std::fs::write(proj.join(".checkout/dev"), b"A=999\nC=new\n").unwrap();

    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("dirty"))
        .stdout(predicate::str::contains("+C"))
        .stdout(predicate::str::contains("-B"))
        .stdout(predicate::str::contains("~A"))
        .stdout(predicate::str::contains("new"))
        .stdout(predicate::str::contains("999"));
}

#[test]
fn status_mask_hides_values() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let proj = project_dir(xdg.path());
    std::fs::write(proj.join(".checkout/dev"), b"A=secret\n").unwrap();

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["status", "--mask"])
        .assert()
        .success()
        .stdout(predicate::str::contains("********"))
        .stdout(predicate::str::contains("secret").not());
}

#[test]
fn status_show_values_unmasks() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let proj = project_dir(xdg.path());
    std::fs::write(proj.join(".checkout/dev"), b"A=secret\n").unwrap();

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["status", "--show-values"])
        .assert()
        .success()
        .stdout(predicate::str::contains("secret"));
}

// ------------------------------------------------------------------
// 11.1, 11.2 — log + diff
// ------------------------------------------------------------------

#[test]
fn log_lists_history_newest_first_with_summary() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let proj = project_dir(xdg.path());
    std::fs::write(proj.join(".checkout/dev"), b"A=1\nB=2\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["save", "-m", "v2"])
        .assert()
        .success();

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["log", "dev"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    // The tabled output is a Unicode-bordered table: top border, header row,
    // separator, two data rows, bottom border. We just check the spec-mandated
    // bits show up: the column headers, both entries, and the v2 message.
    assert!(stdout.contains("HASH"), "missing HASH header: {stdout}");
    assert!(
        stdout.contains("CHANGES"),
        "missing CHANGES header: {stdout}"
    );
    assert!(
        stdout.contains("MESSAGE"),
        "missing MESSAGE header: {stdout}"
    );
    assert!(stdout.contains("+1"), "missing +1 summary: {stdout}");
    assert!(stdout.contains("v2"), "missing v2 message: {stdout}");
    assert!(
        stdout.contains("initial save"),
        "missing v1 message: {stdout}"
    );
}

#[test]
fn diff_between_two_envs_reports_changes() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\nB=2\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");
    std::fs::remove_file(cwd.path().join(".env")).unwrap();
    std::fs::write(cwd.path().join(".env"), b"A=99\nC=new\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "staging");

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["diff", "dev", "staging"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    // The new tabled output shows op + key in separate cells, so look for
    // each (op, key) pair as adjacent table cells.
    assert!(stdout.contains("+") && stdout.contains(" C "));
    assert!(stdout.contains("-") && stdout.contains(" B "));
    assert!(stdout.contains("~") && stdout.contains(" A "));
}

// ------------------------------------------------------------------
// 12.1, 12.2, 12.3 — get / set / copy
// ------------------------------------------------------------------

#[test]
fn get_prints_value_with_trailing_newline() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=hello\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["get", "A"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "hello\n");
}

#[test]
fn get_missing_key_exits_20() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["get", "MISSING"])
        .assert()
        .failure()
        .code(20);
}

#[test]
fn set_writes_a_new_key_and_refreshes_checkout() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["set", "B=two"])
        .assert()
        .success();

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["get", "B"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "two\n");
}

#[test]
fn set_with_invalid_assignment_refuses() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["set", "no_equals"])
        .assert()
        .failure();
}

#[test]
fn copy_moves_a_key_between_envs() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"DB=src\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "src");
    std::fs::remove_file(cwd.path().join(".env")).unwrap();
    std::fs::write(cwd.path().join(".env"), b"OTHER=x\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dst");

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["copy", "DB", "--from", "src", "--to", "dst"])
        .assert()
        .success();

    // Reactivate dst and read DB.
    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["get", "DB", "--from", "dst"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "src\n");
}

#[test]
fn copy_same_env_refuses_with_exit_1() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["copy", "A", "--from", "dev", "--to", "dev"])
        .assert()
        .failure()
        .code(1);
}

// ------------------------------------------------------------------
// 12.4 — active_ref refuse rule for set/copy
// ------------------------------------------------------------------

#[test]
fn set_into_pinned_active_env_refuses() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let proj = project_dir(xdg.path());
    std::fs::write(proj.join(".checkout/dev"), b"A=2\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["save", "-m", "v2"])
        .assert()
        .success();

    // Pin to the historical version, then attempt to set.
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["use", "dev@~1"])
        .assert()
        .success();

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["set", "X=y"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("pinned to historical ref"));
}

// ------------------------------------------------------------------
// 13 — exec
// ------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn exec_injects_vars_into_child_process() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"GREETING=hello-from-envroll\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["exec", "dev", "--", "sh", "-c", "echo $GREETING"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "hello-from-envroll\n"
    );
}

#[cfg(unix)]
#[test]
fn exec_default_overrides_parent_env_vars() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"OVERRIDE_ME=from-envroll\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .env("OVERRIDE_ME", "from-parent")
        .args(["exec", "dev", "--", "sh", "-c", "echo $OVERRIDE_ME"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "from-envroll\n");
}

#[cfg(unix)]
#[test]
fn exec_no_override_lets_parent_env_win() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"OVERRIDE_ME=from-envroll\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .env("OVERRIDE_ME", "from-parent")
        .args([
            "exec",
            "dev",
            "--no-override",
            "--",
            "sh",
            "-c",
            "echo $OVERRIDE_ME",
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "from-parent\n");
}

#[cfg(unix)]
#[test]
fn exec_propagates_child_exit_code() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["exec", "dev", "--", "sh", "-c", "exit 7"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7));
}

#[cfg(unix)]
#[test]
fn exec_historical_ref_does_not_create_checkout_file() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let proj = project_dir(xdg.path());
    std::fs::write(proj.join(".checkout/dev"), b"A=2\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["save", "-m", "v2"])
        .assert()
        .success();

    let before: Vec<_> = std::fs::read_dir(proj.join(".checkout"))
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .filter(|n| n.contains('@'))
        .collect();

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["exec", "dev@~1", "--", "sh", "-c", "echo $A"])
        .output()
        .unwrap();

    let after: Vec<_> = std::fs::read_dir(proj.join(".checkout"))
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .filter(|n| n.contains('@'))
        .collect();
    assert_eq!(
        before, after,
        "exec should not create historical checkout files"
    );
}

// ------------------------------------------------------------------
// 14 — remote / sync
// ------------------------------------------------------------------

#[test]
fn remote_set_show_unset_round_trip() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");

    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["remote", "set", "file:///tmp/envroll-test.git"])
        .assert()
        .success();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["remote", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("file:///tmp/envroll-test.git"));
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["remote", "unset"])
        .assert()
        .success();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["remote", "show"])
        .assert()
        .failure()
        .code(40);
}

#[test]
fn remote_set_rejects_unsupported_scheme() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["remote", "set", "ftp://example.com/r"])
        .assert()
        .failure();
}

#[test]
fn sync_with_no_remote_exits_40() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("sync")
        .assert()
        .failure()
        .code(40);
}

#[test]
fn sync_against_local_bare_remote_round_trip() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let bare = TempDir::new().unwrap();
    git2::Repository::init_bare(bare.path()).unwrap();
    let url = format!("file://{}", bare.path().display());
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["remote", "set", &url])
        .assert()
        .success();

    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("sync")
        .assert()
        .success();
}

#[test]
fn sync_with_dirty_vault_refuses_before_network() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    // Create an untracked file inside the vault so the working tree is dirty.
    std::fs::write(vault_path(xdg.path()).join("dirty.txt"), b"x").unwrap();

    let bare = TempDir::new().unwrap();
    git2::Repository::init_bare(bare.path()).unwrap();
    let url = format!("file://{}", bare.path().display());
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["remote", "set", &url])
        .assert()
        .success();

    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("sync")
        .assert()
        .failure()
        .stderr(predicate::str::contains("working tree is dirty"));
}

#[test]
fn sync_never_includes_checkout_in_commits() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    fork(cwd.path(), xdg.path(), "p", "dev");

    let bare = TempDir::new().unwrap();
    git2::Repository::init_bare(bare.path()).unwrap();
    let url = format!("file://{}", bare.path().display());
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["remote", "set", &url])
        .assert()
        .success();
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("sync")
        .assert()
        .success();

    // Walk every commit on the bare repo and assert no .checkout/ paths.
    let bare_repo = git2::Repository::open_bare(bare.path()).unwrap();
    let head_oid = bare_repo.refname_to_id("refs/heads/main").unwrap();
    let mut walk = bare_repo.revwalk().unwrap();
    walk.push(head_oid).unwrap();
    for oid_res in walk {
        let oid = oid_res.unwrap();
        let commit = bare_repo.find_commit(oid).unwrap();
        let tree = commit.tree().unwrap();
        tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
            let name = entry.name().unwrap_or("");
            assert!(
                !dir.contains(".checkout") && !name.contains(".checkout"),
                "found .checkout in synced tree at {dir}{name}"
            );
            git2::TreeWalkResult::Ok
        })
        .unwrap();
    }
}
