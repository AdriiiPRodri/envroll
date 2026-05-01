# Changelog

All notable changes to envroll are recorded here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`cargo-dist` reads this file during the release pipeline — the section
under each `## [VERSION]` heading becomes the GitHub release notes for
that tag. Keep entries user-facing.

## [0.2.0] - 2026-05-01

### Added

- **envroll is now on crates.io.** Install with `cargo install envroll`
  from any machine that has a Rust toolchain — no need to grab a
  prebuilt binary or build from source. The published crate ships the
  same `envroll` binary the GitHub Releases pipeline produces.

### Changed

- The crates.io tarball is **trimmed to what consumers actually need**.
  The CI workflow (`.github/workflows/`), cargo-dist config
  (`dist-workspace.toml`), and integration tests (`tests/`) live on
  GitHub for anyone who wants them but no longer ship inside the
  published crate. Bundle size dropped from 61 → 52 files
  (516 → 422 KiB uncompressed).
- **Test files are now organized by command/feature area** instead of
  by release version. The old `tests/v0_1_3_features.rs` (which had
  already grown past v0.1.3 with v0.1.4's `--install` tests) is split
  into `tests/completions.rs`, `tests/import_export.rs`, and
  `tests/rename_key.rs`. No behavioral change — same 27 tests, same
  coverage.

### Note on versioning policy

This is the first release that follows the semver policy correctly for
a `0.x` crate: **new features get a MINOR bump**, not a PATCH. The
0.1.0 → 0.1.5 history shipped multiple feature releases as patches
(basename project IDs, `--target`, completions, import/export,
rename-key, `--install`) — those tags stay as published, but every
release from here on gets the right bump. Bug-fix-only releases will
be `0.2.x`; the next batch of features will be `0.3.0`.

## [0.1.5] - 2026-05-01

### Changed

- **Project IDs are now the human-readable repo basename** when a `git
  remote get-url origin` is configured. `git@github.com:acme/prowler.git`
  registers as `prowler` instead of the old `remote-3a1b9c8d4e5f6a7b`. As
  a side effect, **multiple worktrees of the same repo now share their
  envs automatically** — they all derive the same basename, so they all
  point to the same vault entry. The old hash-prefixed format
  (`remote-<16hex>`) is gone for new projects; the full normalized URL is
  still persisted in the manifest's `id_input` field, so the rare
  collision between two unrelated repos that share a basename
  (`acme/prowler` vs `other/prowler`) is detected and refused with a
  clear `--id <custom>` hint.
- Repo basenames are filtered through a filesystem-safe sanitizer:
  lowercase ASCII alphanumerics, dashes, and underscores survive verbatim;
  anything else (uppercase, dots, spaces, unicode) is replaced with `-`.
  `Acme/My-App.git` → `my-app`.

### Migration note

Pre-0.1.5 projects with `remote-<hash>` IDs keep working as-is — the ID
is recorded in the manifest, not re-derived from the URL on every
command. To get the new pretty name on an existing project, either:

1. `envroll init --id <new-name>` to re-register at the new ID (the
   original `remote-<hash>` entry stays in the vault until you `rm -rf`
   it manually), or
2. Wait for the v0.2 `envroll migrate-id` command that does this
   atomically with history preserved.

## [0.1.4] - 2026-05-01

### Added

- **`envroll completions <shell> --install`** — one command that figures
  out the convention path for your shell, creates whatever directories
  are missing, writes the completion file, and (for shells that need it)
  appends a marker-guarded block to your shell's rc file so completion
  loads on next shell start. No sudo, no manual `.zshrc` editing.
  Idempotent — re-running doesn't duplicate anything.
  - Supported shells: bash, zsh, fish, powershell, elvish.
  - The plain `envroll completions <shell>` form (print-to-stdout) still
    works for CI / container builds / users who want to wire it manually.

### Changed

- README's **Shell completions** section rewritten to lead with
  `--install`. The "manual" path is documented as a fallback for custom
  setups (system-wide `/usr/local/share/zsh/site-functions/`, etc.).

### Fixed

- Documentation no longer assumed `/usr/local/share/zsh/site-functions/`
  exists by default — most fresh macOS installs don't have it. The
  `--install` flow uses `~/.zsh/completions/` instead and creates the
  directory as needed.

## [0.1.3] - 2026-05-01

### Added

- **`envroll completions <shell>`** — print bash / zsh / fish /
  powershell / elvish completion scripts to stdout. (See 0.1.4 for the
  one-command install variant.)
- **`envroll import <file> --as <name>`** — adopt an existing
  `.env`-style file lying around on disk as a new env in the current
  project. Onboarding accelerator: when a new contributor arrives with
  five legacy `.env.dev` / `.env.staging` / `.env.bak.2024` files, they
  can now `for f in .env.*; do envroll import "$f" --as "${f#.env.}"; done`
  instead of shuffling files in and out of `./.env`.
- **`envroll export <env> [--output dotenv|json|shell]`** —
  anti-lock-in escape hatch. Decrypts a single env to stdout in one of
  three formats: `dotenv` (the default — round-trips through `import`),
  `json` (single object, ready to pipe into AWS Secrets Manager), or
  `shell` (POSIX-safe `export KEY='...'` lines, eval-able). Never
  masked.
- **`envroll rename-key OLD NEW [--in <env> | --all] [--force]`** —
  rename a key (e.g. `DATABASE_URL` → `DB_URL`) across one or every env
  in the project. Skips envs that don't contain `OLD`; refuses on
  collisions unless `--force`.

### Changed

- `--format` is no longer the flag for `envroll export`'s output shape
  (it conflicts with the global `--format human|json` flag). Use
  `--output dotenv|json|shell` instead.

## [0.1.2] - 2026-05-01

### Added

- **`envroll init --target <filename>`** — register a project with a
  non-default working-copy filename. Designed for modern JS frameworks
  that read from `.env.local` (Next.js, Vite, Astro, Remix, Nuxt) or any
  other custom path (`application.env` for Spring Boot, `config/.env`
  for apps with config in subdirs). Once set, every command (`fork` /
  `save` / `use` / `status` / etc.) reads and writes the configured
  filename instead of `.env`.
- Validation rejects empty, absolute, or path-traversal filenames at
  `init` time.

### Changed

- Manifest schema gains a `target_filename` field.

### Fixed

- Manifests created by 0.1.0 / 0.1.1 (which didn't have the field) are
  fully backwards-compatible — serde defaults the field to `.env` so
  existing vaults keep working.

## [0.1.1] - 2026-05-01

### Added

- **Multi-platform prebuilt binaries** for every release, via cargo-dist
  GitHub Actions pipeline. Tier-1 targets:
  - `aarch64-apple-darwin` (macOS Apple Silicon)
  - `x86_64-apple-darwin` (macOS Intel)
  - `aarch64-unknown-linux-gnu` (Linux ARM, e.g., Raspberry Pi 4+)
  - `x86_64-unknown-linux-gnu` (Linux Intel/AMD — the common one)
  - `x86_64-pc-windows-msvc` (Windows)
- **Curl / iwr installers** auto-generated by cargo-dist:
  - `curl -LsSf https://github.com/AdriiiPRodri/envroll/releases/download/v0.1.1/envroll-installer.sh | sh`
  - `irm https://github.com/AdriiiPRodri/envroll/releases/download/v0.1.1/envroll-installer.ps1 | iex`
- SHA-256 checksums for every artifact + an aggregated `sha256.sum` per
  release.

## [0.1.0] - 2026-05-01

First public release. Single statically-linked Rust binary that
versions, switches, and encrypts environment variables — local-first,
no SaaS, no daemon.

### Added

- **Vault model**: encrypted age-scrypt blobs versioned via libgit2,
  living outside the project repo at `~/.local/share/envroll/`.
- **Atomic env activation**: `envroll use staging` is a tempfile +
  rename of `./.env`'s symlink target — never half-written.
- **Core verbs**: `init`, `projects`, `list` (`ls`), `current`, `fork`,
  `save`, `use`, `status`, `rename`, `rm`, `edit`, `log`, `diff`,
  `get`, `set`, `copy`, `exec`, `remote {set,show,unset}`, `sync`.
- **Stable exit codes** (frozen 10–59 for v1.x, 60–99 reserved).
- **`--format json`** output on every read command, with stable schemas
  documented under `docs/json-schemas/`.
- **Threat model + recovery matrix** documented verbatim in
  `SECURITY.md` and the README — passphrase loss = no recovery, by
  design.
- macOS aarch64 binary (Linux + Windows added in 0.1.1).

[0.2.0]: https://github.com/AdriiiPRodri/envroll/releases/tag/v0.2.0
[0.1.5]: https://github.com/AdriiiPRodri/envroll/releases/tag/v0.1.5
[0.1.4]: https://github.com/AdriiiPRodri/envroll/releases/tag/v0.1.4
[0.1.3]: https://github.com/AdriiiPRodri/envroll/releases/tag/v0.1.3
[0.1.2]: https://github.com/AdriiiPRodri/envroll/releases/tag/v0.1.2
[0.1.1]: https://github.com/AdriiiPRodri/envroll/releases/tag/v0.1.1
[0.1.0]: https://github.com/AdriiiPRodri/envroll/releases/tag/v0.1.0
