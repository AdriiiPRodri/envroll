//! Helpers shared across env-management / switching / variable-ops commands.
//!
//! Most commands follow the same opening sequence: resolve the vault root,
//! open the vault, derive the project ID, load the manifest, infer the mode
//! of `./.env`, acquire the appropriate lock, sweep orphan tempfiles. This
//! module bundles that into [`PreparedProject`] and a couple of constructors,
//! plus a few utilities every command needs (passphrase + canary, atomic
//! symlink swap, manifest commit, "create env from path" — the shared
//! pathway that powers both `envroll fork` and `envroll use --rescue`).

use std::path::{Path, PathBuf};

use age::secrecy::SecretString;

use crate::cli::Context;
use crate::crypto;
use crate::errors::{generic, usage, EnvrollError};
use crate::lock::{acquire_exclusive, acquire_shared, LockGuard};
use crate::manifest::{find_project_for_cwd, Manifest};
use crate::paths::{
    project_checkout, project_checkout_dir, project_dir, project_env_blob, project_envs_dir,
    project_manifest, resolve_vault_root, vault_lock_file,
};
use crate::prompt::{read_passphrase, PassphraseSources};
use crate::vault::fs as vfs;
use crate::vault::git::{RefForm, VaultRepo};
use crate::vault::{infer_mode, Mode, Vault};

/// Modes for [`open_project`]. Determines which lock we take and whether the
/// orphan-tempfile sweep runs.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum LockMode {
    /// `init`, `save`, `fork`, `set`, `copy`, `rm`, `rename`, `use`, `sync`,
    /// `remote *`, and the prepare phase of `edit`.
    Exclusive,
    /// `list`, `log`, `diff`, `get`, `status`, and the decrypt phase of `exec`.
    Shared,
}

/// Per-command state pulled together from cwd + manifest + lock. Drop this
/// struct to release the vault lock (the [`LockGuard`] field handles that
/// automatically).
pub struct PreparedProject {
    pub vault: Vault,
    pub repo: VaultRepo,
    pub manifest: Manifest,
    pub manifest_path: PathBuf,
    pub project_root: PathBuf,
    pub mode: Mode,
    pub _lock: LockGuard,
}

impl PreparedProject {
    pub fn project_id(&self) -> &str {
        &self.manifest.id
    }

    pub fn env_blob_path(&self, env_name: &str) -> PathBuf {
        project_env_blob(self.vault.root(), self.project_id(), env_name)
    }

    pub fn env_blob_relpath(&self, env_name: &str) -> PathBuf {
        PathBuf::from(format!(
            "projects/{}/envs/{}.age",
            self.project_id(),
            env_name
        ))
    }

    pub fn manifest_relpath(&self) -> PathBuf {
        PathBuf::from(format!("projects/{}/manifest.toml", self.project_id()))
    }

    pub fn checkout_path(&self, env_name: &str) -> PathBuf {
        project_checkout(self.vault.root(), self.project_id(), env_name)
    }

    pub fn checkout_dir(&self) -> PathBuf {
        project_checkout_dir(self.vault.root(), self.project_id())
    }

    /// Absolute path to the working-copy file inside the project root.
    /// Honors `manifest.target_filename` (default `.env`).
    pub fn dotenv_path(&self) -> PathBuf {
        self.project_root.join(&self.manifest.target_filename)
    }

    /// Persist `self.manifest` to disk (atomic write) and commit. Returns
    /// the OID of the commit. Use after every mutation that updates manifest
    /// fields.
    pub fn save_and_commit_manifest(&mut self, message: &str) -> Result<(), EnvrollError> {
        let toml = self.manifest.to_toml()?;
        vfs::atomic_write(&self.manifest_path, toml.as_bytes(), 0o644)?;
        self.repo.commit_blob(&self.manifest_relpath(), message)?;
        Ok(())
    }
}

