//! `envroll projects` — list every envroll project on this machine (lock-free).
//!
//! Lock-free: every manifest write is tempfile+rename, so
//! reads can never observe a torn file. Worst case we miss a project that
//! was just registered — which the user can re-run to see.

use std::fs;
use std::path::Path;

use clap::Args as ClapArgs;
use serde::Serialize;
use tabled::{
    settings::{object::Columns, Alignment, Modify, Style},
    Table, Tabled,
};

use crate::cli::Context;
use crate::errors::EnvrollError;
use crate::manifest::{IdSource, Manifest};
use crate::output::OutputFormat;
use crate::paths::{project_envs_dir, projects_dir, resolve_vault_root};

#[derive(Debug, ClapArgs)]
pub struct Args {}

/// JSON shape for `envroll projects --format json`. Lives next to the
/// printer to keep the schema co-located with the code that emits it.
/// Documented in `docs/json-schemas/projects.json` (task 15.4).
#[derive(Debug, Serialize)]
struct ProjectRow {
    id: String,
    envs: usize,
    /// `null` (in JSON) when no env is active.
    #[serde(serialize_with = "serialize_active")]
    active: Option<String>,
    id_source: IdSource,
    id_input: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

fn serialize_active<S: serde::Serializer>(v: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(s_) => s.serialize_str(s_),
        None => s.serialize_none(),
    }
}

pub fn run(_args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    let vault_root = resolve_vault_root(ctx.vault.as_deref())?;
    let mut rows = collect_rows(&vault_root)?;
    rows.sort_by_key(|r| r.created_at);

    match ctx.format {
        OutputFormat::Human => print_human(&rows, !ctx.no_color),
        OutputFormat::Json => print_json(&rows)?,
    }
    Ok(())
}

fn collect_rows(vault_root: &Path) -> Result<Vec<ProjectRow>, EnvrollError> {
    let projects_root = projects_dir(vault_root);
    if !projects_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&projects_root).map_err(EnvrollError::Io)? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        let manifest_path = path.join("manifest.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let m = match Manifest::load(&manifest_path) {
            Ok(m) => m,
            // Malformed or unreadable manifests are skipped rather than
            // failing the whole listing — `projects` is best-effort.
            Err(_) => continue,
        };
        let env_count = count_envs(&project_envs_dir(vault_root, &m.id));
        let active = if m.active.is_empty() {
            None
        } else {
            Some(m.active.clone())
        };
        out.push(ProjectRow {
            id: m.id,
            envs: env_count,
            active,
            id_source: m.id_source,
            id_input: m.id_input,
            created_at: m.created_at,
        });
    }
    Ok(out)
}

fn count_envs(envs_dir: &Path) -> usize {
    let entries = match fs::read_dir(envs_dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("age"))
        })
        .count()
}

/// View struct that controls how each row renders in the human table. Lives
/// next to the printer (and not on `ProjectRow`) so the JSON shape stays
/// completely independent of the table headers.
#[derive(Tabled)]
struct ProjectTableRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "ENVS")]
    envs: usize,
    #[tabled(rename = "ACTIVE")]
    active: String,
    #[tabled(rename = "SOURCE")]
    source: &'static str,
    #[tabled(rename = "CREATED")]
    created: String,
}

fn print_human(rows: &[ProjectRow], _color: bool) {
    if rows.is_empty() {
        println!("no projects registered");
        return;
    }
    let view: Vec<ProjectTableRow> = rows
        .iter()
        .map(|r| ProjectTableRow {
            id: r.id.clone(),
            envs: r.envs,
            active: r.active.clone().unwrap_or_else(|| "-".to_string()),
            source: id_source_str(r.id_source),
            // Trim sub-second precision and the offset suffix so the column
            // stays narrow without losing useful info.
            created: r.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
        })
        .collect();
    let mut table = Table::new(view);
    table
        .with(Style::rounded())
        .with(Modify::new(Columns::one(1)).with(Alignment::right()));
    println!("{table}");
}

fn print_json(rows: &[ProjectRow]) -> Result<(), EnvrollError> {
    let s = serde_json::to_string(&rows)
        .map_err(|e| EnvrollError::Generic(format!("serializing projects JSON: {e}")))?;
    println!("{s}");
    Ok(())
}

fn id_source_str(s: IdSource) -> &'static str {
    match s {
        IdSource::Remote => "remote",
        IdSource::Path => "path",
        IdSource::Manual => "manual",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;
    use crate::paths::{project_envs_dir, project_manifest};
    use std::fs;
    use tempfile::TempDir;

    fn write_project(vault_root: &Path, id: &str, source: IdSource, active: &str) -> Manifest {
        let mut m = Manifest::new(id.to_string(), source, String::new());
        m.active = active.to_string();
        let path = project_manifest(vault_root, id);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, m.to_toml().unwrap()).unwrap();
        let envs = project_envs_dir(vault_root, id);
        fs::create_dir_all(&envs).unwrap();
        m
    }

    #[test]
    fn collect_rows_returns_empty_when_no_projects_dir() {
        let dir = TempDir::new().unwrap();
        let rows = collect_rows(dir.path()).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn collect_rows_lists_every_manifest_with_env_counts() {
        let dir = TempDir::new().unwrap();
        write_project(dir.path(), "remote-aaaa", IdSource::Remote, "dev");
        write_project(dir.path(), "path-bbbb", IdSource::Path, "");
        // Drop two .age files into one project's envs/ dir.
        let envs_a = project_envs_dir(dir.path(), "remote-aaaa");
        fs::write(envs_a.join("dev.age"), b"x").unwrap();
        fs::write(envs_a.join("staging.age"), b"x").unwrap();
        // And a non-.age file that should be ignored.
        fs::write(envs_a.join("readme.txt"), b"x").unwrap();

        let rows = collect_rows(dir.path()).unwrap();
        let by_id: std::collections::HashMap<_, _> =
            rows.into_iter().map(|r| (r.id.clone(), r)).collect();
        let a = by_id.get("remote-aaaa").unwrap();
        assert_eq!(a.envs, 2);
        assert_eq!(a.active.as_deref(), Some("dev"));
        let b = by_id.get("path-bbbb").unwrap();
        assert_eq!(b.envs, 0);
        assert_eq!(b.active, None);
    }

    #[test]
    fn collect_rows_skips_unparseable_manifests() {
        let dir = TempDir::new().unwrap();
        write_project(dir.path(), "remote-aaaa", IdSource::Remote, "");
        // Write a malformed manifest under another id.
        let bad_path = project_manifest(dir.path(), "bad-cccc");
        fs::create_dir_all(bad_path.parent().unwrap()).unwrap();
        fs::write(&bad_path, b"this is = not [valid toml").unwrap();

        let rows = collect_rows(dir.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "remote-aaaa");
    }
}
