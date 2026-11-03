mod common;

use std::fs;
use std::thread::sleep;
use std::time::Duration;

use common::{Sandbox, find, repo_of};
use workuum_core::{Kind, Merge};
use workuum_git::{Repo, dirty, last_activity, merged, reachable, resolve};

fn main_bases(repo: &Repo) -> Vec<String> {
    repo.bases(&[]).unwrap()
}

#[test]
fn fast_forward_merged() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    let wt = sb.add_worktree(&main, "w", "feat");
    sb.commit(&wt, "f", "1");
    sb.git(&main, &["merge", "--ff-only", "feat"]);
    let repo = repo_of(&main);
    let result = merged(&find(&repo, &wt), &main_bases(&repo)).unwrap();
    assert_eq!(
        result,
        Some(Merge::Ancestor {
            base: "main".into()
        })
    );
}

#[test]
fn squash_merged_after_main_moved_on() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    let wt = sb.add_worktree(&main, "w", "feat");
    sb.commit(&wt, "f", "1");
    sb.git(&main, &["merge", "--squash", "feat"]);
    sb.git(&main, &["commit", "-m", "squash"]);
    sb.commit(&main, "other", "2");
    let repo = repo_of(&main);
    let result = merged(&find(&repo, &wt), &main_bases(&repo)).unwrap();
    assert_eq!(
        result,
        Some(Merge::Squash {
            base: "main".into()
        })
    );
}

#[test]
fn open_branch_is_not_merged() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    let wt = sb.add_worktree(&main, "w", "feat");
    sb.commit(&wt, "f", "1");
    sb.commit(&main, "other", "2");
    let repo = repo_of(&main);
    assert_eq!(merged(&find(&repo, &wt), &main_bases(&repo)).unwrap(), None);
}

#[test]
fn new_branch_without_commits_is_ancestor() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    let wt = sb.add_worktree(&main, "w", "feat");
    let repo = repo_of(&main);
    let result = merged(&find(&repo, &wt), &main_bases(&repo)).unwrap();
    assert_eq!(
        result,
        Some(Merge::Ancestor {
            base: "main".into()
        })
    );
}

#[test]
fn dirty_tracked_file() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    let wt = sb.add_worktree(&main, "w", "feat");
    assert!(!dirty(&wt).unwrap());
    fs::write(wt.join("README"), "changed").unwrap();
    assert!(dirty(&wt).unwrap());
}

#[test]
fn untracked_file_is_dirty() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    let wt = sb.add_worktree(&main, "w", "feat");
    fs::write(wt.join("new"), "x").unwrap();
    assert!(dirty(&wt).unwrap());
}

#[test]
fn ignored_only_is_clean_and_removable() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    sb.commit(&main, ".gitignore", "target/\n");
    let wt = sb.add_worktree(&main, "w", "feat");
    fs::create_dir(wt.join("target")).unwrap();
    fs::write(wt.join("target").join("artifact"), "x").unwrap();
    assert!(!dirty(&wt).unwrap());
    repo_of(&main).remove(&wt).unwrap();
    assert!(!wt.exists());
}

#[test]
fn locked_worktree_is_not_removed() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    let wt = sb.add_worktree(&main, "w", "feat");
    sb.git(
        &main,
        &["worktree", "lock", "--reason", "usb", wt.to_str().unwrap()],
    );
    let repo = repo_of(&main);
    assert_eq!(find(&repo, &wt).locked.as_deref(), Some("usb"));
    assert!(repo.remove(&wt).is_err());
    assert!(wt.exists());
}

#[test]
fn deleted_folder_is_prunable() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    let wt = sb.add_worktree(&main, "w", "feat");
    let repo = repo_of(&main);
    let canonical = find(&repo, &wt).path;
    fs::remove_dir_all(&wt).unwrap();
    let worktrees = repo.worktrees().unwrap();
    let gone = worktrees.iter().find(|w| w.path == canonical).unwrap();
    assert!(gone.prunable);
    assert!(last_activity(gone).is_ok());
    repo.prune(Duration::ZERO).unwrap();
    assert!(
        repo.worktrees()
            .unwrap()
            .iter()
            .all(|w| w.path != canonical)
    );
}