/// Open the project this `cwd` belongs to.
pub fn open_project(ctx: &Context, lock_mode: LockMode) -> Result<PreparedProject, EnvrollError> {
    let cwd = std::env::current_dir().map_err(EnvrollError::Io)?;
    let vault_root = resolve_vault_root(ctx.vault.as_deref())?;
    let vault = Vault::open(&vault_root)?;
    let manifest = find_project_for_cwd(&vault, &cwd)?;
    let repo = VaultRepo::open(vault.root())?;
    let project_id = manifest.id.clone();
    let mode = infer_mode(&cwd, &vault, &project_id, &manifest.target_filename);

    // Acquire lock BEFORE any sweep so concurrent envroll commands serialize
    // correctly. The orphan sweep mutates the FS so it must hold the lock.
    let lock_path = vault_lock_file(vault.root());
    let lock = match lock_mode {
        LockMode::Exclusive => {
            let g = acquire_exclusive(&lock_path)?;
            // Sweep orphan tempfiles only on exclusive sessions.
            let _ = vfs::sweep_orphan_tempfiles(vault.root());
            g
        }
        LockMode::Shared => acquire_shared(&lock_path)?,
    };

    let manifest_path = project_manifest(vault.root(), &project_id);

    Ok(PreparedProject {
        vault,
        repo,
        manifest,
        manifest_path,
        project_root: cwd,
        mode,
        _lock: lock,
    })
}

/// Read a passphrase from the configured sources (no canary check).
pub fn read_pass(ctx: &Context) -> Result<SecretString, EnvrollError> {
    let sources = PassphraseSources::new(ctx.passphrase_stdin, ctx.passphrase_env.as_deref());
    read_passphrase(&sources, "envroll passphrase")
}

/// Read passphrase + verify canary. Used as the standard preface for
/// commands that touch encrypted content.
pub fn read_pass_and_verify(
    prep: &PreparedProject,
    ctx: &Context,
) -> Result<SecretString, EnvrollError> {
    let pass = read_pass(ctx)?;
    crypto::verify_canary(prep.vault.root(), &pass)?;
    Ok(pass)
}

/// Read the working copy of the active env. `Mode::Symlink` resolves the
/// symlink (read goes through it). `Mode::Copy` reads `./.env` directly.
/// Other modes return errors per the env-management spec.
pub fn read_working_copy(prep: &PreparedProject) -> Result<Vec<u8>, EnvrollError> {
    let env_path = prep.dotenv_path();
    match prep.mode {
        Mode::Symlink => std::fs::read(&env_path).map_err(EnvrollError::Io),
        Mode::Copy => std::fs::read(&env_path).map_err(EnvrollError::Io),
        Mode::None => Err(generic(
            "no working copy: ./.env does not exist (run `envroll use <name>` to activate one)",
        )),
        Mode::StaleOurSymlink => Err(generic(
            "./.env points into envroll's checkout dir but the target is gone; run `envroll use <active>` to recover",
        )),
        Mode::ForeignSymlink => Err(EnvrollError::UnmanagedEnvPresent(
            "./.env is a foreign symlink (not managed by envroll); resolve manually".to_string(),
        )),
    }
}

/// Encrypt `plaintext` with `pass` and atomically write the result to the env's
/// `.age` blob. Caller is responsible for the libgit2 commit (use
/// [`PreparedProject::repo.commit_blob`] with [`PreparedProject::env_blob_relpath`]).
pub fn write_env_blob(
    prep: &PreparedProject,
    env_name: &str,
    plaintext: &[u8],
    pass: &SecretString,
) -> Result<(), EnvrollError> {
    let blob = crypto::encrypt(plaintext, pass)?;
    let envs_dir = project_envs_dir(prep.vault.root(), prep.project_id());
    vfs::ensure_dir(&envs_dir, 0o700)?;
    vfs::atomic_write(&prep.env_blob_path(env_name), &blob, 0o600)
}

