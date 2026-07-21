//! Command-line interface for diagnostics and local tooling.
//!
//! Subcommands never open the network. `viewr update` only prints how to refresh
//! a local build; it does not download anything (see product privacy rules).

#![allow(unsafe_code)] // Windows AttachConsole / AllocConsole for CLI under GUI subsystem

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;
use std::{io, io::Write};

use crate::decode::DecodedImage;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Parsed invocation: either a GUI launch or a CLI subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// Open the GUI, optionally with an initial image path.
    Gui {
        /// Image path from the command line, if any.
        image: Option<PathBuf>,
    },
    /// Print help to stdout.
    Help,
    /// Print version to stdout.
    Version,
    /// Run local environment diagnostics.
    Doctor,
    /// Time decode on a corpus directory (or a generated temp set).
    Benchmark {
        /// Directory of images; if `None`, a small temp corpus is generated.
        dir: Option<PathBuf>,
    },
    /// Explain how to update without phoning home.
    Update,
}

/// Parse `std::env::args_os()`-style strings into an [`Invocation`].
///
/// # Errors
/// Returns a user-facing message when arguments are invalid.
pub fn parse_args<I, S>(args: I) -> Result<Invocation, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut args: Vec<PathBuf> = args
        .into_iter()
        .map(|s| PathBuf::from(s.as_ref()))
        .collect();
    // argv[0] is the program name.
    if !args.is_empty() {
        args.remove(0);
    }

    if args.is_empty() {
        return Ok(Invocation::Gui { image: None });
    }

    let first = args[0].to_string_lossy();
    match first.as_ref() {
        "help" | "--help" | "-h" | "/?" => Ok(Invocation::Help),
        "version" | "--version" | "-V" => Ok(Invocation::Version),
        "doctor" => Ok(Invocation::Doctor),
        "update" => Ok(Invocation::Update),
        "benchmark" | "bench" => {
            let dir = args.get(1).cloned();
            Ok(Invocation::Benchmark { dir })
        }
        "open" => {
            let image = args.get(1).cloned();
            if image.is_none() {
                return Err("usage: viewr open <path>".into());
            }
            Ok(Invocation::Gui { image })
        }
        // Flags that are not paths.
        s if s.starts_with('-') => {
            Err(format!("unknown option '{s}'. Try `viewr help` for usage."))
        }
        // Treat anything else as an image path (Open With / drag to shortcut).
        _ => Ok(Invocation::Gui {
            image: Some(args[0].clone()),
        }),
    }
}

/// Run a CLI subcommand. Returns a process exit code.
#[must_use]
pub fn run(inv: Invocation) -> ExitCode {
    let stdout = io::stdout();
    let stderr = io::stderr();
    let explicit_worker = std::env::var_os("VIEWR_DECODE_BIN");
    run_with_io(
        inv,
        &mut stdout.lock(),
        &mut stderr.lock(),
        explicit_worker.as_deref(),
    )
    .unwrap_or(ExitCode::FAILURE)
}

