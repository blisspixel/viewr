//! Binary entry point for viewr.
//!
//! GUI launches stay quiet (Windows GUI subsystem in release). CLI subcommands
//! attach a console so `doctor` / `help` / `benchmark` can print.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use viewr::cli::{self, Invocation};

fn main() -> ExitCode {
    // Maximum privacy default: no log output unless the user explicitly opts in.
    // Set `RUST_LOG` or `VIEWR_LOG` (e.g. `RUST_LOG=viewr=debug`) to enable stderr
    // diagnostics. Nothing is written to a log file.
    init_logging_opt_in();

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

/// Initialize `env_logger` only when the user asked for logs.
///
/// Default is silence: no activity log, no path leakage to stderr, no log files.
fn init_logging_opt_in() {
    let filter = std::env::var("RUST_LOG")
        .or_else(|_| std::env::var("VIEWR_LOG"))
        .ok();
    let Some(filter) = filter else {
        return;
    };
    let _ = env_logger::Builder::new().parse_filters(&filter).try_init();
}
