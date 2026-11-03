use std::time::{Duration, SystemTime};

#[derive(Debug, PartialEq, Eq)]
pub struct Rule {
    pub folders: &'static [&'static str],
    pub markers: &'static [&'static str],
    pub ecosystem: &'static str,
}

const fn rule(
    folders: &'static [&'static str],
    markers: &'static [&'static str],
    ecosystem: &'static str,
) -> Rule {
    Rule {
        folders,
        markers,
        ecosystem,
    }
}

pub const RULES: &[Rule] = &[
    rule(&["target"], &["Cargo.toml"], "Rust"),
    rule(&["target"], &["pom.xml"], "Maven"),
    rule(
        &[
            "node_modules",
            ".next",
            ".nuxt",
            ".svelte-kit",
            ".turbo",
            ".parcel-cache",
        ],
        &["package.json"],
        "Node",
    ),
    rule(&[".angular"], &["angular.json"], "Angular"),
    rule(
        &[
            ".venv",
            "venv",
            ".tox",
            ".nox",
            ".mypy_cache",
            ".pytest_cache",
            ".ruff_cache",
        ],
        &[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
            "Pipfile",
            "tox.ini",
        ],
        "Python",
    ),
    rule(
        &["build", ".gradle"],
        &[
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
        "Gradle",
    ),
    rule(
        &["bin", "obj"],
        &["*.csproj", "*.fsproj", "*.vbproj"],
        ".NET",
    ),
    rule(&[".build"], &["Package.swift"], "Swift"),
    rule(&["vendor"], &["composer.json"], "PHP"),
    rule(&["_build", "deps"], &["mix.exs"], "Elixir"),
    rule(&[".dart_tool", "build"], &["pubspec.yaml"], "Dart"),
    rule(
        &["zig-cache", ".zig-cache", "zig-out"],
        &["build.zig"],
        "Zig",
    ),
];

fn has_marker(marker: &str, siblings: &[String]) -> bool {
    match marker.strip_prefix('*') {
        Some(suffix) => siblings.iter().any(|name| name.ends_with(suffix)),
        None => siblings.iter().any(|name| name == marker),
    }
}

/// `siblings` lists the parent directory and is only called when a rule names the folder.
pub fn match_rule(folder: &str, siblings: impl FnOnce() -> Vec<String>) -> Option<&'static Rule> {
    if !RULES.iter().any(|rule| rule.folders.contains(&folder)) {
        return None;
    }
    let siblings = siblings();
    RULES.iter().find(|rule| {
        rule.folders.contains(&folder)
            && rule
                .markers
                .iter()
                .any(|marker| has_marker(marker, &siblings))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheVerdict {
    Active,
    Stale,
    Idle,
}

impl CacheVerdict {
    pub fn label(&self) -> &'static str {
        match self {
            CacheVerdict::Active => "cache-active",
            CacheVerdict::Stale => "cache-stale",
            CacheVerdict::Idle => "cache-idle",
        }
    }
}

pub fn classify_cache(
    last_use: SystemTime,
    min_age: Duration,
    stale_after: Option<Duration>,
    now: SystemTime,
) -> CacheVerdict {
    let idle = now.duration_since(last_use).unwrap_or_default();
    if idle < min_age {
        return CacheVerdict::Active;
    }
    if stale_after.is_some_and(|limit| idle >= limit) {
        return CacheVerdict::Stale;
    }
    CacheVerdict::Idle
}

#[derive(Debug, PartialEq, Eq)]
pub struct GlobalCache {
    pub name: &'static str,
    pub program: &'static str,
    pub locate: &'static [&'static str],
    pub suffix: Option<&'static str>,
    pub clean: &'static [&'static str],
}

