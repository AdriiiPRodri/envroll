//! Passphrase intake.
//!
//! Precedence on every encrypted-content operation:
//!
//! 1. TTY + `--passphrase-stdin` → usage error (exit 2).
//! 2. Non-TTY + `--passphrase-stdin` → read stdin to EOF.
//! 3. TTY → `rpassword` prompt on the controlling terminal.
//! 4. Non-TTY + env var (default `ENVROLL_PASSPHRASE`, override via
//!    `--passphrase-env <NAME>`) → use it.
//! 5. Otherwise → [`EnvrollError::NoPassphraseSource`] with the multi-line
//!    message from D11.
//!
//! Passphrases never touch [`String`] (callers receive [`SecretString`]) so
//! the bytes are zeroized on drop by `secrecy`.
//!
//! ## Demo mode (`ENVROLL_DEMO_MODE`)
//!
//! When BOTH the env vars `ENVROLL_DEMO_MODE=1` AND `$ENVROLL_PASSPHRASE`
//! (or whatever `--passphrase-env` resolves to) are set, the passphrase
//! intake path is replaced by a **purely cosmetic animation** intended for
//! recording demos and screencasts. The animation:
//!
//! 1. Prints the prompt label (e.g. `envroll passphrase: `) to stderr.
//! 2. Sleeps ~300 ms (simulating "user reading the prompt").
//! 3. Prints 8 bullet characters (`•`) one at a time with ~80 ms between
//!    each (simulating typing).
//! 4. Newline, then returns the value of `$ENVROLL_PASSPHRASE` as the
//!    actual passphrase.
//!
//! What demo mode does NOT do:
//!
//! - **It does NOT bypass canary verification.** The passphrase returned
//!   is the real one from the env var; the caller still decrypts the
//!   vault canary with it and fails with [`EnvrollError::WrongPassphrase`]
//!   if it doesn't match. Demo mode is purely about replacing the
//!   interactive `rpassword` prompt with a visible animation — every
//!   security check downstream stays the same.
//! - **It does NOT log the passphrase or expose it anywhere new.** The
//!   bullets printed are literally the `•` glyph; the actual passphrase
//!   bytes never reach the terminal or any logger.
//! - **It does NOT activate accidentally.** Both env vars must be set
//!   explicitly. `ENVROLL_DEMO_MODE` alone (without `ENVROLL_PASSPHRASE`)
//!   produces a hard error rather than silently falling back to the
//!   normal interactive path — a missing passphrase env var while
//!   recording would just hang the demo otherwise.
//!
//! Why it lives in the production binary instead of a fork: keeping it
//! in `main` means demos always exercise the SAME passphrase code path
//! that real users hit, modulo the prompt animation. A fork would drift.
//!
//! See `SECURITY.md` for the operational guidance ("never set this in CI
//! or production").

use std::io::{self, IsTerminal, Read, Write};
use std::thread::sleep;
use std::time::Duration;

use secrecy::SecretString;

use crate::errors::{generic, EnvrollError};

/// Default env-var name for passphrase fallback. Override at the call site
/// via `PassphraseSources::env_var_name`.
pub const DEFAULT_PASSPHRASE_ENV: &str = "ENVROLL_PASSPHRASE";

/// Env var that opts into the demo-mode prompt animation. See the module
/// docs above for the full contract — in short: when set alongside
/// `ENVROLL_PASSPHRASE`, the interactive `rpassword` prompt is replaced
/// by a visible bullet-typing animation, and everything else (canary
/// verification, atomic writes, lock acquisition) stays unchanged.
pub const DEMO_MODE_ENV: &str = "ENVROLL_DEMO_MODE";

/// Aggregated CLI/env inputs that drive [`read_passphrase`].
#[derive(Debug, Clone)]
pub struct PassphraseSources<'a> {
    /// True when `--passphrase-stdin` was passed.
    pub stdin_flag: bool,
    /// Env-var name used as the last-resort fallback. None means "use the default".
    pub env_var_name: Option<&'a str>,
}

impl<'a> PassphraseSources<'a> {
    pub fn new(stdin_flag: bool, env_var_name: Option<&'a str>) -> Self {
        Self {
            stdin_flag,
            env_var_name,
        }
    }

    fn env_name(&self) -> &str {
        self.env_var_name.unwrap_or(DEFAULT_PASSPHRASE_ENV)
    }
}

/// The verbatim no-passphrase-source error message from D11. Kept here so
/// the spec-conformance test in section 15 can compare against it byte-for-byte.
pub const NO_PASSPHRASE_SOURCE_MESSAGE: &str = concat!(
    "envroll: cannot read passphrase. None of the supported sources is available:\n",
    "  • stdin is not a TTY (so interactive prompt is unavailable)\n",
    "  • --passphrase-stdin was not passed\n",
    "  • $ENVROLL_PASSPHRASE is not set (override the variable name with --passphrase-env <NAME>)\n",
    "\n",
    "For interactive use, run envroll from a terminal that owns stdin.\n",
    "For CI or scripts, pipe the passphrase via --passphrase-stdin or set $ENVROLL_PASSPHRASE.\n",
);

