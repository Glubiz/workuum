use crate::walk::Cache;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const INDEX_FILE: &str = "index.json";

pub fn state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("WORKUUM_STATE_DIR") {
        return Some(PathBuf::from(dir));
    }
    dirs::data_local_dir().map(|dir| dir.join("workuum"))
}

/// Writes to a temp file next to `path`, then renames it into place.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub roots: Vec<PathBuf>,
    #[serde(default)]
    pub excludes: Vec<String>,
    pub repos: Vec<PathBuf>,
    #[serde(default)]
    pub caches: Vec<Cache>,
    #[serde(with = "unix_seconds")]
    pub scanned_at: SystemTime,
}

mod unix_seconds {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error> {
        let seconds = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        serializer.serialize_u64(seconds)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SystemTime, D::Error> {
        let seconds = u64::deserialize(deserializer)?;
        Ok(UNIX_EPOCH + Duration::from_secs(seconds))
    }
}

impl Index {
    pub fn load(dir: &Path) -> Option<Index> {
        let bytes = fs::read(dir.join(INDEX_FILE)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        write_atomic(&dir.join(INDEX_FILE), &serde_json::to_vec_pretty(self)?)
    }

    pub fn is_fresh(
        &self,
        roots: &[PathBuf],
        excludes: &[String],
        max_age: Duration,
        now: SystemTime,
    ) -> bool {
        if self.roots != roots || self.excludes != excludes {
            return false;
        }
        now.duration_since(self.scanned_at)
            .is_ok_and(|age| age <= max_age)
    }
}

#[derive(Debug)]
pub struct PassLock {
    _file: File,
}

pub fn lock(dir: &Path) -> io::Result<Option<PassLock>> {
    fs::create_dir_all(dir)?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(PassLock { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn index(roots: &[&str], scanned_at: SystemTime) -> Index {
        Index {
            roots: roots.iter().map(PathBuf::from).collect(),
            excludes: vec!["/x".into()],
            repos: vec![PathBuf::from("/r/.git")],
            caches: vec![Cache {
                path: PathBuf::from("/r/target"),
                ecosystem: "Rust".into(),
            }],
            scanned_at,
        }
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn save_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("state");
        let saved = index(&["/a"], at(1_000));
        saved.save(&dir).unwrap();
        assert_eq!(Index::load(&dir), Some(saved));
    }

    #[test]
    fn index_without_caches_still_loads() {
        let tmp = TempDir::new().unwrap();
        let old = r#"{"roots":["/a"],"repos":["/r/.git"],"scanned_at":5}"#;
        fs::write(tmp.path().join(INDEX_FILE), old).unwrap();
        let loaded = Index::load(tmp.path()).unwrap();
        assert!(loaded.caches.is_empty());
        assert_eq!(loaded.repos, vec![PathBuf::from("/r/.git")]);
    }

    #[test]
    fn load_missing_or_corrupt_is_none() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(Index::load(tmp.path()), None);
        fs::write(tmp.path().join(INDEX_FILE), "{not json").unwrap();
        assert_eq!(Index::load(tmp.path()), None);
    }

    #[test]
    fn is_fresh_rules() {
        let stored = index(&["/a"], at(1_000));
        let roots = [PathBuf::from("/a")];
        let excludes = ["/x".to_string()];
        let max_age = Duration::from_secs(100);
        assert!(stored.is_fresh(&roots, &excludes, max_age, at(1_050)));
        assert!(!stored.is_fresh(&[PathBuf::from("/b")], &excludes, max_age, at(1_050)));
        assert!(!stored.is_fresh(&roots, &excludes, max_age, at(1_101)));
        assert!(!stored.is_fresh(&roots, &excludes, max_age, at(999)));
    }

    #[test]
    fn changed_excludes_make_the_index_stale() {
        let stored = index(&["/a"], at(1_000));
        let roots = [PathBuf::from("/a")];
        let max_age = Duration::from_secs(100);
        let other = ["/x".to_string(), "/y".to_string()];
        assert!(!stored.is_fresh(&roots, &other, max_age, at(1_050)));
        assert!(!stored.is_fresh(&roots, &[], max_age, at(1_050)));
    }

    #[test]
    fn second_lock_is_refused_until_first_dropped() {
        let tmp = TempDir::new().unwrap();
        let first = lock(tmp.path()).unwrap();
        assert!(first.is_some());
        assert!(lock(tmp.path()).unwrap().is_none());
        drop(first);
        assert!(lock(tmp.path()).unwrap().is_some());
    }
}
