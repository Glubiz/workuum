use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Sandbox {
    dir: TempDir,
    config: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let config = dir.path().join("gitconfig");
        fs::write(&config, "").unwrap();
        fs::create_dir(dir.path().join("projects")).unwrap();
        Sandbox { dir, config }
    }

    fn projects(&self) -> PathBuf {
        canonical(&self.dir.path().join("projects"))
    }

    fn isolate(&self, command: &mut Command) {
        command
            .env("GIT_CONFIG_GLOBAL", &self.config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com");
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let mut command = Command::new("git");
        command.current_dir(cwd).args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ]);
        self.isolate(&mut command);
        let output = command.args(args).output().unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn commit(&self, repo: &Path, file: &str, content: &str) {
        fs::write(repo.join(file), content).unwrap();
        self.git(repo, &["add", "-A"]);
        self.git(repo, &["commit", "-m", file]);
    }

    fn repo(&self) -> PathBuf {
        let repo = self.projects().join("api");
        fs::create_dir(&repo).unwrap();
        self.git(&repo, &["init"]);
        self.commit(&repo, "README", "hello");
        repo
    }

    /// A linked worktree on a new branch, one commit ahead of main, optionally merged.
    fn worktree(&self, repo: &Path, name: &str, merged: bool) -> PathBuf {
        let path = repo.join(".worktrees").join(name);
        self.git(
            repo,
            &["worktree", "add", "-b", name, path.to_str().unwrap()],
        );
        self.commit(&path, name, name);
        if merged {
            self.git(repo, &["merge", "--no-edit", name]);
        }
        path
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workuum"));
        self.isolate(&mut command);
        command
            .env("WORKUUM_STATE_DIR", self.dir.path().join("state"))
            .args(args)
            .arg("--root")
            .arg(self.projects());
        command
    }

    fn workuum(&self, args: &[&str]) -> Output {
        self.command(args)
            .arg("--skip-global-caches")
            .output()
            .unwrap()
    }

    /// A linked worktree detached at main with one commit that no ref contains.
    fn orphaned_worktree(&self, repo: &Path) -> PathBuf {
        let path = repo.join(".worktrees").join("orphan");
        self.git(
            repo,
            &["worktree", "add", "--detach", path.to_str().unwrap()],
        );
        self.commit(&path, "orphan", "orphan");
        path
    }

    fn worktree_list(&self, repo: &Path) -> String {
        self.git(repo, &["worktree", "list", "--porcelain"])
    }
}

fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn succeeded(output: &Output) -> String {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout(output)
}

#[test]
fn dry_run_reports_and_changes_nothing() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    let merged = sandbox.worktree(&repo, "done", true);

    let output = succeeded(&sandbox.workuum(&["clean", "--dry-run", "--min-age", "0s"]));

    assert!(output.contains("would remove"), "{output}");
    assert!(output.contains("would free about"), "{output}");
    assert!(merged.exists());
}

#[test]
fn clean_removes_only_what_is_safe() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    let merged = sandbox.worktree(&repo, "done", true);
    let open = sandbox.worktree(&repo, "open", false);
    let dirty = sandbox.worktree(&repo, "dirty", true);
    fs::write(dirty.join("scratch"), "x").unwrap();
    let locked = sandbox.worktree(&repo, "locked", true);
    sandbox.git(&repo, &["worktree", "lock", locked.to_str().unwrap()]);
    let gone = sandbox.worktree(&repo, "gone", false);
    fs::remove_dir_all(&gone).unwrap();

    succeeded(&sandbox.workuum(&["clean", "--min-age", "0s"]));

    assert!(!merged.exists());
    assert!(open.exists());
    assert!(dirty.exists());
    assert!(locked.exists());
    let listed = sandbox.worktree_list(&repo);
    assert!(!listed.contains("gone"), "{listed}");
    assert!(listed.contains("open"), "{listed}");
}

#[test]
fn stale_worktree_is_removed() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    let open = sandbox.worktree(&repo, "open", false);

    succeeded(&sandbox.workuum(&["clean", "--min-age", "0s", "--stale-after", "0s"]));

    assert!(!open.exists());
}

