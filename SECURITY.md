# Security

This document is the canonical statement of envroll's threat model and the
vulnerability disclosure process. The threat-model text below is reproduced
verbatim in the README's "What envroll does and does not protect against"
section so it's the first thing every new user sees — but if you're reading
the source, this is the file to start with.

## Threat model

### envroll protects against

- A passive attacker who reads the configured vault remote (public GitHub repo, leaky S3 bucket, accidental tweet of the URL). All env contents are age-encrypted; without the passphrase the attacker sees only ciphertext + commit metadata (timestamps, commit messages, env *names*).
- A lost or stolen laptop **with full-disk encryption enabled and locked**. Vault content is still encrypted at the envroll layer, so even if FDE is later defeated the attacker still needs the passphrase.
- Casual `.env` exposure in chat, screenshots, screen-shares, or tickets — provided the user pasted the *encrypted* `.age` file, not the plaintext checkout. envroll cannot prevent users from pasting plaintext.
- Unauthorized writes to the vault remote: age messages have a built-in HMAC, so any tampered ciphertext fails to decrypt with `file corrupt or tampered`.

### envroll does NOT protect against

- An active attacker with shell access on the user's machine. They can read `.checkout/<name>` directly.
- A malicious remote serving a *rolled-back but still valid* commit. age's MAC catches modification but not "an older legitimate ciphertext is the current HEAD". Mitigation: `git log` the vault and verify expected history. v0.2 may add signed tags.
- Keyloggers or compromised terminal emulators. The passphrase is typed in the user's terminal; if that terminal is compromised, so is the vault.
- Weak passphrases. scrypt makes brute-force expensive but not impossible. We recommend `>= 6` random words from a long wordlist or equivalent entropy.
- The user committing plaintext somewhere envroll can't see (e.g., pasting `.env` into a chat, configuring their shell to log env vars).
- Side channels (timing, CPU usage). Out of scope.
- envroll's own dependency supply chain. Mitigation: pin `Cargo.lock`, use `cargo audit` / `cargo deny` in CI.

## Cryptography

