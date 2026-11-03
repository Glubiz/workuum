use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use workuum_core::Worktree;

use crate::command::{self, Error, git_in};
use crate::porcelain;

const DEFAULT_BASES: [&str; 5] = [
    "origin/HEAD",
    "origin/main",
    "origin/master",
    "main",
    "master",
];

/// The target of the `gitdir:` line in a `.git` file, relative paths resolved against its folder.
pub(crate) fn read_gitdir(dot_git: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(dot_git).ok()?;
    let target = content.lines().find_map(|l| l.strip_prefix("gitdir:"))?;
    Some(dot_git.parent()?.join(target.trim()))
}

/// The common git dir of the repository a `.git` entry belongs to, None for submodules.
pub fn resolve(dot_git: &Path) -> Option<PathBuf> {
    if dot_git.is_dir() {
        return dunce::canonicalize(dot_git).ok();
    }
    let admin = read_gitdir(dot_git)?;
    let common = fs::read_to_string(admin.join("commondir")).ok()?;
    dunce::canonicalize(admin.join(common.trim())).ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub git_dir: PathBuf,
}

impl Repo {
    pub fn worktrees(&self) -> Result<Vec<Worktree>, Error> {
        let output = command::run(git_in(&self.git_dir).args(["worktree", "list", "--porcelain"]))?;
        let mut worktrees = porcelain::parse(&output, &self.git_dir);
        for worktree in &mut worktrees {
            match dunce::canonicalize(&worktree.path) {
                Ok(path) => worktree.path = path,
                Err(_) => worktree.prunable = true,
            }
        }
        Ok(worktrees)
    }

    /// The refs that exist among `wanted`, or among the defaults when it is empty.
    pub fn bases(&self, wanted: &[String]) -> Result<Vec<String>, Error> {
        let candidates: Vec<&str> = match wanted.is_empty() {
            true => DEFAULT_BASES.to_vec(),
            false => wanted.iter().map(String::as_str).collect(),
        };
        let mut command = git_in(&self.git_dir);
        command.args(["for-each-ref", "--format=%(refname)%09%(symref)"]);
        for candidate in &candidates {
            command.args([
                format!("refs/heads/{candidate}"),
                format!("refs/remotes/{candidate}"),
            ]);
        }
        let output = command::run(&mut command)?;
        let refs: Vec<(&str, &str)> = output
            .lines()
            .map(|line| line.split_once('\t').unwrap_or((line, "")))
            .collect();
        let mut bases: Vec<String> = Vec::new();
        for candidate in candidates {
            let heads = format!("refs/heads/{candidate}");
            let remotes = format!("refs/remotes/{candidate}");
            let Some((_, symref)) = refs
                .iter()
                .find(|(name, _)| *name == heads || *name == remotes)
            else {
                continue;
            };
            let target = symref
                .strip_prefix("refs/heads/")
                .or_else(|| symref.strip_prefix("refs/remotes/"))
                .unwrap_or(candidate);
            if !bases.iter().any(|base| base == target) {
                bases.push(target.to_string());
            }
        }
        Ok(bases)
    }

    pub fn fetch(&self, timeout: Duration) -> Result<(), Error> {
        let mut command = git_in(&self.git_dir);
        command.args(["fetch", "--prune", "--quiet"]);
        if env::var_os("GIT_SSH_COMMAND").is_none() {
            command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
        }
        command::run_timeout(&mut command, timeout)
    }

    pub fn prune(&self, expire: Duration) -> Result<(), Error> {
        let expire = format!("--expire={}.seconds.ago", expire.as_secs());
        command::run(
            git_in(&self.git_dir)
                .args(["worktree", "prune"])
                .arg(expire),
        )
        .map(drop)
    }

    pub fn remove(&self, worktree: &Path) -> Result<(), Error> {
        command::run(
            git_in(&self.git_dir)
                .args(["worktree", "remove"])
                .arg(worktree),
        )
        .map(drop)
    }

    pub fn delete_branch(&self, branch: &str) -> Result<(), Error> {
        command::run(git_in(&self.git_dir).args(["branch", "-d", branch])).map(drop)
    }
}
