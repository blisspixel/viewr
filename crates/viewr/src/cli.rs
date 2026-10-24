//! Command-line interface for diagnostics and local tooling.
//!
//! Subcommands never open the network. `viewr update` only prints the official
//! release and installer locations; it does not download anything.

#![allow(unsafe_code)] // Windows console attach, and the stdout width read used by doctor

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use crate::decode::DecodedImage;
use crate::locale::Language;
use crate::presentation::explain_open_failure;
use crate::startup::{DOCTOR_FAIL, DOCTOR_NOTE, DOCTOR_OK, DOCTOR_WARN};

const VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const OFFICIAL_RELEASES_URL: &str = "https://github.com/blisspixel/viewr/releases";
pub(crate) const OFFICIAL_LATEST_RELEASE_URL: &str =
    "https://github.com/blisspixel/viewr/releases/latest";
pub(crate) const WINDOWS_INSTALL_COMMAND: &str =
    "irm https://github.com/blisspixel/viewr/releases/download/v0.7.5/install.ps1 | iex";
pub(crate) const UNIX_INSTALL_COMMAND: &str =
    "curl -fsSL https://github.com/blisspixel/viewr/releases/download/v0.7.5/install.sh | sh";

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
        /// Print the decoder's own line after each skipped file.
        verbose: bool,
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

fn parse_benchmark(rest: &[String]) -> Result<Invocation, String> {
    let mut verbose = false;
    let mut dir = None;
    for argument in rest {
        if argument == "--verbose" {
            if verbose {
                return Err(unexpected_argument(argument, "viewr benchmark"));
            }
            verbose = true;
            continue;
        }
        if argument.starts_with('-') {
            return Err(unexpected_argument(argument, "viewr benchmark"));
        }
        if dir.is_some() {
            return Err(
                "usage: viewr benchmark [--verbose] [dir]. Try `viewr benchmark --help`.".into(),
            );
        }
        dir = Some(PathBuf::from(argument));
    }
    Ok(Invocation::Benchmark { dir, verbose })
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
            parse_benchmark(&rest)
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
    let mut stdout = stdout.lock();
    let style = ReportStyle::detect(&stdout);
    let benchmark_detail_color = benchmark_detail_color(&stderr);
    let explicit_worker = std::env::var_os("VIEWR_DECODE_BIN");
    run_with_io(
        inv,
        &mut stdout,
        &mut stderr.lock(),
        explicit_worker.as_deref(),
        style,
        benchmark_detail_color,
    )
    .unwrap_or(ExitCode::FAILURE)
}