#[test]
fn disk_pressure_removes_idle_worktrees_and_caches() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    let open = sandbox.worktree(&repo, "open", false);
    fs::write(open.join(".gitignore"), "node_modules/\n").unwrap();
    fs::write(open.join("package.json"), "{}").unwrap();
    sandbox.git(&open, &["add", "-A"]);
    sandbox.git(&open, &["commit", "-m", "node"]);
    fs::create_dir(open.join("node_modules")).unwrap();
    fs::write(open.join("node_modules").join("dep"), "x").unwrap();
    let cache = repo.join("target");
    fs::write(repo.join("Cargo.toml"), "").unwrap();
    fs::write(repo.join(".gitignore"), "target/\n.worktrees/\n").unwrap();
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("out"), "x").unwrap();

    let output = succeeded(&sandbox.workuum(&[
        "clean",
        "--min-age",
        "0s",
        "--stale-after",
        "off",
        "--cache-stale-after",
        "off",
        "--min-free",
        "100%",
    ]));

    assert!(!cache.exists(), "{output}");
    assert!(!open.exists(), "{output}");
}

#[test]
fn build_folders_follow_the_ignore_rules() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    fs::write(repo.join("Cargo.toml"), "").unwrap();
    fs::write(repo.join("build.gradle"), "").unwrap();
    fs::write(repo.join(".gitignore"), "target/\n.worktrees/\n").unwrap();
    fs::create_dir(repo.join("build")).unwrap();
    fs::write(repo.join("build").join("keep.txt"), "tracked").unwrap();
    sandbox.git(&repo, &["add", "-A"]);
    sandbox.git(&repo, &["commit", "-m", "build"]);
    fs::create_dir(repo.join("target")).unwrap();
    fs::write(repo.join("target").join("out"), "x").unwrap();
    let linked = sandbox.worktree(&repo, "feature", false);
    fs::write(linked.join(".gitignore"), "node_modules/\n").unwrap();
    fs::write(linked.join("package.json"), "{}").unwrap();
    fs::create_dir(linked.join("node_modules")).unwrap();
    fs::write(linked.join("node_modules").join("dep"), "x").unwrap();
    let args = ["--min-age", "0s", "--cache-stale-after", "0s"];

    let preview = succeeded(&sandbox.workuum(&[&["clean", "--dry-run"], &args[..]].concat()));
    assert!(preview.contains("would delete"), "{preview}");
    assert!(repo.join("target").exists());
    assert!(linked.join("node_modules").exists());

    succeeded(&sandbox.workuum(&[&["clean"], &args[..]].concat()));
    assert!(!repo.join("target").exists());
    assert!(!linked.join("node_modules").exists());
    assert!(repo.join("build").join("keep.txt").exists());
}

#[test]
fn list_json_has_labels() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    sandbox.worktree(&repo, "done", true);
    sandbox.worktree(&repo, "open", false);
    fs::write(repo.join("Cargo.toml"), "").unwrap();
    fs::write(repo.join(".gitignore"), "target/\n.worktrees/\n").unwrap();
    fs::create_dir(repo.join("target")).unwrap();

    let output = succeeded(&sandbox.workuum(&["list", "--json", "--min-age", "0s"]));

    let rows: serde_json::Value = serde_json::from_str(&output).unwrap();
    let label_of = |kind: &str, name: &str| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|row| row["kind"] == kind && row["path"].as_str().unwrap().ends_with(name))
            .map(|row| row["label"].as_str().unwrap().to_string())
    };
    assert_eq!(label_of("worktree", "done").as_deref(), Some("merged"));
    assert_eq!(label_of("worktree", "open").as_deref(), Some("idle"));
    assert_eq!(label_of("cache", "target").as_deref(), Some("cache-idle"));
    assert!(rows[0]["size_bytes"].is_null());
    succeeded(&sandbox.workuum(&["list"]));
}

#[test]
fn missing_root_is_a_fatal_error() {
    let sandbox = Sandbox::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_workuum"));
    let output = command
        .env("WORKUUM_STATE_DIR", sandbox.dir.path().join("state"))
        .args(["list", "--skip-global-caches", "--root"])
        .arg(sandbox.dir.path().join("missing"))
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("error:"));
}

#[test]
fn invalid_min_free_is_a_usage_error() {
    let sandbox = Sandbox::new();

    let output = sandbox.workuum(&["list", "--min-free", "lots"]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn orphaned_worktree_is_kept_and_listed() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    let orphan = sandbox.orphaned_worktree(&repo);

    succeeded(&sandbox.workuum(&["clean", "--min-age", "0s", "--stale-after", "0s"]));

    assert!(orphan.exists());
    let listed = succeeded(&sandbox.workuum(&["list", "--min-age", "0s", "--stale-after", "0s"]));
    assert!(listed.contains("orphaned"), "{listed}");
}

#[test]
fn orphaned_prunable_worktree_is_not_pruned() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    let orphan = sandbox.orphaned_worktree(&repo);
    fs::remove_dir_all(&orphan).unwrap();

    succeeded(&sandbox.workuum(&["clean", "--min-age", "0s", "--stale-after", "0s"]));

    let listed = sandbox.worktree_list(&repo);
    assert!(listed.contains("orphan"), "{listed}");
}