#[test]
fn upstream_gone_after_fetch_prune() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    let remote = sb.path("remote.git");
    sb.git(sb.dir.path(), &["init", "--bare", remote.to_str().unwrap()]);
    sb.git(
        &main,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    sb.git(&main, &["push", "-u", "origin", "main"]);
    let wt = sb.add_worktree(&main, "w", "feat");
    sb.commit(&wt, "f", "1");
    sb.git(&wt, &["push", "-u", "origin", "feat"]);
    sb.git(&remote, &["branch", "-D", "feat"]);
    let repo = repo_of(&main);
    repo.fetch(Duration::from_secs(30)).unwrap();
    let result = merged(&find(&repo, &wt), &main_bases(&repo)).unwrap();
    assert_eq!(
        result,
        Some(Merge::Gone {
            upstream: "origin/feat".into()
        })
    );
}

#[test]
fn bare_repo_with_worktree() {
    let sb = Sandbox::new();
    let source = sb.init_repo("src");
    let bare = sb.path("bare.git");
    sb.git(
        sb.dir.path(),
        &[
            "clone",
            "--bare",
            source.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let wt = sb.path("w");
    sb.git(
        &bare,
        &[
            "worktree",
            "add",
            "-b",
            "feat",
            wt.to_str().unwrap(),
            "main",
        ],
    );
    let git_dir = resolve(&wt.join(".git")).unwrap();
    assert_eq!(git_dir, dunce::canonicalize(&bare).unwrap());
    let worktrees = Repo { git_dir }.worktrees().unwrap();
    assert_eq!(worktrees.len(), 2);
    assert_eq!(worktrees[0].kind, Kind::Bare);
    assert_eq!(worktrees[1].kind, Kind::Linked);
    assert!(last_activity(&worktrees[0]).is_ok());
}

#[test]
fn relative_paths_worktree() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    let wt = sb.path("w");
    sb.git(
        &main,
        &[
            "worktree",
            "add",
            "--relative-paths",
            "-b",
            "feat",
            wt.to_str().unwrap(),
        ],
    );
    let dot_git = fs::read_to_string(wt.join(".git")).unwrap();
    assert!(dot_git.trim().starts_with("gitdir: .."), "{dot_git}");
    let repo = repo_of(&main);
    assert_eq!(resolve(&wt.join(".git")).unwrap(), repo.git_dir);
    assert!(last_activity(&find(&repo, &wt)).is_ok());
    fs::remove_dir_all(&wt).unwrap();
    let gone = repo
        .worktrees()
        .unwrap()
        .into_iter()
        .find(|w| w.prunable)
        .unwrap();
    assert!(last_activity(&gone).is_ok());
}

#[test]
fn bases_with_and_without_origin_head() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    let remote = sb.path("remote.git");
    sb.git(sb.dir.path(), &["init", "--bare", remote.to_str().unwrap()]);
    sb.git(
        &main,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    sb.git(&main, &["push", "origin", "main", "main:trunk"]);
    let repo = repo_of(&main);
    assert_eq!(main_bases(&repo), ["origin/main", "main"]);
    sb.git(&main, &["remote", "set-head", "origin", "trunk"]);
    assert_eq!(main_bases(&repo), ["origin/trunk", "origin/main", "main"]);
    let wanted = ["main".to_string(), "nope".to_string()];
    assert_eq!(repo.bases(&wanted).unwrap(), ["main"]);
}

#[test]
fn submodule_style_git_file_is_not_a_repo() {
    let sb = Sandbox::new();
    let modules = sb.path("super").join(".git").join("modules").join("x");
    fs::create_dir_all(&modules).unwrap();
    let sub = sb.path("sub");
    fs::create_dir(&sub).unwrap();
    let dot_git = sub.join(".git");
    fs::write(&dot_git, format!("gitdir: {}\n", modules.display())).unwrap();
    assert_eq!(resolve(&dot_git), None);
    fs::write(&dot_git, "garbage").unwrap();
    assert_eq!(resolve(&dot_git), None);
}

#[test]
fn inspection_leaves_admin_index_untouched() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    let wt = sb.add_worktree(&main, "w", "feat");
    let repo = repo_of(&main);
    sleep(Duration::from_millis(50));
    fs::write(wt.join("README"), "hello").unwrap();
    let index = repo.git_dir.join("worktrees").join("w").join("index");
    let before = fs::metadata(&index).unwrap().modified().unwrap();
    sleep(Duration::from_millis(50));
    let worktree = find(&repo, &wt);
    dirty(&wt).unwrap();
    merged(&worktree, &main_bases(&repo)).unwrap();
    last_activity(&worktree).unwrap();
    assert_eq!(fs::metadata(&index).unwrap().modified().unwrap(), before);
}

