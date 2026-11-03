use std::path::{Path, PathBuf};

use workuum_core::{Kind, Worktree};

pub(crate) fn parse(output: &str, repo: &Path) -> Vec<Worktree> {
    let lines: Vec<&str> = output.lines().collect();
    lines
        .split(|line| line.is_empty())
        .filter(|block| !block.is_empty())
        .enumerate()
        .map(|(index, block)| {
            let mut worktree = Worktree {
                path: PathBuf::new(),
                repo: repo.to_path_buf(),
                kind: if index == 0 { Kind::Main } else { Kind::Linked },
                head: None,
                branch: None,
                locked: None,
                prunable: false,
            };
            for line in block {
                let (key, value) = line.split_once(' ').unwrap_or((line, ""));
                match key {
                    "worktree" => worktree.path = PathBuf::from(value),
                    "HEAD" => worktree.head = Some(value.to_string()),
                    "branch" => {
                        let branch = value.strip_prefix("refs/heads/").unwrap_or(value);
                        worktree.branch = Some(branch.to_string());
                    }
                    "bare" => worktree.kind = Kind::Bare,
                    "locked" => worktree.locked = Some(value.to_string()),
                    "prunable" => worktree.prunable = true,
                    _ => {}
                }
            }
            worktree
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one(block: &str) -> Worktree {
        let mut all = parse(block, Path::new("/r/.git"));
        assert_eq!(all.len(), 1);
        all.remove(0)
    }

    #[test]
    fn main_worktree() {
        let w = parse_one("worktree /r\nHEAD abc123\nbranch refs/heads/main\n");
        assert_eq!(w.kind, Kind::Main);
        assert_eq!(w.path, PathBuf::from("/r"));
        assert_eq!(w.repo, PathBuf::from("/r/.git"));
        assert_eq!(w.head.as_deref(), Some("abc123"));
        assert_eq!(w.branch.as_deref(), Some("main"));
        assert_eq!(w.locked, None);
        assert!(!w.prunable);
    }

    #[test]
    fn linked_with_branch() {
        let out = "worktree /r\nHEAD a\nbranch refs/heads/main\n\nworktree /w\nHEAD b\nbranch refs/heads/feat/x\n";
        let all = parse(out, Path::new("/r/.git"));
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].kind, Kind::Linked);
        assert_eq!(all[1].branch.as_deref(), Some("feat/x"));
    }

    #[test]
    fn detached() {
        let w = parse_one("worktree /w\nHEAD b\ndetached\n");
        assert_eq!(w.branch, None);
        assert_eq!(w.head.as_deref(), Some("b"));
    }

    #[test]
    fn locked_without_reason() {
        let w = parse_one("worktree /w\nHEAD b\ndetached\nlocked\n");
        assert_eq!(w.locked.as_deref(), Some(""));
    }

    #[test]
    fn locked_with_reason() {
        let w = parse_one("worktree /w\nHEAD b\ndetached\nlocked on a usb stick\n");
        assert_eq!(w.locked.as_deref(), Some("on a usb stick"));
    }

    #[test]
    fn prunable() {
        let w = parse_one(
            "worktree /w\nHEAD b\ndetached\nprunable gitdir file points to non-existent location\n",
        );
        assert!(w.prunable);
    }

    #[test]
    fn bare_first_block() {
        let out = "worktree /r.git\nbare\n\nworktree /w\nHEAD b\nbranch refs/heads/x\n";
        let all = parse(out, Path::new("/r.git"));
        assert_eq!(all[0].kind, Kind::Bare);
        assert_eq!(all[0].head, None);
        assert_eq!(all[1].kind, Kind::Linked);
    }

    #[test]
    fn windows_path_and_crlf() {
        let w = parse_one("worktree C:/Users/x/my repo\r\nHEAD b\r\nbranch refs/heads/main\r\n");
        assert_eq!(w.path, PathBuf::from("C:/Users/x/my repo"));
        assert_eq!(w.branch.as_deref(), Some("main"));
    }
}
