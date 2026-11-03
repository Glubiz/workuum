mod cli;
mod config;
mod pass;
mod report;
mod watch;

use std::process::ExitCode;

use clap::Parser;

use cli::{Cli, Command};
use config::Config;

fn run(cli: Cli) -> Result<(), String> {
    let config = Config::resolve(cli.options)?;
    match cli.command {
        Command::List { json, size } => {
            let rows = pass::list(&config, size)?;
            report::list(&rows, json, &config.home);
            Ok(())
        }
        Command::Clean { dry_run } => {
            pass::clean(&config, dry_run).map_err(|error| error.to_string())
        }
        Command::Watch { interval } => watch::run(&config, interval),
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}