/// Read a passphrase from the highest-priority available source.
///
/// `prompt_label` is shown to the user only when the TTY branch is taken.
pub fn read_passphrase(
    sources: &PassphraseSources<'_>,
    prompt_label: &str,
) -> Result<SecretString, EnvrollError> {
    // Demo mode: replace the interactive prompt with a visible animation
    // and read from `$ENVROLL_PASSPHRASE`. See module docs + SECURITY.md
    // for the full contract — in particular, the canary check downstream
    // is unchanged.
    if demo_mode_active() {
        return demo_mode_read_one(sources, prompt_label);
    }

    let stdin_is_tty = io::stdin().is_terminal();

    // Rule 1: TTY + --passphrase-stdin → usage error.
    if stdin_is_tty && sources.stdin_flag {
        return Err(generic(
            "--passphrase-stdin requires non-TTY stdin (pipe a value in)",
        ));
    }

    // Rule 2: non-TTY + --passphrase-stdin → read stdin to EOF.
    if !stdin_is_tty && sources.stdin_flag {
        return read_stdin_passphrase();
    }

    // Rule 3: TTY → interactive prompt on the controlling terminal.
    if stdin_is_tty {
        let raw = rpassword::prompt_password(format!("{prompt_label}: "))
            .map_err(|e| generic(format!("failed to read passphrase from terminal: {e}")))?;
        return Ok(SecretString::new(raw.into_boxed_str()));
    }

    // Rule 4: non-TTY + env var.
    if let Ok(val) = std::env::var(sources.env_name()) {
        return Ok(SecretString::new(val.into_boxed_str()));
    }

    // Rule 5: nothing left.
    Err(EnvrollError::NoPassphraseSource)
}

/// Prompt for a passphrase twice and confirm they match. Used by
/// `envroll init` on a fresh vault. Only meaningful on a TTY; in any
/// other source mode this just delegates to [`read_passphrase`] (since
/// piped/env passphrases cannot be re-entered).
pub fn read_passphrase_confirm(
    sources: &PassphraseSources<'_>,
    prompt_label: &str,
) -> Result<SecretString, EnvrollError> {
    // Demo mode: animate BOTH the initial prompt and the "confirm:" prompt
    // so a viewer of the recording sees the full first-run init flow. The
    // value is read once from $ENVROLL_PASSPHRASE — there's nothing to
    // mismatch since both "inputs" come from the same env var.
    if demo_mode_active() {
        let pass = demo_mode_read_one(sources, prompt_label)?;
        animate_passphrase_prompt("confirm");
        return Ok(pass);
    }

    let stdin_is_tty = io::stdin().is_terminal();
    if !stdin_is_tty {
        return read_passphrase(sources, prompt_label);
    }

    let a = rpassword::prompt_password(format!("{prompt_label}: "))
        .map_err(|e| generic(format!("failed to read passphrase: {e}")))?;
    let b = rpassword::prompt_password("confirm: ")
        .map_err(|e| generic(format!("failed to read passphrase confirmation: {e}")))?;
    if a != b {
        return Err(generic("passphrases did not match; aborting"));
    }
    Ok(SecretString::new(a.into_boxed_str()))
}

// ---------------------------------------------------------------------------
// Demo mode helpers
// ---------------------------------------------------------------------------

