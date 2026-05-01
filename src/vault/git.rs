//! libgit2 wrapper for the vault repo (`<vault>/.git`).
//!
//! This module owns every git operation envroll performs. Callers see a small
//! safe surface — [`VaultRepo`] for vault-wide ops (init, commit, dirty
//! check, remotes), [`ProjectScope`] for per-project ref resolution and
//! history walking. We do NOT expose `git2` types in the public signatures
//! beyond [`Oid`] (which is a 20-byte hash and convenient enough to leak).
//!
//! Author identity is fixed at `envroll <envroll@local>` so the user's
//! personal git identity never leaks into the vault history.
//! Default branch is `main`.

use std::path::{Path, PathBuf};

use git2::{
    BranchType, Cred, CredentialType, FetchOptions, Oid, PushOptions, RemoteCallbacks, Repository,
    RepositoryInitOptions, ResetType, Sort, StatusOptions,
};

use crate::errors::EnvrollError;

/// Fixed author/committer identity for every vault commit.
const AUTHOR_NAME: &str = "envroll";
const AUTHOR_EMAIL: &str = "envroll@local";

/// Default branch name. Vaults are always single-branch.
const DEFAULT_BRANCH: &str = "main";

/// Conventional remote name for the optional sync remote.
const ORIGIN: &str = "origin";

/// How to resolve a ref to a single commit on a given env.
#[derive(Clone, Debug)]
pub enum RefForm {
    /// `<name>` — the env's tip.
    Latest,
    /// `<name>@<short-hash>` — the (unique) commit whose OID starts with `s`
    /// AND that touches `envs/<name>.age`. Min length 7 hex chars.
    ShortHash(String),
    /// `<name>@~N` — the commit `N` versions back from the env's tip
    /// (1 == one before tip, etc.).
    Offset(u32),
}

/// Owning handle to the vault's libgit2 repo. Cheap to construct (just opens
/// the repo) and to drop. Holds a `git2::Repository` internally.
pub struct VaultRepo {
    repo: Repository,
    vault_root: PathBuf,
}

impl VaultRepo {
    /// Open or initialize the libgit2 repo at `<vault_root>/.git` (4.1).
    ///
    /// Idempotent: if the repo already exists, this just opens it.
    /// Initializes with the default branch set to [`DEFAULT_BRANCH`].
    pub fn ensure_init(vault_root: &Path) -> Result<Self, EnvrollError> {
        let mut opts = RepositoryInitOptions::new();
        opts.initial_head(DEFAULT_BRANCH);
        opts.no_reinit(false);
        let repo = Repository::init_opts(vault_root, &opts).map_err(map_git_err)?;
        Ok(Self {
            repo,
            vault_root: vault_root.to_path_buf(),
        })
    }

    /// Open an existing repo at `<vault_root>/.git`. Errors if the repo is not
    /// initialized.
    pub fn open(vault_root: &Path) -> Result<Self, EnvrollError> {
        let repo = Repository::open(vault_root).map_err(map_git_err)?;
        Ok(Self {
            repo,
            vault_root: vault_root.to_path_buf(),
        })
    }

    /// 4.2: Stage and commit a single file (relative to the vault root).
    /// Returns the new commit's OID.
    pub fn commit_blob(&self, rel_path: &Path, message: &str) -> Result<Oid, EnvrollError> {
        self.commit_paths(&[rel_path], message)
    }

