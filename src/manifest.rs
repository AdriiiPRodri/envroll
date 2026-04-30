//! Project manifest (`<vault>/projects/<id>/manifest.toml`) and project-ID
//! derivation (design.md D1, D2).
//!
//! The manifest holds ONLY project-level state that travels across machines
//! via vault sync. Three things explicitly NOT in here, per design.md D2/D9:
//! - `path` — re-derived from `cwd` at every command,
//! - `mode` — inferred at runtime from the on-disk type of `./.env`,
//! - `copy_hash` — replaced by parse-then-compare against `.checkout/<name>`.
//!
//! Project-ID derivation (D1): `--id` override → normalized `git remote
//! get-url origin` SHA-256 prefix → canonicalized abs-path SHA-256 prefix.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::errors::EnvrollError;
use crate::paths::{project_manifest, vault_canary};
use crate::vault::Vault;

/// Source of a project's ID. Persisted in `manifest.toml` alongside the ID
/// itself so future commands can reproduce the derivation logic and report
/// useful errors when the inputs change.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdSource {
    /// SHA-256 prefix of the normalized origin URL.
    Remote,
    /// SHA-256 prefix of the canonicalized absolute path of the project root.
    Path,
    /// User-supplied via `envroll init --id`.
    Manual,
}

/// On-disk schema for `<vault>/projects/<id>/manifest.toml`.
///
/// Field order matters for deterministic output via `toml::to_string` —
/// keep in sync with the table in design.md D2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub id: String,
    pub id_source: IdSource,
    /// Empty string unless `id_source = Remote` (where it holds the
    /// normalized origin URL). For `Path` and `Manual` we still emit the key
    /// to keep TOML shape stable.
    #[serde(default)]
    pub id_input: String,
    /// Name of the active env, or empty if no env is active.
    #[serde(default)]
    pub active: String,
    /// Set to `<name>@<short-hash>` only when pinned to a historical ref
    /// via `envroll use <name>@<...>` (design.md D18). Empty otherwise.
    #[serde(default)]
    pub active_ref: String,
    pub created_at: DateTime<Utc>,
}

impl Manifest {
    /// Construct a freshly-registered manifest with `created_at = now()` UTC,
    /// no active env, and no historical pin.
    pub fn new(id: String, id_source: IdSource, id_input: String) -> Self {
        Self {
            schema_version: 1,
            id,
            id_source,
            id_input,
            active: String::new(),
            active_ref: String::new(),
            created_at: Utc::now(),
        }
    }

    /// Read a manifest from disk. Errors:
    /// - file missing → [`EnvrollError::ProjectNotFound`],
    /// - I/O error → [`EnvrollError::Io`],
    /// - parse error → [`EnvrollError::Generic`] with the toml error.
    pub fn load(path: &Path) -> Result<Self, EnvrollError> {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(EnvrollError::ProjectNotFound)
            }
            Err(e) => return Err(EnvrollError::Io(e)),
        };
        toml::from_str(&raw).map_err(|e| {
            EnvrollError::Generic(format!("parsing manifest at {}: {e}", path.display()))
        })
    }

    /// Serialize to a TOML string. Caller is responsible for atomic writing
    /// (use [`crate::vault::fs::atomic_write`]).
    pub fn to_toml(&self) -> Result<String, EnvrollError> {
        toml::to_string(self)
            .map_err(|e| EnvrollError::Generic(format!("serializing manifest: {e}")))
    }
}

/// Outcome of [`derive_project_id`]. Carries enough info that the caller can
/// build a [`Manifest`] without re-running the derivation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdDerivation {
    /// User-supplied via `--id`.
    Manual { id: String },
    /// Derived from the project's `git remote get-url origin`. The
    /// `normalized_url` is what we hashed AND what we'll persist as
    /// `id_input` so future commands can detect URL changes.
    Remote { id: String, normalized_url: String },
    /// Derived from the canonicalized absolute path of the project root.
    /// `id_input` is empty for path-derived projects (the path is
    /// machine-local and is NOT persisted, design.md D2/D9).
    Path { id: String },
}

