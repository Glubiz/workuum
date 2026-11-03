use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

use workuum_core::{
    CacheVerdict, Facts, GLOBAL_CACHES, Kind, Policy, Verdict, Worktree, classify, classify_cache,
    format_duration, plan_evictions,
};
use workuum_git::{Error, Repo, deletable, dirty, last_activity, merged, reachable, resolve};
use workuum_scan::{
    Cache, Index, Located, VolumeId, clean as clear_global, dir_size, find_entries, last_use,
    locate, lock, space, state_dir, volume_id,
};

use crate::config::{Config, matches};
use crate::report::{self, Row};

const FETCH_TIMEOUT: Duration = Duration::from_secs(60);

pub enum Failure {
    Busy,
    Fatal(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Failure {
        Failure::Fatal(message)
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Busy => f.write_str("another workuum pass is running"),
            Failure::Fatal(message) => f.write_str(message),
        }
    }
}

struct Checked {
    verdict: Verdict,
    last_activity: SystemTime,
}

struct WorktreeInfo {
    repo: Repo,
    worktree: Worktree,
    kept: bool,
    status: Result<Checked, String>,
}

struct CacheInfo {
    path: PathBuf,
    ecosystem: String,
    last_use: SystemTime,
    verdict: CacheVerdict,
}

struct Survey {
    now: SystemTime,
    worktrees: Vec<WorktreeInfo>,
    caches: Vec<CacheInfo>,
}

pub enum Target {
    Worktree {
        repo: Repo,
        branch: Option<String>,
        delete_branch: bool,
    },
    Cache,
    Global(Located),
    Prune(Repo),
}

pub struct Action {
    pub target: Target,
    pub path: PathBuf,
    pub detail: String,
    pub bytes: u64,
}

fn idle(now: SystemTime, since: SystemTime) -> Duration {
    now.duration_since(since).unwrap_or_default()
}

fn state() -> Result<PathBuf, String> {
    state_dir()
        .ok_or_else(|| "cannot determine a state directory; set WORKUUM_STATE_DIR".to_string())
}

// A git failure other than a missing binary only costs this one repository.
fn tolerate<T>(result: Result<T, Error>, context: &Path) -> Result<Option<T>, String> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Error::GitNotFound) => Err(Error::GitNotFound.to_string()),
        Err(error) => {
            report::warn(&format!("{}: {error}", context.display()));
            Ok(None)
        }
    }
}

fn check(
    worktree: &Worktree,
    bases: &[String],
    kept: bool,
    policy: &Policy,
    now: SystemTime,
) -> Result<Checked, Error> {
    let last_activity = last_activity(worktree)?;
    let inspectable = worktree.kind == Kind::Linked && !worktree.prunable;
    let (dirty, merged) = match inspectable {
        true => (dirty(&worktree.path)?, merged(worktree, bases)?),
        false => (false, None),
    };
    let orphaned = worktree.kind == Kind::Linked && merged.is_none() && !reachable(worktree)?;
    let facts = Facts {
        dirty,
        orphaned,
        merged,
        last_activity,
        kept,
    };
    Ok(Checked {
        verdict: classify(worktree, &facts, policy, now),
        last_activity,
    })
}

fn inspect_repo(
    config: &Config,
    git_dir: &Path,
    cwd: Option<&Path>,
    now: SystemTime,
) -> Result<Vec<WorktreeInfo>, String> {
    let repo = Repo {
        git_dir: git_dir.to_path_buf(),
    };
    if config.fetch {
        tolerate(repo.fetch(FETCH_TIMEOUT), git_dir)?;
    }
    let Some(bases) = tolerate(repo.bases(&config.base), git_dir)? else {
        return Ok(Vec::new());
    };
    let Some(worktrees) = tolerate(repo.worktrees(), git_dir)? else {
        return Ok(Vec::new());
    };
    let policy = Policy {
        min_age: config.min_age,
        stale_after: config.stale_after,
    };
    let mut infos = Vec::new();
    for worktree in worktrees {
        let kept = matches(&config.keep, &worktree.path.to_string_lossy())
            || worktree
                .branch
                .as_deref()
                .is_some_and(|branch| matches(&config.keep, branch))
            || cwd.is_some_and(|cwd| cwd.starts_with(&worktree.path));
        let status = match check(&worktree, &bases, kept, &policy, now) {
            Ok(checked) => Ok(checked),
            Err(Error::GitNotFound) => return Err(Error::GitNotFound.to_string()),
            Err(error) => Err(error.to_string()),
        };
        infos.push(WorktreeInfo {
            repo: repo.clone(),
            worktree,
            kept,
            status,
        });
    }
    Ok(infos)
}

