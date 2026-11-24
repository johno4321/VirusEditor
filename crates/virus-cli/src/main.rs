use std::process::ExitCode;

use clap::Parser;
use virus_cli::cli::Cli;

fn main() -> ExitCode {
    match virus_cli::commands::run(Cli::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}