fn run_with_io(
    inv: Invocation,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    explicit_worker: Option<&OsStr>,
) -> io::Result<ExitCode> {
    match inv {
        Invocation::Gui { .. } => Ok(ExitCode::FAILURE), // caller should launch GUI
        Invocation::Help => {
            print_help(stdout)?;
            Ok(ExitCode::SUCCESS)
        }
        Invocation::Version => {
            writeln!(stdout, "viewr {VERSION}")?;
            Ok(ExitCode::SUCCESS)
        }
        Invocation::Doctor => {
            if doctor_to(stdout, explicit_worker)? {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::from(1))
            }
        }
        Invocation::Benchmark { dir } => match benchmark_to(dir.as_deref(), stdout, stderr) {
            Ok(()) => Ok(ExitCode::SUCCESS),
            Err(e) => {
                writeln!(stderr, "benchmark failed: {e}")?;
                Ok(ExitCode::from(1))
            }
        },
        Invocation::Update => {
            print_update(stdout)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn print_help(stdout: &mut impl Write) -> io::Result<()> {
    writeln!(
        stdout,
        "\
viewr {VERSION} - a photo viewer that just shows your photos.

Usage:
  viewr [path]                 Open the GUI (optional image path)
  viewr open <path>            Open the GUI on a path
  viewr doctor                 Local diagnostics (no network)
  viewr benchmark [dir]        Time decode on images in dir (or a tiny temp set)
  viewr update                 How to update (local only; never phones home)
  viewr help                   Show this help
  viewr version                Show version

Privacy:
  No network client, no telemetry, no activity log, no log files.
  Doctor and default benchmark are fully in-memory (zero temp files).
  `update` only prints build instructions (never downloads).

Examples:
  viewr photos\\IMG_001.jpg
  viewr doctor
  viewr benchmark corpus
"
    )
}

fn print_update(stdout: &mut impl Write) -> io::Result<()> {
    writeln!(
        stdout,
        "\
viewr update - local only

viewr never phones home and does not download updates for you.
That is intentional (see docs/PRIVACY.md and docs/ROADMAP.md).

From a source checkout:

  git pull
  cargo build --release --workspace

Keep the binaries side by side:

  target/release/viewr
  target/release/viewr-decode

Optional C-backed worker formats (needs system libraries):

  cargo build --release -p viewr-decode --features avif,heic

If you installed with cargo-dist or a manual copy, replace those files with a
fresh build. There is no background updater and no `viewr update --download`.
"
    )
}

/// Run diagnostics. Returns `true` if all critical checks passed.
#[must_use]
pub fn doctor() -> bool {
    let stdout = io::stdout();
    let explicit_worker = std::env::var_os("VIEWR_DECODE_BIN");
    doctor_to(&mut stdout.lock(), explicit_worker.as_deref()).unwrap_or(false)
}

fn doctor_to(stdout: &mut impl Write, explicit_worker: Option<&OsStr>) -> io::Result<bool> {
    writeln!(stdout, "viewr doctor {VERSION}")?;
    writeln!(stdout, "{}", "-".repeat(48))?;
    let mut ok = true;

    // --- binary layout ---
    match std::env::current_exe() {
        Ok(exe) => {
            writeln!(stdout, "[ok]   executable: {}", exe.display())?;
            let mut worker = exe.clone();
            worker.set_file_name(if cfg!(windows) {
                "viewr-decode.exe"
            } else {
                "viewr-decode"
            });
            if worker.is_file() {
                writeln!(stdout, "[ok]   worker beside exe: {}", worker.display())?;
            } else if let Some(explicit) = explicit_worker {
                let explicit = Path::new(explicit);
                if explicit.is_file() {
                    writeln!(
                        stdout,
                        "[ok]   worker via VIEWR_DECODE_BIN: {}",
                        explicit.display()
                    )?;
                } else {
                    writeln!(
                        stdout,
                        "[WARN] VIEWR_DECODE_BIN set but missing: {}",
                        explicit.display()
                    )?;
                    ok = false;
                }
            } else {
                writeln!(
                    stdout,
                    "[WARN] viewr-decode not found beside exe
       (AVIF/HEIC need: cargo build -p viewr-decode)"
                )?;
                // Not a hard failure: core pure-Rust formats still work.
            }
        }
        Err(e) => {
            writeln!(stdout, "[FAIL] cannot resolve current_exe: {e}")?;
            ok = false;
        }
    }

    writeln!(
        stdout,
        "[ok]   platform: {} / {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )?;
    writeln!(
        stdout,
        "[ok]   privacy: no network, no log files, doctor is in-memory (zero temp)"
    )?;

    // --- decode self-test ---
    match decode_self_test() {
        Ok((w, h, ms)) => {
            writeln!(stdout, "[ok]   decode self-test: {w}x{h} PNG in {ms:.2} ms")?;
        }
        Err(e) => {
            writeln!(stdout, "[FAIL] decode self-test: {e}")?;
            ok = false;
        }
    }

    // --- optional source-tree checks ---
    if Path::new("deny.toml").is_file() {
        writeln!(
            stdout,
            "[ok]   source tree: deny.toml present (run `cargo deny check` in CI)"
        )?;
    }
    if Path::new("docs/PRIVACY.md").is_file() {
        writeln!(stdout, "[ok]   source tree: docs/PRIVACY.md present")?;
    }

    writeln!(stdout, "{}", "-".repeat(48))?;
    if ok {
        writeln!(stdout, "doctor: critical checks passed")?;
    } else {
        writeln!(stdout, "doctor: one or more critical checks failed")?;
    }
    Ok(ok)
}

fn decode_self_test() -> Result<(u32, u32, f64), String> {
    // Fully in-memory: zero temp files, zero debris under %TEMP% / /tmp.
    let png = encode_png_memory(64, 48).map_err(|e| e.to_string())?;
    let start = Instant::now();
    let img = DecodedImage::load_from_memory(&png).map_err(|e| e.to_string())?;
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    Ok((img.width, img.height, ms))
}

/// Benchmark decode throughput.
///
/// When `dir` is `None`, runs an **in-memory** synthetic corpus (no temp files).
/// When `dir` is set, times real files the user pointed at (read-only).
pub fn benchmark(dir: Option<&Path>) -> Result<(), String> {
    let stdout = io::stdout();
    let stderr = io::stderr();
    benchmark_to(dir, &mut stdout.lock(), &mut stderr.lock())
}

fn benchmark_to(
    dir: Option<&Path>,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<(), String> {
    const ITERATIONS: u32 = 5;

    writeln!(
        stdout,
        "{:<28} {:>10} {:>12} {:>12}",
        "file", "pixels", "median ms", "MP/s"
    )
    .map_err(|e| e.to_string())?;
    writeln!(stdout, "{}", "-".repeat(64)).map_err(|e| e.to_string())?;

    if let Some(root) = dir {
        if !root.is_dir() {
            return Err("not a directory".into());
        }
        let mut entries: Vec<_> = std::fs::read_dir(root)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && crate::fs::is_supported_image(p))
            .collect();
        entries.sort();
        if entries.is_empty() {
            return Err("no supported images in the given directory".into());
        }
        for path in entries {
            bench_one_path(&path, ITERATIONS, stdout, stderr)?;
        }
    } else {
        writeln!(
            stdout,
            "benchmark: in-memory corpus (no temp files written)"
        )
        .map_err(|e| e.to_string())?;
        for (name, w, h) in [
            ("mem_a.png", 256u32, 256u32),
            ("mem_b.png", 640, 480),
            ("mem_c.png", 128, 128),
        ] {
            let png = encode_png_memory(w, h).map_err(|e| e.to_string())?;
            bench_one_bytes(name, &png, ITERATIONS, stdout, stderr)?;
        }
    }
    Ok(())
}

fn bench_one_path(
    path: &Path,
    iterations: u32,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<(), String> {
    let mut times = Vec::with_capacity(iterations as usize);
    let mut pixels = 0u64;
    for _ in 0..iterations {
        let start = Instant::now();
        match DecodedImage::load(path) {
            Ok(img) => {
                times.push(start.elapsed().as_secs_f64() * 1000.0);
                pixels = u64::from(img.width) * u64::from(img.height);
            }
            Err(e) => {
                let name = path
                    .file_name()
                    .map_or_else(|| "(file)".into(), |s| s.to_string_lossy().into_owned());
                writeln!(stderr, "skip {name}: {e}").map_err(|e| e.to_string())?;
                return Ok(());
            }
        }
    }
    print_bench_row(
        &path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        pixels,
        &times,
        stdout,
    )
    .map_err(|e| e.to_string())
}

fn bench_one_bytes(
    name: &str,
    bytes: &[u8],
    iterations: u32,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<(), String> {
    let mut times = Vec::with_capacity(iterations as usize);
    let mut pixels = 0u64;
    for _ in 0..iterations {
        let start = Instant::now();
        match DecodedImage::load_from_memory(bytes) {
            Ok(img) => {
                times.push(start.elapsed().as_secs_f64() * 1000.0);
                pixels = u64::from(img.width) * u64::from(img.height);
            }
            Err(e) => {
                writeln!(stderr, "skip {name}: {e}").map_err(|e| e.to_string())?;
                return Ok(());
            }
        }
    }
    print_bench_row(name, pixels, &times, stdout).map_err(|e| e.to_string())
}

fn print_bench_row(
    name: &str,
    pixels: u64,
    times: &[f64],
    stdout: &mut impl Write,
) -> io::Result<()> {
    let ms = median(times.to_vec());
    let mps = (pixels as f64 / 1_000_000.0) / (ms / 1000.0);
    writeln!(stdout, "{name:<28} {pixels:>10} {ms:>12.2} {mps:>12.1}")
}

/// Encode a synthetic RGB gradient PNG entirely in RAM (no disk).
fn encode_png_memory(w: u32, h: u32) -> Result<Vec<u8>, image::ImageError> {
    use std::io::Cursor;
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x % 255) as u8, (y % 255) as u8, 40])
    });
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(buf.into_inner())
}

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    samples[samples.len() / 2]
}