fn inspect_all(
    config: &Config,
    repos: &[PathBuf],
    now: SystemTime,
) -> Result<Vec<WorktreeInfo>, String> {
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|cwd| dunce::canonicalize(cwd).ok());
    let threads = thread::available_parallelism().map_or(1, usize::from);
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..threads.min(repos.len()) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(git_dir) = repos.get(index) else {
                        return;
                    };
                    let result = inspect_repo(config, git_dir, cwd.as_deref(), now);
                    results.lock().unwrap().push((index, result));
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(index, _)| *index);
    let mut infos = Vec::new();
    for (_, result) in results {
        infos.extend(result?);
    }
    Ok(infos)
}

fn attach_caches(
    config: &Config,
    caches: Vec<Cache>,
    worktrees: &[WorktreeInfo],
    now: SystemTime,
) -> Result<Vec<CacheInfo>, String> {
    let mut by_owner: BTreeMap<usize, (SystemTime, Vec<Cache>)> = BTreeMap::new();
    for cache in caches {
        if matches(&config.keep, &cache.path.to_string_lossy()) {
            continue;
        }
        let owner = worktrees
            .iter()
            .enumerate()
            .filter(|(_, info)| {
                !info.worktree.prunable && cache.path.starts_with(&info.worktree.path)
            })
            .max_by_key(|(_, info)| info.worktree.path.components().count());
        let Some((index, owner)) = owner else {
            continue;
        };
        let Ok(checked) = &owner.status else {
            continue;
        };
        let leaving = matches!(checked.verdict, Verdict::Merged(_) | Verdict::Stale);
        let protected = owner.kept || checked.verdict == Verdict::Orphaned;
        if protected || owner.worktree.locked.is_some() || leaving {
            continue;
        }
        by_owner
            .entry(index)
            .or_insert_with(|| (checked.last_activity, Vec::new()))
            .1
            .push(cache);
    }
    let mut infos = Vec::new();
    for (index, (owner_activity, caches)) in by_owner {
        let checkout = &worktrees[index].worktree.path;
        let folders: Vec<PathBuf> = caches.iter().map(|cache| cache.path.clone()).collect();
        let Some(allowed) = tolerate(deletable(checkout, &folders), checkout)? else {
            continue;
        };
        for cache in caches {
            if !allowed.contains(&cache.path) {
                continue;
            }
            let Ok(used) = last_use(&cache.path) else {
                continue;
            };
            let used = used.max(owner_activity);
            infos.push(CacheInfo {
                verdict: classify_cache(used, config.min_age, config.cache_stale_after, now),
                path: cache.path,
                ecosystem: cache.ecosystem,
                last_use: used,
            });
        }
    }
    Ok(infos)
}

fn survey(config: &Config, state: &Path, now: SystemTime) -> Result<Survey, String> {
    let excludes: Vec<String> = config
        .excludes
        .iter()
        .map(|pattern| pattern.as_str().to_string())
        .collect();
    let fresh = Index::load(state)
        .filter(|index| index.is_fresh(&config.roots, &excludes, config.rescan, now));
    let (repos, caches) = match fresh {
        Some(index) => (
            index
                .repos
                .into_iter()
                .filter(|repo| repo.exists())
                .collect(),
            index
                .caches
                .into_iter()
                .filter(|cache| cache.path.exists())
                .collect(),
        ),
        None => {
            let found = find_entries(&config.roots, &config.excludes);
            let repos: BTreeSet<PathBuf> =
                found.git.iter().filter_map(|git| resolve(git)).collect();
            let repos: Vec<PathBuf> = repos.into_iter().collect();
            let index = Index {
                roots: config.roots.clone(),
                excludes,
                repos: repos.clone(),
                caches: found.caches.clone(),
                scanned_at: now,
            };
            if let Err(error) = index.save(state) {
                report::warn(&format!("{}: {error}", state.display()));
            }
            (repos, found.caches)
        }
    };
    let worktrees = inspect_all(config, &repos, now)?;
    let caches = attach_caches(config, caches, &worktrees, now)?;
    Ok(Survey {
        now,
        worktrees,
        caches,
    })
}

