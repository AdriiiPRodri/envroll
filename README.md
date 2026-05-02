# envroll

**git for your `.env` files.**
A single statically-linked Rust binary that versions, switches, and encrypts environment variables — local-first, no SaaS, no daemon, no surprises.

<p align="center">
  <a href="https://crates.io/crates/envroll"><img src="https://img.shields.io/crates/v/envroll.svg" alt="crates.io"></a>
  <a href="https://github.com/AdriiiPRodri/envroll/actions"><img src="https://img.shields.io/github/actions/workflow/status/AdriiiPRodri/envroll/ci.yml?branch=main" alt="CI"></a>
  <a href="https://github.com/AdriiiPRodri/envroll/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License"></a>
  <img src="https://img.shields.io/badge/rust-1.89%2B-orange.svg" alt="Rust 1.89+">
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey.svg" alt="Platforms">
</p>

---

<p align="center">
  <a href="https://raw.githubusercontent.com/AdriiiPRodri/envroll/main/docs/demo.mp4" title="Click to watch the MP4 version with pause/scrub controls">
    <img src="https://raw.githubusercontent.com/AdriiiPRodri/envroll/main/docs/demo.gif" alt="envroll demo: init, fork, set, save, use, exec, diff, log, list" width="100%">
  </a>
</p>

---

## What envroll does and does not protect against

> **Read this first.** envroll is honest about its threat model. If your needs do not fit inside what envroll protects against, use a different tool.

**envroll protects against:**

- A passive attacker who reads the configured vault remote (public GitHub repo, leaky S3 bucket, accidental tweet of the URL). All env contents are age-encrypted; without the passphrase the attacker sees only ciphertext + commit metadata (timestamps, commit messages, env *names*).
- A lost or stolen laptop **with full-disk encryption enabled and locked**. Vault content is still encrypted at the envroll layer, so even if FDE is later defeated the attacker still needs the passphrase.
- Casual `.env` exposure in chat, screenshots, screen-shares, or tickets — provided the user pasted the *encrypted* `.age` file, not the plaintext checkout. envroll cannot prevent users from pasting plaintext.
- Unauthorized writes to the vault remote: age messages have a built-in HMAC, so any tampered ciphertext fails to decrypt with `file corrupt or tampered`.

**envroll does NOT protect against:**

- An active attacker with shell access on the user's machine. They can read `.checkout/<name>` directly.
- A malicious remote serving a *rolled-back but still valid* commit. age's MAC catches modification but not "an older legitimate ciphertext is the current HEAD". Mitigation: `git log` the vault and verify expected history. v0.2 may add signed tags.
- Keyloggers or compromised terminal emulators. The passphrase is typed in the user's terminal; if that terminal is compromised, so is the vault.
- Weak passphrases. scrypt makes brute-force expensive but not impossible. We recommend `>= 6` random words from a long wordlist or equivalent entropy.
- The user committing plaintext somewhere envroll can't see (e.g., pasting `.env` into a chat, configuring their shell to log env vars).
- Side channels (timing, CPU usage). Out of scope.
- envroll's own dependency supply chain. Mitigation: pin `Cargo.lock`, use `cargo audit` / `cargo deny` in CI.

## Back up your passphrase

The passphrase you choose at `envroll init` is the **only** thing standing between you and the contents of every env you ever encrypt. **There is no recovery.** No support email, no master key, no escape hatch — that is the design.

Before you run `envroll fork` even once:

1. Pick a passphrase with real entropy. Six random words from a long wordlist (Diceware, EFF large list) is a good baseline. Do not pick a passphrase a human could plausibly guess.
2. Write it down in a password manager — 1Password, Bitwarden, KeePass, an encrypted note in your Apple Keychain — anywhere with its own backup story.
3. If your team shares a vault, share the passphrase out of band: never in the same channel where the encrypted vault travels.

`envroll init` prints this reminder loudly on first run. Heed it.

## Recovery

