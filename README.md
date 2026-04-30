# envroll

**git for your `.env` files.**
A single statically-linked Rust binary that versions, switches, and encrypts environment variables — local-first, no SaaS, no daemon, no surprises.

<p align="center">
  <a href="https://crates.io/crates/envroll"><img src="https://img.shields.io/crates/v/envroll.svg" alt="crates.io"></a>
  <a href="https://github.com/your-org/envroll/actions"><img src="https://img.shields.io/github/actions/workflow/status/your-org/envroll/ci.yml?branch=main" alt="CI"></a>
  <a href="https://github.com/your-org/envroll/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg" alt="License"></a>
  <img src="https://img.shields.io/badge/rust-1.78%2B-orange.svg" alt="Rust 1.78+">
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey.svg" alt="Platforms">
</p>

---

## The problem you already have

You have seven `.env` files. Maybe more. They live in your project as `.env`, `.env.local`, `.env.staging`, `.env.bak`, `.env.bak.2`, `.env.OLD-please-delete`, and the one in your password manager that you can't quite remember if it's current. You copy them around with `cp`. You wrote a shell alias to swap them. You've committed one by accident before.

You don't want a SaaS for this. You don't want a profile system that locks you in. You don't want yet another secrets vault. You want the same thing you already trust for your source code — **branches, history, atomic switches, and a remote you control** — but for `.env` files.

That's `envroll`.

```text
                                    ┌─────────────────────────┐
                                    │     ~/.local/share/envroll │
   ./.env  ──────── symlink ──────▶ │   (encrypted git vault) │
                                    │                         │
   (project repo                    │  dev.age   staging.age  │
    never sees                      │  prod.age  feature.age  │
    your secrets)                   └────────────┬────────────┘
                                                 │
                                                 │  optional, opt-in
                                                 ▼
                                       any git remote you own
                                  (encrypted, safe even if public)
```

---

## envroll in 30 seconds

```bash
# One-time vault setup (asks for a passphrase, twice)
$ envroll init

# Save the .env you already have as a new env
$ envroll fork dev
saved dev as 7a1f3c2

# Branch it for staging, edit, save
$ envroll fork staging -m "swap to staging DB"
$ envroll edit staging
$ envroll save -m "added stripe test key"

# Switch back and forth — the active .env is just a symlink swap
$ envroll use dev
now using dev
$ envroll use staging
now using staging

# Run a one-off command with a different env injected, no symlink change
$ envroll exec prod -- node scripts/migrate.js

# History, diffs, single-key ops — all the things you do with git
$ envroll log staging
* 9c4e1ab  2026-04-30T15:42  added stripe test key       +1 -0 ~0
* 7a1f3c2  2026-04-30T15:11  swap to staging DB          +0 -0 ~1
* 4e8d2af  2026-04-29T18:03  initial save of ./.env      +12 -0 ~0

$ envroll diff dev staging --show-values
~DATABASE_URL  postgres://localhost/app  ->  postgres://staging.db/app
~DEBUG         true                      ->  false
+STRIPE_KEY    sk_test_xxx
```

That's the whole product. No dashboard, no agent, no monthly bill.

---

## Why envroll

- **Local-first, no account.** Your envs live on your disk. Sync is opt-in and points at any git remote you own — GitHub, GitLab, your home Forgejo, a USB drive with `git init --bare`, whatever.
- **Encrypted at rest, always.** Every env blob in the vault is age-encrypted with a passphrase only you know. Push to a public repo if you want — the contents are unreadable without the key.
- **Real history, not backups.** `envroll log` and `envroll diff` are git-grade. Every change is a real commit with structured metadata. You can always go back.
- **Atomic switches.** `envroll use staging` is a symlink swap. There is no edit-window where `./.env` is half-written. Other processes never see a partial file.
- **One verb you don't have to learn.** Subcommands follow git/cargo conventions. `init`, `fork`, `use`, `save`, `log`, `diff`, `exec`, `sync`. If you've used git, you've used envroll.
- **Single binary.** Statically linked. Drop it in your `$PATH` and go. No Python runtime, no Node, no Docker, no `~/.envroll/venv/`.
- **Scriptable.** `--format json` everywhere. Built for CI, shell pipelines, and editor integrations. Stable exit codes, no surprises.

---

## Install

```bash
# crates.io (always the latest stable)
cargo install envroll

# Homebrew (macOS / Linux)
brew install your-org/tap/envroll

# Prebuilt binaries (macOS, Linux, Windows)
curl -LsSf https://github.com/your-org/envroll/releases/latest/download/envroll-installer.sh | sh
```

