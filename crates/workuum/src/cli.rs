use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use glob::Pattern;
use workuum_core::{MinFree, parse_duration, parse_min_free, parse_optional_duration};

use crate::config::parse_glob;

// A plain alias keeps clap from reading Option<Duration> as "optional argument".
type Window = Option<Duration>;

#[derive(Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(flatten)]
    pub options: Options,
    /// Saved settings session to use [default: "default" when saved]
    #[arg(long, global = true, env = "WORKUUM_SESSION")]
    pub session: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args)]
pub struct Options {
    /// Folders searched for git repositories [default: the home directory]
    #[arg(long, global = true, env = "WORKUUM_ROOTS", value_delimiter = ',')]
    pub root: Vec<PathBuf>,
    /// Glob of folders the search skips
    #[arg(long, global = true, env = "WORKUUM_EXCLUDE", value_delimiter = ',', value_parser = parse_glob)]
    pub exclude: Vec<Pattern>,
    /// Glob of worktree paths, branch names or build folder paths that are never removed
    #[arg(long, global = true, env = "WORKUUM_KEEP", value_delimiter = ',', value_parser = parse_glob)]
    pub keep: Vec<Pattern>,
    /// Ref that counts as merged into [default: origin/HEAD, origin/main, origin/master, main, master]
    #[arg(long, global = true, env = "WORKUUM_BASE", value_delimiter = ',')]
    pub base: Vec<String>,
    /// Nothing used more recently than this is touched
    #[arg(long, global = true, env = "WORKUUM_MIN_AGE", default_value = "1d", value_parser = parse_duration)]
    pub min_age: Duration,
    /// Unmerged worktrees idle for this long are removed, or "off"
    #[arg(long, global = true, env = "WORKUUM_STALE_AFTER", default_value = "30d", value_parser = parse_optional_duration)]
    pub stale_after: Window,
    /// Build folders idle for this long are deleted, or "off"
    #[arg(long, global = true, env = "WORKUUM_CACHE_STALE_AFTER", default_value = "14d", value_parser = parse_optional_duration)]
    pub cache_stale_after: Window,
    /// Free space to keep per volume, a size such as 20GB or a percentage
    #[arg(long, global = true, env = "WORKUUM_MIN_FREE", default_value = "10%", value_parser = parse_min_free)]
    pub min_free: MinFree,
    /// Run git fetch --prune in each repository before judging merges
    #[arg(long, global = true, env = "WORKUUM_FETCH")]
    pub fetch: bool,
    /// Also delete the branch of a merged worktree it removes
    #[arg(long, global = true, env = "WORKUUM_DELETE_BRANCH")]
    pub delete_branch: bool,
    /// Never clear npm, bun, pnpm, yarn, go or pip caches under disk pressure
    #[arg(long, global = true, env = "WORKUUM_SKIP_GLOBAL_CACHES")]
    pub skip_global_caches: bool,
    /// Search the roots again when the stored index is older than this
    #[arg(long, global = true, env = "WORKUUM_RESCAN", default_value = "24h", value_parser = parse_duration)]
    pub rescan: Duration,
}

#[derive(Subcommand)]
pub enum Command {
    /// Install the latest workuum release when it is newer than this one
    Update,
    /// Edit saved settings sessions in a full-screen terminal UI
    Settings,
    #[command(flatten)]
    Pass(PassCommand),
}

#[derive(Subcommand)]
pub enum PassCommand {
    /// Show what workuum finds and what it would do, change nothing
    List {
        /// Print a JSON array
        #[arg(long)]
        json: bool,
        /// Measure the folders
        #[arg(long)]
        size: bool,
    },
    /// Run one pass that removes what the rules and disk pressure select
    Clean {
        /// Print what would be removed without removing it
        #[arg(long)]
        dry_run: bool,
    },
    /// Run clean forever, sleeping between passes
    Watch {
        /// Time between passes
        #[arg(long, env = "WORKUUM_INTERVAL", default_value = "1h", value_parser = parse_duration)]
        interval: Duration,
    },
}
