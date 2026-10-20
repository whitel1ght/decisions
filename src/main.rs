use std::process::ExitCode;

use clap::Parser;
use decisions::{cli, config};

fn main() -> ExitCode {
    let args = cli::Cli::parse();
    match config::load() {
        Ok(cfg) => cli::run(args, &cfg),
        Err(e) => {
            eprintln!("decisions: {e}");
            ExitCode::from(2)
        }
    }
}
