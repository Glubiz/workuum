use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Keeps user config (color.ui, diff.external, ...) from changing what we parse.
pub(crate) const PLAIN: [&str; 3] = ["--no-color", "--no-ext-diff", "--no-textconv"];

#[derive(Debug)]
pub enum Error {
    GitNotFound,
    Failed { args: String, detail: String },
    TimedOut { args: String, after: Duration },
    Io { path: PathBuf, source: io::Error },
}

impl Error {
    pub(crate) fn io(path: &Path) -> impl FnOnce(io::Error) -> Error {
        move |source| Error::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::GitNotFound => write!(f, "git not found on PATH"),
            Error::Failed { args, detail } => write!(f, "git {args}: {detail}"),
            Error::TimedOut { args, after } => {
                write!(f, "git {args}: timed out after {}s", after.as_secs())
            }
            Error::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub(crate) fn git<I, S>(args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

pub(crate) fn git_in(git_dir: &Path) -> Command {
    let mut arg = OsString::from("--git-dir=");
    arg.push(git_dir);
    git([arg])
}

fn args_of(command: &Command) -> String {
    let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy()).collect();
    args.join(" ")
}

fn spawn_error(command: &Command, source: io::Error) -> Error {
    if source.kind() == io::ErrorKind::NotFound {
        return Error::GitNotFound;
    }
    Error::Failed {
        args: args_of(command),
        detail: source.to_string(),
    }
}

pub(crate) fn failed(command: &Command, stderr: &[u8]) -> Error {
    let stderr = String::from_utf8_lossy(stderr);
    Error::Failed {
        args: args_of(command),
        detail: stderr.lines().next().unwrap_or("failed").to_string(),
    }
}

/// Runs to completion, the caller inspects the exit code.
pub(crate) fn output(command: &mut Command) -> Result<Output, Error> {
    command.output().map_err(|e| spawn_error(command, e))
}

/// Runs and returns stdout, any non-zero exit is an error.
pub(crate) fn run(command: &mut Command) -> Result<String, Error> {
    let output = output(command)?;
    if !output.status.success() {
        return Err(failed(command, &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs with `input` on stdin, the caller inspects the exit code.
pub(crate) fn output_with_stdin(command: &mut Command, input: &[u8]) -> Result<Output, Error> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| spawn_error(command, e))?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err(failed(command, b"no stdin"));
    };
    // Paths are small; a writer thread keeps a full stdout pipe from blocking us.
    let writer = input.to_vec();
    let feeder = thread::spawn(move || stdin.write_all(&writer));
    let output = child
        .wait_with_output()
        .map_err(|e| spawn_error(command, e))?;
    let _ = feeder.join();
    Ok(output)
}

/// Feeds stdout of `producer` into stdin of `consumer` and returns the consumer's stdout.
pub(crate) fn pipe(producer: &mut Command, consumer: &mut Command) -> Result<String, Error> {
    let mut child = producer
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| spawn_error(producer, e))?;
    let Some(stdout) = child.stdout.take() else {
        return Err(failed(producer, b"no stdout"));
    };
    let output = consumer
        .stdin(Stdio::from(stdout))
        .output()
        .map_err(|e| spawn_error(consumer, e))?;
    let status = child.wait().map_err(|e| spawn_error(producer, e))?;
    if !output.status.success() {
        return Err(failed(consumer, &output.stderr));
    }
    if !status.success() {
        return Err(failed(producer, b"failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(crate) fn run_timeout(command: &mut Command, timeout: Duration) -> Result<(), Error> {
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| spawn_error(command, e))?;
    let deadline = Instant::now() + timeout;
    loop {
        if child
            .try_wait()
            .map_err(|e| spawn_error(command, e))?
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::TimedOut {
                args: args_of(command),
                after: timeout,
            });
        }
        thread::sleep(Duration::from_millis(50));
    }
    let output = child
        .wait_with_output()
        .map_err(|e| spawn_error(command, e))?;
    if !output.status.success() {
        return Err(failed(command, &output.stderr));
    }
    Ok(())
}
