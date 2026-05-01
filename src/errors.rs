//! Structured error taxonomy with stable exit codes.
//!
//! Codes 0/1/2 follow Unix/clap conventions. 10–59 are stable from v1.0.
//! Codes 60–99 are reserved for v2.x category expansion and MUST NOT be
//! used here. Codes >= 100 are forbidden.
//!
//! `nothing to save` is a SUCCESS branch (exit 0, informational stderr) and
//! is deliberately NOT modelled as a variant.
//!
//! Each variant is annotated as a [`miette::Diagnostic`] so the binary
//! boundary in `lib.rs::run` can render it through `miette::Report` for the
//! colored, boxed, icon-decorated output the user sees on stderr. We
//! intentionally do NOT set `code(...)` — miette would otherwise print a
//! `envroll::<category>` line above every error, which is noise for users
//! (the exit code already carries the structured identifier scripts need;
//! the human-readable category lives in [`Self::category`]).

use std::io;
use std::path::PathBuf;

use miette::Diagnostic;
use thiserror::Error;

/// Error variants are deliberately written as transparent (`"{0}"`) wrappers
/// over a String so each call site can hand the exact spec-mandated text.
/// The variant identity carries the exit code; the message is just bytes for
/// the human reader. Variants without an arg fix their canonical message.
#[derive(Debug, Error, Diagnostic)]
pub enum EnvrollError {
    #[error("{0}")]
    Generic(String),

    #[error("wrong passphrase")]
    #[diagnostic(help(
        "Re-run with the correct passphrase. envroll has no recovery for a forgotten one — check your password manager."
    ))]
    WrongPassphrase,

    #[error("{0}")]
    #[diagnostic(help(
        "The age MAC failed. The file may have been tampered with or partially overwritten — check the vault git history with regular git tools."
    ))]
    FileCorrupt(String),

    #[error("{0}")]
    #[diagnostic(help(
        "envroll uses dotenvy to parse .env files. See the README's `Supported .env syntax` section for the accepted shapes."
    ))]
    ParseError(String),

    #[error("{0}")]
    EnvNotFound(String),

    #[error("{0}")]
    #[diagnostic(help(
        "Refs accept three forms: <name>, <name>@<short-hash> (>= 7 hex), <name>@~N (offset >= 1)."
    ))]
    RefNotFound(String),

    #[error("not an envroll project (run `envroll init` here)")]
    ProjectNotFound,

    #[error("{0}")]
    #[diagnostic(help("Pass --force to overwrite the existing env, or pick a different name."))]
    NameCollision(String),

    #[error("{0}")]
    #[diagnostic(help(
        "Pass --force to overwrite ./.env, or --rescue <name> to save it as a new env first."
    ))]
    UnmanagedEnvPresent(String),

    #[error("no remote configured (use `envroll remote set <url>`)")]
    NoRemote,

    #[error("sync conflict: local and remote vault histories have diverged")]
    #[diagnostic(help(
        "cd into the vault and use regular git tools (git log, git merge, git rebase) to reconcile, then re-run `envroll sync`."
    ))]
    SyncConflict,

    #[error("{0}")]
    RemoteTransportError(String),

    #[error("cannot read passphrase: no usable source available")]
    #[diagnostic(help(
        "On a TTY: just run envroll. In CI: pipe the passphrase via --passphrase-stdin or set $ENVROLL_PASSPHRASE."
    ))]
    NoPassphraseSource,

    #[error("vault is locked by another envroll process")]
    #[diagnostic(help("Wait for the other envroll process to finish, then retry."))]
    VaultLockHeld,

    #[error("permission denied on vault path: {0}")]
    PermissionDenied(PathBuf),

    // ---------------------------------------------------------------------
    // Codes 53–59 are available for v1.x stable additions.
    // Codes 60–99 are reserved for v2.x category expansion (key/agent
    // errors, signed-tag verification failures). MUST NOT be used in v1.x.
    // Codes >= 100 are forbidden in v1.x.
    // ---------------------------------------------------------------------
    #[error(transparent)]
    Io(#[from] io::Error),

    /// Rich diagnostic for "you forgot a CLI argument" cases. The `header` is
    /// the short complaint (e.g., `no env name given`), `body` carries the
    /// usage hint or extra context (e.g., the list of available envs and the
    /// canonical invocation). Lives here so miette can render header + body
    /// with the same icon and indentation as every other Diagnostic; we
    /// avoid the temptation to print these via plain `eprintln!` and lose
    /// the visual consistency.
    #[error("{header}")]
    Usage {
        header: String,
        #[help]
        body: Option<String>,
    },
}