    /// 4.3: Stage and commit multiple files (each relative to the vault root)
    /// in a single commit. Files are added (or updated) via `Index::add_path`;
    /// deleted files should be staged via [`Self::commit_paths_with_removals`]
    /// (added in section 9.7).
    pub fn commit_paths(&self, rel_paths: &[&Path], message: &str) -> Result<Oid, EnvrollError> {
        let mut index = self.repo.index().map_err(map_git_err)?;
        for p in rel_paths {
            // git2 panics if given an absolute path; assert relativity early.
            debug_assert!(p.is_relative(), "commit_paths requires repo-relative paths");
            // If the working-copy file is missing, treat as a remove.
            if !self.vault_root.join(p).exists() {
                index.remove_path(p).map_err(map_git_err)?;
            } else {
                index.add_path(p).map_err(map_git_err)?;
            }
        }
        index.write().map_err(map_git_err)?;
        let tree_oid = index.write_tree().map_err(map_git_err)?;
        let tree = self.repo.find_tree(tree_oid).map_err(map_git_err)?;
        let sig = git2::Signature::now(AUTHOR_NAME, AUTHOR_EMAIL).map_err(map_git_err)?;

        let parent_commit = match self.repo.head() {
            Ok(h) => Some(h.peel_to_commit().map_err(map_git_err)?),
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => None,
            Err(e) => return Err(map_git_err(e)),
        };
        let parents: Vec<&git2::Commit<'_>> = parent_commit.as_ref().into_iter().collect();
        let oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .map_err(map_git_err)?;
        Ok(oid)
    }

    /// 4.6: Is the working tree dirty? Per the sync pre-flight,
    /// "dirty" means any tracked file has uncommitted changes OR there is any
    /// non-ignored untracked file under the vault root.
    pub fn is_working_tree_dirty(&self) -> Result<bool, EnvrollError> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        opts.include_ignored(false);
        let statuses = self.repo.statuses(Some(&mut opts)).map_err(map_git_err)?;
        Ok(!statuses.is_empty())
    }

    /// 4.7: Set or replace the `origin` remote URL. No network call.
    pub fn remote_set(&self, url: &str) -> Result<(), EnvrollError> {
        match self.repo.find_remote(ORIGIN) {
            Ok(_) => self.repo.remote_set_url(ORIGIN, url).map_err(map_git_err)?,
            Err(_) => {
                self.repo.remote(ORIGIN, url).map_err(map_git_err)?;
            }
        }
        Ok(())
    }

    /// Show the configured `origin` URL, or `None` if no remote is set.
    pub fn remote_show(&self) -> Result<Option<String>, EnvrollError> {
        match self.repo.find_remote(ORIGIN) {
            Ok(r) => Ok(r.url().map(|s| s.to_string())),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(map_git_err(e)),
        }
    }

    /// Remove the `origin` remote. No-op if absent.
    pub fn remote_unset(&self) -> Result<(), EnvrollError> {
        match self.repo.remote_delete(ORIGIN) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(()),
            Err(e) => Err(map_git_err(e)),
        }
    }

    /// Fetch from `origin`. Maps any transport-layer failure to
    /// [`EnvrollError::RemoteTransportError`] so the caller can exit 42.
    pub fn fetch(&self) -> Result<(), EnvrollError> {
        let mut remote = self
            .repo
            .find_remote(ORIGIN)
            .map_err(|_| EnvrollError::NoRemote)?;
        let mut fo = FetchOptions::new();
        fo.remote_callbacks(default_callbacks());
        let refspec = format!("+refs/heads/*:refs/remotes/{ORIGIN}/*");
        remote
            .fetch(&[refspec.as_str()], Some(&mut fo), None)
            .map_err(|e| EnvrollError::RemoteTransportError(e.to_string()))
    }

    /// Push the local `main` to `origin/main`. The remote will reject a
    /// non-fast-forward push; we additionally check ancestry locally so
    /// callers see [`EnvrollError::SyncConflict`] before any network round-trip
    /// in the divergence case (the conflict-detection lives in the sync
    /// command, not here).
    pub fn push_fast_forward(&self) -> Result<(), EnvrollError> {
        let mut remote = self
            .repo
            .find_remote(ORIGIN)
            .map_err(|_| EnvrollError::NoRemote)?;
        let mut po = PushOptions::new();
        po.remote_callbacks(default_callbacks());
        let refspec = format!("refs/heads/{DEFAULT_BRANCH}:refs/heads/{DEFAULT_BRANCH}");
        remote
            .push(&[refspec.as_str()], Some(&mut po))
            .map_err(|e| EnvrollError::RemoteTransportError(e.to_string()))
    }

    /// Is `ancestor` an ancestor of (or equal to) `descendant`? Used by
    /// `envroll sync` to classify the local-vs-remote relationship before
    /// deciding ff-pull / ff-push / divergence.
    pub fn is_ancestor(&self, ancestor: Oid, descendant: Oid) -> Result<bool, EnvrollError> {
        if ancestor == descendant {
            return Ok(true);
        }
        self.repo
            .graph_descendant_of(descendant, ancestor)
            .map_err(map_git_err)
    }

    /// OID of the local `main` branch tip, or `None` if the branch has no
    /// commits yet (unborn).
    pub fn local_head(&self) -> Result<Option<Oid>, EnvrollError> {
        match self.repo.head() {
            Ok(h) => h
                .target()
                .map(Some)
                .ok_or_else(|| EnvrollError::Generic("HEAD has no target OID".into())),
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => Ok(None),
            Err(e) => Err(map_git_err(e)),
        }
    }

    /// OID of `refs/remotes/origin/<DEFAULT_BRANCH>`, or `None` if the remote
    /// hasn't been fetched yet (or doesn't have a `main`).
    pub fn remote_head(&self) -> Result<Option<Oid>, EnvrollError> {
        let refname = format!("refs/remotes/{ORIGIN}/{DEFAULT_BRANCH}");
        match self.repo.find_reference(&refname) {
            Ok(r) => Ok(r.target()),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(map_git_err(e)),
        }
    }

    /// Hard-reset the working tree to `commit`. Used by `envroll sync` after
    /// a successful fast-forward fetch to advance `main` to the remote tip.
    pub fn fast_forward_to(&self, commit: Oid) -> Result<(), EnvrollError> {
        let obj = self.repo.find_object(commit, None).map_err(map_git_err)?;
        // Move the branch ref AND check out the tree.
        self.repo
            .reset(&obj, ResetType::Hard, None)
            .map_err(map_git_err)?;
        // Make sure `main` itself points at the new commit (in case HEAD was
        // detached for any reason).
        let branch = self
            .repo
            .find_branch(DEFAULT_BRANCH, BranchType::Local)
            .map_err(map_git_err)?;
        branch
            .into_reference()
            .set_target(commit, "envroll fast-forward")
            .map_err(map_git_err)?;
        Ok(())
    }

    /// Scope subsequent ref resolution / history walking to a specific project.
    pub fn project<'r>(&'r self, project_id: &str) -> ProjectScope<'r> {
        ProjectScope {
            vault: self,
            project_id: project_id.to_string(),
        }
    }
}

