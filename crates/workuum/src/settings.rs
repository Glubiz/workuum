use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::{Arg, Command};
use serde::{Deserialize, Serialize};
use toml::Value;
use workuum_scan::write_atomic;

pub type Session = BTreeMap<String, Value>;

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub sessions: BTreeMap<String, Session>,
}

pub fn path() -> Result<PathBuf, String> {
    let dir = match std::env::var_os("WORKUUM_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => dirs::config_dir()
            .ok_or("cannot determine the configuration directory")?
            .join("workuum"),
    };
    Ok(dir.join("settings.toml"))
}

/// The options a session can set: the global options and `watch --interval`.
pub fn options(command: &Command) -> Vec<&Arg> {
    let interval = command
        .find_subcommand("watch")
        .and_then(|watch| watch.get_arguments().find(|arg| arg.get_id() == "interval"));
    command
        .get_arguments()
        .filter(|arg| arg.is_global_set() && arg.get_id() != "session")
        .chain(interval)
        .collect()
}

pub fn is_flag(arg: &Arg) -> bool {
    arg.is_require_equals_set()
}

pub fn is_list(arg: &Arg) -> bool {
    arg.get_value_delimiter().is_some()
}

/// Parses `text` as a flag value, so the option's own clap value parser judges it.
pub fn check(command: &Command, arg: &Arg, text: &str) -> Result<(), String> {
    if is_flag(arg) {
        return Ok(());
    }
    let long = arg.get_long().unwrap_or_default();
    let args = match long {
        "interval" => ["workuum".into(), "watch".into(), format!("--{long}={text}")],
        _ => ["workuum".into(), "list".into(), format!("--{long}={text}")],
    };
    // environment variables must not judge the value
    let probe = command
        .clone()
        .mut_args(|arg| arg.env(None::<&str>))
        .mut_subcommand("watch", |watch| watch.mut_args(|arg| arg.env(None::<&str>)));
    probe.try_get_matches_from(args).map(drop).map_err(|error| {
        std::error::Error::source(&error).map_or_else(|| error.to_string(), ToString::to_string)
    })
}

/// The strings a saved value stands for, each valid for the option's parser.
pub fn texts(command: &Command, arg: &Arg, value: &Value) -> Result<Vec<String>, String> {
    let (flag, list) = (is_flag(arg), is_list(arg));
    let texts: Vec<String> = match value {
        Value::Boolean(on) if flag => vec![on.to_string()],
        Value::String(text) if !flag && !list => vec![text.clone()],
        Value::Array(items) if list => items
            .iter()
            .map(|item| item.as_str().map(String::from))
            .collect::<Option<_>>()
            .ok_or("expected a list of strings")?,
        _ if flag => return Err(format!("expected true or false, found {value}")),
        _ if list => return Err(format!("expected a list of strings, found {value}")),
        _ => return Err(format!("expected a string, found {value}")),
    };
    texts
        .iter()
        .try_for_each(|text| check(command, arg, text))?;
    Ok(texts)
}

impl Settings {
    pub fn load(path: &Path) -> Result<Settings, String> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string(self).map_err(io::Error::other)?;
        write_atomic(path, text.as_bytes())
    }

    /// The named session, or `default` when it is saved and no name is given.
    pub fn select(&self, name: Option<&str>) -> Result<Option<(&String, &Session)>, String> {
        let Some(name) = name else {
            return Ok(self.sessions.get_key_value("default"));
        };
        if let Some(found) = self.sessions.get_key_value(name) {
            return Ok(Some(found));
        }
        let saved = match self.sessions.is_empty() {
            true => "none saved".to_string(),
            false => format!(
                "saved sessions: {}",
                self.sessions.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        };
        Err(format!("unknown session '{name}'; {saved}"))
    }

    /// Makes the session's values the defaults of `command`, so flags and env vars still win.
    pub fn apply(
        &self,
        mut command: Command,
        name: Option<&str>,
        path: &Path,
    ) -> Result<Command, String> {
        let Some((name, session)) = self.select(name)? else {
            return Ok(command);
        };
        let mut built = command.clone();
        built.build();
        for (key, value) in session {
            let fail =
                |message: String| format!("{}: session '{name}': {key}: {message}", path.display());
            let arg = options(&built)
                .into_iter()
                .find(|arg| arg.get_long() == Some(key))
                .ok_or_else(|| fail("unknown option".into()))?;
            let mut texts = texts(&built, arg, value).map_err(fail)?;
            if key == "root" {
                let home = dirs::home_dir().ok_or_else(|| fail("no home directory".into()))?;
                for text in &mut texts {
                    match text.strip_prefix('~') {
                        Some("") => *text = home.display().to_string(),
                        Some(rest) if rest.starts_with(['/', '\\']) => {
                            *text = home.join(&rest[1..]).display().to_string();
                        }
                        _ => {}
                    }
                }
            }
            let id = arg.get_id().clone();
            // clap wants 'static defaults, and the process is one short run
            let defaults: Vec<&'static str> = texts.into_iter().map(|text| &*text.leak()).collect();
            command = match key.as_str() {
                "interval" => command.mut_subcommand("watch", |watch| {
                    watch.mut_arg(id, |arg| arg.default_values(defaults))
                }),
                _ => command.mut_arg(id, |arg| arg.default_values(defaults)),
            };
        }
        Ok(command)
    }
}
