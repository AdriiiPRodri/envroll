//! `envroll diff <a> <b>` — key-level diff between two refs.
//!
//! Both refs are resolved via the shared ref grammar (`<name>`,
//! `<name>@<short-hash>`, `<name>@~N`). The two named envs may be the same
//! (compare two versions of one env) or different (compare current state of
//! two envs). Acquires a shared lock.

use std::collections::BTreeSet;

use clap::Args as ClapArgs;
use serde::Serialize;
use tabled::{builder::Builder, settings::Style};

use crate::cli::common::{
    missing_existing_env_error, open_project, parse_ref, read_pass_and_verify, LockMode,
};
use crate::cli::Context;
use crate::crypto;
use crate::errors::{usage, EnvrollError};
use crate::output::OutputFormat;
use crate::parser;
use crate::vault::git::RefForm;
use crate::vault::sweep_historical_checkouts;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Left-hand ref (a `<name>`, `<name>@<short-hash>`, or `<name>@~N`).
    #[arg(value_name = "A")]
    pub a: Option<String>,

    /// Right-hand ref.
    #[arg(value_name = "B")]
    pub b: Option<String>,

    /// Mask values with `********` instead of printing them. Off by default —
    /// you're on your own machine looking at your own envs. Enable this when
    /// you're about to paste the output into a screenshot, ticket, or chat.
    #[arg(long)]
    pub mask: bool,

    /// Deprecated alias for the inverse of `--mask`. Kept so the spec's
    /// original `--show-values` invocation still works; new scripts should
    /// just stop passing it (showing values is the default now).
    #[arg(long, hide = true)]
    pub show_values: bool,
}

#[derive(Debug, Serialize)]
struct DiffJson {
    a: String,
    b: String,
    added: Vec<KeyVal>,
    removed: Vec<KeyVal>,
    changed: Vec<KeyChange>,
}

#[derive(Debug, Serialize)]
struct KeyVal {
    key: String,
    value: String,
}

#[derive(Debug, Serialize)]
struct KeyChange {
    key: String,
    a: String,
    b: String,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let prep = open_project(ctx, LockMode::Shared)?;
    let _ = sweep_historical_checkouts(
        &prep.vault,
        &prep.repo,
        prep.project_id(),
        &prep.project_root,
        &prep.manifest.target_filename,
    );

    let (a_arg, b_arg) = match (args.a, args.b) {
        (Some(a), Some(b)) => (a, b),
        (None, _) => {
            return Err(missing_existing_env_error(
                &prep,
                "envroll diff <A> <B>  (refs: <name> | <name>@<hash> | <name>@~N)",
            ));
        }
        (Some(_), None) => {
            return Err(usage(
                "missing second ref",
                Some("usage: envroll diff <A> <B>".to_string()),
            ));
        }
    };

    let (a_name, a_form) = parse_ref(&a_arg)?;
    let (b_name, b_form) = parse_ref(&b_arg)?;

    if !prep.env_blob_path(&a_name).exists() {
        return Err(EnvrollError::EnvNotFound(format!(
            "env \"{a_name}\" not found"
        )));
    }
    if !prep.env_blob_path(&b_name).exists() {
        return Err(EnvrollError::EnvNotFound(format!(
            "env \"{b_name}\" not found"
        )));
    }

    let pass = read_pass_and_verify(&prep, ctx)?;
    let a_kv = decrypt_ref(&prep, &a_name, &a_form, &pass)?;
    let b_kv = decrypt_ref(&prep, &b_name, &b_form, &pass)?;

    let a_keys: BTreeSet<&String> = a_kv.keys().collect();
    let b_keys: BTreeSet<&String> = b_kv.keys().collect();

    let added: Vec<KeyVal> = b_keys
        .difference(&a_keys)
        .map(|k| KeyVal {
            key: (*k).clone(),
            value: b_kv[*k].clone(),
        })
        .collect();
    let removed: Vec<KeyVal> = a_keys
        .difference(&b_keys)
        .map(|k| KeyVal {
            key: (*k).clone(),
            value: a_kv[*k].clone(),
        })
        .collect();
    let changed: Vec<KeyChange> = a_keys
        .intersection(&b_keys)
        .filter(|k| a_kv[**k] != b_kv[**k])
        .map(|k| KeyChange {
            key: (*k).clone(),
            a: a_kv[*k].clone(),
            b: b_kv[*k].clone(),
        })
        .collect();