- **Algorithm.** [age](https://github.com/FiloSottile/age) in `scrypt::Recipient` / `scrypt::Identity` mode. The on-disk format is binary age v1 (the wire format declared by `<vault>/.envroll-version = "1"`).
- **Work factor.** `age` crate default — currently calibrated to ~1 second per derivation on commodity hardware. Not user-tunable in v0.1.
- **Authenticity.** age messages carry a per-message HMAC. Any modification to a `.age` file (single bit, swapped header, truncated tail) fails decryption with `file corrupt`. Exit code 11.
- **Passphrase handling in process memory.** Passphrases are held only in `secrecy::SecretString`. They are never `.clone()`-ed into a `String`, never written to disk, never logged, never displayed. `SecretString` zeroizes its bytes on drop.
- **No key material on disk.** envroll stores no key file, no recovery key, no wrapped passphrase. The only key is the one in your head (and in your password manager).

## On-disk hygiene

- Vault root and per-project `.checkout/` directories: mode `0700`.
- Plaintext checkout files and all `.age` blobs (canary + per-env): mode `0600`.
- Plaintext metadata (`manifest.toml`, `.gitignore`, `.envroll-version`): mode `0644`.
- Atomic writes everywhere: tempfile-in-same-directory + `fsync` + `rename` + parent `fsync`. An interrupted write leaves the destination unchanged; the orphan tempfile is reaped on the next vault-locking command.
- `.checkout/` is in the vault's `.gitignore` and is **never** committed. A test in `tests/env_switching.rs::sync_never_includes_checkout_in_commits` walks every commit pushed to a remote and asserts no `.checkout/` path is present.
- Plaintext only ever leaves the project-scoped `.checkout/` directory through three deliberate routes: a symlink at `./.env` (default), a copy at `./.env` (copy-mode), or as an environment variable injected into a child process via `envroll exec` (in-memory only — never on disk).

## What envroll prints, and where

| Stream | Contents |
| --- | --- |
| stdout | The command's primary output (an env name, a key value, a JSON document). Never a passphrase. Never a plaintext env body, except `envroll get <KEY>` which prints exactly the value the user asked for. |
| stderr | Status lines, warnings, and structured errors of the form `envroll: <category>: <message>`. Never a passphrase. Never a plaintext value, except `envroll status --show-values` and `envroll diff --show-values` which the user has explicitly opted into. |
| Logs | None on disk by default. With `--log debug` or `RUST_LOG=debug`, full anyhow chains go to stderr. envroll never writes its own log files. |
| Telemetry | None. envroll makes no network call other than `git fetch`/`git push` to the user-configured `origin`. |

## Demo mode (`ENVROLL_DEMO_MODE`)

envroll ships with an undocumented-on-purpose presentation mode for
recording screencasts and conference demos. It activates only when BOTH
of these env vars are set:

```text
ENVROLL_DEMO_MODE=1
ENVROLL_PASSPHRASE=<your demo vault's passphrase>
```

When active, the interactive `rpassword` prompt is replaced by a visible
animation: the prompt label appears (`envroll passphrase:`), pauses
briefly, then 8 bullet glyphs (`•`) type out one at a time. The
passphrase actually used is `$ENVROLL_PASSPHRASE` — the bullets are
purely cosmetic and the bytes printed are literally the `•` glyph.

### What demo mode does NOT change

- **Canary verification still runs.** The passphrase from
  `$ENVROLL_PASSPHRASE` is still fed to the same `crypto::verify_canary`
  path that every encrypted-content operation uses. A wrong passphrase
  fails with `EnvrollError::WrongPassphrase` (exit 10) just like in
  normal mode.
- **No new logging surface.** The actual passphrase bytes never reach
  the terminal, stdout, stderr, or any logger — only the `•` bullets
  do.
- **No new attack surface.** The animation reads from an env var, which
  was already a supported (and documented) passphrase source. Demo mode
  adds zero new code paths into the crypto / lock / vault layers.

### When to NEVER use it

- **In CI or production.** Demo mode requires `$ENVROLL_PASSPHRASE` to
  be set, and a CI environment with that env var leaked into a process
  group is exactly the failure mode envroll's threat model warns about.
  CI should use `--passphrase-stdin` instead.
- **On a real vault with secrets you care about.** Demo mode is for
  scratch vaults whose passphrase you wouldn't mind leaking. Even
  though demo mode adds no new exposure vector, treating it as a "test
  fixture" reduces the chance of accidentally leaving the env vars set
  in your shell session and then doing something real.

### Why it lives in the production binary

Keeping demo mode in `main` instead of a fork means demos exercise the
same passphrase code path that real users hit, modulo the prompt
animation. A fork would drift over time and quietly stop being
representative. The full implementation is in `src/prompt.rs` — search
for `DEMO_MODE_ENV`.

## Reporting a vulnerability

If you believe you've found a security issue in envroll — anything that
violates the "envroll protects against" list above, leaks plaintext or
passphrase material, or undermines the on-disk integrity guarantees — please
report it privately rather than opening a public issue.

**Email:** `adrianjpr@gmail.com`

Or open a private security advisory on GitHub:
<https://github.com/AdriiiPRodri/envroll/security/advisories/new>.

What helps us triage quickly:

1. A clear description of the vulnerability and the impact you observed.
2. The smallest reproduction you can produce (a few commands or a tiny test
   case is ideal).
3. The envroll version (`envroll --version`), platform, and any non-default
   environment (vault on a network mount, exotic FS, custom `--passphrase-env`
   convention, etc.).
4. Your timeline expectations and whether you'd like public credit when the
   fix ships.

We'll acknowledge receipt within **3 business days** and aim to ship a fix or
a documented mitigation within **30 days** for high-severity issues.

## Out-of-scope reports

The following are deliberately not vulnerabilities (they're documented
limitations or accepted v0.1 trade-offs):

- "An attacker with root on my laptop can read `.checkout/<env>`." Yes — the
  threat model excludes active local attackers.
- "If I forget my passphrase, I can't recover my envs." Yes — that's the
  single security guarantee the entire design is anchored to.
- "Concurrent `envroll set` calls from two terminals can interleave." Yes —
  v0.1 has a vault-wide advisory lock, not per-env locks. v0.2 may revisit.
- "Pushing the vault to a public repo leaks env names and timestamps." Yes —
  the threat model says exactly this; a passive attacker sees ciphertext and
  commit metadata. If env names are themselves sensitive, don't sync.

If you're not sure whether something is in scope, err on the side of
reporting it — we'd rather triage a non-issue than miss a real one.
