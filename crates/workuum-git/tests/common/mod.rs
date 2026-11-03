use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;
use workuum_core::Worktree;
use workuum_git::{Repo, resolve};

pub struct Sandbox {
    pub dir: TempDir,
    config: PathBuf,
}

impl Sandbox {
    pub fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let config = dir.path().join("gitconfig");
        fs::write(&config, "").unwrap();
        Sandbox { dir, config }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    pub fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", &self.config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .args([
                "-c",
                "init.defaultBranch=main",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A repository on main with one commit (README).
    pub fn init_repo(&self, name: &str) -> PathBuf {
        let repo = self.path(name);
        fs::create_dir(&repo).unwrap();
        self.git(&repo, &["init"]);
        self.commit(&repo, "README", "hello");
        repo
    }

    pub fn commit(&self, repo: &Path, file: &str, content: &str) {
        fs::write(repo.join(file), content).unwrap();
        self.git(repo, &["add", "."]);
        self.git(repo, &["commit", "-m", file]);
    }

    pub fn add_worktree(&self, repo: &Path, name: &str, branch: &str) -> PathBuf {
        let path = self.path(name);
        self.git(
            repo,
            &["worktree", "add", "-b", branch, path.to_str().unwrap()],
        );
        path
    }
}

pub fn repo_of(repo: &Path) -> Repo {
    Repo {
        git_dir: resolve(&repo.join(".git")).unwrap(),
    }
}

pub fn find(repo: &Repo, path: &Path) -> Worktree {
    let path = dunce::canonicalize(path).unwrap();
    repo.worktrees()
        .unwrap()
        .into_iter()
        .find(|w| w.path == path)
        .unwrap()
}
