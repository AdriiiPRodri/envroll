//! Structured error taxonomy with stable exit codes (design.md D14).
//!
//! Codes 0/1/2 follow Unix/clap conventions. 10–59 are stable from v1.0.
//! Codes 60–99 are reserved for v2.x category expansion and MUST NOT be
//! used here. Codes >= 100 are forbidden.
//!
//! `nothing to save` is a SUCCESS branch (exit 0, informational stderr) and
//! is deliberately NOT modelled as a variant.

use std::io;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EnvrollError {
    #[error("{0}")]
    Generic(String),

    #[error("wrong passphrase (canary failed to decrypt)")]
    WrongPassphrase,

    #[error("file is corrupt or has been tampered with: {0}")]
    FileCorrupt(String),

    #[error("could not parse .env file: {0}")]
    ParseError(String),

    #[error("env not found: {0}")]
    EnvNotFound(String),

    #[error("ref not found: {0}")]
    RefNotFound(String),

    #[error("not an envroll-registered project (run `envroll init` here)")]
    ProjectNotFound,

    #[error("env name already exists: {0} (pass --force to overwrite)")]
    NameCollision(String),

    #[error("./.env exists and is not managed by envroll: {0}")]
    UnmanagedEnvPresent(String),

    #[error("no remote configured (use `envroll remote set <url>`)")]
    NoRemote,

    #[error("sync conflict: local and remote vault histories have diverged")]
    SyncConflict,

    #[error("remote transport error: {0}")]
    RemoteTransportError(String),

    #[error("cannot read passphrase: no usable source available")]
    NoPassphraseSource,

    #[error("vault lock held by another envroll process")]
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
}

impl EnvrollError {
    /// Return the stable exit code for this error category.
    pub fn exit_code(&self) -> i32 {
        match self {
            EnvrollError::Generic(_) => 1,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_within_v1_stable_range() {
        // Codes 0/1/2 are clap/unix conventions; everything else must be
        // 10..=59 (stable) per D14. Codes 60..=99 are reserved; >=100 forbidden.
        let variants = [
            EnvrollError::Generic("x".into()),
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