| Scenario | Recovery |
| --- | --- |
| Forgotten passphrase | **No recovery.** All envs are inaccessible. Documented loudly in the README and printed by `envroll init` immediately after passphrase confirmation. Recommendation: store the passphrase in a password manager. |
| Corrupt single `<name>.age` file | Use `envroll log <name>` to find a previous commit that decrypts cleanly, then `envroll use <name>@<hash>` to recover. Optionally `envroll save -m "recovered from <hash>"` to make it the new tip. |
| Corrupt vault git directory | If a remote is configured: `mv ~/.local/share/envroll ~/.local/share/envroll.broken && git clone <remote> ~/.local/share/envroll`. The user's passphrase is unchanged and decrypts the cloned vault. If no remote, the vault is lost. |
| **Deleted vault directory** (e.g., `rm -rf ~/.local/share/envroll/`) | Same as "corrupt vault git" — re-clone from remote if synced. The project's `./.env` symlinks become dangling on this machine; the next `envroll use` recreates `.checkout/`. If no remote, all envs are lost. |
| Deleted `.checkout/` directory | Harmless. Recreated on next `envroll use`. The user may see dangling symlinks at `./.env` until the next use; runtime inference will treat these as "stale, re-decrypt on next use". |
| Interrupted operation mid-write | Orphan tempfile is detected and cleaned up on next invocation. Destination file is never partially written because the rename is atomic. |
| Vault on a different machine | First `envroll init` on the new machine creates a fresh vault; setting the same remote and `envroll sync` pulls the encrypted history. The user is prompted for the same passphrase to read it. |

---

## The problem you already have

You have seven `.env` files. Maybe more. They live in your project as `.env`, `.env.local`, `.env.staging`, `.env.bak`, `.env.bak.2`, `.env.OLD-please-delete`, and the one in your password manager that you can't quite remember if it's current. You copy them around with `cp`. You wrote a shell alias to swap them. You've committed one by accident before.

You don't want a SaaS for this. You don't want a profile system that locks you in. You don't want yet another secrets vault. You want the same thing you already trust for your source code — **branches, history, atomic switches, and a remote you control** — but for `.env` files.

That's `envroll`.

```text
                                    ┌─────────────────────────┐
                                    │  ~/.local/share/envroll │
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

## Install

```bash
# crates.io (always the latest stable)
cargo install envroll

# Homebrew (macOS / Linux)
brew install AdriiiPRodri/tap/envroll

# Prebuilt binaries (macOS, Linux, Windows)
curl -LsSf https://github.com/AdriiiPRodri/envroll/releases/latest/download/envroll-installer.sh | sh
```

Or grab a prebuilt binary from the [Releases](https://github.com/AdriiiPRodri/envroll/releases) page. SHA-256 sums are signed.

**Platforms:** macOS (x86_64, aarch64) and Linux (x86_64, aarch64) are first-class. Windows is supported on a best-effort basis with a copy-mode fallback for environments without symlink privileges (set `ENVROLL_USE_COPY=1` or enable Windows Developer Mode).

---

## Quickstart

The four verbs you'll use every day are `init`, `fork`, `use`, and `save`. There is **no `save <name>`** form — `fork <name>` is the canonical way to create a new env.

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
```

### 2. Fork an env from `./.env` (or from the active env)

```bash
$ echo "DATABASE_URL=postgres://localhost/app" > .env
$ envroll fork dev
forked → dev (now active)

# ./.env is now a symlink into the encrypted vault's checkout area:
$ ls -la .env
.env -> ~/.local/share/envroll/projects/remote-3a.../​.checkout/dev
```

### 3. Branch it

```bash
$ envroll fork staging -m "snapshot before db migration"
forked → staging (now active)

$ envroll edit staging
# (your $EDITOR opens; change DATABASE_URL, save, quit)

$ envroll save -m "point at staging db"
saved staging
```

### 4. Switch atomically

```bash
$ envroll use dev
now using dev

$ envroll use staging
now using staging
```

### 5. Run a one-off command in another env

```bash
$ envroll exec prod -- pnpm run smoke-test
# pnpm sees prod's vars, ./.env is untouched
```

### 6. (Optional) Sync to a remote

```bash
$ envroll remote set git@github.com:you/envroll-vault.git
remote set to git@github.com:you/envroll-vault.git

$ envroll sync
pushed (initial)
```

