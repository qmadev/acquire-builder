use std::process::ExitCode;

use anyhow::Result;
use interpreter::Interpreter;

mod aes_stream;
mod interpreter;
mod pystandalone;

fn main() -> ExitCode {
    if let Err(e) = run() {
        eprintln!("{:#}", e);
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn run() -> Result<()> {
    let interpreter = Interpreter::init()?;
    interpreter.run()?;

    Ok(())
}
