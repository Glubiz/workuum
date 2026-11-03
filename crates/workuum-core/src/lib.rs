//! Pure policy for workuum: no filesystem, process or network access.

mod cache;
mod plan;
mod policy;
mod units;
mod worktree;

pub use cache::{
    CacheVerdict, GLOBAL_CACHES, GlobalCache, RULES, Rule, classify_cache, match_rule,
};
pub use plan::{MinFree, plan_evictions};
pub use policy::{Policy, Verdict, classify};
pub use units::{
    ParseError, format_bytes, format_duration, parse_duration, parse_min_free,
    parse_optional_duration,
};
pub use worktree::{Facts, Kind, Merge, Worktree};