/// Write plaintext to `<vault>/projects/<id>/.checkout/<name>` so the symlink
/// (or copy) at `./.env` resolves to it. Mode 0600.
pub fn write_checkout(
    prep: &PreparedProject,
    env_name: &str,
    plaintext: &[u8],
) -> Result<(), EnvrollError> {
    vfs::ensure_dir(&prep.checkout_dir(), 0o700)?;
    vfs::atomic_write(&prep.checkout_path(env_name), plaintext, 0o600)
}

/// Atomically retarget `./.env` to point at `target_abs`. Either creates a
/// symlink (default) or copies the bytes (when `force_copy` is true OR a
/// symlink can't be created on this platform).
///
/// On either path the existing `./.env` is replaced via tempfile + rename,
/// so an interrupted call leaves `./.env` either at its prior state or the
/// new state — never half-written.
pub fn activate_dotenv(
    project_root: &Path,
    target_filename: &str,
    target_abs: &Path,
    force_copy: bool,
) -> Result<(), EnvrollError> {
    let env_path = project_root.join(target_filename);
    let pid = std::process::id();
    let rand = crate::paths::rand_hex6();
    let tmp = project_root.join(format!("{target_filename}.envroll-tmp.{pid}.{rand}"));
    let copy_mode = force_copy || std::env::var_os("ENVROLL_USE_COPY").is_some();

    if copy_mode {
        // Copy the bytes via atomic_write so perms (0600) are applied.
        let data = std::fs::read(target_abs).map_err(EnvrollError::Io)?;
        // atomic_write builds its own internal tempfile; for copy mode we
        // skip our outer tempfile dance.
        return vfs::atomic_write(&env_path, &data, 0o600);
    }

    // Symlink path. Try the OS symlink call first; if it fails (e.g. Windows
    // without Developer Mode), fall back to copy automatically.
    match make_symlink(target_abs, &tmp) {
        Ok(()) => {
            std::fs::rename(&tmp, &env_path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                EnvrollError::Io(e)
            })?;
            Ok(())
        }
        Err(_) => {
            let _ = std::fs::remove_file(&tmp);
            // Fallback to copy mode (Windows ERROR_PRIVILEGE_NOT_HELD path).
            let data = std::fs::read(target_abs).map_err(EnvrollError::Io)?;
            vfs::atomic_write(&env_path, &data, 0o600)
        }
    }
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn make_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(not(any(unix, windows)))]
fn make_symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlinks not supported on this platform",
    ))
}

/// Remove the working-copy file (whether symlink or regular file) if present.
/// No error if missing — the caller may have already removed it.
pub fn clear_dotenv(project_root: &Path, target_filename: &str) -> Result<(), EnvrollError> {
    let env_path = project_root.join(target_filename);
    match std::fs::symlink_metadata(&env_path) {
        Ok(_) => std::fs::remove_file(&env_path).map_err(EnvrollError::Io),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(EnvrollError::Io(e)),
    }
}

/// The shared `create_env_from_path` helper. Encrypts
/// `plaintext` as a new env named `name`, writes the corresponding checkout,
/// retargets `./.env` to point at it, updates the manifest's `active` (and
/// clears `active_ref`), and commits both the new `.age` blob and the
/// updated `manifest.toml` in a single libgit2 commit.
///
/// `default_msg` is used when `msg_override` is `None`. Refuses on collision
/// unless `force = true`, returning [`EnvrollError::NameCollision`].
///
/// This is the single code path for env creation. `envroll fork` calls it
/// directly; `envroll use --rescue` (section 10) calls it before activating
/// the originally-requested ref.
pub fn create_env_from_path(
    prep: &mut PreparedProject,
    name: &str,
    plaintext: &[u8],
    pass: &SecretString,
    default_msg: &str,
    msg_override: Option<&str>,
    force: bool,
) -> Result<(), EnvrollError> {
    if prep.env_blob_path(name).exists() && !force {
        return Err(EnvrollError::NameCollision(format!(
            "env \"{name}\" already exists; pass --force to overwrite"
        )));
    }

    write_env_blob(prep, name, plaintext, pass)?;
    write_checkout(prep, name, plaintext)?;

    // Retarget the working-copy file to the new checkout (absolute path, so
    // the symlink is valid regardless of cwd).
    let target = prep.checkout_path(name);
    activate_dotenv(
        &prep.project_root,
        &prep.manifest.target_filename,
        &target,
        false,
    )?;

    // Update manifest: active=name, clear active_ref.
    prep.manifest.active = name.to_string();
    prep.manifest.active_ref = String::new();

    let toml = prep.manifest.to_toml()?;
    vfs::atomic_write(&prep.manifest_path, toml.as_bytes(), 0o644)?;
    let msg = msg_override.unwrap_or(default_msg);
    prep.repo.commit_paths(
        &[&prep.env_blob_relpath(name), &prep.manifest_relpath()],
        msg,
    )?;
    // Make sure the project_dir itself exists (it does — but ensure_dir is
    // idempotent and cheap, and guarantees mode 0700).
    vfs::ensure_dir(&project_dir(prep.vault.root(), prep.project_id()), 0o700)?;

    Ok(())
}