impl IdDerivation {
    pub fn id(&self) -> &str {
        match self {
            IdDerivation::Manual { id }
            | IdDerivation::Remote { id, .. }
            | IdDerivation::Path { id } => id,
        }
    }

    pub fn source(&self) -> IdSource {
        match self {
            IdDerivation::Manual { .. } => IdSource::Manual,
            IdDerivation::Remote { .. } => IdSource::Remote,
            IdDerivation::Path { .. } => IdSource::Path,
        }
    }

    /// Value to persist as `id_input`. Only the `Remote` variant carries one;
    /// the others are empty strings.
    pub fn id_input(&self) -> String {
        match self {
            IdDerivation::Remote { normalized_url, .. } => normalized_url.clone(),
            _ => String::new(),
        }
    }
}

/// Derive a project ID for `cwd` per design.md D1.
///
/// Order of precedence:
/// 1. `override_id` (from `--id`).
/// 2. Normalized `git remote get-url origin` if a libgit2 repo is discoverable
///    upward from `cwd` and an `origin` remote with a non-empty URL is set.
/// 3. SHA-256 prefix of the canonicalized absolute path of `cwd`.
///
/// `cwd` MUST exist; this function calls [`Path::canonicalize`] in case 3.
pub fn derive_project_id(
    cwd: &Path,
    override_id: Option<&str>,
) -> Result<IdDerivation, EnvrollError> {
    if let Some(id) = override_id {
        return Ok(IdDerivation::Manual { id: id.to_string() });
    }
    if let Some(url) = try_get_origin_url(cwd) {
        let normalized = normalize_origin_url(&url);
        if !normalized.is_empty() {
            let id = format!("remote-{}", short_hash16(normalized.as_bytes()));
            return Ok(IdDerivation::Remote {
                id,
                normalized_url: normalized,
            });
        }
    }
    let canonical = cwd.canonicalize().map_err(EnvrollError::Io)?;
    let id = format!(
        "path-{}",
        short_hash16(canonical.to_string_lossy().as_bytes())
    );
    Ok(IdDerivation::Path { id })
}