The remote can be public, private, on-prem, or a directory mounted from a NAS. envroll doesn't care — every env blob is already encrypted.

---

## Commands reference

| Command | What it does | Lock |
| --- | --- | --- |
| `envroll init [--id <id>] [--target <filename>] [--verify-passphrase]` | Initialize the vault (first run) and register this directory. `--target` overrides the working-copy filename (default `.env`; use `.env.local` for Next.js / Vite / Astro / Remix / Nuxt). `--verify-passphrase` re-prompts and tests the canary. | exclusive |
| `envroll projects` | List every registered project on this machine. | none |
| `envroll list` (alias `ls`) `[--all]` | List envs in the current project (or all projects with `--all`). | shared |
| `envroll current` | Print the active env name. | none |
| `envroll fork <name> [-m <msg>] [--force]` | Create a new env from the active env or the project's working-copy file. | exclusive |
| `envroll import <file> --as <name> [--force]` | Adopt an existing `.env`-style file as a new env. Onboarding shortcut. | exclusive |
| `envroll export <env> [--output dotenv\|json\|shell]` | Print an env's plaintext content to stdout. Anti-lock-in escape hatch — pipe to a file, AWS Secrets Manager, `kubectl create secret`, etc. **Never masked.** | shared |
| `envroll save [-m <msg>] [--force]` | Save the working copy to the active env. `--force` deliberately rewinds when pinned to a historical ref. | exclusive |
| `envroll use <ref> [--force \| --rescue <name>]` | Activate an env. `<ref>` is `<name>` (latest), `<name>@<short-hash>`, or `<name>@~N`. | exclusive |
| `envroll status [--mask]` | Active env, mode (symlink / copy), dirty state, key-level diff. Values shown by default; `--mask` hides them for paste-safe output. | shared |
| `envroll rename <old> <new> [--force]` | Rename an env in place; libgit2 file-rename keeps history. | exclusive |
| `envroll rename-key <OLD> <NEW> [--in <env> \| --all] [--force]` | Rename a key (e.g. `DATABASE_URL` → `DB_URL`) across one or every env in the project. | exclusive |
| `envroll rm <name>` | Remove an env. Use the global `--yes` to skip the confirmation prompt. | exclusive |
| `envroll edit <name>` | Open an env in `$EDITOR` (fallbacks: `$VISUAL` → `vi`/`vim` on Unix, `notepad` on Windows). The vault lock is released for the editor's lifetime. | exclusive (then released) |
| `envroll log <name>` | Commit history for the env, newest-first, with `+N -M ~K` summaries. | shared |
| `envroll diff <a> <b> [--mask]` | Key-level diff between any two refs. Values shown by default; `--mask` hides them. | shared |
| `envroll get <KEY> [--from <env>]` | Print a single value to stdout (script-friendly, never masked). Exits 20 if missing. | shared |
| `envroll set <KEY=value> [--in <env>]` | Set or update a single key. | exclusive |
| `envroll copy <KEY> --from <a> --to <b>` | Copy a single key between envs. | exclusive |
| `envroll exec <ref> -- <cmd> [args...]` | Run a command with the env's vars injected. Decrypts to memory only — no plaintext on disk. `--no-override` lets parent-shell vars win on key collision. | shared (released before child spawn) |
| `envroll remote {set <url> \| show \| unset}` | Configure the optional sync remote. `set` validates the URL scheme but makes no network call. | varies |
| `envroll sync` | Pull-then-push the vault git history. Refuses if the vault working tree is dirty. Refuses on divergence and tells you exactly how to resolve. | exclusive |
| `envroll completions <bash\|zsh\|fish\|powershell\|elvish> [--install]` | With `--install`: write the completion file to its convention path and (idempotently) wire up the user's rc file. Without `--install`: print the script to stdout. See the **Shell completions** section below. | none |

**Global flags** (work on every subcommand): `--format <human|json>`, `--yes`, `--log <off|error|warn|info|debug>`, `--no-color` (also honors `NO_COLOR`), `--passphrase-stdin`, `--passphrase-env <NAME>`. The `--vault <path>` flag exists for testing only and is hidden from `--help`.

---

## Shell completions

