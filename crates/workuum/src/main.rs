mod cli;
mod config;
mod pass;
mod report;
mod settings;
mod tui;
mod update;
mod watch;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches};

use cli::{Cli, Command, Options, PassCommand};
use config::Config;
use settings::Settings;

enum Exit {
    Usage(String),
    Failed(String),
}

fn run(args: Vec<OsString>) -> Result<(), Exit> {
    let matches = Cli::command().get_matches_from(&args);
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    if let Command::Update = cli.command {
        return update::run().map_err(Exit::Failed);
    }
    if let Command::Settings = cli.command {
        return tui::run(cli.session.as_deref()).map_err(Exit::Usage);
    }
    // settings and update ran above so a broken saved value never blocks fixing it
    let path = settings::path().map_err(Exit::Usage)?;
    let command = Settings::load(&path)
        .and_then(|saved| saved.apply(Cli::command(), cli.session.as_deref(), &path))
        .map_err(Exit::Usage)?;
    let matches = command.get_matches_from(&args);
    let options = Options::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    let pass = PassCommand::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    let config = Config::resolve(options).map_err(Exit::Failed)?;
    match pass {
        PassCommand::List { json, size } => {
            let rows = pass::list(&config, size).map_err(Exit::Failed)?;
            report::list(&rows, json, &config.home);
            Ok(())
        }
        PassCommand::Clean { dry_run } => {
            pass::clean(&config, dry_run).map_err(|error| Exit::Failed(error.to_string()))
        }
        PassCommand::Watch { interval } => watch::run(&config, interval).map_err(Exit::Failed),
    }
}

fn main() -> ExitCode {
    match run(std::env::args_os().collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Exit::Usage(message)) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
        Err(Exit::Failed(message)) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}