/// Per-project view of the vault repo. Internal `repo` access is via the
/// borrowed [`VaultRepo`].
pub struct ProjectScope<'r> {
    vault: &'r VaultRepo,
    project_id: String,
}

impl<'r> ProjectScope<'r> {
    /// 4.5: Newest-first list of commits whose tree changed `envs/<env_name>.age`
    /// for this project. The first element is the env's tip (== Latest).
    pub fn commit_history(&self, env_name: &str) -> Result<Vec<Oid>, EnvrollError> {
        let blob_path = self.env_blob_path(env_name);
        let mut walk = self.vault.repo.revwalk().map_err(map_git_err)?;
        walk.push_head().or_else(|e| {
            // A vault that has been `init`ed but has no commits yet hits one
            // of several "no HEAD" error shapes depending on libgit2 version.
            // We treat any error whose class is `Reference` as "no history"
            // and yield an empty result, not a propagated error.
            match e.code() {
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound => Ok(()),
                _ if e.class() == git2::ErrorClass::Reference => Ok(()),
                _ => Err(map_git_err(e)),
            }
        })?;
        walk.set_sorting(Sort::TIME).map_err(map_git_err)?;

        let mut commits: Vec<git2::Commit<'_>> = Vec::new();
        for oid in walk {
            let oid = oid.map_err(map_git_err)?;
            commits.push(self.vault.repo.find_commit(oid).map_err(map_git_err)?);
        }
        // Walk newest-first; for each commit compare blob OID at path against
        // its parent's blob OID. Differing (or missing on one side) => touched.
        let mut out = Vec::new();
        for commit in &commits {
            let cur_blob = blob_at(commit, &blob_path);
            let parent_blob = if commit.parent_count() == 0 {
                None
            } else {
                let parent = commit.parent(0).map_err(map_git_err)?;
                blob_at(&parent, &blob_path)
            };
            if cur_blob != parent_blob {
                out.push(commit.id());
            }
        }
        Ok(out)
    }

