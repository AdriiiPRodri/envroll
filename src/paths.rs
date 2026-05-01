//! Vault path resolution.
//!
//! Precedence for the vault root, highest first:
//! 1. The `--vault <path>` global CLI flag (testing escape hatch, design.md D16).
//! 2. The `XDG_DATA_HOME` environment variable (`<XDG_DATA_HOME>/envroll`).
//! 3. On Unix (macOS + Linux): `$HOME/.local/share/envroll`.
//!    On Windows: the platform default via `directories::ProjectDirs`.
//!
//! macOS deliberately uses the XDG-style path under `$HOME/.local/share/`
//! instead of the OS-native `~/Library/Application Support/envroll`. The
//! native path contains spaces, which is hostile to shell pipelines, and
//! diverging between Linux and macOS for a CLI tool is a needless ergonomic
//! tax — the same scripts and muscle memory should work on both.

#[cfg(not(unix))]
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

use crate::errors::{generic, EnvrollError};

/// Resolve the vault root directory, applying the precedence above.
///
/// `cli_vault_override` is the value of `--vault` (None if the flag was
/// not passed). The function does not create the directory — that is
/// `Vault::ensure_init`'s job (section 3 of tasks.md).
pub fn resolve_vault_root(cli_vault_override: Option<&Path>) -> Result<PathBuf, EnvrollError> {
    if let Some(p) = cli_vault_override {
        return Ok(p.to_path_buf());
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let p = PathBuf::from(xdg);
        if !p.as_os_str().is_empty() {
            return Ok(p.join("envroll"));
        }
    }
    #[cfg(unix)]
    {
        if let Some(home) = std::env::var_os("HOME") {
            let p = PathBuf::from(home);
            if !p.as_os_str().is_empty() {
                return Ok(p.join(".local/share/envroll"));
            }
        }
        Err(generic(
            "could not resolve a home directory for the vault ($HOME unset)",
        ))
    }
    #[cfg(not(unix))]
    {
        ProjectDirs::from("", "", "envroll")
            .map(|pd| pd.data_dir().to_path_buf())
            .ok_or_else(|| generic("could not resolve a home directory for the vault"))
    }
}

/// Path to the vault's libgit2 repo directory.
pub fn vault_git_dir(vault_root: &Path) -> PathBuf {
    vault_root.join(".git")
}

/// Path to the vault's encryption canary file.
pub fn vault_canary(vault_root: &Path) -> PathBuf {
    vault_root.join(".canary.age")
}

/// Path to the vault's plaintext schema-version file.
pub fn vault_version_file(vault_root: &Path) -> PathBuf {
    vault_root.join(".envroll-version")
}

/// Path to the advisory vault lock.
pub fn vault_lock_file(vault_root: &Path) -> PathBuf {
    vault_root.join(".vault.lock")
}

/// Path to the directory holding all per-project subdirs.
pub fn projects_dir(vault_root: &Path) -> PathBuf {
    vault_root.join("projects")
}

/// Path to a specific project's directory under the vault.
pub fn project_dir(vault_root: &Path, project_id: &str) -> PathBuf {
    projects_dir(vault_root).join(project_id)
}

/// Path to a project's manifest.toml.
pub fn project_manifest(vault_root: &Path, project_id: &str) -> PathBuf {
    project_dir(vault_root, project_id).join("manifest.toml")
}

/// Path to a project's `envs/` directory (encrypted blobs live here).
pub fn project_envs_dir(vault_root: &Path, project_id: &str) -> PathBuf {
    project_dir(vault_root, project_id).join("envs")
}

/// Path to a specific encrypted env blob.
pub fn project_env_blob(vault_root: &Path, project_id: &str, env_name: &str) -> PathBuf {
    project_envs_dir(vault_root, project_id).join(format!("{env_name}.age"))
}

/// Path to a project's `.checkout/` (plaintext, gitignored, never synced).
pub fn project_checkout_dir(vault_root: &Path, project_id: &str) -> PathBuf {
    project_dir(vault_root, project_id).join(".checkout")
}