impl EnvrollError {
    /// Return the stable exit code for this error category.
    pub fn exit_code(&self) -> i32 {
        match self {
            EnvrollError::Generic(_) | EnvrollError::Usage { .. } => 1,
            EnvrollError::WrongPassphrase => 10,
            EnvrollError::FileCorrupt(_) => 11,
            EnvrollError::ParseError(_) => 12,
            EnvrollError::EnvNotFound(_) => 20,
            EnvrollError::RefNotFound(_) => 21,
            EnvrollError::ProjectNotFound => 22,
            EnvrollError::NameCollision(_) => 30,
            EnvrollError::UnmanagedEnvPresent(_) => 31,
            EnvrollError::NoRemote => 40,
            EnvrollError::SyncConflict => 41,
            EnvrollError::RemoteTransportError(_) => 42,
            EnvrollError::NoPassphraseSource => 50,
            EnvrollError::VaultLockHeld => 51,
            EnvrollError::PermissionDenied(_) => 52,
            EnvrollError::Io(_) => 1,
        }
    }

    /// Short category tag printed as `envroll: <category>: <message>` on stderr.
    pub fn category(&self) -> &'static str {
        match self {
            EnvrollError::Generic(_) => "error",
            EnvrollError::Usage { .. } => "usage",
            EnvrollError::WrongPassphrase => "wrong passphrase",
            EnvrollError::FileCorrupt(_) => "file corrupt",
            EnvrollError::ParseError(_) => "parse error",
            EnvrollError::EnvNotFound(_) => "env not found",
            EnvrollError::RefNotFound(_) => "ref not found",
            EnvrollError::ProjectNotFound => "project not found",
            EnvrollError::NameCollision(_) => "name collision",
            EnvrollError::UnmanagedEnvPresent(_) => "unmanaged env present",
            EnvrollError::NoRemote => "no remote",
            EnvrollError::SyncConflict => "sync conflict",
            EnvrollError::RemoteTransportError(_) => "remote transport error",
            EnvrollError::NoPassphraseSource => "no passphrase source",
            EnvrollError::VaultLockHeld => "vault lock held",
            EnvrollError::PermissionDenied(_) => "permission denied",
            EnvrollError::Io(_) => "io",
        }
    }
}

/// Convenience for one-off Generic errors at call sites.
pub fn generic(msg: impl Into<String>) -> EnvrollError {
    EnvrollError::Generic(msg.into())
}

/// Convenience for usage-class errors that want a header + multi-line help
/// body rendered together by miette. `header` shows next to the `×` icon,
/// `body` (optional) is rendered as `help:` underneath.
pub fn usage(header: impl Into<String>, body: Option<String>) -> EnvrollError {
    EnvrollError::Usage {
        header: header.into(),
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_within_v1_stable_range() {
        // Codes 0/1/2 are clap/unix conventions; everything else must be
        // 10..=59 (stable) per D14. Codes 60..=99 are reserved; >=100 forbidden.
        let variants = [
            EnvrollError::Generic("x".into()),
            EnvrollError::Usage {
                header: "x".into(),
                body: None,
            },
            EnvrollError::WrongPassphrase,
            EnvrollError::FileCorrupt("x".into()),
            EnvrollError::ParseError("x".into()),
            EnvrollError::EnvNotFound("x".into()),
            EnvrollError::RefNotFound("x".into()),
            EnvrollError::ProjectNotFound,
            EnvrollError::NameCollision("x".into()),
            EnvrollError::UnmanagedEnvPresent("x".into()),
            EnvrollError::NoRemote,
            EnvrollError::SyncConflict,
            EnvrollError::RemoteTransportError("x".into()),
            EnvrollError::NoPassphraseSource,
            EnvrollError::VaultLockHeld,
            EnvrollError::PermissionDenied(PathBuf::from("/x")),
        ];
        for v in variants {
            let code = v.exit_code();
            assert!(
                (0..=59).contains(&code),
                "{v:?} returned code {code}, outside stable v1 range"
            );
        }
    }
}