/// ISO 8601 timestamp suitable for default commit messages and `created_at`
/// fields. Local time per env-management spec.
pub fn iso_now_local() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// The verbatim refuse message when `envroll save` (or `set`/`copy` against
/// the active env) would silently rewind a historically-pinned env
///. Lives here so callers stay byte-identical.
pub fn active_ref_pinned_message(name: &str, hash_part: &str) -> String {
    format!(
        "active env \"{name}\" is pinned to historical ref {hash_part}; saving here \
would create a new tip that rewinds to that historical content. \
To return to the latest version:    envroll use {name}. \
To intentionally rewind, run:       envroll save --force"
    )
}

/// Extract the `<short-hash>` portion of an `active_ref` value of shape
/// `<name>@<short-hash>`. Returns `None` if the ref doesn't have an `@`.
pub fn parse_active_ref_hash(active_ref: &str) -> Option<&str> {
    active_ref.split_once('@').map(|(_, h)| h)
}

/// Parse a `<ref>` argument shared between `use`, `exec`, `log`, `diff`.
/// Forms:
/// - `<name>` — the env's tip ([`RefForm::Latest`]).
/// - `<name>@<short-hash>` — short hash, MUST be `>= 7` ASCII hex chars.
/// - `<name>@~N` — relative offset, `N >= 1`.
///
/// Errors map to [`EnvrollError::RefNotFound`] (exit 21) so callers can treat
/// every malformed-ref case uniformly. The empty-name guard catches inputs
/// like `@deadbeef` that would otherwise sneak past as zero-length env names.
pub fn parse_ref(s: &str) -> Result<(String, RefForm), EnvrollError> {
    let (name, suffix) = match s.split_once('@') {
        None => return Ok((s.to_string(), RefForm::Latest)),
        Some((n, sfx)) => (n, sfx),
    };
    if name.is_empty() {
        return Err(EnvrollError::RefNotFound(format!("invalid ref: {s}")));
    }
    if suffix.is_empty() {
        return Err(EnvrollError::RefNotFound(format!(
            "ref has trailing `@` with no hash or offset: {s}"
        )));
    }
    if let Some(rest) = suffix.strip_prefix('~') {
        let n: u32 = rest
            .parse()
            .map_err(|_| EnvrollError::RefNotFound(format!("invalid offset in ref: {s}")))?;
        if n == 0 {
            return Err(EnvrollError::RefNotFound(format!(
                "offset must be >= 1: {s}"
            )));
        }
        return Ok((name.to_string(), RefForm::Offset(n)));
    }
    if suffix.len() < 7 {
        return Err(EnvrollError::RefNotFound(format!(
            "short hash must be >= 7 hex chars: {s}"
        )));
    }
    if !suffix.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(EnvrollError::RefNotFound(format!(
            "short hash must be hex: {s}"
        )));
    }
    Ok((name.to_string(), RefForm::ShortHash(suffix.to_string())))
}