    match ctx.format {
        OutputFormat::Human => {
            // Default: show values. `--mask` opts into masking; the legacy
            // `--show-values` is honoured (its presence forces show even if
            // `--mask` was somehow combined with it, since the user's later
            // intent is the more interesting flag).
            let show = !args.mask || args.show_values;
            print_human_table(&a_arg, &b_arg, &added, &removed, &changed, show);
        }
        OutputFormat::Json => {
            let payload = DiffJson {
                a: a_arg,
                b: b_arg,
                added,
                removed,
                changed,
            };
            let s = serde_json::to_string(&payload)
                .map_err(|e| EnvrollError::Generic(format!("serializing diff JSON: {e}")))?;
            println!("{s}");
        }
    }
    Ok(())
}

fn decrypt_ref(
    prep: &crate::cli::common::PreparedProject,
    env_name: &str,
    form: &RefForm,
    pass: &age::secrecy::SecretString,
) -> Result<std::collections::BTreeMap<String, String>, EnvrollError> {
    let scope = prep.repo.project(prep.project_id());
    let bytes = match form {
        RefForm::Latest => std::fs::read(prep.env_blob_path(env_name)).map_err(EnvrollError::Io)?,
        _ => {
            let oid = scope.resolve_ref(env_name, form.clone())?;
            read_blob_at_commit(prep.vault.root(), env_name, prep.project_id(), oid)?
        }
    };
    let plaintext = crypto::decrypt(&bytes, pass)?;
    Ok(parser::as_key_value_map(&parser::parse_buf(&plaintext)?))
}

fn read_blob_at_commit(
    vault_root: &std::path::Path,
    env_name: &str,
    project_id: &str,
    oid: git2::Oid,
) -> Result<Vec<u8>, EnvrollError> {
    let repo = git2::Repository::open(vault_root)
        .map_err(|e| EnvrollError::Generic(format!("libgit2: {e}")))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|e| EnvrollError::Generic(format!("libgit2 find_commit: {e}")))?;
    let tree = commit
        .tree()
        .map_err(|e| EnvrollError::Generic(format!("libgit2 tree: {e}")))?;
    let relpath = format!("projects/{project_id}/envs/{env_name}.age");
    let entry = tree
        .get_path(std::path::Path::new(&relpath))
        .map_err(|e| EnvrollError::Generic(format!("blob missing at commit: {e}")))?;
    let object = entry
        .to_object(&repo)
        .map_err(|e| EnvrollError::Generic(format!("libgit2 to_object: {e}")))?;
    let blob = object
        .as_blob()
        .ok_or_else(|| EnvrollError::Generic("expected blob".to_string()))?;
    Ok(blob.content().to_vec())
}

fn display(value: &str, show: bool) -> String {
    if show {
        value.to_string()
    } else {
        "********".to_string()
    }
}

/// Render the diff as a tabled `Builder` so the A/B column headers can carry
/// the actual env-arg strings (`dev`, `stg@a1b2c3d4`, etc.) instead of the
/// static `A` / `B` placeholders the derive macro forces. Values stay masked
/// unless `--show-values` was passed.
fn print_human_table(
    a_arg: &str,
    b_arg: &str,
    added: &[KeyVal],
    removed: &[KeyVal],
    changed: &[KeyChange],
    show_values: bool,
) {
    if added.is_empty() && removed.is_empty() && changed.is_empty() {
        println!("no differences between {a_arg} and {b_arg}");
        return;
    }
    let mut builder = Builder::default();
    builder.push_record(["OP", "KEY", a_arg, b_arg]);
    for KeyVal { key, value } in added {
        builder.push_record(["+", key, "—", &display(value, show_values)]);
    }
    for KeyVal { key, value } in removed {
        builder.push_record(["-", key, &display(value, show_values), "—"]);
    }
    for KeyChange { key, a, b } in changed {
        builder.push_record(["~", key, &display(a, show_values), &display(b, show_values)]);
    }
    let mut table = builder.build();
    table.with(Style::rounded());
    println!("{table}");
}