fn run_with_io(
    inv: Invocation,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    explicit_worker: Option<&OsStr>,
    style: ReportStyle,
    benchmark_detail_color: bool,
) -> io::Result<ExitCode> {
    match inv {
        Invocation::Gui { .. } => Ok(ExitCode::FAILURE), // caller should launch GUI
        Invocation::Help(topic) => {
            let text = help_text(topic, example_image_path());
            write!(stdout, "{}", wrap_text(&text, style.width))?;
            Ok(ExitCode::SUCCESS)
        }
        Invocation::Version => {
            writeln!(stdout, "viewr {VERSION}")?;
            Ok(ExitCode::SUCCESS)
        }
        Invocation::Doctor => {
            if doctor_to(stdout, explicit_worker, style)? {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::from(1))
            }
        }
        Invocation::Benchmark { dir, verbose } => match benchmark_to(
            dir.as_deref(),
            stdout,
            stderr,
            verbose,
            benchmark_detail_color,
        ) {
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
read as a command name. A folder opens its most recently modified image first.

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
  viewr benchmark [--verbose] [dir]

  viewr benchmark              Time a small in-memory corpus (no temp files)
  viewr benchmark <dir>        Time supported images in <dir>, read-only
  viewr benchmark --verbose    Also print the decoder line for a skipped file

Prints file, pixel count, median milliseconds over five decodes, and megapixels
per second. A skipped file prints one short reason. `--verbose` adds the
decoder's own line on the next row, dimmed on a color terminal. Files in <dir>
are only read. Nothing is written, cached on disk, or sent anywhere.

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

/// How doctor and help present themselves.
///
/// Color is independent of width. A pipe is 80 columns and plain. A terminal
/// uses its measured width, or `COLUMNS` when that measurement is unavailable,
/// and colors tags unless `NO_COLOR` is non-empty or `TERM` is `dumb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReportStyle {
    width: usize,
    color: bool,
}

impl ReportStyle {
    #[cfg(test)]
    fn plain(width: usize) -> Self {
        Self {
            width,
            color: false,
        }
    }

    fn detect(stdout: &impl IsTerminal) -> Self {
        let terminal = stdout.is_terminal();
        style_for(
            terminal,
            no_color_disables(std::env::var_os("NO_COLOR").as_deref()),
            std::env::var("TERM").ok().as_deref(),
            std::env::var("COLUMNS").ok().as_deref(),
            terminal.then(console_columns).flatten(),
        )
    }
}

/// `NO_COLOR` disables color when it is present and not empty.
fn no_color_disables(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

fn parse_columns(columns: Option<&str>) -> Option<usize> {
    let width = columns?.parse::<usize>().ok()?;
    (20..=500).contains(&width).then_some(width)
}

/// Whether a benchmark skip may dim its decoder line. Color follows stderr, not stdout.
fn benchmark_detail_color(stderr: &impl IsTerminal) -> bool {
    style_for(
        stderr.is_terminal(),
        no_color_disables(std::env::var_os("NO_COLOR").as_deref()),
        std::env::var("TERM").ok().as_deref(),
        None,
        None,
    )
    .color
}

fn style_for(
    terminal: bool,
    no_color: bool,
    term: Option<&str>,
    columns: Option<&str>,
    measured: Option<usize>,
) -> ReportStyle {
    let color = terminal && !no_color && term != Some("dumb");
    let width = if terminal {
        measured
            .filter(|width| (20..=500).contains(width))
            .or_else(|| parse_columns(columns))
            .unwrap_or(80)
    } else {
        80
    };
    ReportStyle { width, color }
}

/// Visible console columns, when stdout is a real console.
///
/// Unix uses `COLUMNS` instead of a new ioctl. The Windows read stays beside
/// the existing console attach.
fn console_columns() -> Option<usize> {
    #[cfg(windows)]
    {
        // SAFETY: this reads the process stdout console rectangle. It does not
        // allocate a console or change its mode. A pipe or a closed handle
        // fails the call and the caller keeps the default width.
        unsafe {
            use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
            use windows_sys::Win32::System::Console::{
                CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle,
                STD_OUTPUT_HANDLE,
            };
            let handle = GetStdHandle(STD_OUTPUT_HANDLE);
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut info = std::mem::zeroed::<CONSOLE_SCREEN_BUFFER_INFO>();
            if GetConsoleScreenBufferInfo(handle, &raw mut info) == 0 {
                return None;
            }
            let width = i32::from(info.srWindow.Right) - i32::from(info.srWindow.Left) + 1;
            usize::try_from(width)
                .ok()
                .filter(|width| (20..=500).contains(width))
        }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

fn split_prefix(line: &str) -> (&str, &str, String) {
    for tag in [DOCTOR_OK, DOCTOR_WARN, DOCTOR_FAIL, DOCTOR_NOTE] {
        if let Some(body) = line.strip_prefix(tag) {
            return (tag, body, " ".repeat(tag.chars().count()));
        }
    }
    let indent_chars = line.chars().take_while(|ch| *ch == ' ').count();
    let indent_bytes = line
        .char_indices()
        .nth(indent_chars)
        .map_or(line.len(), |(index, _)| index);
    let indent = &line[..indent_bytes];
    (indent, &line[indent_bytes..], indent.to_owned())
}

/// Break one token so each piece fits `budget`, preferring a path separator.
fn split_overlong(token: &str, budget: usize) -> Vec<String> {
    let budget = budget.max(1);
    if token.chars().count() <= budget {
        return vec![token.to_owned()];
    }
    let mut parts = Vec::new();
    let mut rest = token;
    while rest.chars().count() > budget {
        let mut end = 0;
        let mut last_sep = None;
        let mut used = 0;
        for (index, ch) in rest.char_indices() {
            used += 1;
            if used > budget {
                break;
            }
            end = index + ch.len_utf8();
            if ch == '/' || ch == '\\' {
                last_sep = Some(end);
            }
        }
        if let Some(sep) = last_sep {
            end = sep;
        }
        if end == 0 {
            end = rest.chars().next().map_or(rest.len(), char::len_utf8);
        }
        parts.push(rest[..end].to_owned());
        rest = &rest[end..];
    }
    if !rest.is_empty() {
        parts.push(rest.to_owned());
    }
    parts
}

fn content_width(words: &[String]) -> usize {
    words.iter().map(|word| word.chars().count()).sum::<usize>() + words.len().saturating_sub(1)
}

/// Wrap one logical row. A tag or existing indent is the hanging indent.
fn wrap_logical_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let (prefix, body, continuation) = split_prefix(line);
    let mut pending: VecDeque<String> = body.split_whitespace().map(str::to_owned).collect();
    if pending.is_empty() {
        return vec![prefix.trim_end().to_owned()];
    }
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut words = Vec::new();
    let mut first = true;
    while let Some(word) = pending.pop_front() {
        let head_len = if first {
            prefix.chars().count()
        } else {
            continuation.chars().count()
        };
        let limit = width.saturating_sub(head_len).max(1);
        let gap = usize::from(!words.is_empty());
        let room = limit.saturating_sub(content_width(&words) + gap);
        if word.chars().count() <= room {
            words.push(word);
            continue;
        }
        if !words.is_empty() {
            lines.push(finish_line(
                if first { prefix } else { &continuation },
                &words,
            ));
            words.clear();
            first = false;
            pending.push_front(word);
            continue;
        }
        let mut pieces = split_overlong(&word, limit);
        let first_piece = pieces.remove(0);
        words.push(first_piece);
        for extra in pieces.into_iter().rev() {
            pending.push_front(extra);
        }
        let head = if first { prefix } else { continuation.as_str() };
        lines.push(finish_line(head, &words));
        words.clear();
        first = false;
    }
    if !words.is_empty() {
        let head = if first { prefix } else { continuation.as_str() };
        lines.push(finish_line(head, &words));
    }
    lines
}

fn finish_line(head: &str, words: &[String]) -> String {
    let text = words.join(" ");
    format!("{head}{text}")
}

/// Wrap a help screen at the write site. `help_text` itself stays unwrapped.
fn wrap_text(text: &str, width: usize) -> String {
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let logical = line.trim_end_matches(['\r', '\n']);
        for physical in wrap_logical_line(logical, width) {
            out.push_str(&physical);
            out.push('\n');
        }
    }
    out
}

/// Color only a status tag, after its padding spaces, so columns stay aligned.
fn paint_tag(line: &str) -> String {
    const ROLES: [(&str, &str); 4] = [
        (DOCTOR_OK, "32"),
        (DOCTOR_WARN, "33"),
        (DOCTOR_FAIL, "31"),
        (DOCTOR_NOTE, "2"),
    ];
    for (tag, code) in ROLES {
        if let Some(rest) = line.strip_prefix(tag) {
            return format!("\u{1b}[{code}m{tag}\u{1b}[0m{rest}");
        }
    }
    line.to_owned()
}

fn write_report_line(out: &mut impl Write, line: &str, style: ReportStyle) -> io::Result<()> {
    for physical in wrap_logical_line(line, style.width) {
        let painted = if style.color {
            paint_tag(&physical)
        } else {
            physical
        };
        writeln!(out, "{painted}")?;
    }
    Ok(())
}

fn count_tag_rows(lines: &[String], tag: &str) -> usize {
    lines.iter().filter(|line| line.starts_with(tag)).count()
}

/// Failure footer. The install sentence follows window `[fail]` rows only.
fn doctor_closing(
    passed: bool,
    critical: usize,
    warnings: usize,
    window_fails: usize,
) -> Vec<String> {
    if passed {
        return vec![
            "doctor: critical checks passed".to_owned(),
            "doctor: no window was opened here; run `viewr <path>` to see the picture or the exact failure"
                .to_owned(),
        ];
    }
    let checks = if critical == 1 {
        "1 critical check failed".to_owned()
    } else {
        format!("{critical} critical checks failed")
    };
    let warnings = if warnings == 1 {
        "1 warning".to_owned()
    } else {
        format!("{warnings} warnings")
    };
    let mut lines = vec![format!("doctor: {checks}, {warnings}")];
    let install = match window_fails {
        0 => None,
        1 => Some("doctor: this host cannot open a viewr window until the item above is installed"),
        _ => {
            Some("doctor: this host cannot open a viewr window until the items above are installed")
        }
    };
    if let Some(install) = install {
        lines.push(install.to_owned());
    }
    lines
}

struct DoctorReport<'a, W> {
    out: &'a mut W,
    style: ReportStyle,
    critical: usize,
    warnings: usize,
}

impl<W: Write> DoctorReport<'_, W> {
    fn plain(&mut self, line: &str) -> io::Result<()> {
        write_report_line(self.out, line, self.style)
    }

    fn row(&mut self, line: &str) -> io::Result<()> {
        if line.starts_with(DOCTOR_FAIL) {
            self.critical += 1;
        } else if line.starts_with(DOCTOR_WARN) {
            self.warnings += 1;
        }
        self.plain(line)
    }

    /// Count a warning that also fails doctor, then print its warning tag.
    fn failing_warning(&mut self, line: &str) -> io::Result<()> {
        self.critical += 1;
        self.row(line)
    }
}