pub const GLOBAL_CACHES: &[GlobalCache] = &[
    GlobalCache {
        name: "npm",
        program: "npm",
        locate: &["config", "get", "cache"],
        suffix: Some("_cacache"),
        clean: &["cache", "clean", "--force"],
    },
    GlobalCache {
        name: "bun",
        program: "bun",
        locate: &["pm", "cache"],
        suffix: None,
        clean: &["pm", "cache", "rm"],
    },
    GlobalCache {
        name: "pnpm",
        program: "pnpm",
        locate: &["store", "path"],
        suffix: None,
        clean: &["store", "prune"],
    },
    GlobalCache {
        name: "yarn",
        program: "yarn",
        locate: &["cache", "dir"],
        suffix: None,
        clean: &["cache", "clean"],
    },
    GlobalCache {
        name: "go build",
        program: "go",
        locate: &["env", "GOCACHE"],
        suffix: None,
        clean: &["clean", "-cache"],
    },
    GlobalCache {
        name: "go modules",
        program: "go",
        locate: &["env", "GOMODCACHE"],
        suffix: None,
        clean: &["clean", "-modcache"],
    },
    GlobalCache {
        name: "pip",
        program: "pip",
        locate: &["cache", "dir"],
        suffix: None,
        clean: &["cache", "purge"],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::HashSet;
    use std::time::UNIX_EPOCH;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    fn ecosystem(folder: &str, siblings: &[&str]) -> Option<&'static str> {
        match_rule(folder, || names(siblings)).map(|rule| rule.ecosystem)
    }

    #[test]
    fn matches_each_ecosystem() {
        let cases = [
            ("target", "Cargo.toml", "Rust"),
            ("target", "pom.xml", "Maven"),
            ("node_modules", "package.json", "Node"),
            (".next", "package.json", "Node"),
            (".angular", "angular.json", "Angular"),
            (".venv", "pyproject.toml", "Python"),
            (".tox", "tox.ini", "Python"),
            (".gradle", "settings.gradle.kts", "Gradle"),
            ("obj", "App.fsproj", ".NET"),
            (".build", "Package.swift", "Swift"),
            ("vendor", "composer.json", "PHP"),
            ("deps", "mix.exs", "Elixir"),
            (".dart_tool", "pubspec.yaml", "Dart"),
            ("zig-out", "build.zig", "Zig"),
        ];
        for (folder, marker, expected) in cases {
            assert_eq!(ecosystem(folder, &["src", marker]), Some(expected));
        }
    }

    #[test]
    fn star_marker_matches_by_suffix() {
        assert_eq!(ecosystem("bin", &["App.csproj"]), Some(".NET"));
        assert_eq!(ecosystem("bin", &["csproj"]), None);
        assert_eq!(ecosystem("bin", &["App.csproj.user"]), None);
    }

    #[test]
    fn name_without_marker_is_none() {
        assert_eq!(ecosystem("target", &["src", "README.md"]), None);
        assert_eq!(ecosystem("node_modules", &[]), None);
    }

    #[test]
    fn unrelated_name_does_not_list_siblings() {
        let calls = Cell::new(0);
        let result = match_rule("src", || {
            calls.set(calls.get() + 1);
            names(&["Cargo.toml"])
        });
        assert_eq!(result, None);
        assert_eq!(calls.get(), 0);
        match_rule("target", || {
            calls.set(calls.get() + 1);
            names(&["Cargo.toml"])
        });
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn build_folder_follows_its_marker() {
        assert_eq!(ecosystem("build", &["build.gradle.kts"]), Some("Gradle"));
        assert_eq!(ecosystem("build", &["pubspec.yaml"]), Some("Dart"));
        assert_eq!(ecosystem("build", &["Cargo.toml"]), None);
    }

    #[test]
    fn classify_cache_boundaries() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let ago = |seconds| now - Duration::from_secs(seconds);
        let min_age = Duration::from_secs(10);
        let stale = Some(Duration::from_secs(100));
        assert_eq!(
            classify_cache(ago(9), min_age, stale, now),
            CacheVerdict::Active
        );
        assert_eq!(
            classify_cache(ago(10), min_age, stale, now),
            CacheVerdict::Idle
        );
        assert_eq!(
            classify_cache(ago(99), min_age, stale, now),
            CacheVerdict::Idle
        );
        assert_eq!(
            classify_cache(ago(100), min_age, stale, now),
            CacheVerdict::Stale
        );
        assert_eq!(
            classify_cache(ago(1_000), min_age, None, now),
            CacheVerdict::Idle
        );
        let future = now + Duration::from_secs(50);
        assert_eq!(
            classify_cache(future, min_age, stale, now),
            CacheVerdict::Active
        );
        assert_eq!(
            classify_cache(future, Duration::ZERO, stale, now),
            CacheVerdict::Idle
        );
    }

    #[test]
    fn cache_verdict_labels() {
        assert_eq!(CacheVerdict::Active.label(), "cache-active");
        assert_eq!(CacheVerdict::Stale.label(), "cache-stale");
        assert_eq!(CacheVerdict::Idle.label(), "cache-idle");
    }

    #[test]
    fn global_cache_names_are_unique() {
        let names: HashSet<_> = GLOBAL_CACHES.iter().map(|cache| cache.name).collect();
        assert_eq!(names.len(), GLOBAL_CACHES.len());
    }
}
