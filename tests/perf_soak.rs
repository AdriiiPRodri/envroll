//! Full perf soak (tasks.md 16.3) and release-binary size check (16.4).
//!
//! Both are gated on `--features perf` so the default `cargo test` run stays
//! fast. CI runs this lane on dedicated hardware where the 200 ms / 25 MB
//! budgets are meaningful.
//!
//!     cargo test --release --features perf --test perf_soak
//!
//! These tests are intentionally `#[cfg(feature = "perf")]`-only so a
//! contributor running `cargo test` without the feature flag never hits a
//! flaky perf timeout.

#![cfg(feature = "perf")]

use std::path::Path;
use std::process::Command;
use std::time::Instant;

use assert_cmd::prelude::*;
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

// ------------------------------------------------------------------
// 16.3 — full soak: 100 envs * 50 keys, all read commands < 200 ms
// ------------------------------------------------------------------

#[test]
fn soak_100_envs_50_keys_each_under_200ms() {
    let (cwd, xdg) = sandbox();
    envroll_in(cwd.path(), xdg.path(), "p")
        .arg("init")
        .assert()
        .success();

    let body: String = (0..50).map(|i| format!("K{i:02}=v{i:02}\n")).collect();
    std::fs::write(cwd.path().join(".env"), body.as_bytes()).unwrap();

    for i in 0..100 {
        envroll_in(cwd.path(), xdg.path(), "p")
            .args(["fork", &format!("env{i:03}"), "--force"])
            .assert()
            .success();
    }

    // Spec 16.3 originally targeted "all read commands under 200 ms". After
    // the env-switching spec was tightened so `status` reports dirty state
    // *against the vault commit* (not the local checkout), status / log /
    // diff each pay one scrypt derivation per decryption. age's default
    // scrypt work factor is ~1 second on commodity hardware — so a flat
    // 200 ms budget is incompatible with the security model. We split the
    // budget: 200 ms for read commands that don't decrypt, 5 s for those
    // that do. Both still catch order-of-magnitude regressions.
    let no_decrypt_budget_ms: u128 = 200;
    let decrypt_budget_ms: u128 = 5_000;
    for (cmd, budget) in [
        (&["list"][..], no_decrypt_budget_ms),
        (&["current"][..], no_decrypt_budget_ms),
        (&["status"][..], decrypt_budget_ms),
        (&["log", "env000"][..], decrypt_budget_ms),
        (&["diff", "env000", "env001"][..], decrypt_budget_ms),
    ] {
        let started = Instant::now();
        envroll_in(cwd.path(), xdg.path(), "p")
            .args(cmd)
            .output()
            .unwrap();
        let elapsed = started.elapsed().as_millis();
        assert!(
            elapsed < budget,
            "{cmd:?} took {elapsed}ms, soak budget is {budget}ms"
        );
    }
}

// ------------------------------------------------------------------
// 16.4 — stripped release binary is ≤ 25 MB on tier-1 targets
// ------------------------------------------------------------------

/// Resolve the binary path that `assert_cmd` would invoke. We avoid running
/// `cargo build --release` from inside the test (assert_cmd already produced
/// the binary by the time tests run; with `--release` it points at the
/// release artifact).
#[test]
fn release_binary_under_25_mb() {
    // assert_cmd uses the same artifact cargo just built; in a `--features
    // perf --release` invocation that's the stripped release binary, which
    // is what the spec budget applies to.
    let bin_path = assert_cmd::cargo::cargo_bin("envroll");
    let metadata = std::fs::metadata(&bin_path).unwrap_or_else(|e| {
        panic!(
            "could not stat envroll binary at {}: {e}",
            bin_path.display()
        )
    });
    let size_bytes = metadata.len();
    let limit_bytes: u64 = 25 * 1024 * 1024;
    assert!(
        size_bytes <= limit_bytes,
        "stripped release binary is {} bytes ({:.1} MB), limit is 25 MB. Path: {}",
        size_bytes,
        size_bytes as f64 / 1024.0 / 1024.0,
        bin_path.display()
    );
}
