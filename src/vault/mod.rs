//! Vault filesystem abstraction.
//!
//! - [`Vault`]: the resolved vault root + the operations that load / initialize
//!   it on disk.
//! - [`Mode`] + [`infer_mode`]: runtime classification of `./.env`.
//!   No persisted `mode` field; we always re-derive at command time.
//! - [`fs`]: low-level filesystem helpers (atomic write, perms, orphan sweep).
//!
//! This module owns creation of everything *except* the libgit2 repo
//! (handled separately in `src/vault/git.rs`) and the per-project subdirs
//! (handled by the project lifecycle commands).

pub mod fs;
pub mod git;

use age::secrecy::SecretString;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::crypto;
use crate::errors::EnvrollError;
use crate::paths::{project_checkout_dir, vault_canary, vault_git_dir, vault_version_file};

/// Schema version this binary writes and is willing to read. A vault with a
/// `.envroll-version` value greater than this MUST be refused per the
/// project-lifecycle spec ("Binary encounters a future-version vault").
pub const VAULT_SCHEMA_VERSION: u32 = 1;

/// Vault root mode.
const VAULT_ROOT_MODE: u32 = 0o700;

/// Mode for plaintext metadata files (`.envroll-version`, `.gitignore`,
/// `manifest.toml`). No secrets, so 0644 is fine.
const META_FILE_MODE: u32 = 0o644;

/// Lines written into `<vault>/.gitignore` on first init.
/// Every commit synced to a remote MUST have these excluded so plaintext
/// `.checkout/` never leaves the local machine.
const GITIGNORE_BODY: &str = concat!(
    "# Managed by envroll — do not edit\n",
    "**/.checkout/\n",
    ".vault.lock\n",
);

/// Handle to an initialized vault. Construct via [`Vault::open`] (read-only)
/// or [`Vault::ensure_init`] (creates the on-disk layout if missing).
#[derive(Debug, Clone)]
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    /// Open an existing vault at `root`.
    ///
    /// Verifies that:
    /// - the root directory exists,
    /// - `.envroll-version` exists and parses to a number,
    /// - the recorded version is `<= VAULT_SCHEMA_VERSION` — newer vaults are
    ///   refused with the spec-mandated upgrade message.
    ///
    /// Does NOT verify the canary (that's a separate step performed by every
    /// command that touches encrypted content; the canary check needs the
    /// passphrase, which `Vault::open` deliberately does not).
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, EnvrollError> {
        let root = root.into();
        if !root.is_dir() {
            return Err(EnvrollError::ProjectNotFound);
        }
        let version = read_vault_version(&root)?;
        if version > VAULT_SCHEMA_VERSION {
            return Err(EnvrollError::Generic(
                "this vault was created by a newer envroll; please upgrade".to_string(),
            ));
        }
        Ok(Self { root })
    }

    /// Ensure the vault layout exists at `root`, creating any missing pieces
    /// idempotently:
    /// - root dir at 0700,
    /// - `.envroll-version` (if missing) with the current schema version,
    /// - `.gitignore` (if missing) with the documented body,
    /// - `.canary.age` (if missing) encrypted with `passphrase`.
    ///
    /// Does NOT touch the libgit2 repo at `<root>/.git` — that is the caller's
    /// responsibility (the `init` command, once the git layer lands in section 4).
    /// Does NOT prompt the user; the caller is responsible for sourcing the
    /// passphrase via [`crate::prompt`].
    ///
    /// Returns a [`Vault`] handle pointing at the now-initialized root.
    pub fn ensure_init(
        root: impl Into<PathBuf>,
        passphrase: &SecretString,
    ) -> Result<Self, EnvrollError> {
        let root = root.into();
        fs::ensure_dir(&root, VAULT_ROOT_MODE)?;

        let version_path = vault_version_file(&root);
        if !version_path.exists() {
            fs::atomic_write(
                &version_path,
                format!("{VAULT_SCHEMA_VERSION}\n").as_bytes(),
                META_FILE_MODE,
            )?;
        } else {
            // Already present — re-validate that we can read it. This catches
            // a future-version vault on a re-init attempt.
            let v = read_vault_version(&root)?;
            if v > VAULT_SCHEMA_VERSION {
                return Err(EnvrollError::Generic(
                    "this vault was created by a newer envroll; please upgrade".to_string(),
                ));
            }
        }

        let gitignore = root.join(".gitignore");
        if !gitignore.exists() {
            fs::atomic_write(&gitignore, GITIGNORE_BODY.as_bytes(), META_FILE_MODE)?;
        }

        let canary = vault_canary(&root);
        if !canary.exists() {
            crypto::create_canary(&root, passphrase)?;
        }

        Ok(Self { root })
    }

    /// Absolute path to the vault root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path to the libgit2 working dir (`<root>/.git`).
    pub fn git_dir(&self) -> PathBuf {
        vault_git_dir(&self.root)
    }
}