/// 12-hex-char prefix of an OID. Used as the canonical
/// suffix on historical checkout filenames `<name>@<12hex>`.
pub fn short_oid_12(oid: git2::Oid) -> String {
    let s = oid.to_string();
    s[..12].to_string()
}

/// List the env names registered in `prep`'s project, alphabetically sorted.
/// Used by the missing-env-name error helpers to give a useful hint instead
/// of clap's bare "the following required arguments were not provided" line.
pub fn list_env_names(prep: &PreparedProject) -> Vec<String> {
    let envs_dir = crate::paths::project_envs_dir(prep.vault.root(), prep.project_id());
    let mut names: Vec<String> = std::fs::read_dir(&envs_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let p = e.path();
            let stem = p.file_stem()?.to_str()?.to_string();
            let ext = p.extension().and_then(|s| s.to_str())?;
            if ext.eq_ignore_ascii_case("age") {
                Some(stem)
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

/// Build a "no env name given" error for a command whose positional refers
/// to an EXISTING env (use, edit, rm, log, diff, exec, rename's first arg).
/// Lists the envs in the current project so the user does not have to
/// context-switch to `envroll list`.
///
/// `usage_line` is the documented invocation shown under the env list
/// (e.g., `"envroll log <ENV>"`). Returned as an [`EnvrollError::Usage`] so
/// miette renders the header and the help body with separate visual weight
/// (the `×` line and the `help:` block, respectively).
pub fn missing_existing_env_error(prep: &PreparedProject, usage_line: &str) -> EnvrollError {
    let names = list_env_names(prep);
    let body = if names.is_empty() {
        format!("This project has no envs yet — create one with `envroll fork <name>`.\nusage: {usage_line}")
    } else {
        format!(
            "Envs in this project: {}\nusage: {usage_line}",
            names.join(", ")
        )
    };
    usage("no env name given", Some(body))
}

/// Variant of [`missing_existing_env_error`] for commands whose positional
/// is the name of a NEW env that must NOT collide with an existing one
/// (currently just `fork`). The list is framed as "names to avoid" instead
/// of "pick one".
pub fn missing_new_env_error(prep: &PreparedProject, usage_line: &str) -> EnvrollError {
    let names = list_env_names(prep);
    let body = if names.is_empty() {
        format!("usage: {usage_line}")
    } else {
        format!(
            "Pick a name other than the existing envs ({}).\nusage: {usage_line}",
            names.join(", ")
        )
    };
    usage("no name given", Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ref_bare_name_is_latest() {
        let (n, f) = parse_ref("dev").unwrap();
        assert_eq!(n, "dev");
        assert!(matches!(f, RefForm::Latest));
    }

    #[test]
    fn parse_ref_short_hash_minimum_seven_chars() {
        let err = parse_ref("dev@abc1234").err().is_none(); // exactly 7
        assert!(err);
        let err = parse_ref("dev@abc123").unwrap_err(); // 6 — too short
        assert!(matches!(err, EnvrollError::RefNotFound(_)));
    }

    #[test]
    fn parse_ref_short_hash_must_be_hex() {
        let err = parse_ref("dev@xyzxyzxyz").unwrap_err();
        assert!(matches!(err, EnvrollError::RefNotFound(_)));
    }

    #[test]
    fn parse_ref_offset_form() {
        let (n, f) = parse_ref("dev@~3").unwrap();
        assert_eq!(n, "dev");
        match f {
            RefForm::Offset(3) => {}
            _ => panic!("expected Offset(3)"),
        }
    }

    #[test]
    fn parse_ref_offset_zero_is_rejected() {
        let err = parse_ref("dev@~0").unwrap_err();
        assert!(matches!(err, EnvrollError::RefNotFound(_)));
    }

    #[test]
    fn parse_ref_empty_name_is_rejected() {
        let err = parse_ref("@deadbeefcafe").unwrap_err();
        assert!(matches!(err, EnvrollError::RefNotFound(_)));
    }
}