/// Ensure a console is attached so CLI output is visible on Windows GUI builds.
pub fn ensure_console() {
    #[cfg(windows)]
    {
        // SAFETY: Win32 console attach/alloc is process-global and called before
        // any multi-threaded CLI output.
        unsafe {
            use windows_sys::Win32::System::Console::{
                ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole,
            };
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                let _ = AllocConsole();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Invocation, benchmark_to, decode_self_test, median, parse_args, run_with_io};
    use crate::ephemeral::TempWorkspace;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;

    fn invoke(invocation: Invocation) -> (ExitCode, String, String) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_with_io(invocation, &mut stdout, &mut stderr, None).unwrap();
        (
            code,
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
        )
    }

    #[test]
    fn parse_empty_is_gui() {
        assert_eq!(
            parse_args(["viewr"]).unwrap(),
            Invocation::Gui { image: None }
        );
    }

    #[test]
    fn parse_help_and_doctor() {
        assert_eq!(parse_args(["viewr", "help"]).unwrap(), Invocation::Help);
        assert_eq!(parse_args(["viewr", "--help"]).unwrap(), Invocation::Help);
        assert_eq!(parse_args(["viewr", "doctor"]).unwrap(), Invocation::Doctor);
        assert_eq!(parse_args(["viewr", "update"]).unwrap(), Invocation::Update);
        assert_eq!(
            parse_args(["viewr", "version"]).unwrap(),
            Invocation::Version
        );
    }

    #[test]
    fn parse_benchmark_dir() {
        assert_eq!(
            parse_args(["viewr", "benchmark", "corpus"]).unwrap(),
            Invocation::Benchmark {
                dir: Some(PathBuf::from("corpus"))
            }
        );
        assert_eq!(
            parse_args(["viewr", "bench"]).unwrap(),
            Invocation::Benchmark { dir: None }
        );
    }

    #[test]
    fn parse_image_path() {
        assert_eq!(
            parse_args(["viewr", r"C:\photos\a.jpg"]).unwrap(),
            Invocation::Gui {
                image: Some(PathBuf::from(r"C:\photos\a.jpg"))
            }
        );
    }

    #[test]
    fn parse_open_requires_path() {
        assert!(parse_args(["viewr", "open"]).is_err());
        assert_eq!(
            parse_args(["viewr", "open", "x.png"]).unwrap(),
            Invocation::Gui {
                image: Some(PathBuf::from("x.png"))
            }
        );
    }

    #[test]
    fn parse_rejects_unknown_option() {
        let error = parse_args(["viewr", "--download"]).unwrap_err();
        assert!(error.contains("unknown option '--download'"));
    }

    #[test]
    fn static_commands_report_their_contract() {
        let (code, help, error) = invoke(Invocation::Help);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(error.is_empty());
        assert!(help.contains("Usage:"));
        assert!(help.contains("No network client"));

        let (code, version, error) = invoke(Invocation::Version);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(error.is_empty());
        assert!(version.starts_with("viewr "));

        let (code, update, error) = invoke(Invocation::Update);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(error.is_empty());
        assert!(update.contains("never phones home"));
        assert!(update.contains("There is no background updater"));

        let (code, output, error) = invoke(Invocation::Gui { image: None });
        assert_eq!(code, ExitCode::FAILURE);
        assert!(output.is_empty());
        assert!(error.is_empty());
    }

    #[test]
    fn doctor_runs_the_in_memory_decode_check() {
        let (width, height, elapsed_ms) = decode_self_test().unwrap();
        assert_eq!((width, height), (64, 48));
        assert!(elapsed_ms >= 0.0);

        let (code, output, error) = invoke(Invocation::Doctor);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(error.is_empty());
        assert!(output.contains("decode self-test: 64x48 PNG"));
        assert!(output.contains("doctor: critical checks passed"));
    }

    #[test]
    fn in_memory_benchmark_decodes_the_synthetic_corpus() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        benchmark_to(None, &mut stdout, &mut stderr).unwrap();

        let output = String::from_utf8(stdout).unwrap();
        assert!(stderr.is_empty());
        assert!(output.contains("in-memory corpus (no temp files written)"));
        assert!(output.contains("mem_a.png"));
        assert!(output.contains("mem_b.png"));
        assert!(output.contains("mem_c.png"));
    }

    #[test]
    fn directory_benchmark_reports_images_and_skips_bad_inputs() {
        let workspace = TempWorkspace::new("cli_benchmark_directory").unwrap();
        let good = workspace.path().join("good.png");
        image::RgbImage::from_pixel(4, 3, image::Rgb([1, 2, 3]))
            .save(&good)
            .unwrap();
        fs::write(workspace.path().join("bad.png"), b"not a PNG").unwrap();
        fs::write(workspace.path().join("ignored.txt"), b"not an image").unwrap();

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        benchmark_to(Some(workspace.path()), &mut stdout, &mut stderr).unwrap();

        let output = String::from_utf8(stdout).unwrap();
        let error = String::from_utf8(stderr).unwrap();
        assert!(output.contains("good.png"));
        assert!(!output.contains("ignored.txt"));
        assert!(error.contains("skip bad.png"));
    }

    #[test]
    fn directory_benchmark_rejects_missing_or_empty_directories() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            benchmark_to(
                Some(Path::new("definitely_missing_viewr_benchmark_dir")),
                &mut stdout,
                &mut stderr,
            )
            .unwrap_err(),
            "not a directory"
        );

        let workspace = TempWorkspace::new("cli_benchmark_empty").unwrap();
        assert_eq!(
            benchmark_to(Some(workspace.path()), &mut stdout, &mut stderr).unwrap_err(),
            "no supported images in the given directory"
        );

        let (code, output, error) = invoke(Invocation::Benchmark {
            dir: Some(PathBuf::from("definitely_missing_viewr_benchmark_dir")),
        });
        assert_eq!(code, ExitCode::from(1));
        assert!(output.contains("median ms"));
        assert!(error.contains("benchmark failed: not a directory"));
    }

    #[test]
    fn median_selects_the_middle_ordered_sample() {
        assert!((median(vec![9.0, 1.0, 5.0, 3.0, 7.0]) - 5.0).abs() < f64::EPSILON);
    }
}