/// Read `<root>/.envroll-version` and parse it as a `u32`. Whitespace
/// (including a trailing newline) is trimmed. Errors:
/// - file missing → `Generic("vault not initialized — run `envroll init`")`,
/// - unparseable → `Generic(...)` with a descriptive message.
fn read_vault_version(root: &Path) -> Result<u32, EnvrollError> {
    let path = vault_version_file(root);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(EnvrollError::Generic(
                "vault not initialized — run `envroll init` here first".to_string(),
            ));
        }
        Err(e) => return Err(EnvrollError::Io(e)),
    };
    raw.trim().parse::<u32>().map_err(|_| {
        EnvrollError::Generic(format!(
            "vault version file at {} is unparseable",
            path.display()
        ))
    })
}

/// Runtime classification of `./.env`.
///
/// We never persist this — every command re-derives it from the on-disk type
/// of `./.env`. That keeps `manifest.toml` machine-independent (the same file
/// is shared across machines via vault sync) while still letting each machine
/// behave correctly for its local mode.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Mode {
    /// `./.env` is absent — no working copy on this machine.
    None,

    /// `./.env` is a symlink whose target lives inside our project's
    /// `.checkout/` directory and the target file exists. Reads/writes go
    /// through the symlink (which resolves to the live checkout).
    Symlink,

    /// `./.env` is a symlink into our `.checkout/` but the target file is
    /// gone (e.g., the user `rm -rf`'d the checkout dir). Recoverable: the
    /// next `envroll use <name>` re-decrypts and the rename overwrites the
    /// dangling link, no `--force` required (env-switching spec).
    StaleOurSymlink,

    /// `./.env` is a symlink to anywhere outside our `.checkout/`, broken or
    /// not. Refuse all working-copy ops without `--force` / `--rescue`.
    ForeignSymlink,

    /// `./.env` is a regular file (not a symlink). This is copy-mode — either
    /// because `ENVROLL_USE_COPY=1` was set, or because the platform refused
    /// symlink creation (Windows without Developer Mode).
    Copy,
}

/// Inspect `./.env` under `project_root` and classify it.
///
/// `project_id` is the registered ID for this project; we use it to compute
/// the path of `<vault>/projects/<id>/.checkout/` so we can decide whether a
/// symlink target is "ours". The function does NOT take the vault lock and
/// does NOT touch any encrypted blob — it is pure FS inspection, suitable
/// for `status` (shared lock) or any read path.
pub fn infer_mode(project_root: &Path, vault: &Vault, project_id: &str) -> Mode {
    let env_path = project_root.join(".env");
    let symlink_meta = match std::fs::symlink_metadata(&env_path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Mode::None,
        // Permission / IO errors here are surprising; treat as foreign so the
        // command refuses safely rather than silently overwriting.
        Err(_) => return Mode::ForeignSymlink,
    };

    if symlink_meta.file_type().is_symlink() {
        let target = match std::fs::read_link(&env_path) {
            Ok(t) => t,
            Err(_) => return Mode::ForeignSymlink,
        };
        let absolute_target = if target.is_absolute() {
            target
        } else {
            env_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(target)
        };
        let normalized_target = lexically_normalize(&absolute_target);
        let checkout_root = lexically_normalize(&project_checkout_dir(vault.root(), project_id));
        if normalized_target.starts_with(&checkout_root) {
            // Check if the resolved target file actually exists.
            if std::fs::metadata(&env_path).is_ok() {
                Mode::Symlink
            } else {
                Mode::StaleOurSymlink
            }
        } else {
            Mode::ForeignSymlink
        }
    } else if symlink_meta.file_type().is_file() {
        Mode::Copy
    } else {
        // Directory or other unexpected type at `./.env`. Treat as foreign so
        // we refuse instead of clobbering.
        Mode::ForeignSymlink
    }
}