Get `<TAB>` completion for every subcommand and flag. **Pick your shell, run one command, restart your shell.** No sudo, no manual edits.

```bash
envroll completions bash --install        # bash
envroll completions zsh --install         # zsh
envroll completions fish --install        # fish
envroll completions powershell --install  # powershell
envroll completions elvish --install      # elvish
```

`--install` figures out the right user-local path for your shell, creates whatever directories are missing, writes the completion file, and (for shells that need it) appends a small marker-guarded block to your shell's rc file so completion loads on next shell start. **Re-running is safe** — the rc append is idempotent (we don't double-add).

After installing, the command tells you exactly how to activate now:

```text
$ envroll completions zsh --install
✓ wrote completion file: /Users/you/.zsh/completions/_envroll
✓ added marker block to /Users/you/.zshrc

To activate now: rm -f ~/.zcompdump* && exec zsh
```

That's it. `envroll <TAB>` should show every subcommand on the next shell.

### Where things land

| Shell      | Completion file                                           | rc edit                                       |
| ---------- | --------------------------------------------------------- | --------------------------------------------- |
| bash       | `~/.local/share/bash-completion/completions/envroll`      | none (bash-completion auto-scans this dir)    |
| zsh        | `~/.zsh/completions/_envroll`                             | `~/.zshrc` (fpath + compinit, marker-guarded) |
| fish       | `~/.config/fish/completions/envroll.fish`                 | none (fish auto-loads)                        |
| powershell | `~/.config/powershell/envroll-completions.ps1` *(Unix-ish)*<br>`Documents/PowerShell/envroll-completions.ps1` *(Windows)* | `Microsoft.PowerShell_profile.ps1` (dot-source, marker-guarded) |
| elvish     | `~/.config/elvish/lib/envroll-completions.elv`            | `~/.config/elvish/rc.elv` (`use` line, marker-guarded) |

### If `--install` doesn't fit your setup

Drop `--install` and the script goes to stdout — wire it up however you like:

```bash
# Custom path (e.g., a system-wide Homebrew zsh install):
envroll completions zsh | sudo tee /usr/local/share/zsh/site-functions/_envroll > /dev/null

# Or pipe straight into PowerShell on the spot:
envroll completions powershell | Out-String | Invoke-Expression
```

### Troubleshooting

**`envroll <TAB>` does nothing.** The most common causes:

1. **You forgot to restart the shell.** Use the activation hint that `--install` printed (e.g., `exec zsh` or `. $PROFILE`).
2. **Stale completion cache (zsh).** `rm -f ~/.zcompdump* && exec zsh`.
3. **You used the manual (no-`--install`) path with a custom directory** that isn't on your shell's load path. Run `echo $fpath | tr ' ' '\n'` (zsh) and confirm the directory is listed; if it isn't, easier to just re-run with `--install`.

**`envroll use <TAB>` doesn't list my envs.** Correct, not supported in v0.1.x. It would require running `envroll list` synchronously inside every TAB press — too slow without the session-cache layer planned for v0.3. Subcommands and flags do complete.

---

## Importing existing `.env` files

A new contributor typically arrives with a folder full of `.env.dev`, `.env.staging`, `.env.bak.2024`, etc. `envroll import` adopts each one as a vault env without making you shuffle files around.

```bash
# Adopt one file
envroll import .env.dev --as dev

# Bulk-import everything matching .env.*
for f in .env.*; do
  name=${f#.env.}
  envroll import "$f" --as "$name"
done

# Source can be anywhere on disk — it's left untouched
envroll import ~/Downloads/legacy-prod-secrets.env --as prod
```

What gets imported is the parsed key-value content. Comments, blank lines, and key ordering are NOT preserved (envroll commits the canonical key-value set, same semantics as `save`). After importing you can safely `rm` the source file — the encrypted copy in the vault is the authoritative one.

If `<file>` won't parse as a valid `.env`, import refuses with **exit 12** before prompting for the passphrase. If `<name>` collides with an existing env, **exit 30** unless you pass `--force`.

---

## Exporting plaintext (anti-lock-in)

`envroll export` is the deliberate, audited path to get plaintext OUT of the vault — for piping into a hosted secrets manager, driving `kubectl create secret`, or migrating off envroll entirely. Three formats:

