//! Best-effort advisory vault lock (design.md D15).
//!
//! Lock kind per command:
//! - **Exclusive:** `init`, `save`, `fork`, `set`, `copy`, `rm`, `rename`,
//!   `use`, `sync`, `remote *`, `edit` (only during decrypt-to-checkout).
//! - **Shared:** `list`, `log`, `diff`, `get`, `status`, `exec` (only during
//!   decrypt-to-memory).
//! - **None:** `current`, `projects` — these only read `manifest.toml`, which
//!   is always written via tempfile+rename, so no torn read is possible.
//!
//! Implementation note: tasks.md 2.4 originally specified `fs2::FileExt`, but
//! Rust 1.89+ shipped `File::try_lock` / `try_lock_shared` in std with the
//! same flock semantics, making fs2 redundant. We use std directly and skip
//! the dep.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

use crate::errors::EnvrollError;

/// RAII guard for a held vault lock. Drop releases the lock automatically
/// when the underlying file descriptor closes.
///
/// `acquire_none()` returns a guard with `_file == None` so callers can use
/// a uniform return type whether they took the lock or deliberately skipped it.
pub struct LockGuard {
    _file: Option<File>,
}

/// Acquire an exclusive lock on the vault.
///
/// Non-blocking: if another envroll process holds the lock we surface
/// [`EnvrollError::VaultLockHeld`] immediately rather than blocking the
/// user's terminal. The lock file is created at `lock_path` if missing.
pub fn acquire_exclusive(lock_path: &Path) -> Result<LockGuard, EnvrollError> {
    let file = open_lock_file(lock_path)?;
    file.try_lock().map_err(map_try_lock_err)?;
    Ok(LockGuard { _file: Some(file) })
}

/// Acquire a shared lock on the vault. Multiple readers may hold shared locks
/// concurrently; a writer with [`acquire_exclusive`] is excluded.
pub fn acquire_shared(lock_path: &Path) -> Result<LockGuard, EnvrollError> {
    let file = open_lock_file(lock_path)?;
    file.try_lock_shared().map_err(map_try_lock_err)?;
    Ok(LockGuard { _file: Some(file) })
}

/// Return a no-op guard. For commands like `current` and `projects` that
/// deliberately take no lock; using this keeps call sites uniform.
pub fn acquire_none() -> LockGuard {
    LockGuard { _file: None }
}

fn open_lock_file(lock_path: &Path) -> Result<File, EnvrollError> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(EnvrollError::Io)
}

fn map_try_lock_err(e: TryLockError) -> EnvrollError {
    match e {
        TryLockError::WouldBlock => EnvrollError::VaultLockHeld,
        TryLockError::Error(io_err) => EnvrollError::Io(io_err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn exclusive_lock_blocks_a_second_exclusive() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".vault.lock");
        let _g1 = acquire_exclusive(&path).unwrap();
        let r2 = acquire_exclusive(&path);
        assert!(matches!(r2, Err(EnvrollError::VaultLockHeld)));
    }

    #[test]
    fn shared_locks_coexist() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".vault.lock");
        let _g1 = acquire_shared(&path).unwrap();
        let _g2 = acquire_shared(&path).unwrap();
    }

    #[test]
    fn dropping_exclusive_lets_a_new_one_acquire() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".vault.lock");
        {
            let _g = acquire_exclusive(&path).unwrap();
        }
        let _g2 = acquire_exclusive(&path).unwrap();
    }
}
