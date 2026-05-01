//! `envroll list` (alias `ls`) — list envs in the current project.
//!
//! Acquires a shared lock. With `--all` it reads every
//! registered project's manifest and groups by project; the per-project
//! variant uses [`open_project`] which does the cwd lookup.

use std::path::Path;

use clap::Args as ClapArgs;
use serde::Serialize;

use crate::cli::common::{open_project, LockMode};
use crate::cli::Context;
use crate::errors::EnvrollError;
use crate::manifest::Manifest;
use crate::output::{style_active, style_env_name, styled, use_color, OutputFormat};
use crate::paths::{project_envs_dir, projects_dir, resolve_vault_root};

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Include envs from every registered project, not just the current one.
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Serialize)]
struct ProjectEnvs {
    project_id: String,
    active: Option<String>,
    envs: Vec<String>,
}

pub fn run(args: Args, ctx: &Context) -> Result<(), EnvrollError> {
    if args.all {
        return run_all(ctx);
    }
    let prep = open_project(ctx, LockMode::Shared)?;
    let envs = list_envs_for(prep.vault.root(), &prep.manifest.id);
    let active = if prep.manifest.active.is_empty() {
        None
    } else {
        Some(prep.manifest.active.clone())
    };
    let row = ProjectEnvs {
        project_id: prep.manifest.id.clone(),
        active,
        envs,
    };
    match ctx.format {
        OutputFormat::Human => print_one_human(&row, !ctx.no_color),
        OutputFormat::Json => print_json(std::slice::from_ref(&row))?,
    }
    Ok(())
}

fn run_all(ctx: &Context) -> Result<(), EnvrollError> {
    let vault_root = resolve_vault_root(ctx.vault.as_deref())?;
    let mut rows: Vec<ProjectEnvs> = Vec::new();
    let projects_root = projects_dir(&vault_root);
    if projects_root.is_dir() {
        for entry in std::fs::read_dir(&projects_root).map_err(EnvrollError::Io)? {
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
                Err(_) => continue,
            };
            let envs = list_envs_for(&vault_root, &m.id);
            let active = if m.active.is_empty() {
                None
            } else {
                Some(m.active.clone())
            };
            rows.push(ProjectEnvs {
                project_id: m.id,
                active,
                envs,
            });
        }
    }
    rows.sort_by(|a, b| a.project_id.cmp(&b.project_id));

    match ctx.format {
        OutputFormat::Human => {
            for row in &rows {
                print_one_human(row, !ctx.no_color);
            }
        }
        OutputFormat::Json => print_json(&rows)?,
    }
    Ok(())
}

fn list_envs_for(vault_root: &Path, project_id: &str) -> Vec<String> {
    let envs_dir = project_envs_dir(vault_root, project_id);
    let mut names: Vec<String> = match std::fs::read_dir(&envs_dir) {
        Ok(it) => it
            .filter_map(Result::ok)
            .filter_map(|e| {
                let p = e.path();
                let stem = p.file_stem()?.to_str()?.to_string();
                let ext = p.extension().and_then(|s| s.to_str())?;
                if ext.eq_ignore_ascii_case("age") {
                    Some(stem)
                } else {
                    None
                }
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

fn print_one_human(row: &ProjectEnvs, color: bool) {
    let use_color_now = use_color(!color);
    println!("{}", row.project_id);
    if row.envs.is_empty() {
        println!("  no envs in this project");
        return;
    }
    for name in &row.envs {
        let is_active = row.active.as_deref() == Some(name.as_str());
        let prefix = if is_active { "*" } else { " " };
        let label = if is_active {
            styled(use_color_now, style_active(), name)
        } else {
            styled(use_color_now, style_env_name(), name)
        };
        println!("  {prefix} {label}");
    }
}

fn print_json(rows: &[ProjectEnvs]) -> Result<(), EnvrollError> {
    let s = serde_json::to_string(rows)
        .map_err(|e| EnvrollError::Generic(format!("serializing list JSON: {e}")))?;
    println!("{s}");
    Ok(())
}