/// Run diagnostics. Returns `true` if all critical checks passed.
#[must_use]
pub fn doctor() -> bool {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let style = ReportStyle::detect(&stdout);
    let explicit_worker = std::env::var_os("VIEWR_DECODE_BIN");
    doctor_to(&mut stdout, explicit_worker.as_deref(), style).unwrap_or(false)
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
fn report_binary_layout<W: Write>(
    report: &mut DoctorReport<'_, W>,
    explicit_worker: Option<&OsStr>,
) -> io::Result<(bool, bool)> {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            report.row(&format!("{DOCTOR_FAIL}cannot resolve current_exe: {e}"))?;
            return Ok((false, false));
        }
    };
    report.row(&format!("{DOCTOR_OK}executable: {}", exe.display()))?;

    let mut worker = exe;
    worker.set_file_name(if cfg!(windows) {
        "viewr-decode.exe"
    } else {
        "viewr-decode"
    });
    if worker.is_file() {
        report.row(&format!(
            "{DOCTOR_OK}worker beside exe: {}",
            worker.display()
        ))?;
        return Ok((true, true));
    }
    if let Some(explicit) = explicit_worker {
        let explicit = Path::new(explicit);
        if explicit.is_file() {
            report.row(&format!(
                "{DOCTOR_OK}worker via VIEWR_DECODE_BIN: {}",
                explicit.display()
            ))?;
            return Ok((true, true));
        }
        report.failing_warning(&format!(
            "{DOCTOR_WARN}VIEWR_DECODE_BIN set but missing: {}",
            explicit.display()
        ))?;
        return Ok((false, false));
    }
    // Not a hard failure: core pure-Rust formats still work.
    report.row(&format!("{DOCTOR_WARN}viewr-decode not found beside exe"))?;
    report.plain(
        "       Release archives ship bin/viewr and bin/viewr-decode together; keep the pair.",
    )?;
    report
        .plain("       Core formats still open. Optional AVIF and HEIC files report an error.")?;
    Ok((true, false))
}