    /// 4.4: Resolve a ref form for an env to a single commit OID.
    pub fn resolve_ref(&self, env_name: &str, form: RefForm) -> Result<Oid, EnvrollError> {
        let history = self.commit_history(env_name)?;
        if history.is_empty() {
            return Err(EnvrollError::EnvNotFound(env_name.to_string()));
        }
        match form {
            RefForm::Latest => Ok(history[0]),
            RefForm::Offset(n) => {
                let idx = n as usize;
                if idx == 0 {
                    return Err(EnvrollError::RefNotFound(format!(
                        "offset must be >= 1: {env_name}@~{n}"
                    )));
                }
                history.get(idx).copied().ok_or_else(|| {
                    EnvrollError::RefNotFound(format!(
                        "{env_name}@~{n}: only {} historical version(s) available",
                        history.len().saturating_sub(1)
                    ))
                })
            }
            RefForm::ShortHash(s) => self.resolve_short_hash(env_name, &s, &history),
        }
    }

    fn resolve_short_hash(
        &self,
        env_name: &str,
        short: &str,
        history: &[Oid],
    ) -> Result<Oid, EnvrollError> {
        if short.len() < 7 {
            return Err(EnvrollError::RefNotFound(format!(
                "short hash must be >= 7 hex chars: {env_name}@{short}"
            )));
        }
        if !short.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(EnvrollError::RefNotFound(format!(
                "short hash must be hex: {env_name}@{short}"
            )));
        }
        let lower = short.to_ascii_lowercase();
        let matches: Vec<Oid> = history
            .iter()
            .copied()
            .filter(|oid| oid.to_string().starts_with(&lower))
            .collect();
        match matches.len() {
            0 => Err(EnvrollError::RefNotFound(format!("{env_name}@{short}"))),
            1 => Ok(matches[0]),
            _ => Err(EnvrollError::RefNotFound(format!(
                "ambiguous: {env_name}@{short} matches {} commits",
                matches.len()
            ))),
        }
    }

    fn env_blob_path(&self, env_name: &str) -> PathBuf {
        PathBuf::from(format!(
            "projects/{}/envs/{}.age",
            self.project_id, env_name
        ))
    }
}

/// Look up the blob OID at `path` in `commit`'s tree, or `None` if absent.
fn blob_at(commit: &git2::Commit<'_>, path: &Path) -> Option<Oid> {
    let tree = commit.tree().ok()?;
    let entry = tree.get_path(path).ok()?;
    Some(entry.id())
}

/// Default credential callbacks for fetch/push: SSH agent for ssh remotes,
/// the user's git credential helper for https. Sufficient for `file://`
/// (which needs no creds) and the common interactive setups.
fn default_callbacks<'cb>() -> RemoteCallbacks<'cb> {
    let mut cb = RemoteCallbacks::new();
    cb.credentials(|url, username_from_url, allowed_types| {
        if allowed_types.contains(CredentialType::SSH_KEY) {
            return Cred::ssh_key_from_agent(username_from_url.unwrap_or("git"));
        }
        if allowed_types.contains(CredentialType::USER_PASS_PLAINTEXT) {
            if let Ok(config) = git2::Config::open_default() {
                return Cred::credential_helper(&config, url, username_from_url);
            }
        }
        if allowed_types.contains(CredentialType::DEFAULT) {
            return Cred::default();
        }
        Err(git2::Error::from_str(
            "no supported credential type for this remote",
        ))
    });
    cb
}

