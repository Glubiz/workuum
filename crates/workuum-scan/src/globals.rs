use std::io;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use workuum_core::GlobalCache;

#[derive(Debug)]
pub struct Located {
    pub cache: &'static GlobalCache,
    pub path: PathBuf,
}

fn run(program: &str, args: &[&str]) -> io::Result<Output> {
    let spawn = |program: &str| {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
    };
    // npm, pnpm and yarn are .cmd shims on Windows, which Command does not resolve.
    #[cfg(windows)]
    match spawn(&format!("{program}.cmd")) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        result => return result,
    }
    spawn(program)
}

pub fn locate(cache: &'static GlobalCache) -> Option<Located> {
    let output = run(cache.program, cache.locate).ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let located = stdout.trim();
    if located.is_empty() {
        return None;
    }
    let mut path = PathBuf::from(located);
    if let Some(suffix) = cache.suffix {
        path.push(suffix);
    }
    path.is_dir().then_some(Located { cache, path })
}

pub fn clean(located: &Located) -> io::Result<()> {
    let cache = located.cache;
    let output = run(cache.program, cache.clean)?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.lines().next().unwrap_or("failed");
    Err(io::Error::other(format!("{}: {detail}", cache.name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    static MISSING: GlobalCache = GlobalCache {
        name: "missing",
        program: "workuum-no-such-program",
        locate: &["dir"],
        suffix: None,
        clean: &["clean"],
    };

    fn on_path(program: &str) -> bool {
        run(program, &["--version"]).is_ok()
    }

    #[test]
    fn missing_program_is_not_located() {
        assert!(locate(&MISSING).is_none());
    }

    #[test]
    fn missing_program_clean_is_an_error() {
        let located = Located {
            cache: &MISSING,
            path: PathBuf::new(),
        };
        assert!(clean(&located).is_err());
    }

    #[test]
    fn go_build_cache_is_located_when_go_is_installed() {
        if !on_path("go") {
            return;
        }
        let cache = &workuum_core::GLOBAL_CACHES[4];
        assert_eq!(cache.name, "go build");
        if let Some(located) = locate(cache) {
            assert!(located.path.is_dir());
        }
    }

    #[test]
    fn npm_cache_is_located_when_npm_is_installed() {
        if !on_path("npm") {
            return;
        }
        let cache = &workuum_core::GLOBAL_CACHES[0];
        assert_eq!(cache.name, "npm");
        if let Some(located) = locate(cache) {
            assert!(located.path.ends_with("_cacache"));
        }
    }
}
