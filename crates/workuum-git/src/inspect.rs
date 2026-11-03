use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use workuum_core::{Kind, Merge, Worktree};

use crate::command::{self, Error, PLAIN, git, git_in};
use crate::repo::read_gitdir;

const SQUASH_LOG_LIMIT: &str = "1000";

pub fn dirty(worktree: &Path) -> Result<bool, Error> {
    let output = command::run(git(["-C"]).arg(worktree).args([
        "status",
        "--porcelain",
        "--untracked-files=normal",
    ]))?;
    Ok(!output.trim().is_empty())
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// The `<git_dir>/worktrees/<id>` folder of a linked worktree.
fn admin_dir(worktree: &Worktree) -> Result<PathBuf, Error> {
    let dot_git = worktree.path.join(".git");
    if let Some(admin) = read_gitdir(&dot_git) {
        return Ok(admin);
    }
    let worktrees = worktree.repo.join("worktrees");
    let entries = fs::read_dir(&worktrees).map_err(Error::io(&worktrees))?;
    for entry in entries.flatten() {
        let admin = entry.path();
        let Ok(content) = fs::read_to_string(admin.join("gitdir")) else {
            continue;
        };
        if normalize(&admin.join(content.trim())) == normalize(&dot_git) {
            return Ok(admin);
        }
    }
    Err(Error::Io {
        path: worktree.path.clone(),
        source: io::Error::new(io::ErrorKind::NotFound, "no admin dir for worktree"),
    })
}

fn newest(path: &Path) -> io::Result<Option<SystemTime>> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return meta.modified().map(Some);
    }
    let mut best = None;
    for entry in fs::read_dir(path)? {
        best = best.max(newest(&entry?.path())?);
    }
    Ok(best)
}

/// Subset of `folders` (inside `checkout`) that git ignores and that hold no tracked files.
pub fn deletable(checkout: &Path, folders: &[PathBuf]) -> Result<Vec<PathBuf>, Error> {
    let mut relative = Vec::new();
    for folder in folders {
        let Ok(inside) = folder.strip_prefix(checkout) else {
            continue;
        };
        let slashed = inside.to_string_lossy().replace('\\', "/");
        relative.push((folder, slashed));
    }
    if relative.is_empty() {
        return Ok(Vec::new());
    }
    // The trailing slash lets directory-only patterns such as `target/` match.
    let input: String = relative
        .iter()
        .map(|(_, path)| format!("{path}/\0"))
        .collect();
    let mut check = git(["-C"]);
    check.arg(checkout).args(["check-ignore", "--stdin", "-z"]);
    let output = command::output_with_stdin(&mut check, input.as_bytes())?;
    // Exit 1 means nothing matched.
    if !matches!(output.status.code(), Some(0 | 1)) {
        return Err(command::failed(&check, &output.stderr));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let ignored: Vec<&str> = stdout.split('\0').collect();
    let ignored: Vec<&(&PathBuf, String)> = relative
        .iter()
        .filter(|(_, path)| ignored.contains(&format!("{path}/").as_str()))
        .collect();
    if ignored.is_empty() {
        return Ok(Vec::new());
    }
    let tracked = command::run(
        git(["-C"])
            .arg(checkout)
            .args(["ls-files", "-z", "--"])
            .args(ignored.iter().map(|(_, path)| format!(":(literal){path}"))),
    )?;
    let tracked: Vec<&str> = tracked
        .split('\0')
        .filter(|path| !path.is_empty())
        .collect();
    Ok(ignored
        .into_iter()
        .filter(|(_, path)| {
            !tracked
                .iter()
                .any(|file| file.starts_with(&format!("{path}/")))
        })
        .map(|(folder, _)| (*folder).clone())
        .collect())
}

pub fn last_activity(worktree: &Worktree) -> Result<SystemTime, Error> {
    let (root, newest) = match worktree.kind {
        Kind::Linked => {
            let admin = admin_dir(worktree)?;
            let newest = newest(&admin).map_err(Error::io(&admin))?;
            (admin, newest)
        }
        _ => {
            let newest = ["index", "HEAD"]
                .iter()
                .filter_map(|name| newest(&worktree.repo.join(name)).ok().flatten())
                .max();
            (worktree.repo.clone(), newest)
        }
    };
    newest.ok_or_else(|| Error::Io {
        path: root,
        source: io::Error::new(io::ErrorKind::NotFound, "no files to take a time from"),
    })
}

fn is_ancestor(git_dir: &Path, head: &str, base: &str) -> Result<bool, Error> {
    let mut command = git_in(git_dir);
    command.args(["merge-base", "--is-ancestor", head, base]);
    let output = command::output(&mut command)?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(command::failed(&command, &output.stderr)),
    }
}

fn gone_upstream(git_dir: &Path, branch: &str) -> Result<Option<String>, Error> {
    let output = command::run(git_in(git_dir).args([
        "for-each-ref",
        "--format=%(upstream:short)%09%(upstream:track)",
        &format!("refs/heads/{branch}"),
    ]))?;
    let Some((upstream, track)) = output.trim_end().split_once('\t') else {
        return Ok(None);
    };
    Ok((track == "[gone]").then(|| upstream.to_string()))
}

fn patch_ids(producer: &mut Command) -> Result<Vec<String>, Error> {
    let output = command::pipe(producer, &mut git(["patch-id", "--stable"]))?;
    Ok(output
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect())
}

fn is_squashed(git_dir: &Path, head: &str, base: &str) -> Result<bool, Error> {
    let mut merge_base = git_in(git_dir);
    merge_base.args(["merge-base", base, head]);
    let output = command::output(&mut merge_base)?;
    if !output.status.success() {
        return Ok(false);
    }
    let merge_base = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let branch_ids = patch_ids(
        git_in(git_dir)
            .arg("diff")
            .args(PLAIN)
            .args([&merge_base, head]),
    )?;
    let Some(branch_id) = branch_ids.first() else {
        return Ok(false);
    };
    let range = format!("{merge_base}..{base}");
    let base_ids = patch_ids(
        git_in(git_dir)
            .args(["log", "-p", "--pretty=medium", "-n", SQUASH_LOG_LIMIT])
            .args(PLAIN)
            .arg(range),
    )?;
    Ok(base_ids.contains(branch_id))
}

/// False for a detached worktree whose commits are on no branch, remote or tag.
pub fn reachable(worktree: &Worktree) -> Result<bool, Error> {
    let (Some(head), None) = (&worktree.head, &worktree.branch) else {
        return Ok(true);
    };
    let output = command::run(git_in(&worktree.repo).args([
        "for-each-ref",
        "--count=1",
        "--format=%(refname)",
        "--contains",
        head,
        "refs/heads",
        "refs/remotes",
        "refs/tags",
    ]))?;
    Ok(!output.trim().is_empty())
}

pub fn merged(worktree: &Worktree, bases: &[String]) -> Result<Option<Merge>, Error> {
    let Some(head) = &worktree.head else {
        return Ok(None);
    };
    for base in bases {
        if is_ancestor(&worktree.repo, head, base)? {
            return Ok(Some(Merge::Ancestor { base: base.clone() }));
        }
    }
    if let Some(branch) = &worktree.branch
        && let Some(upstream) = gone_upstream(&worktree.repo, branch)?
    {
        return Ok(Some(Merge::Gone { upstream }));
    }
    for base in bases {
        if is_squashed(&worktree.repo, head, base)? {
            return Ok(Some(Merge::Squash { base: base.clone() }));
        }
    }
    Ok(None)
}