/// True iff `ENVROLL_DEMO_MODE` is set to ANY non-empty value.
///
/// The check is intentionally permissive on the value (`1`, `true`, `yes`,
/// or anything else non-empty all activate it) because demo-mode callers
/// are scripts setting it via shell exports — we want the obvious values
/// to all work. The strict requirement is paired with `$ENVROLL_PASSPHRASE`
/// being set, enforced at use time so the failure mode is "demo mode
/// active but no passphrase to feed it" rather than "silently fell back
/// to real prompt and the recording hung".
fn demo_mode_active() -> bool {
    std::env::var(DEMO_MODE_ENV)
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Animate a single passphrase prompt and return the env-var passphrase.
/// Errors loudly if `$ENVROLL_PASSPHRASE` isn't set — silent fallback
/// to the real prompt would block any non-interactive recording.
fn demo_mode_read_one(
    sources: &PassphraseSources<'_>,
    prompt_label: &str,
) -> Result<SecretString, EnvrollError> {
    let env_name = sources.env_name();
    let value = std::env::var(env_name).map_err(|_| {
        generic(format!(
            "{DEMO_MODE_ENV} is set but {env_name} is not — demo mode requires \
             a passphrase env var so the animation has something to return"
        ))
    })?;
    animate_passphrase_prompt(prompt_label);
    Ok(SecretString::new(value.into_boxed_str()))
}

/// Print a fake passphrase prompt with a typing-bullets animation. Pure
/// cosmetic — the bytes printed are literally the `•` glyph; the actual
/// passphrase never reaches the terminal.
///
/// Output goes to stderr to match `rpassword`'s real behavior (rpassword
/// prompts on stderr / the controlling tty, never stdout, so structured
/// stdout output stays parseable). Skipped silently when stderr is not a
/// TTY because there's no point animating into a pipe — but the env-var
/// passphrase is still returned so the rest of envroll works the same.
fn animate_passphrase_prompt(label: &str) {
    let mut stderr = io::stderr();
    if !stderr.is_terminal() {
        return;
    }
    let _ = write!(stderr, "{label}: ");
    let _ = stderr.flush();
    sleep(Duration::from_millis(300));
    for _ in 0..8 {
        let _ = write!(stderr, "•");
        let _ = stderr.flush();
        sleep(Duration::from_millis(80));
    }
    let _ = writeln!(stderr);
}

fn read_stdin_passphrase() -> Result<SecretString, EnvrollError> {
    let mut buf = String::new();
    io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| generic(format!("failed to read passphrase from stdin: {e}")))?;
    // Strip exactly one trailing newline (\n or \r\n) for ergonomic shell
    // pipelines like `echo "pass" | envroll save --passphrase-stdin`.
    // Multiple trailing newlines are preserved — those are part of the
    // user's intended passphrase.
    if let Some(stripped) = buf.strip_suffix('\n') {
        buf = stripped.strip_suffix('\r').unwrap_or(stripped).to_string();
    }
    Ok(SecretString::new(buf.into_boxed_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;
    use std::sync::Mutex;

    /// Tests that mutate process-wide env vars share this mutex so they
    /// don't race when cargo runs the test binary with multiple threads.
    /// Without it, two tests setting/unsetting `ENVROLL_DEMO_MODE` in
    /// parallel would observe each other's state and flake.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn no_passphrase_source_message_mentions_all_three_options() {
        // The spec test in section 15 will compare verbatim; here we sanity-
        // check that the message lists each source.
        assert!(NO_PASSPHRASE_SOURCE_MESSAGE.contains("stdin is not a TTY"));
        assert!(NO_PASSPHRASE_SOURCE_MESSAGE.contains("--passphrase-stdin"));
        assert!(NO_PASSPHRASE_SOURCE_MESSAGE.contains("ENVROLL_PASSPHRASE"));
    }

    #[test]
    fn default_env_var_name_is_envroll_passphrase() {
        let s = PassphraseSources::new(false, None);
        assert_eq!(s.env_name(), "ENVROLL_PASSPHRASE");
    }

    #[test]
    fn explicit_env_var_name_overrides_default() {
        let s = PassphraseSources::new(false, Some("MY_PASS"));
        assert_eq!(s.env_name(), "MY_PASS");
    }

    #[test]
    fn demo_mode_active_requires_non_empty_env_var() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = std::env::var_os(DEMO_MODE_ENV);

        std::env::remove_var(DEMO_MODE_ENV);
        assert!(!demo_mode_active(), "missing env var → not active");

        std::env::set_var(DEMO_MODE_ENV, "");
        assert!(!demo_mode_active(), "empty env var → not active");

        std::env::set_var(DEMO_MODE_ENV, "1");
        assert!(demo_mode_active(), "any non-empty value → active");

        std::env::set_var(DEMO_MODE_ENV, "true");
        assert!(
            demo_mode_active(),
            "alternative truthy values also activate"
        );

        // Restore.
        match saved {
            Some(v) => std::env::set_var(DEMO_MODE_ENV, v),
            None => std::env::remove_var(DEMO_MODE_ENV),
        }
    }

    #[test]
    fn demo_mode_read_one_returns_the_env_var_passphrase_value() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Use a test-unique passphrase var name so we don't stomp on the
        // user's real ENVROLL_PASSPHRASE if they happen to have one set.
        let pass_name = "ENVROLL_TEST_DEMO_PASS_alpha";
        std::env::set_var(pass_name, "demo-secret-42");

        let sources = PassphraseSources::new(false, Some(pass_name));
        let result = demo_mode_read_one(&sources, "test").expect("should succeed");

        std::env::remove_var(pass_name);

        assert_eq!(result.expose_secret(), "demo-secret-42");
    }

    #[test]
    fn demo_mode_read_one_errors_when_passphrase_env_var_missing() {
        let _guard = ENV_LOCK.lock().unwrap();
        let pass_name = "ENVROLL_TEST_DEMO_PASS_beta";
        // Make sure it isn't set from a prior failing test.
        std::env::remove_var(pass_name);

        let sources = PassphraseSources::new(false, Some(pass_name));
        let err = demo_mode_read_one(&sources, "test").expect_err("should fail");

        // Error message must point at BOTH env vars so the user knows what
        // to set — silent fallback would just hang the recording.
        let msg = err.to_string();
        assert!(msg.contains(DEMO_MODE_ENV), "missing demo var hint: {msg}");
        assert!(
            msg.contains(pass_name),
            "missing passphrase var hint: {msg}"
        );
    }
}
