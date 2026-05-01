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

use std::io::{self, IsTerminal, Read};

use secrecy::SecretString;

use crate::errors::{generic, EnvrollError};

/// Default env-var name for passphrase fallback. Override at the call site
/// via `PassphraseSources::env_var_name`.
pub const DEFAULT_PASSPHRASE_ENV: &str = "ENVROLL_PASSPHRASE";

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
}