#[test]
fn changed_exclude_triggers_a_rescan() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    let merged = sandbox.worktree(&repo, "done", true);
    succeeded(&sandbox.workuum(&["list"]));

    succeeded(&sandbox.workuum(&[
        "clean",
        "--min-age",
        "0s",
        "--exclude",
        &repo.to_string_lossy(),
    ]));

    assert!(merged.exists());
}

#[cfg(unix)]
#[test]
fn global_caches_respect_min_age() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    sandbox.worktree(&repo, "open", false);
    let cache = sandbox.dir.path().join("npm-cache");
    fs::create_dir_all(cache.join("_cacache")).unwrap();
    let marker = sandbox.dir.path().join("marker");
    let npm_bin = sandbox.dir.path().join("npm-bin");
    fs::create_dir(&npm_bin).unwrap();
    let script = format!(
        "#!/bin/sh\ncase \"$*\" in\n\"config get cache\") echo '{}' ;;\n\"cache clean --force\") echo cleaned >> '{}' ;;\nesac\n",
        cache.display(),
        marker.display()
    );
    fs::write(npm_bin.join("npm"), script).unwrap();
    fs::set_permissions(npm_bin.join("npm"), fs::Permissions::from_mode(0o755)).unwrap();
    let git_bin = sandbox.dir.path().join("git-bin");
    fs::create_dir(&git_bin).unwrap();
    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .unwrap();
    symlink(real_git, git_bin.join("git")).unwrap();
    let path = std::env::join_paths([&npm_bin, &git_bin]).unwrap();
    let run = |min_age: &str| {
        let output = sandbox
            .command(&[
                "clean",
                "--min-free",
                "100%",
                "--stale-after",
                "off",
                "--cache-stale-after",
                "off",
                "--min-age",
                min_age,
            ])
            .env("PATH", &path)
            .output()
            .unwrap();
        succeeded(&output);
    };

    run("1d");
    assert!(!marker.exists());

    run("0s");
    assert!(marker.exists());
}

#[test]
fn keep_glob_on_the_path_protects_a_merged_worktree() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    let merged = sandbox.worktree(&repo, "done", true);

    succeeded(&sandbox.workuum(&["clean", "--min-age", "0s", "--keep", "*/.worktrees/done"]));

    assert!(merged.exists());
}

#[test]
fn keep_glob_on_the_branch_protects_a_merged_worktree() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    let merged = sandbox.worktree(&repo, "done", true);

    succeeded(&sandbox.workuum(&["clean", "--min-age", "0s", "--keep", "done"]));

    assert!(merged.exists());
}

#[test]
fn the_current_directory_worktree_is_not_removed() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    let merged = sandbox.worktree(&repo, "done", true);

    let output = sandbox
        .command(&["clean", "--min-age", "0s", "--skip-global-caches"])
        .current_dir(&merged)
        .output()
        .unwrap();

    succeeded(&output);
    assert!(merged.exists());
}

#[test]
fn untracked_file_protects_a_merged_worktree() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    let merged = sandbox.worktree(&repo, "done", true);
    fs::write(merged.join("notes.txt"), "mine").unwrap();

    succeeded(&sandbox.workuum(&["clean", "--min-age", "0s"]));

    assert!(merged.join("notes.txt").exists());
}

#[cfg(unix)]
#[test]
fn a_target_symlink_leaving_the_checkout_is_not_followed() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    let outside = sandbox.dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("precious"), "x").unwrap();
    fs::write(repo.join("Cargo.toml"), "").unwrap();
    fs::write(repo.join(".gitignore"), "target\n.worktrees/\n").unwrap();
    std::os::unix::fs::symlink(&outside, repo.join("target")).unwrap();

    succeeded(&sandbox.workuum(&["clean", "--min-age", "0s", "--cache-stale-after", "0s"]));

    assert!(outside.join("precious").exists());
}

#[test]
fn list_and_dry_run_leave_the_admin_dir_untouched() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    sandbox.worktree(&repo, "open", false);
    let admin = repo.join(".git").join("worktrees").join("open");
    let mtimes = || {
        ["index", "HEAD"].map(|name| fs::metadata(admin.join(name)).unwrap().modified().unwrap())
    };
    let before = mtimes();
    std::thread::sleep(std::time::Duration::from_millis(50));

    succeeded(&sandbox.workuum(&["list", "--min-age", "0s"]));
    succeeded(&sandbox.workuum(&["clean", "--dry-run", "--min-age", "0s"]));

    assert_eq!(mtimes(), before);
}
