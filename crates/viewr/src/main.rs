//! Binary entry point for viewr.
//!
//! GUI launches stay quiet (Windows GUI subsystem in release). CLI subcommands
//! attach a console so `doctor` / `help` / `benchmark` can print.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use viewr::cli::{self, Invocation};

fn main() -> ExitCode {
    // Respect RUST_LOG if set; otherwise stay quiet for GUI. CLI commands still
    // print their own structured output on stdout.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let inv = match cli::parse_args(std::env::args_os()) {
        Ok(i) => i,
        Err(msg) => {
            cli::ensure_console();
            eprintln!("viewr: {msg}");
            eprintln!("Try `viewr help`.");
            return ExitCode::from(2);
        }
    };

    match inv {
        Invocation::Gui { image } => match viewr::app::run_with_image(image) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                cli::ensure_console();
                eprintln!("viewr: {e}");
                ExitCode::FAILURE
            }
        },
        other => {
            cli::ensure_console();
            cli::run(other)
        }
    }
}
