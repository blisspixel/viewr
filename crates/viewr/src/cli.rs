//! Command-line interface for diagnostics and local tooling.
//!
//! Subcommands never open the network. `viewr update` only prints the official
//! release and installer locations; it does not download anything.

#![allow(unsafe_code)] // Windows AttachConsole / AllocConsole for CLI under GUI subsystem

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;
use std::{io, io::Write};

use crate::decode::DecodedImage;

const VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const OFFICIAL_RELEASES_URL: &str = "https://github.com/blisspixel/viewr/releases";
pub(crate) const OFFICIAL_LATEST_RELEASE_URL: &str =
    "https://github.com/blisspixel/viewr/releases/latest";
pub(crate) const WINDOWS_INSTALL_COMMAND: &str =
    "irm https://github.com/blisspixel/viewr/releases/download/v0.5.0/install.ps1 | iex";
pub(crate) const UNIX_INSTALL_COMMAND: &str =
    "curl -fsSL https://github.com/blisspixel/viewr/releases/download/v0.5.0/install.sh | sh";

/// Which help screen the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpTopic {
    /// The command list and privacy summary.
    Overview,
    /// `viewr open`.
    Open,
    /// `viewr doctor`.
    Doctor,
    /// `viewr benchmark`.
    Benchmark,
    /// `viewr update`.
    Update,
    /// `viewr performance-probe`.
    PerformanceProbe,
}

impl HelpTopic {
    /// Resolve a `viewr help <topic>` argument.
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "help" => Some(Self::Overview),
            "open" => Some(Self::Open),
            "doctor" => Some(Self::Doctor),
            "benchmark" | "bench" => Some(Self::Benchmark),
            "update" => Some(Self::Update),
            "performance-probe" => Some(Self::PerformanceProbe),
            _ => None,
        }
    }
}

/// Parsed invocation: either a GUI launch or a CLI subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// Open the GUI, optionally with an initial image path.
    Gui {
        /// Image path from the command line, if any.
        image: Option<PathBuf>,
    },
    /// Print help for one command or the overview to stdout.
    Help(HelpTopic),
    /// Print version to stdout.
    Version,
    /// Run local environment diagnostics.
    Doctor,
    /// Time decode on a corpus directory (or a generated temp set).
    Benchmark {
        /// Directory of images; if `None`, a small temp corpus is generated.
        dir: Option<PathBuf>,
    },
    /// Run the explicit local GUI startup/navigation/memory probe, then exit.
    PerformanceProbe {
        /// Initial image whose containing folder supplies the probe corpus.
        image: PathBuf,
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
    parse_args_with(args, &|path| path.exists())
}

/// `--help` and `-h` are reserved on every command, so a stranger who adds them
/// to any subcommand reads help instead of running the command.
fn is_help_flag(argument: &str) -> bool {
    matches!(argument, "--help" | "-h" | "/?" | "help")
}

/// Decide whether an unrecognized first token was meant as a file or folder.
///
/// A token that exists, carries a separator, an extension, or an explicit
/// relative or absolute prefix is a path. Anything else is a mistyped command,
/// and opening a GUI on it would hide the mistake.
fn looks_like_path(path: &Path, exists: &dyn Fn(&Path) -> bool) -> bool {
    if exists(path) || path.is_absolute() || path.extension().is_some() {
        return true;
    }
    let text = path.to_string_lossy();
    text.starts_with('.')
        || text.starts_with('~')
        || text.contains('/')
        || text.contains('\\')
        || text.contains(':')
}

fn unexpected_argument(argument: &str, command: &str) -> String {
    if argument.starts_with('-') {
        format!("unknown option '{argument}' for `{command}`. Try `{command} --help`.")
    } else {
        format!("`{command}` takes no argument '{argument}'. Try `{command} --help`.")
    }
}

