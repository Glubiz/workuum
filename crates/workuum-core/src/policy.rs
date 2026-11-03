use std::fmt;
use std::time::{Duration, SystemTime};

use crate::worktree::{Facts, Kind, Merge, Worktree};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub min_age: Duration,
    pub stale_after: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Never,
    Locked(String),
    Dirty,
    Orphaned,
    Kept,
    Active,
    Prune,
    Merged(Merge),
    Stale,
    Idle,
}

/// First matching rule wins; a last_activity in the future counts as idle zero.
pub fn classify(worktree: &Worktree, facts: &Facts, policy: &Policy, now: SystemTime) -> Verdict {
    if worktree.kind != Kind::Linked {
        return Verdict::Never;
    }
    if let Some(reason) = &worktree.locked {
        return Verdict::Locked(reason.clone());
    }
    if facts.dirty {
        return Verdict::Dirty;
    }
    if facts.orphaned {
        return Verdict::Orphaned;
    }
    if facts.kept {
        return Verdict::Kept;
    }
    let idle = now.duration_since(facts.last_activity).unwrap_or_default();
    if idle < policy.min_age {
        return Verdict::Active;
    }
    if worktree.prunable {
        return Verdict::Prune;
    }
    if let Some(merge) = &facts.merged {
        return Verdict::Merged(merge.clone());
    }
    match policy.stale_after {
        Some(stale_after) if idle >= stale_after => Verdict::Stale,
        _ => Verdict::Idle,
    }
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Never => "never",
            Verdict::Locked(_) => "locked",
            Verdict::Dirty => "dirty",
            Verdict::Orphaned => "orphaned",
            Verdict::Kept => "kept",
            Verdict::Active => "active",
            Verdict::Prune => "prune",
            Verdict::Merged(Merge::Ancestor { .. }) => "merged",
            Verdict::Merged(Merge::Squash { .. }) => "squashed",
            Verdict::Merged(Merge::Gone { .. }) => "gone",
            Verdict::Stale => "stale",
            Verdict::Idle => "idle",
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Verdict::Never => f.write_str("main or bare checkout"),
            Verdict::Locked(reason) if reason.is_empty() => f.write_str("locked"),
            Verdict::Locked(reason) => write!(f, "locked: {reason}"),
            Verdict::Dirty => f.write_str("modified or untracked files"),
            Verdict::Orphaned => f.write_str("detached HEAD with commits on no branch"),
            Verdict::Kept => f.write_str("matches --keep"),
            Verdict::Active => f.write_str("used recently"),
            Verdict::Prune => f.write_str("folder no longer exists"),
            Verdict::Merged(Merge::Ancestor { base }) => write!(f, "merged into {base}"),
            Verdict::Merged(Merge::Squash { base }) => write!(f, "squash-merged into {base}"),
            Verdict::Merged(Merge::Gone { upstream }) => write!(f, "upstream {upstream} deleted"),
            Verdict::Stale => f.write_str("unmerged, idle past --stale-after"),
            Verdict::Idle => f.write_str("unmerged, idle"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const HOUR: Duration = Duration::from_secs(3600);

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000)
    }

    fn worktree() -> Worktree {
        Worktree {
            path: PathBuf::from("/w"),
            repo: PathBuf::from("/r/.git"),
            kind: Kind::Linked,
            head: None,
            branch: Some("feat".into()),
            locked: None,
            prunable: false,
        }
    }

    fn facts(idle: Duration) -> Facts {
        Facts {
            dirty: false,
            orphaned: false,
            merged: None,
            last_activity: now() - idle,
            kept: false,
        }
    }

    fn policy() -> Policy {
        Policy {
            min_age: HOUR,
            stale_after: Some(24 * HOUR),
        }
    }

    fn ancestor() -> Option<Merge> {
        Some(Merge::Ancestor {
            base: "main".into(),
        })
    }

    #[test]
    fn classify_table() {
        let old = 100 * HOUR;
        let mid = 2 * HOUR;
        let cases: Vec<(&str, Worktree, Facts, Policy, Verdict)> = vec![
            (
                "main",
                Worktree {
                    kind: Kind::Main,
                    ..worktree()
                },
                facts(old),
                policy(),
                Verdict::Never,
            ),
            (
                "bare",
                Worktree {
                    kind: Kind::Bare,
                    ..worktree()
                },
                facts(old),
                policy(),
                Verdict::Never,
            ),
            (
                "main prunable merged",
                Worktree {
                    kind: Kind::Main,
                    prunable: true,
                    ..worktree()
                },
                Facts {
                    merged: ancestor(),
                    ..facts(old)
                },
                policy(),
                Verdict::Never,
            ),
            (
                "bare prunable",
                Worktree {
                    kind: Kind::Bare,
                    prunable: true,
                    ..worktree()
                },
                facts(old),
                policy(),
                Verdict::Never,
            ),
            (
                "locked with reason",
                Worktree {
                    locked: Some("wip".into()),
                    ..worktree()
                },
                facts(old),
                policy(),
                Verdict::Locked("wip".into()),
            ),
            (
                "locked beats merged",
                Worktree {
                    locked: Some(String::new()),
                    ..worktree()
                },
                Facts {
                    merged: ancestor(),
                    ..facts(old)
                },
                policy(),
                Verdict::Locked(String::new()),
            ),
            (
                "dirty",
                worktree(),
                Facts {
                    dirty: true,
                    ..facts(old)
                },
                policy(),
                Verdict::Dirty,
            ),
            (
                "dirty beats stale",
                worktree(),
                Facts {
                    dirty: true,
                    ..facts(old)
                },
                policy(),
                Verdict::Dirty,
            ),
            (
                "orphaned beats stale",
                worktree(),
                Facts {
                    orphaned: true,
                    ..facts(old)
                },
                policy(),
                Verdict::Orphaned,
            ),
            (
                "orphaned beats prunable",
                Worktree {
                    prunable: true,
                    ..worktree()
                },
                Facts {
                    orphaned: true,
                    ..facts(old)
                },
                policy(),
                Verdict::Orphaned,
            ),
            (
                "dirty beats orphaned",
                worktree(),
                Facts {
                    dirty: true,
                    orphaned: true,
                    ..facts(old)
                },
                policy(),
                Verdict::Dirty,
            ),
            (
                "kept",
                worktree(),
                Facts {
                    kept: true,
                    ..facts(old)
                },
                policy(),
                Verdict::Kept,
            ),
            (
                "kept beats merged",
                worktree(),
                Facts {
                    kept: true,
                    merged: ancestor(),
                    ..facts(old)
                },
                policy(),
                Verdict::Kept,
            ),
            (
                "active",
                worktree(),
                facts(Duration::ZERO),
                policy(),
                Verdict::Active,
            ),
            (
                "recent beats merged",
                worktree(),
                Facts {
                    merged: ancestor(),
                    ..facts(Duration::from_secs(60))
                },
                policy(),
                Verdict::Active,
            ),
            (
                "recent beats prunable",
                Worktree {
                    prunable: true,
                    ..worktree()
                },
                facts(Duration::from_secs(60)),
                policy(),
                Verdict::Active,
            ),
            (
                "future activity",
                worktree(),
                Facts {
                    last_activity: now() + HOUR,
                    ..facts(old)
                },
                policy(),
                Verdict::Active,
            ),
            (
                "old prunable",
                Worktree {
                    prunable: true,
                    ..worktree()
                },
                facts(old),
                policy(),
                Verdict::Prune,
            ),
            (
                "prunable beats merged",
                Worktree {
                    prunable: true,
                    ..worktree()
                },
                Facts {
                    merged: ancestor(),
                    ..facts(old)
                },
                policy(),
                Verdict::Prune,
            ),
            (
                "merged",
                worktree(),
                Facts {
                    merged: ancestor(),
                    ..facts(mid)
                },
                policy(),
                Verdict::Merged(Merge::Ancestor {
                    base: "main".into(),
                }),
            ),
            (
                "gone",
                worktree(),
                Facts {
                    merged: Some(Merge::Gone {
                        upstream: "origin/x".into(),
                    }),
                    ..facts(mid)
                },
                policy(),
                Verdict::Merged(Merge::Gone {
                    upstream: "origin/x".into(),
                }),
            ),
            ("stale", worktree(), facts(old), policy(), Verdict::Stale),
            (
                "stale exactly at threshold",
                worktree(),
                facts(24 * HOUR),
                policy(),
                Verdict::Stale,
            ),
            (
                "idle below stale",
                worktree(),
                facts(mid),
                policy(),
                Verdict::Idle,
            ),
            (
                "stale_after none",
                worktree(),
                facts(old),
                Policy {
                    stale_after: None,
                    ..policy()
                },
                Verdict::Idle,
            ),
            (
                "exactly min_age is not active",
                worktree(),
                facts(HOUR),
                policy(),
                Verdict::Idle,
            ),
        ];
        for (name, wt, facts, policy, expected) in cases {
            assert_eq!(classify(&wt, &facts, &policy, now()), expected, "{name}");
        }
    }

    #[test]
    fn labels_and_display() {
        let cases = [
            (Verdict::Never, "never", "main or bare checkout"),
            (Verdict::Locked("wip".into()), "locked", "locked: wip"),
            (Verdict::Locked(String::new()), "locked", "locked"),
            (Verdict::Dirty, "dirty", "modified or untracked files"),
            (
                Verdict::Orphaned,
                "orphaned",
                "detached HEAD with commits on no branch",
            ),
            (Verdict::Kept, "kept", "matches --keep"),
            (Verdict::Active, "active", "used recently"),
            (Verdict::Prune, "prune", "folder no longer exists"),
            (
                Verdict::Merged(Merge::Ancestor {
                    base: "main".into(),
                }),
                "merged",
                "merged into main",
            ),
            (
                Verdict::Merged(Merge::Squash {
                    base: "main".into(),
                }),
                "squashed",
                "squash-merged into main",
            ),
            (
                Verdict::Merged(Merge::Gone {
                    upstream: "origin/x".into(),
                }),
                "gone",
                "upstream origin/x deleted",
            ),
            (Verdict::Stale, "stale", "unmerged, idle past --stale-after"),
            (Verdict::Idle, "idle", "unmerged, idle"),
        ];
        for (verdict, label, text) in cases {
            assert_eq!(verdict.label(), label);
            assert_eq!(verdict.to_string(), text);
        }
    }
}