fn doctor_to(
    stdout: &mut impl Write,
    explicit_worker: Option<&OsStr>,
    style: ReportStyle,
) -> io::Result<bool> {
    let mut report = DoctorReport {
        out: stdout,
        style,
        critical: 0,
        warnings: 0,
    };
    report.plain(&format!("viewr doctor {VERSION}"))?;
    report.plain(&"-".repeat(48))?;
    report.plain("Binaries, decode, and privacy")?;

    let (mut ok, worker_available) = report_binary_layout(&mut report, explicit_worker)?;

    if worker_available {
        match crate::sandbox::probe_worker() {
            Ok(()) => report.row(&format!(
                "{DOCTOR_OK}worker IPC: bounded in-memory probe passed"
            ))?,
            Err(error) => {
                report.row(&format!("{DOCTOR_FAIL}worker IPC: {error}"))?;
                ok = false;
            }
        }
    }

    report.row(&format!(
        "{DOCTOR_OK}platform: {} / {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    ))?;
    report.row(&format!(
        "{DOCTOR_OK}privacy: no network, no log files, doctor is in-memory (zero temp)"
    ))?;

    match decode_self_test() {
        Ok((w, h, ms)) => {
            report.row(&format!(
                "{DOCTOR_OK}decode self-test: {w}x{h} PNG in {ms:.2} ms, in memory, no network"
            ))?;
        }
        Err(e) => {
            report.row(&format!("{DOCTOR_FAIL}decode self-test: {e}"))?;
            ok = false;
        }
    }

    if Path::new("deny.toml").is_file() {
        report.row(&format!(
            "{DOCTOR_OK}source tree: deny.toml present (run `cargo deny check` in CI)"
        ))?;
    }
    if Path::new("docs/PRIVACY.md").is_file() {
        report.row(&format!("{DOCTOR_OK}source tree: docs/PRIVACY.md present"))?;
    }

    let readiness = window_readiness();
    let window_fails = count_tag_rows(&readiness.lines, DOCTOR_FAIL);
    report.plain("")?;
    report.plain("Window presentation")?;
    for line in &readiness.lines {
        report.row(line)?;
    }
    if !readiness.critical_ok {
        ok = false;
    }
    if !ok && report.critical == 0 {
        report.critical = 1;
    }

    report.plain(&"-".repeat(48))?;
    for line in doctor_closing(ok, report.critical, report.warnings, window_fails) {
        report.plain(&line)?;
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
    let mut stderr = stderr.lock();
    let color_details = benchmark_detail_color(&stderr);
    benchmark_to(dir, &mut stdout.lock(), &mut stderr, false, color_details)
}

fn benchmark_to(
    dir: Option<&Path>,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    verbose: bool,
    color_details: bool,
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
            bench_one_path(&path, ITERATIONS, stdout, stderr, verbose, color_details)?;
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
            bench_one_bytes(
                name,
                &png,
                ITERATIONS,
                stdout,
                stderr,
                verbose,
                color_details,
            )?;
        }
    }
    Ok(())
}

fn bench_one_path(
    path: &Path,
    iterations: u32,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    verbose: bool,
    color_details: bool,
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
            Err(error) => {
                let name = path
                    .file_name()
                    .map_or_else(|| "(file)".into(), |s| s.to_string_lossy().into_owned());
                write_benchmark_skip(stderr, &name, &error, verbose, color_details)?;
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
    verbose: bool,
    color_details: bool,
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
            Err(error) => {
                write_benchmark_skip(stderr, name, &error, verbose, color_details)?;
                return Ok(());
            }
        }
    }
    print_bench_row(name, pixels, &times, stdout).map_err(|e| e.to_string())
}