Or grab a prebuilt binary from the [Releases](https://github.com/your-org/envroll/releases) page. SHA-256 sums are signed.

**Platforms:** macOS (x86_64, aarch64) and Linux (x86_64, aarch64) are first-class. Windows is supported on a best-effort basis with a copy-mode fallback for environments without symlink privileges.

---

## Quickstart

### 1. Initialize the vault

```bash
$ cd ~/code/my-api
$ envroll init
envroll passphrase: ********
confirm: ********

  Vault created at ~/.local/share/envroll
  Project registered as remote-3a1b9c8d4e5f6a7b

  IMPORTANT: write your passphrase down somewhere safe.
  If you lose it, every env in this vault is gone forever.
  No recovery, no support email, no exceptions.

$ ls -la .env
.env -> /Users/you/.local/share/envroll/projects/remote-3a1b9c8d4e5f6a7b/.checkout/dev
```

### 2. Branch and edit

```bash
$ envroll fork staging -m "snapshot before db migration"
saved staging as f4d8a1c
now using staging

$ envroll edit staging
# (your $EDITOR opens; change DATABASE_URL, save, quit)

$ envroll save -m "point at staging db"
saved staging as 9c4e1ab
```

### 3. Run a command in a one-off env

```bash
$ envroll exec prod -- pnpm run smoke-test
# pnpm sees prod's vars, ./.env is untouched
```

### 4. (Optional) Sync to a remote

```bash
$ envroll remote set git@github.com:you/envroll-vault.git
remote set: git@github.com:you/envroll-vault.git

$ envroll sync
pushed 4 commits to origin
```

The remote can be public, private, on-prem, or a directory mounted from a NAS. envroll doesn't care — every env blob is already encrypted.

---

## How it works

```text
~/.local/share/envroll/
├── .git/                          libgit2-managed, every change is a real commit
├── .gitignore                     keeps plaintext checkouts out of history
├── .canary.age                    "is the passphrase right?" sentinel
├── .envroll-version                  on-disk schema pin
└── projects/
    └── remote-3a1b9c8d4e5f6a7b/
        ├── manifest.toml          project state (active env, etc.)
        ├── envs/
        │   ├── dev.age            ← what gets synced
        │   ├── staging.age
        │   └── prod.age
        └── .checkout/             ← never committed, never synced
            ├── dev                ← decrypted plaintext (symlink target)
            └── staging
```

When you run `envroll use staging`:

1. envroll prompts for the passphrase, decrypts the canary to verify it.
2. It decrypts `staging.age` to `.checkout/staging` via tempfile-then-rename (atomic).
3. It atomically swaps `./.env` to be a symlink pointing at that file.
4. It commits the manifest update to the vault git.

When you run `envroll save`:

1. envroll reads the current working copy, parses it, compares the parsed key-value set to the env's last commit.
2. If nothing changed semantically (reordering, comments, trailing newlines don't count) → exit 0 with `nothing to save`. No empty commits.
3. Otherwise: encrypt, atomic-write to `envs/<active>.age`, commit.

When you run `envroll sync`:

1. Pre-flight: refuse if the vault working tree is dirty.
2. Fetch from `origin`, then fast-forward push or pull as appropriate.
3. On divergent histories, refuse honestly and tell you exactly how to resolve in `~/.local/share/envroll/` with regular git tools. No silent merges of binary blobs.

Encryption is [age](https://github.com/FiloSottile/age) in scrypt-passphrase mode, binary format, MAC-authenticated. Versioning is libgit2 — no shelling out to `git`, no surprise behavior from your global git config (commits use a fixed `envroll <envroll@local>` author).

---

## Compared to ...

| Tool | Local-first | Versioned | Encrypted at rest | Atomic switch | Single binary | Scope |
| --- | :---: | :---: | :---: | :---: | :---: | --- |
| **envroll** | yes | yes (libgit2) | yes (age, passphrase) | yes (symlink swap) | yes | Manage many envs per project |
| `direnv` | yes | no | no | no | yes | Activate one env on `cd` |
| `sops` | yes | via your repo | yes (KMS / age / GPG) | no | yes | Encrypt arbitrary files |
| `lazyenv` | yes | no | no | no | yes | TUI editor for `.env` |
| `dotenv-vault` | partial | partial | yes | no | no | Hosted secrets sharing |
| Doppler / Infisical | no | yes (server) | yes | yes | no | SaaS secrets platform |

envroll fills the gap where you want **`direnv`-style local control** with **SaaS-style versioning and safety** — without paying for SaaS or running a server.

---

## Security: what envroll protects against, and what it doesn't

envroll is honest about its threat model. Read this section before you trust it with anything that matters.

**envroll protects against**

- A passive attacker who reads your sync remote — public repo, leaky bucket, accidental tweet. The contents are age-encrypted; without your passphrase the attacker sees ciphertext, env names, and timestamps. Nothing else.
- A lost or stolen laptop with full-disk encryption enabled and locked.
- Casual exposure in chat, screenshots, screen-shares — provided you pasted the encrypted blob, not the plaintext.
- Tampering with the remote: age messages are MAC-authenticated. Modified ciphertext fails to decrypt and you get a clear error.

**envroll does NOT protect against**

- An active attacker with shell access on your machine. They can read `.checkout/<env>` directly — it's plaintext on disk.
- A malicious remote that replays an older but valid commit. age catches modification; it does not catch rollback to a previously-valid version. Inspect `git log` of the vault if you care.
- Keyloggers, compromised terminals, weak passphrases.
- You committing your plaintext somewhere envroll can't see (a chat, a paste-bin, a `.env.bak` outside the vault).

If your threat model includes any of those, envroll is not the right tool — and we'll happily tell you so. Use a hardware-backed secrets manager.

---

## Recovery

| What broke | What to do |
| --- | --- |
| **Forgot your passphrase** | No recovery. Everything in the vault is gone. Back up your passphrase in a password manager. envroll will not save you from this. |
| **Single env file is corrupt** | `envroll log <name>` shows history. `envroll use <name>@<hash>` rolls back to a previous commit. `envroll save -m "recovered"` makes that the new tip. |
| **Vault git is corrupt** | If you have a remote: `mv ~/.local/share/envroll ~/envroll-broken && git clone <remote> ~/.local/share/envroll`. Your passphrase still decrypts the cloned vault. |
| **`.checkout/` got deleted** | Harmless. The next `envroll use` recreates it. |
| **An envroll command was killed mid-write** | Tempfiles use a recognizable pattern (`.envroll-tmp.<pid>.<rand>`) and are swept on the next run. The destination file is never partially written — every write is atomic. |
| **Working on a new machine** | `envroll init` (creates a fresh vault), set the same remote, `envroll sync`, type the same passphrase. Done. |

---

## CI and non-interactive use

envroll reads passphrases from three sources, in this order:

1. `--passphrase-stdin` (preferred — secrets piped from stdin do not appear in `ps`).
2. Interactive TTY prompt (when stdin is a terminal).
3. The `ENVROLL_PASSPHRASE` env var (last resort; visible in `/proc` and parent process snapshots).

```bash
# In CI
pass envroll-vault | envroll save --passphrase-stdin -m "deploy"

# Or, if your CI secret store can only set env vars
ENVROLL_PASSPHRASE=$VAULT_PASS envroll exec prod -- ./run.sh
```

`--format json` is supported on every read command (`projects`, `list`, `log`, `diff`, `status`) with a stable schema for tooling. Exit codes are stable from `1.0` onward.

---

## Status

envroll is **pre-1.0**. The on-disk format and the CLI surface are frozen and won't change incompatibly within `0.1.x`, but expect:

- Performance and ergonomics polish.
- A signed-tag scheme to defeat remote-rollback attacks (`v0.2`).
- Auto-rebase when two machines push non-overlapping changes (`v0.2`).
- A passphrase-rotation command (`v0.2`).
- A session cache or `envroll-agent` so you don't type the passphrase every time (`v0.3`).

What's **not** on the roadmap, ever:

- A SaaS backend.
- A required cloud account.
- Telemetry, analytics, "anonymous usage data".
- Plaintext envs leaving your machine. Period.

---

## Contributing

PRs welcome. The codebase is meant to stay small and legible — one binary, no clever macros, dependencies pinned tight. If you're not sure whether a change fits, open an issue first and we'll talk it through.

```bash
git clone https://github.com/your-org/envroll
cd envroll
cargo test
cargo run -- --help
```

Run the full perf soak with `cargo test --features perf`.

---

## License

Dual-licensed under either of:

- MIT License ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option.

---

> Built because we got tired of `cp .env.staging .env`. If you've ever lost a key that way, you'll get it.
