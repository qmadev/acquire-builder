use std::process::ExitCode;

use log::error;

mod cli;
mod compile;
mod http;
mod platform;
mod python;

use crate::cli::run;

fn main() -> ExitCode {
    if let Err(e) = run() {
        error!("{:?}", e);
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