```bash
# dotenv (default) — KEY="value" lines that round-trip through `envroll import`
envroll export prod > prod.env

# json — single object, perfect for AWS Secrets Manager
envroll export prod --output json | \
  aws secretsmanager put-secret-value \
    --secret-id myapp/prod \
    --secret-string file:///dev/stdin

# kubernetes secret
envroll export prod --output dotenv | \
  kubectl create secret generic prod-env --from-env-file=/dev/stdin

# eval into your current shell (testing only — leaks to `ps`)
eval "$(envroll export dev --output shell)"

# migrate AWAY from envroll
for e in $(envroll list --format json | jq -r '.[0].envs[]'); do
  envroll export "$e" > ".env.$e"
done
```

Output is **never** masked. The whole point of the command is plaintext. If you want a paste-safe summary, use `envroll status --mask` or `envroll diff --mask` instead.

The `shell` format uses POSIX single-quote escaping (the `'\''` trick), so values containing `$`, backticks, double-quotes, or newlines are safe to `eval`.

---

## Renaming keys across envs

Refactoring helper. Renaming `DATABASE_URL` to `DB_URL` across `dev`, `staging`, `prod`, and `feature-x` used to be eight `envroll set` invocations plus making sure you remembered to delete the old key everywhere.

```bash
# Rename in the active env
envroll rename-key DATABASE_URL DB_URL

# Rename in a specific env
envroll rename-key STRIPE_SK STRIPE_SECRET --in prod

# Rename across every env that has the key (silently skips envs that don't)
envroll rename-key DATABASE_URL DB_URL --all

# Force overwrite if NEW already exists in some target env
envroll rename-key DATABASE_URL DB_URL --all --force
```

One commit per affected env, with the message `rename-key OLD → NEW in <env> at <ts>`. Envs that don't contain `OLD` are skipped (no empty commits) and reported on stderr. Same `active_ref` refuse rule as `save` / `set` / `copy` — we won't silently rewind a historically-pinned env.

---

## How it works

