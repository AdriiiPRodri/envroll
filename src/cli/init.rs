//! `envroll init` — initialize the vault (first run) and/or register this directory.
//!
//! Three operating modes:
//!
//! 1. **First run**: vault root does not exist. Prompt for a passphrase
//!    (with confirm), create the vault layout, init the libgit2 repo, write
//!    the canary, then register the cwd as the first project. Print the
//!    spec-mandated banner about passphrase loss.
//! 2. **Already-initialized vault, new directory**: don't reprompt for the
//!    passphrase (registering doesn't write encrypted content). Derive the
//!    project ID, write the manifest, commit.
//! 3. **`--verify-passphrase`**: prompt for the passphrase, decrypt the
//!    canary, print `passphrase verified` (exit 0) or fail with
//!    `EnvrollError::WrongPassphrase` (exit 10).

use std::path::Path;

use clap::Args as ClapArgs;

use crate::cli::Context;
use crate::crypto;
use crate::errors::{generic, EnvrollError};
use crate::manifest::{
    derive_project_id, validate_target_filename, IdDerivation, Manifest, DEFAULT_TARGET_FILENAME,
};
use crate::output::OutputFormat;
use crate::paths::{
    project_checkout_dir, project_dir, project_envs_dir, project_manifest, resolve_vault_root,
    vault_canary,
};
use crate::prompt::{read_passphrase, read_passphrase_confirm, PassphraseSources};
use crate::vault::fs as vfs;
use crate::vault::git::VaultRepo;
use crate::vault::Vault;

