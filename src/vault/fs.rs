//! Filesystem helpers for the vault layer.
//!
//! - [`atomic_write`] writes via tempfile-in-same-dir + fsync + rename + parent
//!   fsync. The destination either contains the prior content unchanged or the
//!   full new content; no partial writes are observable.
//! - [`set_perms`] applies POSIX modes (best-effort no-op on Windows).
//! - [`sweep_orphan_tempfiles`] reaps tempfiles older than 60 seconds left
//!   behind by killed envroll processes.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::errors::EnvrollError;
use crate::paths::{rand_hex6, tempfile_path_with, TEMPFILE_PREFIX_INFIX};

/// Mtime threshold for orphan tempfile cleanup.
const ORPHAN_TEMPFILE_MAX_AGE: Duration = Duration::from_secs(60);

/// Atomically replace `dest` with `data`, applying POSIX `mode` to the new file.
///
/// Sequence:
///   1. Create `<dirname>/.<filename>.envroll-tmp.<pid>.<rand6>` with `mode` 0600/0644 etc.
///   2. Write the full payload, `flush`, `fsync`.
///   3. `rename` over `dest` (atomic on POSIX same-fs).
///   4. `fsync` the parent directory so the rename is durable across power loss.
///
/// Callers MUST ensure the parent directory exists; this function does not
/// create it (the vault layout creator does).
pub fn atomic_write(dest: &Path, data: &[u8], mode: u32) -> Result<(), EnvrollError> {
    let parent = dest.parent().ok_or_else(|| {
        EnvrollError::Generic(format!(
            "atomic_write: destination has no parent directory: {}",
            dest.display()
        ))
    })?;
    let pid = std::process::id();
    let tmp = tempfile_path_with(dest, pid, &rand_hex6());

    write_tempfile(&tmp, data, mode)?;
    // Best-effort cleanup if the rename fails: rename consumes the tempfile
    // on success, so we only need to remove it on the error path.
    if let Err(e) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(EnvrollError::Io(e));
    }
    fsync_dir(parent)?;
    Ok(())
}

fn write_tempfile(tmp: &Path, data: &[u8], mode: u32) -> Result<(), EnvrollError> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    apply_mode_to_open_options(&mut opts, mode);
    let mut f = opts.open(tmp).map_err(EnvrollError::Io)?;

    // On Windows the mode bits above are ignored; set_perms is also a no-op
    // there so this is a single code path.
    set_perms(tmp, mode)?;

    f.write_all(data).map_err(EnvrollError::Io)?;
    f.flush().map_err(EnvrollError::Io)?;
    f.sync_all().map_err(EnvrollError::Io)?;
    Ok(())
}

#[cfg(unix)]
fn apply_mode_to_open_options(opts: &mut OpenOptions, mode: u32) {
    use std::os::unix::fs::OpenOptionsExt;
    opts.mode(mode);
}

#[cfg(not(unix))]
fn apply_mode_to_open_options(_opts: &mut OpenOptions, _mode: u32) {
    // Windows: handled (best-effort) via set_perms after creation.
}

/// Apply POSIX mode bits to an existing path.
///
/// On Windows this is a best-effort no-op; the design accepts that mode bits
/// don't translate, and a future enhancement may set a current-user-only ACL
///.
pub fn set_perms(path: &Path, mode: u32) -> Result<(), EnvrollError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(path, perms).map_err(EnvrollError::Io)?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

/// Create `dir` if missing, then enforce `mode` on it (0700 for vault root /
/// `.checkout/`).
pub fn ensure_dir(dir: &Path, mode: u32) -> Result<(), EnvrollError> {
    std::fs::create_dir_all(dir).map_err(EnvrollError::Io)?;
    set_perms(dir, mode)?;
    Ok(())
}