pub fn list(config: &Config, size: bool) -> Result<Vec<Row>, String> {
    let survey = survey(config, &state()?, SystemTime::now())?;
    let measure = |path: &Path| size.then(|| dir_size(path));
    let mut rows = Vec::new();
    for info in &survey.worktrees {
        if info.worktree.kind != Kind::Linked {
            continue;
        }
        let (label, reason, idle_for) = match &info.status {
            Ok(checked) => (
                checked.verdict.label(),
                checked.verdict.to_string(),
                idle(survey.now, checked.last_activity),
            ),
            Err(error) => ("error", error.clone(), Duration::ZERO),
        };
        rows.push(Row {
            kind: "worktree",
            path: info.worktree.path.clone(),
            label,
            reason,
            branch: info.worktree.branch.clone(),
            idle: idle_for,
            size: measure(&info.worktree.path),
        });
    }
    for cache in &survey.caches {
        rows.push(Row {
            kind: "cache",
            path: cache.path.clone(),
            label: cache.verdict.label(),
            reason: cache.ecosystem.clone(),
            branch: None,
            idle: idle(survey.now, cache.last_use),
            size: measure(&cache.path),
        });
    }
    if !size {
        return Ok(rows);
    }
    for located in GLOBAL_CACHES.iter().filter_map(locate) {
        let Ok(used) = last_use(&located.path) else {
            continue;
        };
        rows.push(Row {
            kind: "global",
            label: "global",
            reason: located.cache.name.to_string(),
            branch: None,
            idle: idle(survey.now, used),
            size: measure(&located.path),
            path: located.path,
        });
    }
    Ok(rows)
}

fn detail(reason: impl fmt::Display, idle_for: Duration) -> String {
    format!("{reason} · idle {}", format_duration(idle_for))
}

fn rule_actions(config: &Config, survey: &Survey) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut prune: BTreeMap<&Path, usize> = BTreeMap::new();
    // `git worktree prune` would also drop the record and reflog of an orphaned prunable worktree.
    let mut orphaned_repos: BTreeSet<&Path> = BTreeSet::new();
    for info in &survey.worktrees {
        let Ok(checked) = &info.status else {
            continue;
        };
        if checked.verdict == Verdict::Orphaned {
            orphaned_repos.insert(&info.repo.git_dir);
        }
        let merged = matches!(checked.verdict, Verdict::Merged(_));
        if checked.verdict == Verdict::Prune {
            *prune.entry(&info.repo.git_dir).or_default() += 1;
        }
        if !merged && checked.verdict != Verdict::Stale {
            continue;
        }
        actions.push(Action {
            target: Target::Worktree {
                repo: info.repo.clone(),
                branch: info.worktree.branch.clone(),
                delete_branch: config.delete_branch && merged,
            },
            path: info.worktree.path.clone(),
            detail: detail(&checked.verdict, idle(survey.now, checked.last_activity)),
            bytes: dir_size(&info.worktree.path),
        });
    }
    for cache in survey
        .caches
        .iter()
        .filter(|cache| cache.verdict == CacheVerdict::Stale)
    {
        actions.push(Action {
            target: Target::Cache,
            path: cache.path.clone(),
            detail: detail(
                format!("stale {} build folder", cache.ecosystem),
                idle(survey.now, cache.last_use),
            ),
            bytes: dir_size(&cache.path),
        });
    }
    for (git_dir, count) in prune {
        if orphaned_repos.contains(git_dir) {
            continue;
        }
        actions.push(Action {
            target: Target::Prune(Repo {
                git_dir: git_dir.to_path_buf(),
            }),
            path: git_dir.to_path_buf(),
            detail: format!("{count} worktree record(s) whose folder no longer exists"),
            bytes: 0,
        });
    }
    actions
}

