//! Integration tests for section 15 (error handling, exit codes, output)
//! and 16.2 (lightweight smoke perf check).

use std::path::Path;
use std::process::Command;
use std::time::Instant;

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

// ---------- 15.1 / 15.2 — exit codes are within stable range ----------

#[test]
fn project_not_found_exits_22() {
    // Run `current` outside any registered project — but with a valid vault.
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    let other = TempDir::new().unwrap();
    envroll_in(other.path(), xdg.path(), "p")
        .arg("current")
        .assert()
        .failure()
        .code(22);
}

#[test]
fn unknown_subcommand_uses_clap_exit_2() {
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("nonexistent-subcommand")
        .assert()
        .failure()
        .code(2);
}

#[test]
fn help_text_lists_supported_subcommands() {
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("fork"))
        .stdout(predicate::str::contains("use"))
        .stdout(predicate::str::contains("save"));
}

#[test]
fn structured_error_renders_through_miette() {
    // Errors now go through miette::Report which prints `× <message>` and an
    // optional `help: ...` block — no leading `envroll:` prefix any more
    // (the "envroll: <category>: <message>" line was the pre-miette format,
    // dropped on 2026-05-01 in favor of the boxed diagnostic look).
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("current")
        .assert()
        .failure()
        .stderr(predicate::str::contains("×"));
}

// ---------- 15.5 — current/projects do NOT take the lock ----------

#[test]
fn projects_does_not_block_on_held_exclusive_lock() {
    // We can't easily simulate a held lock from inside this test (the lock is
    // process-scoped). Instead, this is a smoke test that `projects` returns
    // quickly even on a populated vault — the lock-free path.
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    let started = Instant::now();
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("projects")
        .assert()
        .success();
    assert!(
        started.elapsed().as_millis() < 1500,
        "projects took too long without contention: {:?}",
        started.elapsed()
    );
}

// ---------- 16.2 — lightweight smoke perf check ----------

/// Build a vault with 10 envs * 10 keys, then assert each read command stays
/// well under a coarse 1-second wall-clock budget. The 50 ms target
/// 16.2 assumes a release build; we run in debug here and use a generous cap
/// so the test stays signal-positive on slow CI without being noisy.
#[test]
fn smoke_perf_read_commands_complete_in_reasonable_time() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");

    // Bootstrap with a populated .env then fork 10 envs of 10 keys each.
    let env_body: String = (0..10).map(|i| format!("K{i}=v{i}\n")).collect();
    std::fs::write(cwd.path().join(".env"), env_body.as_bytes()).unwrap();
    for i in 0..10 {
        envroll_in(cwd.path(), xdg.path(), "p")
            .args(["fork", &format!("env{i}"), "--force"])
            .assert()
            .success();
    }

    // The 50 ms / 200 ms targets 16.2 / 16.3 assume a release
    // build. Debug mode runs scrypt 10×–50× slower; commands that decrypt
    // (status, log, diff) inherit that. We use coarse separate budgets so
    // the smoke test still catches order-of-magnitude regressions on PR CI.
    let no_decrypt_budget_ms: u128 = 1500; // list, current
    let decrypt_budget_ms: u128 = 10_000; // status, log, diff (one scrypt per ref)
    for (cmd, budget) in [
        (&["list"][..], no_decrypt_budget_ms),
        (&["current"][..], no_decrypt_budget_ms),
        (&["status"][..], decrypt_budget_ms),
        (&["log", "env0"][..], decrypt_budget_ms),
        (&["diff", "env0", "env1"][..], decrypt_budget_ms),
    ] {
        let started = Instant::now();
        envroll_in(cwd.path(), xdg.path(), "p")
            .args(cmd)
            .output()
            .unwrap();
        let elapsed = started.elapsed().as_millis();
        assert!(
            elapsed < budget,
            "{cmd:?} took {elapsed}ms, budget is {budget}ms"
        );
    }
}

// ---------- 15.4 — JSON output validates against documented schemas ----------

/// Check the JSON shape exposed by `envroll list --format json`. The full
/// schema lives at `docs/json-schemas/list.schema.json`; we exercise the
/// fields most likely to drift (active toggle, env list ordering).
#[test]
fn list_json_shape_is_stable() {
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
    let stdout = String::from_utf8(out.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(parsed.is_array());
    let row = &parsed[0];
    assert!(row.get("project_id").is_some());
    assert!(row.get("envs").is_some());
    assert!(row.get("active").is_some());
}

#[test]
fn log_json_shape_is_stable() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "dev"])
        .assert()
        .success();

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["log", "dev", "--format", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(parsed.is_array());
    let entry = &parsed[0];
    for required in [
        "hash",
        "added",
        "removed",
        "changed",
        "message",
        "timestamp",
    ] {
        assert!(
            entry.get(required).is_some(),
            "log entry missing field \"{required}\": {entry}"
        );
    }
    assert_eq!(
        entry["hash"].as_str().unwrap().len(),
        12,
        "hash should be 12 hex chars"
    );
}

#[test]
fn diff_json_shape_is_stable() {
    let (cwd, xdg) = sandbox();
    init(cwd.path(), xdg.path(), "p");
    std::fs::write(cwd.path().join(".env"), b"A=1\nB=2\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "a"])
        .assert()
        .success();
    std::fs::remove_file(cwd.path().join(".env")).unwrap();
    std::fs::write(cwd.path().join(".env"), b"A=99\nC=new\n").unwrap();
    envroll_in(cwd.path(), xdg.path(), "p")
        .args(["fork", "b"])
        .assert()
        .success();

    let out = envroll_in(cwd.path(), xdg.path(), "p")
        .args(["diff", "a", "b", "--format", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    for required in ["a", "b", "added", "removed", "changed"] {
        assert!(
            parsed.get(required).is_some(),
            "missing {required}: {parsed}"
        );
    }
}
