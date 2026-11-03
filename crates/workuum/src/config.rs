use std::path::PathBuf;
use std::time::Duration;

use glob::{Pattern, PatternError};
use workuum_core::MinFree;
use workuum_scan::default_excludes;

use crate::cli::Options;

pub struct Config {
    pub home: PathBuf,
    pub roots: Vec<PathBuf>,
    pub excludes: Vec<Pattern>,
    pub keep: Vec<Pattern>,
    pub base: Vec<String>,
    pub min_age: Duration,
    pub stale_after: Option<Duration>,
    pub cache_stale_after: Option<Duration>,
    pub min_free: MinFree,
    pub fetch: bool,
    pub delete_branch: bool,
    pub skip_global_caches: bool,
    pub rescan: Duration,
}

pub fn parse_glob(pattern: &str) -> Result<Pattern, PatternError> {
    Pattern::new(&pattern.replace('\\', "/"))
}

pub fn matches(patterns: &[Pattern], text: &str) -> bool {
    let text = text.replace('\\', "/");
    patterns.iter().any(|pattern| pattern.matches(&text))
}

impl Config {
    pub fn resolve(options: Options) -> Result<Config, String> {
        let home = std::env::home_dir().ok_or("cannot determine the home directory")?;
        let home = dunce::canonicalize(&home).unwrap_or(home);
        let roots = match options.root.is_empty() {
            true => vec![home.clone()],
            false => options.root,
        };
        let roots = roots
            .iter()
            .map(|root| {
                dunce::canonicalize(root).map_err(|error| format!("{}: {error}", root.display()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut excludes = default_excludes(&home);
        excludes.extend(options.exclude);
        Ok(Config {
            home,
            roots,
            excludes,
            keep: options.keep,
            base: options.base,
            min_age: options.min_age,
            stale_after: options.stale_after,
            cache_stale_after: options.cache_stale_after,
            min_free: options.min_free,
            fetch: options.fetch,
            delete_branch: options.delete_branch,
            skip_global_caches: options.skip_global_caches,
            rescan: options.rescan,
        })
    }
}
