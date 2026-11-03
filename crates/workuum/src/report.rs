use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use workuum_core::{format_bytes, format_duration};

use crate::pass::{Action, Target};

pub struct Row {
    pub kind: &'static str,
    pub path: PathBuf,
    pub label: &'static str,
    pub reason: String,
    pub branch: Option<String>,
    pub idle: Duration,
    pub size: Option<u64>,
}

pub fn warn(message: &str) {
    eprintln!("warning: {message}");
}

fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => Path::new("~").join(rest).display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

pub fn action(home: &Path, action: &Action, result: &Result<(), String>, dry_run: bool) {
    let (done, planned) = match action.target {
        Target::Worktree { .. } => ("removed", "would remove"),
        Target::Cache => ("deleted", "would delete"),
        Target::Global(_) => ("cleared", "would clear"),
        Target::Prune(_) => ("pruned", "would prune"),
    };
    let verb = match (result, dry_run) {
        (Err(_), _) => "failed",
        (Ok(()), true) => planned,
        (Ok(()), false) => done,
    };
    let width = if dry_run { 13 } else { 10 };
    let mut text = match result {
        Ok(()) => action.detail.clone(),
        Err(error) => error.replace('\n', " "),
    };
    if result.is_ok() && action.bytes > 0 {
        text = format!("{text} · {}", format_bytes(action.bytes));
    }
    println!("{verb:<width$}{}   {text}", tilde(&action.path, home));
}

pub fn summary(actions: usize, freed: u64, dry_run: bool) {
    if actions == 0 {
        println!("nothing to clean");
        return;
    }
    let verb = if dry_run { "would free" } else { "freed" };
    println!("{verb} about {}", format_bytes(freed));
}

pub fn list(rows: &[Row], json: bool, home: &Path) {
    if json {
        let rows: Vec<Value> = rows
            .iter()
            .map(|row| {
                json!({
                    "kind": row.kind,
                    "path": row.path.to_string_lossy(),
                    "label": row.label,
                    "reason": row.reason,
                    "branch": row.branch,
                    "idle_secs": row.idle.as_secs(),
                    "size_bytes": row.size,
                })
            })
            .collect();
        println!("{}", Value::Array(rows));
        return;
    }
    for row in rows {
        let mut text = format!("{} · idle {}", row.reason, format_duration(row.idle));
        if let Some(size) = row.size {
            text = format!("{text} · {}", format_bytes(size));
        }
        println!("{:<12} {}   {text}", row.label, tilde(&row.path, home));
    }
}