#[cfg(unix)]
fn fsync_dir(dir: &Path) -> Result<(), EnvrollError> {
    let f = File::open(dir).map_err(EnvrollError::Io)?;
    // Some filesystems (notably tmpfs) reject directory fsync with EINVAL —
    // crash safety degrades but the directory is still consistent because the
    // rename was atomic; treat it as best-effort.
    if let Err(e) = f.sync_all() {
        if e.raw_os_error() != Some(libc_einval()) {
            return Err(EnvrollError::Io(e));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn fsync_dir(_dir: &Path) -> Result<(), EnvrollError> {
    // Windows has no equivalent of POSIX directory fsync; rename is durable
    // through the FS journal.
    Ok(())
}

#[cfg(unix)]
const fn libc_einval() -> i32 {
    22
}

/// Walk `vault_root` recursively and delete any file whose name matches the
/// tempfile pattern (`<dotfile>.envroll-tmp.<pid>.<rand6>`) AND whose mtime
/// is older than 60 seconds. Returns the count of files removed.
///
/// Errors during traversal are tolerated (best-effort cleanup); the function
/// returns `Ok(count)` even if some directories were unreadable.
pub fn sweep_orphan_tempfiles(vault_root: &Path) -> Result<usize, EnvrollError> {
    if !vault_root.exists() {
        return Ok(0);
    }
    let now = SystemTime::now();
    let mut removed = 0usize;
    let mut stack: Vec<PathBuf> = vec![vault_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        // Skip the libgit2 internal directory — git owns its own tempfiles
        // (e.g., `.lock`) and we must not touch them.
        if dir.file_name().and_then(|s| s.to_str()) == Some(".git") {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !is_envroll_tempfile_name(&path) {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mtime = match metadata.modified() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let is_old = now
                .duration_since(mtime)
                .map(|age| age >= ORPHAN_TEMPFILE_MAX_AGE)
                .unwrap_or(false);
            if is_old && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// Match the tempfile name pattern:
///   `^\.[^/]+\.envroll-tmp\.[0-9]+\.[0-9a-f]{6}$`
fn is_envroll_tempfile_name(path: &Path) -> bool {
    let name = match path.file_name().and_then(|s| s.to_str()) {
        Some(n) => n,
        None => return false,
    };
    if !name.starts_with('.') {
        return false;
    }
    let infix_at = match name.find(TEMPFILE_PREFIX_INFIX) {
        Some(i) => i,
        None => return false,
    };
    // After the infix we expect "<pid>.<rand6>" — pid digits, dot, 6 hex chars.
    let suffix = &name[infix_at + TEMPFILE_PREFIX_INFIX.len()..];
    let dot = match suffix.find('.') {
        Some(i) => i,
        None => return false,
    };
    let (pid_part, hex_part) = (&suffix[..dot], &suffix[dot + 1..]);
    if pid_part.is_empty() || !pid_part.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if hex_part.len() != 6
        || !hex_part
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn atomic_write_creates_destination_with_payload() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("envs").join("dev.age");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        atomic_write(&dest, b"hello", 0o600).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"hello");
    }

    #[test]
    fn atomic_write_replaces_existing_atomically() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("manifest.toml");
        fs::write(&dest, b"old").unwrap();
        atomic_write(&dest, b"new", 0o644).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_applies_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("secret.age");
        atomic_write(&dest, b"x", 0o600).unwrap();
        let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_dir_applies_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("nested").join(".checkout");
        ensure_dir(&target, 0o700).unwrap();
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn tempfile_pattern_match_accepts_documented_form() {
        assert!(is_envroll_tempfile_name(Path::new(
            ".dev.age.envroll-tmp.1234.abcdef"
        )));
        assert!(is_envroll_tempfile_name(Path::new(
            ".manifest.toml.envroll-tmp.7.012345"
        )));
    }

    #[test]
    fn tempfile_pattern_match_rejects_non_matches() {
        assert!(!is_envroll_tempfile_name(Path::new("dev.age"))); // no leading dot
        assert!(!is_envroll_tempfile_name(Path::new(
            ".dev.age.envroll-tmp.abc.012345"
        ))); // pid not numeric
        assert!(!is_envroll_tempfile_name(Path::new(
            ".dev.age.envroll-tmp.1234.AB12CD"
        ))); // uppercase hex
        assert!(!is_envroll_tempfile_name(Path::new(
            ".dev.age.envroll-tmp.1234.abcde"
        ))); // hex too short
        assert!(!is_envroll_tempfile_name(Path::new(
            ".dev.age.envroll-tmp.1234.abcdefg"
        ))); // hex too long
        assert!(!is_envroll_tempfile_name(Path::new(".vault.lock"))); // not our tempfile
    }

    #[test]
    fn sweep_removes_old_orphan_tempfiles() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("projects").join("p").join("envs");
        fs::create_dir_all(&nested).unwrap();
        let orphan = nested.join(".dev.age.envroll-tmp.999.abcdef");
        fs::write(&orphan, b"junk").unwrap();
        backdate_mtime(&orphan, Duration::from_secs(90));
        let removed = sweep_orphan_tempfiles(dir.path()).unwrap();
        assert_eq!(removed, 1, "old orphan tempfile should have been swept");
        assert!(!orphan.exists());
    }

    /// Push the file's mtime into the past by `delta`. Uses `std::fs::FileTimes`
    /// (stable since Rust 1.75; our MSRV is 1.89) so we don't need an external
    /// crate.
    fn backdate_mtime(path: &Path, delta: Duration) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        let past = SystemTime::now() - delta;
        let times = std::fs::FileTimes::new().set_modified(past);
        f.set_times(times).unwrap();
    }

    #[test]
    fn sweep_keeps_recent_orphan_tempfiles() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("projects").join("p").join("envs");
        fs::create_dir_all(&nested).unwrap();
        let recent = nested.join(".dev.age.envroll-tmp.1.abc123");
        fs::write(&recent, b"junk").unwrap();
        let removed = sweep_orphan_tempfiles(dir.path()).unwrap();
        assert_eq!(removed, 0);
        assert!(recent.exists());
    }

    #[test]
    fn sweep_skips_non_matching_files() {
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("projects").join("p").join("envs");
        fs::create_dir_all(&real).unwrap();
        let real_file = real.join("dev.age");
        fs::write(&real_file, b"real").unwrap();
        let removed = sweep_orphan_tempfiles(dir.path()).unwrap();
        assert_eq!(removed, 0);
        assert!(real_file.exists());
    }

    #[test]
    fn sweep_skips_dotgit_directory() {
        let dir = TempDir::new().unwrap();
        let git = dir.path().join(".git");
        fs::create_dir_all(&git).unwrap();
        // Even something that LOOKS like our tempfile pattern under .git
        // must be left alone — git owns that subtree.
        let inside = git.join(".HEAD.envroll-tmp.1.abc123");
        fs::write(&inside, b"hands off").unwrap();
        let _ = sweep_orphan_tempfiles(dir.path()).unwrap();
        assert!(inside.exists());
    }
}
