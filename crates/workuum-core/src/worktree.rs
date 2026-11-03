use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Main,
    Bare,
    Linked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// The repository's common git dir.
    pub repo: PathBuf,
    pub kind: Kind,
    pub head: Option<String>,
    /// Short name, None when detached.
    pub branch: Option<String>,
    /// Some(reason), the reason may be empty.
    pub locked: Option<String>,
    /// The folder no longer exists.
    pub prunable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merge {
    Ancestor { base: String },
    Squash { base: String },
    Gone { upstream: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// Modified or untracked files.
    pub dirty: bool,
    /// Detached HEAD whose commits are on no branch, remote or tag.
    pub orphaned: bool,
    pub merged: Option<Merge>,
    pub last_activity: SystemTime,
    /// Matched --keep, or holds workuum's own cwd.
    pub kept: bool,
}
