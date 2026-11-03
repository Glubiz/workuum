use glob::{MatchOptions, Pattern};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use workuum_core::match_rule;

const HEAVY_FOLDERS: [&str; 9] = [
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    "vendor",
    ".gradle",
    "Pods",
    "DerivedData",
];

const MATCH_OPTIONS: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cache {
    pub path: PathBuf,
    pub ecosystem: String,
}

#[derive(Debug, Default)]
pub struct Found {
    pub git: Vec<PathBuf>,
    pub caches: Vec<Cache>,
}

fn sibling_names(path: &Path) -> Vec<String> {
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

pub fn find_entries(roots: &[PathBuf], exclude: &[Pattern]) -> Found {
    let mut git = BTreeSet::new();
    let mut caches = BTreeSet::new();
    for root in roots {
        let mut walker = WalkDir::new(root).follow_links(false).into_iter();
        while let Some(entry) = walker.next() {
            let Ok(entry) = entry else { continue };
            if entry.depth() == 0 {
                continue;
            }
            let is_dir = entry.file_type().is_dir();
            let name = entry.file_name().to_string_lossy();
            if is_dir && is_excluded(entry.path(), exclude) {
                walker.skip_current_dir();
                continue;
            }
            if name == ".git" {
                if is_dir {
                    walker.skip_current_dir();
                }
                git.insert(entry.into_path());
                continue;
            }
            if !is_dir {
                continue;
            }
            if let Some(rule) = match_rule(&name, || sibling_names(entry.path())) {
                let ecosystem = rule.ecosystem.to_string();
                walker.skip_current_dir();
                caches.insert(Cache {
                    path: entry.into_path(),
                    ecosystem,
                });
                continue;
            }
            if HEAVY_FOLDERS.contains(&&*name) {
                walker.skip_current_dir();
            }
        }
    }
    Found {
        git: git.into_iter().collect(),
        caches: caches.into_iter().collect(),
    }
}

fn is_excluded(path: &Path, exclude: &[Pattern]) -> bool {
    let normalised = path.to_string_lossy().replace('\\', "/");
    exclude
        .iter()
        .any(|pattern| pattern.matches_with(&normalised, MATCH_OPTIONS))
}

pub fn default_excludes(home: &Path) -> Vec<Pattern> {
    let mut relative = vec![
        ".cargo",
        ".rustup",
        ".npm",
        "go/pkg/mod",
        ".bun",
        ".local/share/pnpm",
    ];
    #[cfg(target_os = "macos")]
    relative.extend(["Library", ".Trash"]);
    #[cfg(target_os = "linux")]
    relative.extend([".cache", ".local/share/Trash"]);
    #[cfg(windows)]
    relative.push("AppData");

    let home = Pattern::escape(&home.to_string_lossy().replace('\\', "/"));
    relative
        .into_iter()
        .filter_map(|path| Pattern::new(&format!("{home}/{path}")).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn mkdir(base: &Path, relative: &str) -> PathBuf {
        let path = base.join(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn finds_repo_git_dir() {
        let tmp = TempDir::new().unwrap();
        let git = mkdir(tmp.path(), "a/repo/.git");
        assert_eq!(find_entries(&[tmp.path().into()], &[]).git, vec![git]);
    }

    #[test]
    fn finds_nested_worktree_git_file() {
        let tmp = TempDir::new().unwrap();
        mkdir(tmp.path(), "repo/.git");
        let worktree = mkdir(tmp.path(), "repo/.claude/worktrees/x");
        fs::write(worktree.join(".git"), "gitdir: ../../../.git/worktrees/x").unwrap();
        let found = find_entries(&[tmp.path().into()], &[]).git;
        assert_eq!(found.len(), 2);
        assert!(found.contains(&worktree.join(".git")));
    }

    #[test]
    fn does_not_descend_into_git_dir() {
        let tmp = TempDir::new().unwrap();
        let git = mkdir(tmp.path(), "repo/.git");
        mkdir(&git, "modules/sub/.git");
        assert_eq!(find_entries(&[tmp.path().into()], &[]).git, vec![git]);
    }

    #[test]
    fn skips_heavy_and_excluded_folders() {
        let tmp = TempDir::new().unwrap();
        mkdir(tmp.path(), "Pods/pkg/.git");
        mkdir(tmp.path(), "skipme/repo/.git");
        let kept = mkdir(tmp.path(), "keep/.git");
        let base = tmp.path().to_string_lossy().replace('\\', "/");
        let exclude = [Pattern::new(&format!("{}/skipme", Pattern::escape(&base))).unwrap()];
        assert_eq!(find_entries(&[tmp.path().into()], &exclude).git, vec![kept]);
    }

    #[test]
    fn skips_with_wildcard_pattern() {
        let tmp = TempDir::new().unwrap();
        mkdir(tmp.path(), "a/cache/repo/.git");
        let kept = mkdir(tmp.path(), "a/repo/.git");
        let exclude = [Pattern::new("**/cache").unwrap()];
        assert_eq!(find_entries(&[tmp.path().into()], &exclude).git, vec![kept]);
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinked_folders() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        mkdir(outside.path(), "repo/.git");
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();
        assert!(find_entries(&[tmp.path().into()], &[]).git.is_empty());
    }

    #[test]
    fn deduplicates_overlapping_roots() {
        let tmp = TempDir::new().unwrap();
        let git = mkdir(tmp.path(), "a/repo/.git");
        let roots = [tmp.path().into(), tmp.path().join("a")];
        assert_eq!(find_entries(&roots, &[]).git, vec![git]);
    }

    fn cache_paths(found: &Found) -> Vec<PathBuf> {
        found
            .caches
            .iter()
            .map(|cache| cache.path.clone())
            .collect()
    }

    #[test]
    fn records_cache_next_to_marker_without_descending() {
        let tmp = TempDir::new().unwrap();
        fs::write(mkdir(tmp.path(), "app").join("Cargo.toml"), "").unwrap();
        let target = mkdir(tmp.path(), "app/target");
        mkdir(&target, "debug/checkout/.git");
        let found = find_entries(&[tmp.path().into()], &[]);
        assert_eq!(
            found.caches,
            vec![Cache {
                path: target,
                ecosystem: "Rust".into()
            }]
        );
        assert!(found.git.is_empty());
    }

    #[test]
    fn folder_without_marker_is_not_a_cache() {
        let tmp = TempDir::new().unwrap();
        mkdir(tmp.path(), "app/target");
        mkdir(tmp.path(), "app/node_modules");
        assert!(find_entries(&[tmp.path().into()], &[]).caches.is_empty());
    }

    #[test]
    fn monorepo_records_every_package_cache() {
        let tmp = TempDir::new().unwrap();
        let mut expected = Vec::new();
        for package in ["a", "b"] {
            let dir = mkdir(tmp.path(), &format!("mono/packages/{package}"));
            fs::write(dir.join("package.json"), "{}").unwrap();
            expected.push(mkdir(&dir, "node_modules"));
        }
        let found = find_entries(&[tmp.path().into()], &[]);
        assert_eq!(cache_paths(&found), expected);
    }

    #[test]
    fn excluded_folder_caches_are_not_recorded() {
        let tmp = TempDir::new().unwrap();
        fs::write(mkdir(tmp.path(), "skipme").join("Cargo.toml"), "").unwrap();
        mkdir(tmp.path(), "skipme/target");
        let base = tmp.path().to_string_lossy().replace('\\', "/");
        let exclude = [Pattern::new(&format!("{}/skipme", Pattern::escape(&base))).unwrap()];
        assert!(
            find_entries(&[tmp.path().into()], &exclude)
                .caches
                .is_empty()
        );
    }

    #[test]
    fn deduplicates_overlapping_root_caches() {
        let tmp = TempDir::new().unwrap();
        fs::write(mkdir(tmp.path(), "a").join("Cargo.toml"), "").unwrap();
        let target = mkdir(tmp.path(), "a/target");
        let roots = [tmp.path().into(), tmp.path().join("a")];
        assert_eq!(cache_paths(&find_entries(&roots, &[])), vec![target]);
    }

    #[test]
    fn default_excludes_contain_os_entry() {
        let home = Path::new("/home/x");
        let excludes = default_excludes(home);
        let matches = |path: &str| excludes.iter().any(|p| p.matches_with(path, MATCH_OPTIONS));
        assert!(matches("/home/x/.cargo"));
        assert!(matches("/home/x/go/pkg/mod"));
        assert!(matches("/home/x/.bun"));
        assert!(matches("/home/x/.local/share/pnpm"));
        #[cfg(target_os = "macos")]
        assert!(matches("/home/x/Library"));
        #[cfg(target_os = "linux")]
        assert!(matches("/home/x/.cache"));
        #[cfg(windows)]
        assert!(matches("/home/x/AppData"));
    }
}