/// Default TTL (in days) for historical-checkout files when neither
/// `ENVROLL_HISTORICAL_TTL_DAYS` nor a config-file override is set.
pub const DEFAULT_HISTORICAL_TTL_DAYS: u32 = 7;

/// Read the configured TTL for historical checkouts, in days. Precedence:
/// 1. `ENVROLL_HISTORICAL_TTL_DAYS` env var (any non-numeric value falls
///    through to the default — silent ignore is safer here than refusing the
///    whole command).
/// 2. *(future: `<vault>/.config.toml` `historical_ttl_days = N`)*
/// 3. [`DEFAULT_HISTORICAL_TTL_DAYS`].
///
/// A value of `0` disables sweeping entirely.
pub fn historical_ttl_days(_vault_root: &Path) -> u32 {
    if let Ok(raw) = std::env::var("ENVROLL_HISTORICAL_TTL_DAYS") {
        if let Ok(n) = raw.trim().parse::<u32>() {
            return n;
        }
    }
    DEFAULT_HISTORICAL_TTL_DAYS
}

/// Sweep stale historical-checkout files (`<name>@<12hex>`) under one project.
///
/// Eligibility: file matches `<name>@<12 hex chars>` AND its mtime is older
/// than the configured TTL AND no commit on `<name>`'s history starts with
/// `<12 hex>` (i.e., the commit it pinned has been orphaned). The sweeper
/// MUST NOT delete a file that is the live target of `./.env` for this
/// project, even past TTL — checked via the optional `project_root`.
///
/// Returns the count of files removed. Errors are tolerated per-file (the
/// sweeper is best-effort cleanup, not a critical path).
///
/// Callers should only invoke this from commands that already touch
/// `.checkout/` (use, save, fork, set, copy, edit, rm, rename, status, diff,
/// log). Read-only commands that don't (current, projects, list, get) MUST
/// NOT call this — sweeping under a shared lock breaks the lock-kind invariant.
pub fn sweep_historical_checkouts(
    vault: &Vault,
    repo: &git::VaultRepo,
    project_id: &str,
    project_root: &Path,
) -> usize {
    let ttl_days = historical_ttl_days(vault.root());
    if ttl_days == 0 {
        return 0;
    }
    let ttl = Duration::from_secs(u64::from(ttl_days) * 86_400);
    let checkout_dir = project_checkout_dir(vault.root(), project_id);
    let entries = match std::fs::read_dir(&checkout_dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    let now = SystemTime::now();
    let live_target = read_dotenv_target(project_root);

    // Memoize per-env reachability so we never re-walk history for the same env.
    let scope = repo.project(project_id);
    let mut reachable_per_env: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let (env_name, hash_part) = match name.split_once('@') {
            Some(parts) => parts,
            None => continue,
        };
        if hash_part.len() != 12 || !hash_part.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }

        // In-use guard: never delete the live target of ./.env.
        if let Some(t) = live_target.as_ref() {
            if lexically_normalize(t) == lexically_normalize(&path) {
                continue;
            }
        }

        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = match meta.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let aged = now.duration_since(mtime).map(|d| d >= ttl).unwrap_or(false);
        if !aged {
            continue;
        }

        let reachable = reachable_per_env
            .entry(env_name.to_string())
            .or_insert_with(|| {
                scope
                    .commit_history(env_name)
                    .map(|oids| oids.iter().map(|o| o.to_string()).collect())
                    .unwrap_or_default()
            });
        if reachable.iter().any(|oid| oid.starts_with(hash_part)) {
            continue;
        }

        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Read `./.env`'s symlink target as an absolute path, if it exists and is a
/// symlink. Returns `None` for regular files, missing files, or read errors.
fn read_dotenv_target(project_root: &Path) -> Option<PathBuf> {
    let env_path = project_root.join(".env");
    let target = std::fs::read_link(&env_path).ok()?;
    if target.is_absolute() {
        Some(target)
    } else {
        let parent = env_path.parent()?;
        Some(parent.join(target))
    }
}

/// Lexically normalize a path: collapse `.` and `..` against earlier components
/// without consulting the filesystem. Required for [`infer_mode`]'s
/// "is the symlink target inside our .checkout?" check, which must work even
/// when the target is broken (FS canonicalize would fail there).
///
/// This is a deliberately small subset of what crates like `path-clean` do:
/// we never need to handle Windows verbatim prefixes here because every input
/// originates from either an absolute path we built or a `read_link` result
/// joined onto an absolute parent.
fn lexically_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in p.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let last = out.components().next_back();
                match last {
                    Some(Component::Normal(_)) => {
                        out.pop();
                    }
                    // Parent of root (or a Windows prefix) is itself; absorb
                    // the `..` silently. This matches POSIX `cd /; cd ..`.
                    Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                    // Either `out` is empty (relative path starting with `..`)
                    // or it already ends in `..` — keep stacking.
                    _ => out.push(".."),
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use age::secrecy::SecretString;
    use std::fs;
    use tempfile::TempDir;

    fn pass(s: &str) -> SecretString {
        SecretString::from(s.to_string())
    }

    // ---------- ensure_init / open ----------

    #[test]
    fn ensure_init_creates_full_layout() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("envroll");
        let v = Vault::ensure_init(&root, &pass("p")).unwrap();
        assert_eq!(v.root(), root);
        assert!(root.is_dir());
        assert!(vault_version_file(&root).exists());
        assert!(root.join(".gitignore").exists());
        assert!(vault_canary(&root).exists());

        let version = fs::read_to_string(vault_version_file(&root)).unwrap();
        assert_eq!(version.trim(), "1");

        let gi = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gi.contains("**/.checkout/"));
        assert!(gi.contains(".vault.lock"));
    }

    #[test]
    fn ensure_init_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("envroll");
        Vault::ensure_init(&root, &pass("p")).unwrap();
        let canary_before = fs::read(vault_canary(&root)).unwrap();
        // Second call with a DIFFERENT passphrase must not rewrite the canary
        // (that would silently rotate the vault key).
        Vault::ensure_init(&root, &pass("different")).unwrap();
        let canary_after = fs::read(vault_canary(&root)).unwrap();
        assert_eq!(canary_before, canary_after);
    }

    #[test]
    fn open_succeeds_on_an_initialized_vault() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("envroll");
        Vault::ensure_init(&root, &pass("p")).unwrap();
        let v = Vault::open(&root).unwrap();
        assert_eq!(v.root(), root);
    }

    #[test]
    fn open_fails_on_uninitialized_root() {
        let dir = TempDir::new().unwrap();
        let err = Vault::open(dir.path()).unwrap_err();
        // Either Generic("vault not initialized…") if the dir exists but no
        // version file, or ProjectNotFound if the dir is missing entirely.
        // tempfile creates the dir, so we expect Generic.
        match err {
            EnvrollError::Generic(msg) => assert!(msg.contains("vault not initialized")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn open_fails_on_future_version() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("envroll");
        Vault::ensure_init(&root, &pass("p")).unwrap();
        // Simulate a vault created by a v0.2 binary.
        fs::write(vault_version_file(&root), "2\n").unwrap();
        let err = Vault::open(&root).unwrap_err();
        match err {
            EnvrollError::Generic(msg) => assert!(msg.contains("created by a newer envroll")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn ensure_init_refuses_future_version_when_root_already_exists() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("envroll");
        fs::create_dir_all(&root).unwrap();
        fs::write(vault_version_file(&root), "99\n").unwrap();
        let err = Vault::ensure_init(&root, &pass("p")).unwrap_err();
        match err {
            EnvrollError::Generic(msg) => assert!(msg.contains("created by a newer envroll")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    // ---------- infer_mode ----------

    fn setup_project(root: &Path, project_id: &str) -> Vault {
        let v = Vault::ensure_init(root, &pass("p")).unwrap();
        // Manually create the project's .checkout dir (project lifecycle
        // commands normally do this in 8.1; here we shortcut for the FS
        // inspection test).
        let co = project_checkout_dir(v.root(), project_id);
        fs::create_dir_all(&co).unwrap();
        v
    }

    #[test]
    fn infer_mode_none_when_dotenv_absent() {
        let proj = TempDir::new().unwrap();
        let vault_dir = TempDir::new().unwrap();
        let v = setup_project(vault_dir.path(), "remote-abc");
        assert_eq!(infer_mode(proj.path(), &v, "remote-abc"), Mode::None);
    }

    #[test]
    fn infer_mode_copy_when_dotenv_is_regular_file() {
        let proj = TempDir::new().unwrap();
        let vault_dir = TempDir::new().unwrap();
        let v = setup_project(vault_dir.path(), "remote-abc");
        fs::write(proj.path().join(".env"), b"FOO=bar\n").unwrap();
        assert_eq!(infer_mode(proj.path(), &v, "remote-abc"), Mode::Copy);
    }

    #[cfg(unix)]
    #[test]
    fn infer_mode_symlink_when_pointing_into_our_checkout() {
        let proj = TempDir::new().unwrap();
        let vault_dir = TempDir::new().unwrap();
        let v = setup_project(vault_dir.path(), "remote-abc");
        let target = project_checkout_dir(v.root(), "remote-abc").join("dev");
        fs::write(&target, b"FOO=bar\n").unwrap();
        std::os::unix::fs::symlink(&target, proj.path().join(".env")).unwrap();
        assert_eq!(infer_mode(proj.path(), &v, "remote-abc"), Mode::Symlink);
    }

    #[cfg(unix)]
    #[test]
    fn infer_mode_stale_when_symlink_into_checkout_target_missing() {
        let proj = TempDir::new().unwrap();
        let vault_dir = TempDir::new().unwrap();
        let v = setup_project(vault_dir.path(), "remote-abc");
        let target = project_checkout_dir(v.root(), "remote-abc").join("dev");
        // do NOT write target — the symlink will be dangling
        std::os::unix::fs::symlink(&target, proj.path().join(".env")).unwrap();
        assert_eq!(
            infer_mode(proj.path(), &v, "remote-abc"),
            Mode::StaleOurSymlink
        );
    }

    #[cfg(unix)]
    #[test]
    fn infer_mode_foreign_when_symlink_target_outside_checkout() {
        let proj = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let vault_dir = TempDir::new().unwrap();
        let v = setup_project(vault_dir.path(), "remote-abc");
        let target = other.path().join("foreign.env");
        fs::write(&target, b"FOO=bar\n").unwrap();
        std::os::unix::fs::symlink(&target, proj.path().join(".env")).unwrap();
        assert_eq!(
            infer_mode(proj.path(), &v, "remote-abc"),
            Mode::ForeignSymlink
        );
    }

    #[cfg(unix)]
    #[test]
    fn infer_mode_foreign_when_broken_symlink_outside_checkout() {
        let proj = TempDir::new().unwrap();
        let vault_dir = TempDir::new().unwrap();
        let v = setup_project(vault_dir.path(), "remote-abc");
        std::os::unix::fs::symlink("/no/such/path/foreign.env", proj.path().join(".env")).unwrap();
        assert_eq!(
            infer_mode(proj.path(), &v, "remote-abc"),
            Mode::ForeignSymlink
        );
    }

    #[test]
    fn lexical_normalize_collapses_parent_dirs() {
        assert_eq!(
            lexically_normalize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
    }

    #[test]
    fn lexical_normalize_keeps_root() {
        assert_eq!(
            lexically_normalize(Path::new("/../../a")),
            PathBuf::from("/a")
        );
    }
}