fn map_git_err(e: git2::Error) -> EnvrollError {
    EnvrollError::Generic(format!("libgit2: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn touch(root: &Path, rel: &str, content: &[u8]) -> PathBuf {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, content).unwrap();
        p
    }

    fn fresh_vault() -> (TempDir, VaultRepo) {
        let dir = TempDir::new().unwrap();
        let repo = VaultRepo::ensure_init(dir.path()).unwrap();
        (dir, repo)
    }

    // ---------- 4.1, 4.2, 4.3 ----------

    #[test]
    fn ensure_init_creates_main_branch_repo() {
        let (dir, repo) = fresh_vault();
        assert!(dir.path().join(".git").is_dir());
        // initial_head sets HEAD to refs/heads/main even before any commit.
        let head_ref = repo.repo.find_reference("HEAD").unwrap();
        assert_eq!(
            head_ref.symbolic_target(),
            Some(format!("refs/heads/{DEFAULT_BRANCH}").as_str())
        );
    }

    #[test]
    fn ensure_init_is_idempotent() {
        let dir = TempDir::new().unwrap();
        VaultRepo::ensure_init(dir.path()).unwrap();
        // Second call must not error.
        VaultRepo::ensure_init(dir.path()).unwrap();
    }

    #[test]
    fn commit_blob_creates_an_initial_commit() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        let oid = repo
            .commit_blob(Path::new("projects/p1/envs/dev.age"), "first")
            .unwrap();
        let head = repo.local_head().unwrap().unwrap();
        assert_eq!(head, oid);
        let commit = repo.repo.find_commit(oid).unwrap();
        assert_eq!(commit.message(), Some("first"));
        assert_eq!(commit.author().name(), Some(AUTHOR_NAME));
        assert_eq!(commit.author().email(), Some(AUTHOR_EMAIL));
    }

    #[test]
    fn commit_paths_handles_multi_file_commit() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        touch(dir.path(), "projects/p1/manifest.toml", b"id=\"p1\"\n");
        let oid = repo
            .commit_paths(
                &[
                    Path::new("projects/p1/envs/dev.age"),
                    Path::new("projects/p1/manifest.toml"),
                ],
                "register",
            )
            .unwrap();
        let commit = repo.repo.find_commit(oid).unwrap();
        let tree = commit.tree().unwrap();
        assert!(tree.get_path(Path::new("projects/p1/envs/dev.age")).is_ok());
        assert!(tree
            .get_path(Path::new("projects/p1/manifest.toml"))
            .is_ok());
    }

    #[test]
    fn commit_blob_handles_deletion_when_file_missing() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        repo.commit_blob(Path::new("projects/p1/envs/dev.age"), "create")
            .unwrap();
        fs::remove_file(dir.path().join("projects/p1/envs/dev.age")).unwrap();
        let oid = repo
            .commit_blob(Path::new("projects/p1/envs/dev.age"), "remove")
            .unwrap();
        let tree = repo.repo.find_commit(oid).unwrap().tree().unwrap();
        assert!(tree
            .get_path(Path::new("projects/p1/envs/dev.age"))
            .is_err());
    }

    // ---------- 4.4, 4.5 ----------

    #[test]
    fn commit_history_returns_only_commits_touching_the_blob() {
        let (dir, repo) = fresh_vault();
        // dev v1
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        let c1 = repo
            .commit_blob(Path::new("projects/p1/envs/dev.age"), "dev v1")
            .unwrap();
        // unrelated commit on staging
        touch(dir.path(), "projects/p1/envs/staging.age", b"sv1");
        let _ = repo
            .commit_blob(Path::new("projects/p1/envs/staging.age"), "staging v1")
            .unwrap();
        // dev v2
        touch(dir.path(), "projects/p1/envs/dev.age", b"v2");
        let c2 = repo
            .commit_blob(Path::new("projects/p1/envs/dev.age"), "dev v2")
            .unwrap();

        let history = repo.project("p1").commit_history("dev").unwrap();
        assert_eq!(history, vec![c2, c1]);
    }

    #[test]
    fn resolve_ref_latest_returns_tip() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        let _ = repo
            .commit_blob(Path::new("projects/p1/envs/dev.age"), "v1")
            .unwrap();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v2");
        let c2 = repo
            .commit_blob(Path::new("projects/p1/envs/dev.age"), "v2")
            .unwrap();

        let resolved = repo
            .project("p1")
            .resolve_ref("dev", RefForm::Latest)
            .unwrap();
        assert_eq!(resolved, c2);
    }

    #[test]
    fn resolve_ref_offset_n_returns_n_commits_back() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        let c1 = repo
            .commit_blob(Path::new("projects/p1/envs/dev.age"), "v1")
            .unwrap();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v2");
        let _ = repo
            .commit_blob(Path::new("projects/p1/envs/dev.age"), "v2")
            .unwrap();
        let r = repo
            .project("p1")
            .resolve_ref("dev", RefForm::Offset(1))
            .unwrap();
        assert_eq!(r, c1);
    }

    #[test]
    fn resolve_ref_offset_zero_is_rejected() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        repo.commit_blob(Path::new("projects/p1/envs/dev.age"), "v1")
            .unwrap();
        let err = repo
            .project("p1")
            .resolve_ref("dev", RefForm::Offset(0))
            .unwrap_err();
        assert!(matches!(err, EnvrollError::RefNotFound(_)));
    }

    #[test]
    fn resolve_ref_offset_out_of_range_is_rejected() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        repo.commit_blob(Path::new("projects/p1/envs/dev.age"), "v1")
            .unwrap();
        let err = repo
            .project("p1")
            .resolve_ref("dev", RefForm::Offset(5))
            .unwrap_err();
        assert!(matches!(err, EnvrollError::RefNotFound(_)));
    }

    #[test]
    fn resolve_ref_short_hash_min_length_enforced() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        repo.commit_blob(Path::new("projects/p1/envs/dev.age"), "v1")
            .unwrap();
        let err = repo
            .project("p1")
            .resolve_ref("dev", RefForm::ShortHash("abcdef".into())) // only 6
            .unwrap_err();
        match err {
            EnvrollError::RefNotFound(msg) => assert!(msg.contains(">= 7")),
            other => panic!("expected RefNotFound(>= 7…), got {other:?}"),
        }
    }

    #[test]
    fn resolve_ref_short_hash_resolves_to_full_oid() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        let c1 = repo
            .commit_blob(Path::new("projects/p1/envs/dev.age"), "v1")
            .unwrap();
        let prefix = c1.to_string()[..8].to_string();
        let r = repo
            .project("p1")
            .resolve_ref("dev", RefForm::ShortHash(prefix))
            .unwrap();
        assert_eq!(r, c1);
    }

    #[test]
    fn commit_history_on_unborn_branch_is_empty_not_error() {
        let (_dir, repo) = fresh_vault();
        let h = repo.project("p1").commit_history("dev").unwrap();
        assert!(h.is_empty());
    }

    #[test]
    fn resolve_ref_unknown_env_yields_env_not_found() {
        let (dir, repo) = fresh_vault();
        // We need at least one commit so the branch isn't unborn (otherwise
        // the underlying error path is different). Commit something
        // unrelated, then ask for a ghost env.
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        repo.commit_blob(Path::new("projects/p1/envs/dev.age"), "v1")
            .unwrap();
        let err = repo
            .project("p1")
            .resolve_ref("ghost", RefForm::Latest)
            .unwrap_err();
        assert!(
            matches!(err, EnvrollError::EnvNotFound(_)),
            "expected EnvNotFound, got {err:?}"
        );
    }

    // ---------- 4.6 ----------

    #[test]
    fn dirty_check_clean_after_commit() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        repo.commit_blob(Path::new("projects/p1/envs/dev.age"), "v1")
            .unwrap();
        assert!(!repo.is_working_tree_dirty().unwrap());
    }

    #[test]
    fn dirty_check_detects_untracked_file() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "stray.txt", b"hello");
        assert!(repo.is_working_tree_dirty().unwrap());
    }

    #[test]
    fn dirty_check_detects_modified_tracked_file() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        repo.commit_blob(Path::new("projects/p1/envs/dev.age"), "v1")
            .unwrap();
        // Modify in place without re-committing.
        fs::write(dir.path().join("projects/p1/envs/dev.age"), b"v1+").unwrap();
        assert!(repo.is_working_tree_dirty().unwrap());
    }

    // ---------- 4.7 ----------

    #[test]
    fn remote_set_show_unset_round_trip() {
        let (_dir, repo) = fresh_vault();
        assert_eq!(repo.remote_show().unwrap(), None);
        repo.remote_set("file:///tmp/remote.git").unwrap();
        assert_eq!(
            repo.remote_show().unwrap().as_deref(),
            Some("file:///tmp/remote.git")
        );
        // set again with a different URL replaces (no second-add error)
        repo.remote_set("file:///tmp/other.git").unwrap();
        assert_eq!(
            repo.remote_show().unwrap().as_deref(),
            Some("file:///tmp/other.git")
        );
        repo.remote_unset().unwrap();
        assert_eq!(repo.remote_show().unwrap(), None);
        // unset on an absent remote is a no-op (used by sync recovery paths).
        repo.remote_unset().unwrap();
    }

    #[test]
    fn fetch_with_no_remote_yields_no_remote() {
        let (_dir, repo) = fresh_vault();
        let err = repo.fetch().unwrap_err();
        assert!(matches!(err, EnvrollError::NoRemote));
    }

    #[test]
    fn is_ancestor_returns_true_for_self() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        let c1 = repo
            .commit_blob(Path::new("projects/p1/envs/dev.age"), "v1")
            .unwrap();
        assert!(repo.is_ancestor(c1, c1).unwrap());
    }

    #[test]
    fn is_ancestor_returns_true_for_parent_of_descendant() {
        let (dir, repo) = fresh_vault();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        let c1 = repo
            .commit_blob(Path::new("projects/p1/envs/dev.age"), "v1")
            .unwrap();
        touch(dir.path(), "projects/p1/envs/dev.age", b"v2");
        let c2 = repo
            .commit_blob(Path::new("projects/p1/envs/dev.age"), "v2")
            .unwrap();
        assert!(repo.is_ancestor(c1, c2).unwrap());
        assert!(!repo.is_ancestor(c2, c1).unwrap());
    }

    /// End-to-end fetch+push against a local bare repo as `origin`. Exercises
    /// the credential callbacks (no creds needed for file://) and validates
    /// that the spec-mandated transport-error path is reachable.
    #[test]
    fn local_file_remote_round_trip() {
        let (dir, repo) = fresh_vault();
        // Make at least one commit so push has content.
        touch(dir.path(), "projects/p1/envs/dev.age", b"v1");
        repo.commit_blob(Path::new("projects/p1/envs/dev.age"), "v1")
            .unwrap();

        // Bare remote.
        let remote_dir = TempDir::new().unwrap();
        let _bare = Repository::init_bare(remote_dir.path()).unwrap();
        let url = format!("file://{}", remote_dir.path().display());
        repo.remote_set(&url).unwrap();

        // First push should succeed (remote is empty so it's trivially ff).
        repo.push_fast_forward().unwrap();

        // Now fetch — remote_head should resolve to the same OID we pushed.
        repo.fetch().unwrap();
        let local = repo.local_head().unwrap().unwrap();
        let remote = repo.remote_head().unwrap().unwrap();
        assert_eq!(local, remote);
    }

    #[test]
    fn fetch_against_nonexistent_path_is_transport_error() {
        let (_dir, repo) = fresh_vault();
        repo.remote_set("file:///definitely/not/here/for/envroll-test")
            .unwrap();
        let err = repo.fetch().unwrap_err();
        assert!(matches!(err, EnvrollError::RemoteTransportError(_)));
    }
}
