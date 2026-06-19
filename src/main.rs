mod application;
mod cli;
mod domain;
mod infrastructure;
mod support;

use clap::Parser;
use cli::Cli;
use support::{AppError, write_error, write_json};

fn main() {
    let cli = Cli::parse();
    let pretty = cli.pretty;

    match application::run(cli) {
        Ok(value) => {
            if let Err(err) = write_json(&value, pretty) {
                let app_err = AppError::new("OUTPUT_ERROR", err.to_string());
                let _ = write_error(&app_err, pretty);
                std::process::exit(app_err.exit_code);
            }
        }
        Err(err) => {
            let _ = write_error(&err, pretty);
            std::process::exit(err.exit_code);
        }
    }
}