/// Permissions: 0700 for project dirs (parity with the vault root and
/// `.checkout/` — denies a local attacker enumeration).
const PROJECT_DIR_MODE: u32 = 0o700;
const META_FILE_MODE: u32 = 0o644;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Override project ID derivation. Use when the auto-derived ID would
    /// collide (monorepo subdirs sharing an origin) or when reattaching a
    /// renamed project.
    #[arg(long, value_name = "ID")]
    pub id: Option<String>,

    /// Optional override for the persisted `id_input` field (e.g., the
    /// origin URL string when `id_source = "manual"`). Rare.
    #[arg(long, value_name = "STRING")]
    pub id_input: Option<String>,

    /// Verify the vault passphrase by decrypting the canary, then exit.
    /// Does not register a project. Exits 10 on a wrong passphrase.
    #[arg(long)]
    pub verify_passphrase: bool,

    /// Override the working-copy filename inside the project root. Defaults
    /// to `.env` (Node, Python, Ruby, Go conventions). Set to `.env.local`
    /// for Next.js / Vite / Astro / Remix / Nuxt projects, or any other
    /// relative filename your stack reads from.
    #[arg(long, value_name = "FILENAME")]
    pub target: Option<String>,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let vault_root = resolve_vault_root(ctx.vault.as_deref())?;
    let cwd = std::env::current_dir().map_err(EnvrollError::Io)?;
    let sources = PassphraseSources::new(ctx.passphrase_stdin, ctx.passphrase_env.as_deref());

    if args.verify_passphrase {
        return run_verify_passphrase(&vault_root, &sources);
    }

    let vault_existed_before = vault_canary(&vault_root).exists();

    // Phase 1: ensure the vault layout + canary + .git repo exist. If the
    // vault is fresh we prompt with confirmation; otherwise we don't prompt
    // at all (registering doesn't write encrypted content).
    let vault = if vault_existed_before {
        Vault::open(&vault_root)?
    } else {
        let pass = read_passphrase_confirm(&sources, "envroll passphrase")?;
        let v = Vault::ensure_init(&vault_root, &pass)?;
        // Initialize the libgit2 repo and commit the freshly-laid-out files
        // so the vault is a clean, committed git tree from the very first run.
        let repo = VaultRepo::ensure_init(&vault_root)?;
        repo.commit_paths(
            &[
                Path::new(".envroll-version"),
                Path::new(".gitignore"),
                Path::new(".canary.age"),
            ],
            "envroll: initialize vault",
        )?;
        v
    };

    // Phase 2: derive the project ID and either register or report-already.
    let derived = derive_project_id(&cwd, args.id.as_deref())?;
    let manifest_path = project_manifest(vault.root(), derived.id());

    if manifest_path.exists() {
        // Idempotent: if the manifest exists, we don't touch it. The spec
        // says we still print and exit 0 cleanly.
        println!("envroll: project already registered as {}", derived.id());
        if !vault_existed_before {
            print_first_init_banner(ctx);
        }
        return Ok(());
    }

    // Validate the optional --target filename now so we fail fast before
    // touching the filesystem layout.
    let target_filename = match args.target.as_deref() {
        Some(t) => {
            validate_target_filename(t)?;
            t.to_string()
        }
        None => DEFAULT_TARGET_FILENAME.to_string(),
    };

    // Build the manifest. For Manual derivation, the user can override
    // id_input via `--id-input` (rare).
    let id_input = match (&derived, args.id_input.as_deref()) {
        (IdDerivation::Manual { .. }, Some(s)) => s.to_string(),
        _ => derived.id_input(),
    };
    let manifest = Manifest::new_with_target(
        derived.id().to_string(),
        derived.source(),
        id_input,
        target_filename,
    );

    // Lay out the project's vault directory (envs + .checkout) and write the
    // manifest atomically. Note we do NOT write a `.gitkeep` in `envs/` —
    // the directory will be created by the first env's `.age` write.
    vfs::ensure_dir(&project_dir(vault.root(), derived.id()), PROJECT_DIR_MODE)?;
    vfs::ensure_dir(
        &project_envs_dir(vault.root(), derived.id()),
        PROJECT_DIR_MODE,
    )?;
    vfs::ensure_dir(
        &project_checkout_dir(vault.root(), derived.id()),
        PROJECT_DIR_MODE,
    )?;
    let toml = manifest.to_toml()?;
    vfs::atomic_write(&manifest_path, toml.as_bytes(), META_FILE_MODE)?;

    // Commit the new manifest to the vault repo.
    let repo = VaultRepo::open(vault.root())?;
    let rel_manifest = manifest_relative_path(derived.id());
    repo.commit_blob(
        &rel_manifest,
        &format!("envroll: register project {}", derived.id()),
    )?;

    // Output to stdout.
    match ctx.format {
        OutputFormat::Human => {
            println!("registered {}", derived.id());
            println!("vault: {}", vault.root().display());
        }
        OutputFormat::Json => {
            let value = serde_json::json!({
                "id": derived.id(),
                "id_source": manifest.id_source,
                "vault": vault.root().display().to_string(),
            });
            println!("{value}");
        }
    }

    if !vault_existed_before {
        print_first_init_banner(ctx);
    }

    Ok(())
}

/// Per-project relative path to `manifest.toml`. Used as the libgit2 commit
/// pathspec.
fn manifest_relative_path(project_id: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("projects/{project_id}/manifest.toml"))
}

/// `--verify-passphrase` flow (8.3 / D20). The vault MUST already be
/// initialized; we never create one in this mode.
fn run_verify_passphrase(
    vault_root: &Path,
    sources: &PassphraseSources<'_>,
) -> Result<(), EnvrollError> {
    if !vault_canary(vault_root).exists() {
        return Err(generic("vault not initialized — run `envroll init` first"));
    }
    let pass = read_passphrase(sources, "envroll passphrase")?;
    crypto::verify_canary(vault_root, &pass)?;
    println!("passphrase verified");
    Ok(())
}

/// Print the spec-mandated banner after first-run init (vault creation +
/// first project registration). Goes to stderr so it doesn't pollute the
/// scriptable stdout in `--format json` mode.
fn print_first_init_banner(_ctx: &Context) {
    eprintln!();
    eprintln!("⚠ Back up your passphrase.");
    eprintln!(
        "  envroll has no recovery for a forgotten passphrase — every env in this vault would"
    );
    eprintln!("  become permanently inaccessible. Store it in a password manager now.");
}