fn try_get_origin_url(cwd: &Path) -> Option<String> {
    let repo = git2::Repository::discover(cwd).ok()?;
    let remote = repo.find_remote("origin").ok()?;
    remote
        .url()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Canonicalize an origin URL per design.md D1. The two SSH/HTTPS forms of
/// the same project MUST normalize to the same string; otherwise the same
/// project would get two different IDs depending on which protocol the user's
/// `origin` was configured with.
///
/// Output shape: `<lowercased-host>/<path-without-.git-suffix>`. We keep path
/// case-sensitive because some hosts (e.g., gitlab.com self-hosted) treat
/// repo paths as case-sensitive.
pub fn normalize_origin_url(url: &str) -> String {
    let url = url.trim();

    // git@host:owner/repo[.git]
    if let Some(rest) = url.strip_prefix("git@") {
        if let Some((host, path)) = rest.split_once(':') {
            return format!(
                "{}/{}",
                host.to_ascii_lowercase(),
                strip_dot_git(path.trim_start_matches('/'))
            );
        }
    }

    for scheme in ["https://", "http://", "ssh://", "git://", "file://"] {
        if let Some(rest) = url.strip_prefix(scheme) {
            // Optional userinfo: drop everything up to and including the last '@'
            // before the first '/'.
            let host_part_end = rest.find('/').unwrap_or(rest.len());
            let (authority, slash_path) = rest.split_at(host_part_end);
            let host_with_port = authority
                .rsplit_once('@')
                .map(|(_, h)| h)
                .unwrap_or(authority);
            let host = host_with_port
                .split_once(':')
                .map(|(h, _)| h)
                .unwrap_or(host_with_port);
            let path = slash_path.trim_start_matches('/');
            if path.is_empty() {
                return host.to_ascii_lowercase();
            }
            return format!("{}/{}", host.to_ascii_lowercase(), strip_dot_git(path));
        }
    }

    // Unknown scheme: fall back to a best-effort lowercase + .git strip so
    // we still have *some* deterministic representation.
    strip_dot_git(&url.to_ascii_lowercase()).to_string()
}

fn strip_dot_git(s: &str) -> String {
    let trimmed = s.trim_end_matches('/');
    trimmed.strip_suffix(".git").unwrap_or(trimmed).to_string()
}

fn short_hash16(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(16);
    for b in digest.iter().take(8) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Locate the project this `cwd` belongs to (design.md D1, task 7.3).
///
/// 1. Re-derive the ID from `cwd` via [`derive_project_id`] (no override).
/// 2. Load `<vault>/projects/<id>/manifest.toml`.
/// 3. If it loads AND the manifest's `id_source` is `Remote`, sanity-check
///    that the manifest's stored `id_input` equals the URL we just normalized.
///    A mismatch here means either someone hand-edited the manifest, the
///    normalizer changed semantics, or there's a (vanishingly unlikely) hash
///    collision — surface the spec-mandated URL-change message in any case.
/// 4. If the manifest is missing, return [`EnvrollError::ProjectNotFound`]
///    (exit 22).
///
/// Callers that want to bypass derivation (e.g., `envroll save --id <prev>`)
/// should use [`find_project_by_id`] instead.
pub fn find_project_for_cwd(vault: &Vault, cwd: &Path) -> Result<Manifest, EnvrollError> {
    let derived = derive_project_id(cwd, None)?;
    let manifest_path = project_manifest(vault.root(), derived.id());
    let manifest = Manifest::load(&manifest_path)?;

    if let IdDerivation::Remote { normalized_url, .. } = &derived {
        if manifest.id_source == IdSource::Remote && manifest.id_input != *normalized_url {
            return Err(EnvrollError::Generic(format!(
                "this directory's origin URL has changed since registration (was {}, now {}). \
                 Pass --id {} to confirm reuse, or run `envroll init` to register as a new project.",
                manifest.id_input, normalized_url, manifest.id
            )));
        }
    }
    Ok(manifest)
}

/// Look up a project manifest by explicit ID (used when the user passes `--id`).
/// Skips derivation and the URL-change defensive check entirely — passing
/// `--id` is the user's way of saying "yes, I know, treat this directory as
/// that project".
pub fn find_project_by_id(vault: &Vault, project_id: &str) -> Result<Manifest, EnvrollError> {
    let manifest_path = project_manifest(vault.root(), project_id);
    Manifest::load(&manifest_path)
}

/// Sanity check that the vault canary file exists at `vault.root()`. Useful
/// for callers who want a one-line guard before a passphrase prompt; the
/// actual decrypt-the-canary check lives in `crypto::verify_canary`.
pub fn canary_present(vault: &Vault) -> bool {
    vault_canary(vault.root()).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use age::secrecy::SecretString;
    use std::fs;
    use tempfile::TempDir;

    // ---------- normalize_origin_url ----------

    #[test]
    fn normalize_ssh_and_https_yield_same_canonical() {
        assert_eq!(
            normalize_origin_url("git@github.com:acme/widgets.git"),
            normalize_origin_url("https://github.com/acme/widgets")
        );
    }

    #[test]
    fn normalize_strips_dot_git_suffix() {
        assert_eq!(
            normalize_origin_url("https://github.com/acme/widgets.git"),
            "github.com/acme/widgets"
        );
    }

    #[test]
    fn normalize_lowercases_host() {
        assert_eq!(
            normalize_origin_url("https://GITHUB.com/Acme/Widgets"),
            "github.com/Acme/Widgets"
        );
    }

    #[test]
    fn normalize_drops_userinfo() {
        assert_eq!(
            normalize_origin_url("https://user:token@github.com/acme/widgets.git"),
            "github.com/acme/widgets"
        );
    }

    #[test]
    fn normalize_drops_port() {
        assert_eq!(
            normalize_origin_url("ssh://git@github.com:22/acme/widgets.git"),
            "github.com/acme/widgets"
        );
    }

    #[test]
    fn normalize_handles_trailing_slash() {
        assert_eq!(
            normalize_origin_url("https://github.com/acme/widgets/"),
            "github.com/acme/widgets"
        );
    }

    // ---------- derive_project_id ----------

    /// Initialize a git repo at `dir` with the given `origin` URL.
    fn init_repo_with_origin(dir: &Path, url: &str) {
        let repo = git2::Repository::init(dir).unwrap();
        repo.remote("origin", url).unwrap();
    }

    #[test]
    fn derive_with_override_id_short_circuits() {
        let dir = TempDir::new().unwrap();
        let d = derive_project_id(dir.path(), Some("custom-id")).unwrap();
        assert!(matches!(&d, IdDerivation::Manual { id } if id == "custom-id"));
        assert_eq!(d.source(), IdSource::Manual);
        assert_eq!(d.id_input(), "");
    }

    #[test]
    fn derive_uses_origin_url_when_present() {
        let dir = TempDir::new().unwrap();
        init_repo_with_origin(dir.path(), "git@github.com:acme/widgets.git");
        let d = derive_project_id(dir.path(), None).unwrap();
        match d {
            IdDerivation::Remote { id, normalized_url } => {
                assert!(id.starts_with("remote-"));
                assert_eq!(id.len(), "remote-".len() + 16);
                assert_eq!(normalized_url, "github.com/acme/widgets");
            }
            other => panic!("expected Remote, got {other:?}"),
        }
    }

    #[test]
    fn derive_ssh_and_https_origins_yield_same_id() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        init_repo_with_origin(dir_a.path(), "git@github.com:acme/widgets.git");
        init_repo_with_origin(dir_b.path(), "https://github.com/acme/widgets");
        let id_a = derive_project_id(dir_a.path(), None)
            .unwrap()
            .id()
            .to_string();
        let id_b = derive_project_id(dir_b.path(), None)
            .unwrap()
            .id()
            .to_string();
        assert_eq!(id_a, id_b);
    }

    #[test]
    fn derive_renamed_folder_with_same_origin_keeps_same_id() {
        // Same origin, two different physical paths → same id.
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        init_repo_with_origin(dir_a.path(), "git@github.com:acme/widgets.git");
        init_repo_with_origin(dir_b.path(), "git@github.com:acme/widgets.git");
        assert_eq!(
            derive_project_id(dir_a.path(), None).unwrap().id(),
            derive_project_id(dir_b.path(), None).unwrap().id()
        );
    }

    #[test]
    fn derive_falls_back_to_path_when_no_remote() {
        // Plain directory, no .git at all.
        let dir = TempDir::new().unwrap();
        let d = derive_project_id(dir.path(), None).unwrap();
        match &d {
            IdDerivation::Path { id } => {
                assert!(id.starts_with("path-"));
                assert_eq!(id.len(), "path-".len() + 16);
            }
            other => panic!("expected Path, got {other:?}"),
        }
        assert_eq!(d.id_input(), "");
    }

    #[test]
    fn derive_path_same_dir_yields_same_id() {
        let dir = TempDir::new().unwrap();
        let id1 = derive_project_id(dir.path(), None)
            .unwrap()
            .id()
            .to_string();
        let id2 = derive_project_id(dir.path(), None)
            .unwrap()
            .id()
            .to_string();
        assert_eq!(id1, id2);
    }

    #[test]
    fn derive_path_different_dirs_yield_different_ids() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let id_a = derive_project_id(a.path(), None).unwrap().id().to_string();
        let id_b = derive_project_id(b.path(), None).unwrap().id().to_string();
        assert_ne!(id_a, id_b);
    }

    // ---------- Manifest round-trip ----------

    #[test]
    fn manifest_round_trips_through_toml() {
        let m = Manifest::new(
            "remote-deadbeef12345678".into(),
            IdSource::Remote,
            "github.com/acme/widgets".into(),
        );
        let s = m.to_toml().unwrap();
        let m2: Manifest = toml::from_str(&s).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn manifest_load_missing_file_returns_project_not_found() {
        let dir = TempDir::new().unwrap();
        let err = Manifest::load(&dir.path().join("nope.toml")).unwrap_err();
        assert!(matches!(err, EnvrollError::ProjectNotFound));
    }

    // ---------- find_project_for_cwd / find_project_by_id ----------

    fn fresh_vault() -> (TempDir, Vault) {
        let dir = TempDir::new().unwrap();
        let v = Vault::ensure_init(dir.path(), &SecretString::from("p".to_string())).unwrap();
        (dir, v)
    }

    fn write_project_manifest(vault: &Vault, m: &Manifest) {
        let path = project_manifest(vault.root(), &m.id);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, m.to_toml().unwrap()).unwrap();
    }

    #[test]
    fn find_project_for_cwd_returns_manifest_when_origin_matches() {
        let (_vd, vault) = fresh_vault();
        let proj = TempDir::new().unwrap();
        init_repo_with_origin(proj.path(), "git@github.com:acme/widgets.git");
        let derived = derive_project_id(proj.path(), None).unwrap();
        let manifest = Manifest::new(
            derived.id().to_string(),
            IdSource::Remote,
            derived.id_input(),
        );
        write_project_manifest(&vault, &manifest);

        let loaded = find_project_for_cwd(&vault, proj.path()).unwrap();
        assert_eq!(loaded.id, manifest.id);
        assert_eq!(loaded.id_source, IdSource::Remote);
        assert_eq!(loaded.id_input, "github.com/acme/widgets");
    }

    #[test]
    fn find_project_for_cwd_when_unregistered_returns_project_not_found() {
        let (_vd, vault) = fresh_vault();
        let proj = TempDir::new().unwrap();
        let err = find_project_for_cwd(&vault, proj.path()).unwrap_err();
        assert!(matches!(err, EnvrollError::ProjectNotFound));
    }

    #[test]
    fn find_project_for_cwd_refuses_when_id_input_drifts_from_origin() {
        // Manifest was registered with one URL; the project's current origin
        // hash-collides with the same project ID but the persisted id_input
        // differs. We simulate this by hand-editing the manifest's id_input
        // to a different normalized URL after the fact — the spec-mandated
        // refuse path triggers.
        let (_vd, vault) = fresh_vault();
        let proj = TempDir::new().unwrap();
        init_repo_with_origin(proj.path(), "git@github.com:acme/widgets.git");
        let derived = derive_project_id(proj.path(), None).unwrap();
        let mut manifest = Manifest::new(
            derived.id().to_string(),
            IdSource::Remote,
            "github.com/acme/STALE-URL".to_string(),
        );
        manifest.id_input = "github.com/acme/STALE-URL".to_string();
        write_project_manifest(&vault, &manifest);

        let err = find_project_for_cwd(&vault, proj.path()).unwrap_err();
        match err {
            EnvrollError::Generic(msg) => {
                assert!(msg.contains("origin URL has changed"), "msg was: {msg}");
                assert!(msg.contains("Pass --id"), "msg was: {msg}");
            }
            other => panic!("expected Generic with URL-change message, got {other:?}"),
        }
    }

    #[test]
    fn find_project_by_id_bypasses_derivation_and_url_check() {
        let (_vd, vault) = fresh_vault();
        let m = Manifest::new(
            "manual-anything".to_string(),
            IdSource::Manual,
            String::new(),
        );
        write_project_manifest(&vault, &m);
        let loaded = find_project_by_id(&vault, "manual-anything").unwrap();
        assert_eq!(loaded.id, "manual-anything");
        assert_eq!(loaded.id_source, IdSource::Manual);
    }
}