#[test]
fn detached_worktree_reachability() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    let repo = repo_of(&main);
    let on_main = sb.path("on-main");
    sb.git(
        &main,
        &["worktree", "add", "--detach", on_main.to_str().unwrap()],
    );
    let ahead = sb.path("ahead");
    sb.git(
        &main,
        &["worktree", "add", "--detach", ahead.to_str().unwrap()],
    );
    sb.commit(&ahead, "extra", "x");
    let branched = sb.add_worktree(&main, "branched", "feat");
    sb.commit(&branched, "more", "y");

    assert!(reachable(&find(&repo, &on_main)).unwrap());
    assert!(!reachable(&find(&repo, &ahead)).unwrap());
    assert!(reachable(&find(&repo, &branched)).unwrap());

    sb.git(
        &main,
        &["tag", "keep", sb.git(&ahead, &["rev-parse", "HEAD"]).trim()],
    );
    assert!(reachable(&find(&repo, &ahead)).unwrap());
}

fn deletable_names(checkout: &std::path::Path, names: &[&str]) -> Vec<String> {
    let folders: Vec<_> = names.iter().map(|name| checkout.join(name)).collect();
    workuum_git::deletable(checkout, &folders)
        .unwrap()
        .iter()
        .map(|path| {
            path.strip_prefix(checkout)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

fn ignore(sb: &Sandbox, repo: &std::path::Path, pattern: &str) {
    sb.commit(repo, ".gitignore", pattern);
}

#[test]
fn ignored_folder_via_directory_pattern_is_deletable() {
    let sb = Sandbox::new();
    let repo = sb.init_repo("r");
    ignore(&sb, &repo, "target/\n");
    fs::create_dir_all(repo.join("target")).unwrap();
    fs::write(repo.join("target/out"), "x").unwrap();
    assert_eq!(deletable_names(&repo, &["target"]), ["target"]);
}

#[test]
fn ignored_folder_via_anchored_pattern_is_deletable() {
    let sb = Sandbox::new();
    let repo = sb.init_repo("r");
    ignore(&sb, &repo, "/target\n");
    fs::create_dir_all(repo.join("target")).unwrap();
    fs::write(repo.join("target/out"), "x").unwrap();
    assert_eq!(deletable_names(&repo, &["target"]), ["target"]);
}

#[test]
fn ignored_folder_with_tracked_file_is_not_deletable() {
    let sb = Sandbox::new();
    let repo = sb.init_repo("r");
    ignore(&sb, &repo, "target/\n");
    fs::create_dir_all(repo.join("target")).unwrap();
    fs::write(repo.join("target/kept"), "x").unwrap();
    sb.git(&repo, &["add", "-f", "target/kept"]);
    sb.git(&repo, &["commit", "-m", "force"]);
    assert!(deletable_names(&repo, &["target"]).is_empty());
}

#[test]
fn folder_that_is_not_ignored_is_not_deletable() {
    let sb = Sandbox::new();
    let repo = sb.init_repo("r");
    fs::create_dir_all(repo.join("target")).unwrap();
    fs::write(repo.join("target/out"), "x").unwrap();
    assert!(deletable_names(&repo, &["target"]).is_empty());
}

#[test]
fn nested_folder_matches_root_pattern() {
    let sb = Sandbox::new();
    let repo = sb.init_repo("r");
    ignore(&sb, &repo, "node_modules/\n");
    fs::create_dir_all(repo.join("packages/a/node_modules")).unwrap();
    fs::write(repo.join("packages/a/node_modules/m"), "x").unwrap();
    fs::create_dir_all(repo.join("packages/b/target")).unwrap();
    assert_eq!(
        deletable_names(&repo, &["packages/a/node_modules", "packages/b/target"]),
        ["packages/a/node_modules"]
    );
}

#[test]
fn deletable_works_in_linked_worktree() {
    let sb = Sandbox::new();
    let main = sb.init_repo("r");
    ignore(&sb, &main, "target/\n");
    let wt = sb.add_worktree(&main, "w", "feat");
    fs::create_dir_all(wt.join("target")).unwrap();
    fs::write(wt.join("target/out"), "x").unwrap();
    assert_eq!(deletable_names(&wt, &["target"]), ["target"]);
}