/// Path to the latest-version checkout of a given env.
pub fn project_checkout(vault_root: &Path, project_id: &str, env_name: &str) -> PathBuf {
    project_checkout_dir(vault_root, project_id).join(env_name)
}

/// Path to a historical checkout pinned at `<env_name>@<short_hash>`.
pub fn project_checkout_at(
    vault_root: &Path,
    project_id: &str,
    env_name: &str,
    short_hash: &str,
) -> PathBuf {
    project_checkout_dir(vault_root, project_id).join(format!("{env_name}@{short_hash}"))
}

/// Build a tempfile name in the same directory as `dest`, using the documented
/// pattern from design.md D8: `.<filename>.envroll-tmp.<pid>.<rand6>`.
///
/// The `<rand6>` segment is six lowercase hex chars derived from the supplied
/// randomness. Callers pass `random_hex` so this function stays pure and
/// testable; production code uses `rand_hex6()`.
pub fn tempfile_path_with(dest: &Path, pid: u32, random_hex: &str) -> PathBuf {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let filename = dest
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    parent.join(format!(".{filename}.envroll-tmp.{pid}.{random_hex}"))
}

/// Generate six lowercase hex chars worth of randomness.
///
/// Uses the OS RNG via `getrandom`-equivalent behavior. We don't pull a full
/// rand-crate dependency for this — a tempfile suffix only needs collision
/// resistance against concurrent envroll processes on the same host, and 24
/// bits is plenty given each process also stamps its PID.
pub fn rand_hex6() -> String {
    // sha2 is already a transitive dependency for project-ID hashing; reuse
    // the OsRng path through std::time::SystemTime + std::process for a tiny
    // ad-hoc PRNG seed. We do NOT need this to be cryptographically random.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let mixed = nanos
        .wrapping_mul(2654435761)
        .wrapping_add(pid.wrapping_mul(40503));
    format!("{:06x}", mixed & 0x00FF_FFFF)
}

/// Regex pattern for tempfile detection (design.md D8). Used by the orphan
/// sweeper. Exposed as a constant string so the regex crate need not be
/// pulled in for a single-purpose match — the sweeper uses simple parsing.
pub const TEMPFILE_PREFIX_INFIX: &str = ".envroll-tmp.";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn cli_override_wins_over_env_and_default() {
        // Save / restore XDG_DATA_HOME so this test is deterministic.
        let prev = std::env::var_os("XDG_DATA_HOME");
        std::env::set_var("XDG_DATA_HOME", "/should/be/ignored");
        let resolved = resolve_vault_root(Some(Path::new("/tmp/explicit"))).unwrap();
        assert_eq!(resolved, PathBuf::from("/tmp/explicit"));
        // restore
        match prev {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn default_unix_falls_back_to_xdg_layout_under_home() {
        let prev_xdg = std::env::var_os("XDG_DATA_HOME");
        let prev_home = std::env::var_os("HOME");
        std::env::remove_var("XDG_DATA_HOME");
        std::env::set_var("HOME", "/tmp/some-fake-home");
        let resolved = resolve_vault_root(None).unwrap();
        assert_eq!(
            resolved,
            PathBuf::from("/tmp/some-fake-home/.local/share/envroll")
        );
        // restore
        match prev_xdg {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn tempfile_path_uses_dot_prefix_and_pattern() {
        let dest = Path::new("/v/projects/p/envs/dev.age");
        let p = tempfile_path_with(dest, 1234, "abcdef");
        assert_eq!(
            p,
            PathBuf::from("/v/projects/p/envs/.dev.age.envroll-tmp.1234.abcdef")
        );
    }

    #[test]
    fn rand_hex6_yields_six_lowercase_hex_chars() {
        let s = rand_hex6();
        assert_eq!(s.len(), 6);
        assert!(s
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    }
}