fn parse_args_with<I, S>(args: I, exists: &dyn Fn(&Path) -> bool) -> Result<Invocation, String>
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

    let first = args[0].to_string_lossy().into_owned();
    let rest: Vec<String> = args[1..]
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let asked_for_help = rest.first().is_some_and(|argument| is_help_flag(argument));

    match first.as_str() {
        "help" | "--help" | "-h" | "/?" => match rest.first() {
            None => Ok(Invocation::Help(HelpTopic::Overview)),
            Some(topic) => HelpTopic::from_name(topic).map(Invocation::Help).ok_or_else(|| {
                format!("`viewr help` has no topic '{topic}'. Try `viewr help` for the command list.")
            }),
        },
        "version" | "--version" | "-V" => Ok(Invocation::Version),
        "doctor" | "update" => {
            if asked_for_help {
                let topic = if first == "doctor" {
                    HelpTopic::Doctor
                } else {
                    HelpTopic::Update
                };
                return Ok(Invocation::Help(topic));
            }
            if let Some(extra) = rest.first() {
                return Err(unexpected_argument(extra, &format!("viewr {first}")));
            }
            if first == "doctor" {
                Ok(Invocation::Doctor)
            } else {
                Ok(Invocation::Update)
            }
        }
        "benchmark" | "bench" => {
            if asked_for_help {
                return Ok(Invocation::Help(HelpTopic::Benchmark));
            }
            if let Some(flag) = rest.first().filter(|argument| argument.starts_with('-')) {
                return Err(unexpected_argument(flag, "viewr benchmark"));
            }
            if rest.len() > 1 {
                return Err("usage: viewr benchmark [dir]. Try `viewr benchmark --help`.".into());
            }
            Ok(Invocation::Benchmark {
                dir: args.get(1).cloned(),
            })
        }
        "performance-probe" => {
            if asked_for_help {
                return Ok(Invocation::Help(HelpTopic::PerformanceProbe));
            }
            if args.len() != 2 {
                return Err(
                    "usage: viewr performance-probe <path>. Try `viewr performance-probe --help`."
                        .into(),
                );
            }
            Ok(Invocation::PerformanceProbe {
                image: args[1].clone(),
            })
        }
        "open" => {
            if asked_for_help {
                return Ok(Invocation::Help(HelpTopic::Open));
            }
            if let Some(flag) = rest.first().filter(|argument| argument.starts_with('-')) {
                return Err(unexpected_argument(flag, "viewr open"));
            }
            if rest.len() > 1 {
                return Err("usage: viewr open <path>. Try `viewr open --help`.".into());
            }
            let Some(image) = args.get(1).cloned() else {
                return Err("usage: viewr open <path>. Try `viewr open --help`.".into());
            };
            Ok(Invocation::Gui { image: Some(image) })
        }
        // Flags that are not paths.
        flag if flag.starts_with('-') => Err(format!(
            "unknown option '{flag}'. Try `viewr help` for usage."
        )),
        // Open With, drag-to-shortcut, and ordinary paths still open the GUI.
        _ if looks_like_path(&args[0], exists) => Ok(Invocation::Gui {
            image: Some(args[0].clone()),
        }),
        word => Err(format!(
            "'{word}' is not a viewr command, and no such file or folder exists. \
Try `viewr help` for usage, or `viewr open <path>` for a path."
        )),
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
        Invocation::Help(topic) => {
            write!(stdout, "{}", help_text(topic, example_image_path()))?;
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
        Invocation::PerformanceProbe { .. } => {
            writeln!(
                stderr,
                "performance probe must be launched by the viewr binary"
            )?;
            Ok(ExitCode::from(2))
        }
        Invocation::Update => {
            print_update(stdout)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// The example path in help uses the separator of the running platform.
fn example_image_path() -> &'static str {
    if cfg!(windows) {
        r"photos\IMG_001.jpg"
    } else {
        "photos/IMG_001.jpg"
    }
}

/// Complete text of one help screen, ending with a newline.
fn help_text(topic: HelpTopic, example_image: &str) -> String {
    match topic {
        HelpTopic::Overview => format!(
            "\
viewr {VERSION} - a photo viewer that just shows your photos.

Usage:
  viewr [file or folder]       Open the GUI (optional image file or folder)
  viewr open <file or folder>  Open the GUI on a file or folder
  viewr doctor                 Local diagnostics (no network)
  viewr benchmark [dir]        Time decode on images in dir (or a tiny temp set)
  viewr update                 Manual update guidance (no network request)
  viewr help [command]         Show this help, or help for one command
  viewr version                Show version

Add --help to any command, for example `viewr doctor --help`.

Privacy:
  No network client, no telemetry, no activity log, no log files.
  Doctor and default benchmark are fully in-memory (zero temp files).
  `update` only prints official manual install and build instructions (never downloads).

Examples:
  viewr {example_image}
  viewr doctor
  viewr benchmark corpus
"
        ),
        HelpTopic::Open => format!(
            "\
viewr open - open the GUI on a file or folder

Usage:
  viewr open <file or folder>

Identical to `viewr <file or folder>`, and the way to open a path that could be
read as a command name. A folder opens its first naturally sorted image.

Examples:
  viewr open {example_image}
  viewr open photos
"
        ),
        HelpTopic::Doctor => "\
viewr doctor - local diagnostics

Usage:
  viewr doctor

Reports binary placement, the decode worker and its in-memory IPC probe,
platform identity, privacy boundaries, an in-memory PNG decode self-test, and
the windowing prerequisites of the current desktop session.

Everything runs in memory: no network request, no temp file, no log file.
Exit status is 1 when a critical check fails, including a desktop session that
is missing a library viewr needs to open a window. Creating a GPU surface is
proven when viewr opens a window, not by doctor.
"
        .to_owned(),
        HelpTopic::Benchmark => "\
viewr benchmark - time local decoding

Usage:
  viewr benchmark              Time a small in-memory corpus (no temp files)
  viewr benchmark <dir>        Time supported images in <dir>, read-only

Prints file, pixel count, median milliseconds over five decodes, and megapixels
per second. Files in <dir> are only read. Nothing is written, cached on disk, or
sent anywhere.

Exit status is 1 when <dir> is not a directory or contains no supported image.
"
        .to_owned(),
        HelpTopic::Update => "\
viewr update - manual update guidance

Usage:
  viewr update

Prints the official release page and the installer command for this platform,
plus the source rebuild. viewr never checks for, downloads, or installs an
update by itself, and there is no `viewr update --download`.
"
        .to_owned(),
        HelpTopic::PerformanceProbe => format!(
            "\
viewr performance-probe - explicit local startup and navigation measurement

Usage:
  viewr performance-probe <path>

Opens the GUI, samples startup, navigation, and memory across the folder that
contains <path>, prints one JSON report, and exits. Maintainers and CI budgets
use this command; ordinary launches measure nothing and keep no report.

Example:
  viewr performance-probe {example_image}
"
        ),
    }
}

fn print_update(stdout: &mut impl Write) -> io::Result<()> {
    writeln!(
        stdout,
        "\
viewr update - manual and explicit

viewr never checks for or downloads updates by itself.
Official releases:

  {OFFICIAL_RELEASES_URL}

Run the installer again to update from a verified release archive.

Windows PowerShell:

  {WINDOWS_INSTALL_COMMAND}

macOS or Linux:

  {UNIX_INSTALL_COMMAND}

The installer contacts only the official GitHub repository after you run it and
verifies the release checksum and manifest. The viewr application remains
network-free and creates no background updater.

For a source checkout, close viewr and rebuild the updated checkout with:

  cargo build --release --workspace --locked

Keep the binaries side by side:

  target/release/viewr
  target/release/viewr-decode

Optional C-backed worker formats (needs system libraries):

  cargo build --release -p viewr-decode --features avif,heic

Keep both binaries from the same trusted release or build side by side.
There is no `viewr update --download`.
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

/// Window-presentation readiness for the current platform.
///
/// Linux asks the dynamic loader the same questions the windowing and graphics
/// stacks will ask later. Windows and macOS link their window systems, so
/// doctor reports what it can and does not claim a window was created.
fn window_readiness() -> crate::startup::WindowReadiness {
    crate::startup::host_window_readiness()
}

/// Report where the executable and its decode worker live.
///
/// Returns whether every critical layout check passed and whether a worker is
/// available for the IPC probe.
fn report_binary_layout(
    stdout: &mut impl Write,
    explicit_worker: Option<&OsStr>,
) -> io::Result<(bool, bool)> {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            writeln!(stdout, "[FAIL] cannot resolve current_exe: {e}")?;
            return Ok((false, false));
        }
    };
    writeln!(stdout, "[ok]   executable: {}", exe.display())?;

    let mut worker = exe;
    worker.set_file_name(if cfg!(windows) {
        "viewr-decode.exe"
    } else {
        "viewr-decode"
    });
    if worker.is_file() {
        writeln!(stdout, "[ok]   worker beside exe: {}", worker.display())?;
        return Ok((true, true));
    }
    if let Some(explicit) = explicit_worker {
        let explicit = Path::new(explicit);
        if explicit.is_file() {
            writeln!(
                stdout,
                "[ok]   worker via VIEWR_DECODE_BIN: {}",
                explicit.display()
            )?;
            return Ok((true, true));
        }
        writeln!(
            stdout,
            "[WARN] VIEWR_DECODE_BIN set but missing: {}",
            explicit.display()
        )?;
        return Ok((false, false));
    }
    // Not a hard failure: core pure-Rust formats still work.
    writeln!(
        stdout,
        "[WARN] viewr-decode not found beside exe
       Release archives ship bin/viewr and bin/viewr-decode together; keep the pair.
       Core formats still open. Optional AVIF and HEIC files report an error."
    )?;
    Ok((true, false))
}

fn doctor_to(stdout: &mut impl Write, explicit_worker: Option<&OsStr>) -> io::Result<bool> {
    writeln!(stdout, "viewr doctor {VERSION}")?;
    writeln!(stdout, "{}", "-".repeat(48))?;
    writeln!(stdout, "Binaries, decode, and privacy")?;

    let (mut ok, worker_available) = report_binary_layout(stdout, explicit_worker)?;

    if worker_available {
        match crate::sandbox::probe_worker() {
            Ok(()) => writeln!(stdout, "[ok]   worker IPC: bounded in-memory probe passed")?,
            Err(error) => {
                writeln!(stdout, "[FAIL] worker IPC: {error}")?;
                ok = false;
            }
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

    // --- window presentation ---
    let readiness = window_readiness();
    writeln!(stdout)?;
    writeln!(stdout, "Window presentation")?;
    for line in &readiness.lines {
        writeln!(stdout, "{line}")?;
    }
    if !readiness.critical_ok {
        ok = false;
    }

    writeln!(stdout, "{}", "-".repeat(48))?;
    if ok {
        writeln!(stdout, "doctor: critical checks passed")?;
        // The last line a stranger reads must not imply more than was proven.
        writeln!(
            stdout,
            "doctor: no window was opened here; run `viewr <path>` to see the picture or the exact failure"
        )?;
    } else {
        writeln!(stdout, "doctor: one or more critical checks failed")?;
        if !readiness.critical_ok {
            writeln!(
                stdout,
                "doctor: this host cannot open a viewr window until the item above is installed"
            )?;
        }
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
    use super::{
        HelpTopic, Invocation, OFFICIAL_LATEST_RELEASE_URL, OFFICIAL_RELEASES_URL,
        UNIX_INSTALL_COMMAND, WINDOWS_INSTALL_COMMAND, benchmark_to, decode_self_test, help_text,
        median, parse_args, parse_args_with, run_with_io, window_readiness,
    };
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
        assert_eq!(
            parse_args(["viewr", "help"]).unwrap(),
            Invocation::Help(HelpTopic::Overview)
        );
        assert_eq!(
            parse_args(["viewr", "--help"]).unwrap(),
            Invocation::Help(HelpTopic::Overview)
        );
        assert_eq!(parse_args(["viewr", "doctor"]).unwrap(), Invocation::Doctor);
        assert_eq!(parse_args(["viewr", "update"]).unwrap(), Invocation::Update);
        assert_eq!(
            parse_args(["viewr", "version"]).unwrap(),
            Invocation::Version
        );
    }

    #[test]
    fn every_command_reserves_its_own_help() {
        for (arguments, topic) in [
            (["viewr", "doctor", "--help"], HelpTopic::Doctor),
            (["viewr", "doctor", "-h"], HelpTopic::Doctor),
            (["viewr", "benchmark", "--help"], HelpTopic::Benchmark),
            (["viewr", "bench", "--help"], HelpTopic::Benchmark),
            (["viewr", "update", "--help"], HelpTopic::Update),
            (["viewr", "open", "--help"], HelpTopic::Open),
            (
                ["viewr", "performance-probe", "--help"],
                HelpTopic::PerformanceProbe,
            ),
            (["viewr", "help", "doctor"], HelpTopic::Doctor),
            (["viewr", "help", "open"], HelpTopic::Open),
        ] {
            assert_eq!(
                parse_args(arguments).unwrap(),
                Invocation::Help(topic),
                "{arguments:?}"
            );
        }

        let unknown_topic = parse_args(["viewr", "help", "frobnicate"]).unwrap_err();
        assert!(unknown_topic.contains("has no topic 'frobnicate'"));
    }

    #[test]
    fn commands_reject_arguments_they_do_not_accept() {
        for (arguments, expected) in [
            (
                ["viewr", "doctor", "--verbose"],
                "unknown option '--verbose' for `viewr doctor`",
            ),
            (
                ["viewr", "update", "--download"],
                "unknown option '--download' for `viewr update`",
            ),
            (
                ["viewr", "doctor", "corpus"],
                "`viewr doctor` takes no argument 'corpus'",
            ),
            (
                ["viewr", "benchmark", "--fast"],
                "unknown option '--fast' for `viewr benchmark`",
            ),
            (
                ["viewr", "open", "--force"],
                "unknown option '--force' for `viewr open`",
            ),
        ] {
            let error = parse_args(arguments).unwrap_err();
            assert!(error.contains(expected), "{arguments:?} produced {error}");
        }

        assert!(
            parse_args(["viewr", "benchmark", "corpus", "extra"])
                .unwrap_err()
                .contains("usage: viewr benchmark [dir]")
        );
        assert!(
            parse_args(["viewr", "open", "a.png", "b.png"])
                .unwrap_err()
                .contains("usage: viewr open <path>")
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
    fn parse_performance_probe_requires_exactly_one_path() {
        assert_eq!(
            parse_args(["viewr", "performance-probe", "image.png"]).unwrap(),
            Invocation::PerformanceProbe {
                image: PathBuf::from("image.png")
            }
        );
        assert!(parse_args(["viewr", "performance-probe"]).is_err());
        assert!(parse_args(["viewr", "performance-probe", "a.png", "b.png"]).is_err());
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
    fn a_mistyped_command_reports_itself_instead_of_opening_the_gui() {
        let missing = |_: &Path| false;
        for word in ["nosuch", "docter", "xyzzy"] {
            let error = parse_args_with(["viewr", word], &missing).unwrap_err();
            assert!(error.contains(&format!("'{word}' is not a viewr command")));
            assert!(error.contains("no such file or folder exists"));
        }

        // Paths still open the viewer, including files that vanished before the
        // handler ran, so Open With and drag-to-shortcut keep working.
        for path in [
            "photo.jpg",
            "./photos",
            "photos/holiday",
            r"photos\holiday",
            "~/photos",
        ] {
            assert_eq!(
                parse_args_with(["viewr", path], &missing).unwrap(),
                Invocation::Gui {
                    image: Some(PathBuf::from(path))
                },
                "{path}"
            );
        }

        // An extension-free name that exists on disk is a folder, not a typo.
        let existing = |path: &Path| path == Path::new("photos");
        assert_eq!(
            parse_args_with(["viewr", "photos"], &existing).unwrap(),
            Invocation::Gui {
                image: Some(PathBuf::from("photos"))
            }
        );
        assert!(parse_args_with(["viewr", "photoss"], &existing).is_err());
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
        let (code, help, error) = invoke(Invocation::Help(HelpTopic::Overview));
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(error.is_empty());
        assert!(help.contains("Usage:"));
        assert!(help.contains("No network client"));
        assert!(help.contains("Add --help to any command"));

        let (code, version, error) = invoke(Invocation::Version);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(error.is_empty());
        assert!(version.starts_with("viewr "));

        let (code, update, error) = invoke(Invocation::Update);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(error.is_empty());
        assert!(update.contains("never checks for or downloads updates"));
        assert!(update.contains(OFFICIAL_RELEASES_URL));
        assert!(OFFICIAL_LATEST_RELEASE_URL.starts_with(OFFICIAL_RELEASES_URL));
        assert!(update.contains(WINDOWS_INSTALL_COMMAND));
        assert!(update.contains(UNIX_INSTALL_COMMAND));
        assert!(WINDOWS_INSTALL_COMMAND.contains("/releases/download/v0.5.0/"));
        assert!(UNIX_INSTALL_COMMAND.contains("/releases/download/v0.5.0/"));
        assert!(!WINDOWS_INSTALL_COMMAND.contains("/main/"));
        assert!(!UNIX_INSTALL_COMMAND.contains("/main/"));
        assert!(update.contains("verifies the release checksum and manifest"));
        assert!(update.contains("cargo build --release --workspace --locked"));
        assert!(update.contains("creates no background updater"));
        assert!(!update.contains("git pull"));
        assert!(!update.contains("latest version"));

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

        let readiness = window_readiness();
        let (code, output, error) = invoke(Invocation::Doctor);
        assert!(error.is_empty());
        assert!(output.contains("Binaries, decode, and privacy"));
        assert!(output.contains("decode self-test: 64x48 PNG"));
        assert!(output.contains("Window presentation"));
        for line in &readiness.lines {
            assert!(output.contains(line), "missing doctor line: {line}");
        }
        // Doctor is honest about what it did not prove.
        assert!(output.contains("proven only when viewr opens a window"));
        if readiness.critical_ok {
            assert_eq!(code, ExitCode::SUCCESS);
            assert!(output.contains("doctor: critical checks passed"));
        } else {
            assert_eq!(code, ExitCode::from(1));
            assert!(output.contains("doctor: one or more critical checks failed"));
            assert!(output.contains("cannot open a viewr window"));
        }
    }

    #[test]
    fn every_help_screen_documents_its_command_on_this_platform() {
        for (topic, expected) in [
            (HelpTopic::Overview, "viewr [file or folder]"),
            (HelpTopic::Open, "viewr open <file or folder>"),
            (HelpTopic::Doctor, "viewr doctor - local diagnostics"),
            (HelpTopic::Benchmark, "viewr benchmark <dir>"),
            (HelpTopic::Update, "there is no `viewr update --download`"),
            (
                HelpTopic::PerformanceProbe,
                "viewr performance-probe <path>",
            ),
        ] {
            for example in ["photos/IMG_001.jpg", r"photos\IMG_001.jpg"] {
                let text = help_text(topic, example);
                assert!(text.contains(expected), "{topic:?} missing {expected}");
                assert!(text.ends_with('\n'));
            }
        }

        // The example path uses the separator of the platform reading it.
        let unix = help_text(HelpTopic::Overview, "photos/IMG_001.jpg");
        assert!(unix.contains("viewr photos/IMG_001.jpg"));
        assert!(!unix.contains(r"photos\IMG_001.jpg"));
        let windows = help_text(HelpTopic::Overview, r"photos\IMG_001.jpg");
        assert!(windows.contains(r"viewr photos\IMG_001.jpg"));

        // Doctor help states the boundary the report itself keeps.
        let doctor = help_text(HelpTopic::Doctor, "photos/IMG_001.jpg");
        assert!(doctor.contains("missing a library viewr needs to open a window"));
        assert!(doctor.contains("no network request, no temp file, no log file"));
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