fn pressure_actions(config: &Config, survey: &Survey, rules: &[Action]) -> Vec<Action> {
    let idle_worktrees = survey.worktrees.iter().filter_map(|info| {
        let checked = info.status.as_ref().ok()?;
        if checked.verdict != Verdict::Idle {
            return None;
        }
        let action = Action {
            target: Target::Worktree {
                repo: info.repo.clone(),
                branch: info.worktree.branch.clone(),
                delete_branch: false,
            },
            path: info.worktree.path.clone(),
            detail: detail(
                "low disk space, unmerged",
                idle(survey.now, checked.last_activity),
            ),
            bytes: 0,
        };
        Some((action, checked.last_activity))
    });
    let idle_caches = survey
        .caches
        .iter()
        .filter(|cache| cache.verdict == CacheVerdict::Idle)
        .map(|cache| {
            let action = Action {
                target: Target::Cache,
                path: cache.path.clone(),
                detail: detail(
                    format!("low disk space, {} build folder", cache.ecosystem),
                    idle(survey.now, cache.last_use),
                ),
                bytes: 0,
            };
            (action, cache.last_use)
        });
    let mut idle_worktrees: Vec<_> = idle_worktrees.collect();
    let mut idle_caches: Vec<_> = idle_caches.collect();

    let mut volumes: Vec<(VolumeId, &Path)> = Vec::new();
    let paths = survey
        .caches
        .iter()
        .map(|cache| cache.path.as_path())
        .chain(
            survey
                .worktrees
                .iter()
                .map(|info| info.worktree.path.as_path()),
        );
    for path in paths {
        let Ok(id) = volume_id(path) else {
            continue;
        };
        if volumes.iter().all(|(known, _)| *known != id) {
            volumes.push((id, path));
        }
    }

    let on = |volume: &VolumeId, path: &Path| volume_id(path).is_ok_and(|id| id == *volume);
    let mut globals: Option<Vec<(Action, SystemTime)>> = None;
    let mut actions = Vec::new();
    for (volume, sample) in volumes {
        let Ok(space) = space(sample) else {
            continue;
        };
        let removed: u64 = rules
            .iter()
            .filter(|action| on(&volume, &action.path))
            .map(|action| action.bytes)
            .sum();
        let mut deficit = config
            .min_free
            .deficit(space.total, space.available)
            .saturating_sub(removed);
        if deficit == 0 {
            continue;
        }
        let located = globals.get_or_insert_with(|| {
            if config.skip_global_caches {
                return Vec::new();
            }
            GLOBAL_CACHES
                .iter()
                .filter_map(locate)
                .filter_map(|located| {
                    let used = last_use(&located.path).ok()?;
                    if idle(survey.now, used) < config.min_age {
                        return None;
                    }
                    let action = Action {
                        path: located.path.clone(),
                        detail: format!("low disk space, {} cache", located.cache.name),
                        target: Target::Global(located),
                        bytes: 0,
                    };
                    Some((action, used))
                })
                .collect()
        });
        let (mine, rest) = std::mem::take(located)
            .into_iter()
            .partition(|(action, _)| on(&volume, &action.path));
        *located = rest;
        let (worktrees, rest) = std::mem::take(&mut idle_worktrees)
            .into_iter()
            .partition(|(action, _)| on(&volume, &action.path));
        idle_worktrees = rest;
        let (caches, rest) = std::mem::take(&mut idle_caches)
            .into_iter()
            .partition(|(action, _)| on(&volume, &action.path));
        idle_caches = rest;
        let tiers: [Vec<(Action, SystemTime)>; 3] = [caches, mine, worktrees];
        for tier in tiers {
            if deficit == 0 {
                break;
            }
            for (mut action, size) in plan_evictions(tier, deficit, |action| dir_size(&action.path))
            {
                action.bytes = size;
                deficit = deficit.saturating_sub(size);
                actions.push(action);
            }
        }
        if deficit > 0 {
            report::warn(&format!(
                "cannot free enough space on the volume holding {}",
                sample.display()
            ));
        }
    }
    actions
}

fn execute(action: &Action, min_age: Duration) -> Result<(), String> {
    match &action.target {
        Target::Worktree {
            repo,
            branch,
            delete_branch,
        } => {
            repo.remove(&action.path)
                .map_err(|error| error.to_string())?;
            if *delete_branch
                && let Some(branch) = branch
                && let Err(error) = repo.delete_branch(branch)
            {
                report::warn(&format!("branch {branch}: {error}"));
            }
            Ok(())
        }
        Target::Cache => std::fs::remove_dir_all(&action.path).map_err(|error| error.to_string()),
        Target::Global(located) => clear_global(located).map_err(|error| error.to_string()),
        Target::Prune(repo) => repo.prune(min_age).map_err(|error| error.to_string()),
    }
}

pub fn clean(config: &Config, dry_run: bool) -> Result<(), Failure> {
    let state = state()?;
    let _lock = lock(&state)
        .map_err(|error| format!("{}: {error}", state.display()))?
        .ok_or(Failure::Busy)?;
    let survey = survey(config, &state, SystemTime::now())?;
    let mut actions = rule_actions(config, &survey);
    let pressure = pressure_actions(config, &survey, &actions);
    actions.extend(pressure);
    let mut freed = 0;
    for action in &actions {
        let result = match dry_run {
            true => Ok(()),
            false => execute(action, config.min_age),
        };
        report::action(&config.home, action, &result, dry_run);
        if result.is_ok() {
            freed += action.bytes;
        }
    }
    report::summary(actions.len(), freed, dry_run);
    Ok(())
}
