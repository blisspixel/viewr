//! Binary entry point for viewr.
//!
//! GUI launches stay quiet (Windows GUI subsystem in release). CLI subcommands
//! attach a console so `doctor` / `help` / `benchmark` can print.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use viewr::cli::{self, Invocation};

fn main() -> ExitCode {
    let application_started = std::time::Instant::now();
    if let Err(error) = viewr::privacy::apply_startup_hardening() {
        cli::ensure_console();
        eprintln!("viewr: {error}");
        return ExitCode::FAILURE;
    }
    // Maximum privacy default: no log output unless the user explicitly opts in.
    // Set `RUST_LOG` or `VIEWR_LOG` (e.g. `RUST_LOG=viewr=debug`) to enable
    // viewr-owned stderr diagnostics. Dependency targets remain disabled so their
    // payloads cannot cross viewr's path-private logging boundary.
    init_logging_opt_in();

    let inv = match cli::parse_args(std::env::args_os()) {
        Ok(i) => i,
        Err(msg) => {
            cli::ensure_console();
            eprintln!("viewr: {msg}");
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
        Invocation::PerformanceProbe { image } => {
            cli::ensure_console();
            match viewr::app::run_performance_probe(image, application_started) {
                Ok(report) => {
                    println!("{}", report.to_json());
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("viewr performance probe: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        other => {
            cli::ensure_console();
            cli::run(other)
        }
    }
}

/// Initialize `env_logger` only when the user asked for viewr-owned logs.
///
/// Default is silence: no activity log, no path leakage to stderr, no log files.
fn init_logging_opt_in() {
    let rust_log = std::env::var("RUST_LOG").ok();
    let viewr_log = std::env::var("VIEWR_LOG").ok();
    let Some(logger) = build_viewr_logger(selected_logging_filter(
        rust_log.as_deref(),
        viewr_log.as_deref(),
    )) else {
        return;
    };
    let max_level = logger.max_level();
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(max_level);
    }
}

fn selected_logging_filter<'a>(
    rust_log: Option<&'a str>,
    viewr_log: Option<&'a str>,
) -> Option<&'a str> {
    rust_log.or(viewr_log)
}

fn build_viewr_logger(filter: Option<&str>) -> Option<ViewrOnlyLogger> {
    let filter = filter?;
    let mut builder = env_logger::Builder::new();
    configure_viewr_logger(&mut builder, filter).then(|| ViewrOnlyLogger::new(builder.build()))
}

struct ViewrOnlyLogger {
    inner: env_logger::Logger,
}

impl ViewrOnlyLogger {
    fn new(inner: env_logger::Logger) -> Self {
        Self { inner }
    }

    fn max_level(&self) -> log::LevelFilter {
        self.inner.filter()
    }
}

impl log::Log for ViewrOnlyLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        is_viewr_log_target(metadata.target()) && self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            self.inner.log(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

fn is_viewr_log_target(target: &str) -> bool {
    target == "viewr" || target.starts_with("viewr::")
}

/// Accept the documented bare level or `viewr=<level>` directive while ignoring
/// external module directives. A global level is interpreted as a viewr level,
/// never as permission for dependency logs.
fn requested_viewr_log_level(filter: &str) -> Option<log::LevelFilter> {
    filter
        .split(',')
        .filter_map(viewr_log_directive)
        .next_back()
}

fn viewr_log_directive(directive: &str) -> Option<log::LevelFilter> {
    let directive = directive.trim();
    let level = match directive.split_once('=') {
        Some((target, level)) if target.trim() == "viewr" => level.trim(),
        Some(_) => return None,
        None => directive,
    };
    level.parse().ok()
}

fn configure_viewr_logger(builder: &mut env_logger::Builder, filter: &str) -> bool {
    let Some(level) = requested_viewr_log_level(filter) else {
        return false;
    };
    builder
        .filter_level(log::LevelFilter::Off)
        .filter_module("viewr", level);
    true
}

#[cfg(test)]
mod tests {
    use super::{
        ViewrOnlyLogger, build_viewr_logger, configure_viewr_logger, requested_viewr_log_level,
        selected_logging_filter,
    };
    use log::{Level, LevelFilter, Log, Record};
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
    };

    #[derive(Clone, Default)]
    struct CapturedOutput(Arc<Mutex<Vec<u8>>>);

    impl Write for CapturedOutput {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("capture lock")
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn logging_requires_an_explicit_supported_filter() {
        assert_eq!(selected_logging_filter(None, None), None);
        assert!(build_viewr_logger(None).is_none());
        assert!(build_viewr_logger(Some("trash=trace")).is_none());

        assert_eq!(
            selected_logging_filter(Some("viewr=debug"), Some("info")),
            Some("viewr=debug")
        );
        assert_eq!(selected_logging_filter(None, Some("warn")), Some("warn"));
        assert!(build_viewr_logger(Some("info")).is_some());
    }

    #[test]
    fn log_configuration_accepts_only_viewr_levels() {
        assert_eq!(requested_viewr_log_level("info"), Some(LevelFilter::Info));
        assert_eq!(
            requested_viewr_log_level("warn,viewr=debug,trash=trace"),
            Some(LevelFilter::Debug)
        );
        assert_eq!(requested_viewr_log_level("trash=trace"), None);
        assert_eq!(requested_viewr_log_level("viewr=invalid"), None);
    }

    #[test]
    fn freedesktop_trash_dependency_paths_do_not_cross_logging_boundary() {
        let output = CapturedOutput::default();
        let captured = Arc::clone(&output.0);
        let mut builder = env_logger::Builder::new();
        assert!(configure_viewr_logger(
            &mut builder,
            "viewr=info,trash=trace"
        ));
        builder
            .format_timestamp(None)
            .target(env_logger::Target::Pipe(Box::new(output)));
        let logger = ViewrOnlyLogger::new(builder.build());

        logger.log(
            &Record::builder()
                .level(Level::Info)
                .target("viewr::curate")
                .args(format_args!("trash receipt unavailable"))
                .build(),
        );
        logger.log(
            &Record::builder()
                .level(Level::Warn)
                .target("trash::freedesktop")
                .args(format_args!(
                    "info file path is /home/private/.local/share/Trash/info/photo.trashinfo"
                ))
                .build(),
        );
        logger.log(
            &Record::builder()
                .level(Level::Warn)
                .target("viewr_untrusted")
                .args(format_args!("lookalike target leaked /home/private"))
                .build(),
        );

        let rendered = String::from_utf8(captured.lock().expect("capture lock").clone())
            .expect("logger output is UTF-8");
        assert!(rendered.contains("trash receipt unavailable"));
        assert!(!rendered.contains("lookalike target"));
        assert!(!rendered.contains("/home/private"));
        assert!(!rendered.contains("trash::freedesktop"));
    }
}