/// Print one short skip reason. `--verbose` adds the decoder line, dimmed only
/// on a color terminal. The reason line itself stays plain.
fn write_benchmark_skip(
    stderr: &mut impl Write,
    name: &str,
    error: &impl std::fmt::Display,
    verbose: bool,
    color_details: bool,
) -> Result<(), String> {
    let failure = explain_open_failure(Language::English, error);
    let reason = benchmark_reason(&failure.sentence);
    writeln!(stderr, "skip {name}: {reason}").map_err(|error| error.to_string())?;
    if verbose {
        let detail = error.to_string();
        if color_details {
            writeln!(stderr, "\u{1b}[2m{detail}\u{1b}[0m").map_err(|error| error.to_string())?;
        } else {
            writeln!(stderr, "{detail}").map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// Lowercase the first character and drop one trailing period.
fn benchmark_reason(sentence: &str) -> String {
    let sentence = sentence.trim();
    let mut chars = sentence.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut reason: String = first.to_lowercase().collect();
    reason.push_str(chars.as_str());
    if reason.ends_with('.') {
        reason.pop();
    }
    reason
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
        HelpTopic, Invocation, OFFICIAL_LATEST_RELEASE_URL, OFFICIAL_RELEASES_URL, ReportStyle,
        UNIX_INSTALL_COMMAND, WINDOWS_INSTALL_COMMAND, benchmark_to, console_columns,
        count_tag_rows, decode_self_test, doctor_closing, doctor_to, help_text, median,
        no_color_disables, paint_tag, parse_args, parse_args_with, run_with_io, style_for,
        window_readiness, wrap_logical_line, wrap_text,
    };
    use crate::ephemeral::TempWorkspace;
    use crate::startup::DOCTOR_FAIL;
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;

    fn invoke(invocation: Invocation) -> (ExitCode, String, String) {
        invoke_with(invocation, ReportStyle::plain(80))
    }

    fn invoke_with(invocation: Invocation, style: ReportStyle) -> (ExitCode, String, String) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_with_io(invocation, &mut stdout, &mut stderr, None, style, false).unwrap();
        (
            code,
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
        )
    }

    fn flattened(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn strip_ansi(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\u{1b}' {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for next in chars.by_ref() {
                        if next.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                continue;
            }
            out.push(ch);
        }
        out
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
                .contains("usage: viewr benchmark [--verbose] [dir]")
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
                dir: Some(PathBuf::from("corpus")),
                verbose: false,
            }
        );
        assert_eq!(
            parse_args(["viewr", "bench"]).unwrap(),
            Invocation::Benchmark {
                dir: None,
                verbose: false,
            }
        );
        assert_eq!(
            parse_args(["viewr", "benchmark", "--verbose", "corpus"]).unwrap(),
            Invocation::Benchmark {
                dir: Some(PathBuf::from("corpus")),
                verbose: true,
            }
        );
        assert_eq!(
            parse_args(["viewr", "benchmark", "corpus", "--verbose"]).unwrap(),
            Invocation::Benchmark {
                dir: Some(PathBuf::from("corpus")),
                verbose: true,
            }
        );
        assert!(
            parse_args(["viewr", "benchmark", "--verbose", "--verbose"])
                .unwrap_err()
                .contains("unknown option '--verbose'")
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
        assert_eq!(error, "");
        assert!(help.contains("Usage:"));
        assert!(help.contains("No network client"));
        assert!(help.contains("Add --help to any command"));

        let (code, version, error) = invoke(Invocation::Version);
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(error, "");
        assert!(version.starts_with("viewr "));

        let (code, update, error) = invoke(Invocation::Update);
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(error, "");
        assert!(update.contains("never checks for or downloads updates"));
        assert!(update.contains(OFFICIAL_RELEASES_URL));
        assert!(OFFICIAL_LATEST_RELEASE_URL.starts_with(OFFICIAL_RELEASES_URL));
        assert!(update.contains(WINDOWS_INSTALL_COMMAND));
        assert!(update.contains(UNIX_INSTALL_COMMAND));
        assert!(WINDOWS_INSTALL_COMMAND.contains("/releases/download/v0.7.5/"));
        assert!(UNIX_INSTALL_COMMAND.contains("/releases/download/v0.7.5/"));
        assert!(!WINDOWS_INSTALL_COMMAND.contains("/main/"));
        assert!(!UNIX_INSTALL_COMMAND.contains("/main/"));
        assert!(update.contains("verifies the release checksum and manifest"));
        assert!(update.contains("cargo build --release --workspace --locked"));
        assert!(update.contains("creates no background updater"));
        assert!(!update.contains("git pull"));
        assert!(!update.contains("latest version"));

        let (code, output, error) = invoke(Invocation::Gui { image: None });
        assert_eq!(code, ExitCode::FAILURE);
        assert_eq!(output, "");
        assert_eq!(error, "");
    }

    #[test]
    fn doctor_runs_the_in_memory_decode_check() {
        let (width, height, elapsed_ms) = decode_self_test().unwrap();
        assert_eq!((width, height), (64, 48));
        assert!(elapsed_ms >= 0.0);

        let readiness = window_readiness();
        let (code, output, error) = invoke(Invocation::Doctor);
        assert_eq!(error, "");
        assert!(output.contains("Binaries, decode, and privacy"));
        assert!(output.contains("decode self-test: 64x48 PNG"));
        assert!(
            output.contains(" ms, in memory, no network"),
            "doctor self-test should name memory and no network: {output}"
        );
        assert!(output.contains("Window presentation"));
        assert!(!output.contains("[FAIL]"));
        assert!(!output.contains("[WARN]"));
        assert!(!output.contains('\u{1b}'));
        let flat = flattened(&output);
        for line in &readiness.lines {
            assert!(
                flat.contains(&flattened(line)),
                "missing doctor line: {line}"
            );
        }
        // Doctor is honest about what it did not prove.
        assert!(flat.contains("proven only when viewr opens a window"));
        if readiness.critical_ok {
            assert_eq!(code, ExitCode::SUCCESS);
            assert!(output.contains("doctor: critical checks passed"));
        } else {
            assert_eq!(code, ExitCode::from(1));
            assert!(output.contains("critical check"));
            assert!(output.contains("cannot open a viewr window"));
            let window_fails = count_tag_rows(&readiness.lines, DOCTOR_FAIL);
            if window_fails == 1 {
                assert!(output.contains("the item above is installed"));
            } else if window_fails > 1 {
                assert!(output.contains("the items above are installed"));
            }
        }
    }

    #[test]
    fn narrow_doctor_and_help_keep_the_hanging_indent() {
        let source = help_text(HelpTopic::Overview, "photos/IMG_001.jpg");
        assert!(
            source.lines().any(|line| line.chars().count() > 60),
            "help_text stays unwrapped so the write site owns wrapping"
        );
        let (code, help, error) = invoke_with(
            Invocation::Help(HelpTopic::Overview),
            ReportStyle::plain(60),
        );
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(error, "");
        assert!(!help.contains('\u{1b}'));
        for line in help.lines() {
            assert!(line.chars().count() <= 60, "{line}");
        }
        assert!(help.lines().any(|line| line.starts_with("  `update`")));
        assert!(
            help.lines()
                .filter(|line| {
                    line.contains("downloads")
                        || line.contains("instructions")
                        || line.contains("manual")
                })
                .all(|line| line.starts_with("  "))
        );

        let (_code, doctor, doctor_error) = invoke_with(Invocation::Doctor, ReportStyle::plain(60));
        assert_eq!(doctor_error, "");
        assert!(!doctor.contains('\u{1b}'));
        for line in doctor.lines() {
            assert!(line.chars().count() <= 60, "{line}");
        }
        assert!(doctor.contains("GPU surface") || doctor.contains("not by doctor"));
        assert!(flattened(&doctor).contains("not by doctor"));
    }

    #[test]
    fn a_long_path_breaks_on_a_separator_under_the_tag() {
        let path = format!("C:\\{}", ["photos"].repeat(12).join("\\"));
        let line = format!("[ok]   executable: {path}");
        let wrapped = wrap_logical_line(&line, 60);
        assert!(wrapped.len() > 1);
        assert!(wrapped.iter().all(|row| row.chars().count() <= 60));
        assert!(wrapped[0].starts_with("[ok]   executable:"));
        assert!(wrapped[1].starts_with("       "));
        assert!(
            wrapped
                .iter()
                .any(|row| row.ends_with('\\') || row.contains("photos"))
        );
    }

    #[test]
    fn the_gpu_explanation_hangs_under_the_failure_tag() {
        let rows = [
            "[fail] gpu runtime: no usable Vulkan or OpenGL runtime, so no GPU surface can be created",
            "       (EGL absent, Vulkan loader present without a driver)",
            "       OpenGL through EGL:",
            "         Debian or Ubuntu: sudo apt install libegl1 libegl-mesa0",
        ];
        let mut rendered = String::new();
        for row in rows {
            for physical in wrap_logical_line(row, 60) {
                rendered.push_str(&physical);
                rendered.push('\n');
            }
        }
        for line in rendered.lines() {
            assert!(line.chars().count() <= 60, "{line}");
        }
        let lines: Vec<_> = rendered.lines().collect();
        assert!(lines[0].starts_with("[fail] gpu runtime:"));
        assert!(lines[1].starts_with("       "));
        assert!(
            lines
                .iter()
                .any(|line| line.trim_start().starts_with("(EGL absent,"))
        );
        assert!(
            lines.iter().any(
                |line| line.starts_with("         Debian") || line.starts_with("         sudo")
            )
        );
    }

    #[test]
    fn colored_tags_cover_only_the_padded_tag() {
        let rows = [
            "[ok]   platform: windows / x86_64",
            "[warn] viewr-decode not found beside exe",
            "[fail] decode self-test: boom",
            "[note] a GPU surface is proven only when viewr opens a window, not by doctor",
            "       continuation keeps the remedy indent",
        ];
        let mut plain = String::new();
        let mut colored = String::new();
        for row in rows {
            for physical in wrap_logical_line(row, 60) {
                plain.push_str(&physical);
                plain.push('\n');
                colored.push_str(&paint_tag(&physical));
                colored.push('\n');
            }
        }
        assert_eq!(strip_ansi(&colored), plain);
        assert!(colored.contains("\u{1b}[32m[ok]   \u{1b}[0mplatform"));
        assert!(colored.contains("\u{1b}[33m[warn] \u{1b}[0mviewr-decode"));
        assert!(colored.contains("\u{1b}[31m[fail] \u{1b}[0mdecode"));
        assert!(colored.contains("\u{1b}[2m[note] \u{1b}[0m"));
        let plain_colored = strip_ansi(&colored);
        assert_eq!(plain_colored.find("platform"), Some(7));
        let fail_at = plain_colored.find("[fail] ").unwrap();
        assert_eq!(plain_colored[fail_at..].find("decode self-test"), Some(7));
        for line in colored.lines() {
            if !line.starts_with('\u{1b}') {
                assert!(!line.contains('\u{1b}'), "{line}");
            }
        }
    }

    #[test]
    fn a_pipe_and_no_color_are_plain_and_match_at_the_same_width() {
        assert!(no_color_disables(Some(OsStr::new("1"))));
        assert!(!no_color_disables(Some(OsStr::new(""))));
        assert!(!no_color_disables(None));

        let piped = style_for(false, false, Some("xterm-256color"), Some("60"), Some(60));
        let no_color = style_for(true, true, Some("xterm-256color"), None, None);
        let dumb = style_for(true, false, Some("dumb"), Some("60"), None);
        let colored = style_for(true, false, Some("xterm-256color"), None, Some(60));
        assert_eq!(
            piped,
            ReportStyle {
                width: 80,
                color: false
            }
        );
        assert_eq!(
            no_color,
            ReportStyle {
                width: 80,
                color: false
            }
        );
        assert_eq!(piped, no_color);
        assert!(!dumb.color);
        assert_eq!(dumb.width, 60);
        assert!(colored.color);
        assert_eq!(colored.width, 60);
        assert_eq!(
            style_for(true, false, Some("xterm-256color"), Some("40"), Some(100)).width,
            100
        );
        assert_eq!(style_for(true, false, None, Some("10"), None).width, 80);
        assert_eq!(style_for(true, false, None, Some("900"), None).width, 80);
        assert_eq!(style_for(true, false, None, Some("nope"), None).width, 80);

        let fixture = "[fail] gpu runtime: no usable Vulkan or OpenGL runtime, so no GPU surface can be created\n";
        assert_eq!(
            wrap_text(fixture, piped.width),
            wrap_text(fixture, no_color.width)
        );
        assert!(!wrap_text(fixture, piped.width).contains('\u{1b}'));
        let painted: String = wrap_logical_line(fixture.trim_end(), colored.width)
            .into_iter()
            .map(|line| paint_tag(&line))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(painted.contains('\u{1b}'));
        assert_eq!(
            strip_ansi(&painted),
            wrap_text(fixture, colored.width).trim_end()
        );

        if let Some(width) = console_columns() {
            assert!((20..=500).contains(&width));
        }
    }

    #[test]
    fn the_footer_counts_checks_and_matches_the_install_sentence() {
        let two = doctor_closing(false, 2, 1, 2);
        let two = two.join("\n");
        assert!(two.contains("2 critical checks failed, 1 warning"));
        assert!(two.contains("items above are installed"));
        assert!(!two.contains("item above is"));

        let one = doctor_closing(false, 1, 0, 1).join("\n");
        assert!(one.contains("1 critical check failed, 0 warnings"));
        assert!(one.contains("the item above is installed"));
        assert!(!one.contains("items above"));

        let worker = doctor_closing(false, 1, 1, 0);
        assert_eq!(
            worker,
            vec!["doctor: 1 critical check failed, 1 warning".to_owned()]
        );

        let passed = doctor_closing(true, 0, 1, 0);
        assert_eq!(passed[0], "doctor: critical checks passed");
        assert!(passed[1].contains("no window was opened here"));

        let rows = [
            "[fail] windowing library: libxkbcommon-x11.so.0 is missing".to_owned(),
            "       Debian or Ubuntu: sudo apt install libxkbcommon-x11-0".to_owned(),
            "[fail] gpu runtime: no usable Vulkan or OpenGL runtime, so no GPU surface can be created"
                .to_owned(),
            "       (EGL absent, Vulkan absent)".to_owned(),
            "[warn] display session: none".to_owned(),
        ];
        assert_eq!(count_tag_rows(&rows, "[fail] "), 2);
        assert_eq!(count_tag_rows(&rows, "[warn] "), 1);
    }

    #[test]
    fn a_missing_explicit_worker_fails_and_is_counted() {
        let mut stdout = Vec::new();
        let ok = doctor_to(
            &mut stdout,
            Some(OsStr::new("definitely_missing_viewr_decode_bin")),
            ReportStyle::plain(60),
        )
        .unwrap();
        assert!(!ok);
        let output = String::from_utf8(stdout).unwrap();
        assert!(output.contains("[warn] VIEWR_DECODE_BIN set but missing"));
        assert!(output.contains("critical check"));
        assert!(output.contains("warning"));
        assert!(!output.contains("[WARN]"));
        assert!(!output.contains("[FAIL]"));
        for line in output.lines() {
            assert!(line.chars().count() <= 60, "{line}");
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
        benchmark_to(None, &mut stdout, &mut stderr, false, false).unwrap();

        let output = String::from_utf8(stdout).unwrap();
        assert_eq!(stderr, [] as [u8; 0]);
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
        benchmark_to(
            Some(workspace.path()),
            &mut stdout,
            &mut stderr,
            false,
            false,
        )
        .unwrap();

        let output = String::from_utf8(stdout).unwrap();
        let error = String::from_utf8(stderr).unwrap();
        assert!(output.contains("good.png"));
        assert!(!output.contains("ignored.txt"));
        assert_eq!(
            error.trim(),
            "skip bad.png: the file is damaged or incomplete",
            "{error}"
        );

        let mut verbose_out = Vec::new();
        let mut verbose_err = Vec::new();
        benchmark_to(
            Some(workspace.path()),
            &mut verbose_out,
            &mut verbose_err,
            true,
            false,
        )
        .unwrap();
        let verbose = String::from_utf8(verbose_err).unwrap();
        let mut lines = verbose.lines();
        assert_eq!(
            lines.next(),
            Some("skip bad.png: the file is damaged or incomplete"),
            "{verbose}"
        );
        let raw = lines.next().unwrap_or("");
        assert!(
            raw.contains("Format error"),
            "verbose skip hid the decoder line: {verbose}"
        );
        assert!(!verbose.contains('\u{1b}'), "{verbose}");

        let mut colored_err = Vec::new();
        benchmark_to(
            Some(workspace.path()),
            &mut verbose_out,
            &mut colored_err,
            true,
            true,
        )
        .unwrap();
        let colored = String::from_utf8(colored_err).unwrap();
        assert_eq!(
            colored.lines().next(),
            Some("skip bad.png: the file is damaged or incomplete"),
            "{colored}"
        );
        assert!(colored.contains("\u{1b}[2m"), "{colored}");
        assert!(colored.contains("\u{1b}[0m"), "{colored}");
        assert!(
            !colored.lines().next().unwrap_or("").contains('\u{1b}'),
            "the skip reason was colored: {colored}"
        );
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
                false,
                false,
            )
            .unwrap_err(),
            "not a directory"
        );

        let workspace = TempWorkspace::new("cli_benchmark_empty").unwrap();
        assert_eq!(
            benchmark_to(
                Some(workspace.path()),
                &mut stdout,
                &mut stderr,
                false,
                false
            )
            .unwrap_err(),
            "no supported images in the given directory"
        );

        let (code, output, error) = invoke(Invocation::Benchmark {
            dir: Some(PathBuf::from("definitely_missing_viewr_benchmark_dir")),
            verbose: false,
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
