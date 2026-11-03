use std::thread;
use std::time::Duration;

use crate::config::Config;
use crate::pass::{self, Failure};
use crate::report;

pub fn run(config: &Config, interval: Duration) -> Result<(), String> {
    loop {
        match pass::clean(config, false) {
            Ok(()) => {}
            Err(Failure::Busy) => report::warn(&format!("{}, skipping this pass", Failure::Busy)),
            Err(error) => return Err(error.to_string()),
        }
        thread::sleep(interval);
    }
}
