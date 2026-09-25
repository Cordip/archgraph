#![forbid(unsafe_code)]

use archgraph::cli::{self, Cli};
use clap::{error::ErrorKind, Parser};
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    // Clap's default parse-error exit is 2. Reserve 2 exclusively for a
    // successfully compiled check that found architecture violations.
    let arguments = match Cli::try_parse() {
        Ok(arguments) => arguments,
        Err(error) => {
            let code = if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                0
            } else {
                1
            };
            let _ = error.print();
            return ExitCode::from(code);
        }
    };
    match cli::run(arguments).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(1)
        }
    }
}