```text
~/.local/share/envroll/
├── .git/                          libgit2-managed, every change is a real commit
├── .gitignore                     keeps plaintext checkouts out of history
├── .canary.age                    "is the passphrase right?" sentinel
├── .envroll-version               on-disk schema pin
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

## Supported `.env` syntax

envroll **does not roll its own `.env` parser**. Every read goes through the [`dotenvy`](https://crates.io/crates/dotenvy) crate, which is the most actively maintained `.env` parser in the Rust ecosystem and the de-facto reference for `.env` semantics in Rust services.

What's reliably supported:

- `KEY=value` and `export KEY=value` (the `export ` prefix is stripped on parse).
- Whitespace around `=` is ignored on the input side. Emitted output is canonical (`KEY="value"`).
- Single- and double-quoted values; the four standard escapes `\\`, `\"`, `\$`, and `\n` inside double-quoted values.
- Empty values: `EMPTY=` is parsed as an empty string.
- Multi-line values inside double quotes (use `\n` as the line break).
- Comments starting with `#` on their own line, and trailing comments after a value.
- Duplicate keys: the **last assignment wins** (matches `dotenvy`'s runtime behavior).

What `envroll save`'s "nothing to save" detection ignores:

- Key reordering. `A=1\nB=2` and `B=2\nA=1` are equivalent.
- Comment edits and blank-line changes.
- A missing trailing newline at end-of-file.

What it preserves as a real change:

- Any byte difference inside a value (including trailing whitespace inside quotes).
- Added or removed keys.

If `dotenvy` cannot parse your file, `envroll save` exits 12 (`parse error`) with the parser's message — it never silently falls back to byte-comparison.

---

## v0.1 limitations (read these before adopting)

These are deliberate v0.1 trade-offs. Each has a tracking direction for v0.2+.

- **No passphrase rotation command.** Workaround: `envroll exec <each env> -- printenv > backup` for every env, then `mv ~/.local/share/envroll ~/.local/share/envroll.old`, run `envroll init` with the new passphrase, and re-`fork` each backup. v0.2 will ship `envroll passphrase change` as a first-class command.
- **No session cache, no `envroll-agent`.** Every encrypted-content operation prompts for the passphrase (or reads `--passphrase-stdin` / `$ENVROLL_PASSPHRASE`). v0.3 may add a session cache or daemon.
- **No auto-merge or auto-rebase on `sync`.** When two machines push divergent histories, envroll refuses and tells you to resolve manually with regular git tools in `~/.local/share/envroll/`. v0.2 will add per-blob non-conflicting auto-rebase for the common case where two machines touched different envs entirely.
- **No Windows-first parity.** macOS and Linux are tier-1; Windows is best-effort with a copy-mode fallback (`ENVROLL_USE_COPY=1` or Developer Mode). The CLI surface is identical, but the symlink path requires a privilege that Windows doesn't grant by default.
- **Unsaved local edits to `./.env` are lost on `envroll use`.** v0.1 does not warn or prompt — `use` overwrites unconditionally. Use `envroll status` to spot dirty state before switching. v0.2 may add a `--force-clobber` confirmation prompt.
- **Concurrent same-env mutation from two terminals is undefined.** The vault has a single advisory lock, not per-env locks. If two shells `envroll set` into the same env at the same time, the last writer wins on `.checkout/<name>` and the commit ordering depends on which terminal grabbed the lock first. v0.2 may add per-env advisory locks if anyone hits this in practice.

---

## CI and non-interactive use

envroll reads passphrases from three sources, in this order:

1. `--passphrase-stdin` (preferred — secrets piped from stdin do not appear in `ps`).
2. Interactive TTY prompt (when stdin is a terminal).
3. The `ENVROLL_PASSPHRASE` env var (last resort; visible in `/proc` and parent-process snapshots). The variable name can be overridden with `--passphrase-env <NAME>` for organizations that prefer a different convention.

```bash
# In CI
pass envroll-vault | envroll save --passphrase-stdin -m "deploy"

# Or, if your CI secret store can only set env vars
ENVROLL_PASSPHRASE=$VAULT_PASS envroll exec prod -- ./run.sh
```

`--format json` is supported on every read command (`projects`, `list`, `log`, `diff`, `status`) with a stable schema for tooling. Schemas live under [`docs/json-schemas/`](docs/json-schemas/) and are versioned alongside the binary. Exit codes are stable from `1.0` onward (codes 10–59 are frozen; 60–99 reserved for v2.x).

---

## Compared to ...

| Tool | Local-first | Versioned | Encrypted at rest | Atomic switch | Single binary | Scope |
| --- | :---: | :---: | :---: | :---: | :---: | --- |
| **envroll** | yes | yes (libgit2) | yes (age, passphrase) | yes (symlink swap) | yes | Manage many envs per project |
| `direnv` | yes | no | no | no | yes | Activate one env on `cd` |
| `sops` | yes | via your repo | yes (KMS / age / GPG) | no | yes | Encrypt arbitrary files |
| `lazyenv` | yes | no | no | no | yes | TUI editor for `.env` |
| `envx` | yes | no | no | no | yes | Run a command with `.env` vars injected |
| Doppler | no | yes (server) | yes | yes | no | SaaS secrets platform |

envroll fills the gap where you want **`direnv`-style local control** with **SaaS-style versioning and safety** — without paying for SaaS or running a server.

---

## Changelog

Every release's user-facing changes are documented in [CHANGELOG.md](CHANGELOG.md). The release pipeline reads from there to generate GitHub release notes, so the changelog is the canonical source.

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

See [SECURITY.md](SECURITY.md) for the full threat model and how to report vulnerabilities.

---

## Contributing

PRs welcome. The codebase is meant to stay small and legible — one binary, no clever macros, dependencies pinned tight. If you're not sure whether a change fits, open an issue first and we'll talk it through.

```bash
git clone https://github.com/AdriiiPRodri/envroll
cd envroll
cargo test
cargo run -- --help
```

Run the full perf soak with `cargo test --features perf`.

---

## License

MIT — see [LICENSE](LICENSE).

Copyright © 2026 Adrián Peña.

---

> Built because we got tired of `cp .env.staging .env`. If you've ever lost a key that way, you'll get it.
