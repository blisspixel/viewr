//! Explicit, local-only GUI performance probe measurements.
//!
//! The normal viewer never records performance data. The opt-in probe reports a
//! single machine-readable line to its caller and exits after exercising a
//! bounded set of navigation and thumbnail-cache states.

#![allow(unsafe_code)] // read-only OS process-memory counters

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use winit::event_loop::EventLoopProxy;

pub(crate) const PERFORMANCE_PROBE_TIMEOUT: Duration = Duration::from_mins(1);
pub(crate) const PERFORMANCE_IDLE_OBSERVATION: Duration = Duration::from_millis(500);

/// Measurements produced by one explicit GUI performance probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerformanceReport {
    /// Application-entry to a visible initialized window, in microseconds.
    pub window_ready_us: u64,
    /// Application-entry to the first presented image frame, in microseconds.
    pub first_pixel_us: u64,
    /// Slowest sampled navigation-to-present interval, in microseconds.
    pub max_navigation_us: u64,
    /// Delivered redraw events observed during the settled 500 ms idle window.
    pub idle_redraws: u64,
    /// Non-redraw window events delivered during the idle window.
    pub idle_non_redraw_events: u64,
    /// Event-driven egui repaint requests issued during the idle window.
    pub idle_event_repaint_requests: u64,
    /// Scheduled egui repaint deadlines that requested redraw during the idle window.
    pub idle_scheduled_egui_repaints: u64,
    /// Whether the window had focus when the idle window completed.
    pub idle_window_focused: bool,
    /// Whether egui reported a pointer inside the window when idle completed.
    pub idle_pointer_inside: bool,
    /// Peak resident set observed after caches settled, in bytes.
    pub peak_resident_bytes: u64,
    /// Number of image paths discovered in the containing folder.
    pub playlist_entries: usize,
    /// Number of full decoded neighbor images retained by the LRU.
    pub decoded_cache_entries: usize,
    /// Total decoded RGBA bytes retained by the neighbor LRU.
    pub decoded_cache_bytes: u64,
    /// Number of uploaded folder-preview textures retained at completion.
    pub thumbnail_texture_entries: usize,
}

impl PerformanceReport {
    /// Return a stable one-line JSON object for the dependency-free CI gate.
    #[must_use]
    pub fn to_json(&self) -> String {
        format!(
            concat!(
                "{{\"window_ready_us\":{},",
                "\"first_pixel_us\":{},",
                "\"max_navigation_us\":{},",
                "\"idle_redraws\":{},",
                "\"idle_non_redraw_events\":{},",
                "\"idle_event_repaint_requests\":{},",
                "\"idle_scheduled_egui_repaints\":{},",
                "\"idle_window_focused\":{},",
                "\"idle_pointer_inside\":{},",
                "\"peak_resident_bytes\":{},",
                "\"playlist_entries\":{},",
                "\"decoded_cache_entries\":{},",
                "\"decoded_cache_bytes\":{},",
                "\"thumbnail_texture_entries\":{}}}"
            ),
            self.window_ready_us,
            self.first_pixel_us,
            self.max_navigation_us,
            self.idle_redraws,
            self.idle_non_redraw_events,
            self.idle_event_repaint_requests,
            self.idle_scheduled_egui_repaints,
            self.idle_window_focused,
            self.idle_pointer_inside,
            self.peak_resident_bytes,
            self.playlist_entries,
            self.decoded_cache_entries,
            self.decoded_cache_bytes,
            self.thumbnail_texture_entries,
        )
    }
}

pub(crate) fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// Read the process peak resident set without adding a general system-inspection
/// dependency. This is called only by the explicit performance probe.
#[cfg(windows)]
pub(crate) fn peak_resident_bytes() -> io::Result<u64> {
    use std::mem::{size_of, zeroed};

    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: GetCurrentProcess returns a valid pseudo-handle for this process.
    // The initialized structure and exact byte size remain valid for the call.
    let mut counters = unsafe { zeroed::<PROCESS_MEMORY_COUNTERS_EX>() };
    counters.cb = u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS_EX>())
        .expect("PROCESS_MEMORY_COUNTERS_EX size fits u32");
    // SAFETY: the API accepts the common prefix pointer and the size above tells
    // Windows the concrete extended structure available to write.
    let succeeded = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
    };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        u64::try_from(counters.PeakWorkingSetSize)
            .map_err(|_| io::Error::other("peak working set does not fit u64"))
    }
}

#[cfg(unix)]
pub(crate) fn peak_resident_bytes() -> io::Result<u64> {
    // SAFETY: `usage` points to writable storage for the exact structure expected
    // by getrusage, and RUSAGE_SELF requires no external process capability.
    let mut usage = unsafe { std::mem::zeroed::<libc::rusage>() };
    // SAFETY: arguments satisfy getrusage's contract described above.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &raw mut usage) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let raw = u64::try_from(usage.ru_maxrss)
        .map_err(|_| io::Error::other("peak resident set was negative"))?;
    #[cfg(target_os = "macos")]
    {
        Ok(raw)
    }
    #[cfg(not(target_os = "macos"))]
    {
        raw.checked_mul(1024)
            .ok_or_else(|| io::Error::other("peak resident set overflowed bytes"))
    }
}

pub(crate) struct PerformanceProbe {
    pub(crate) started_at: Instant,
    pub(crate) deadline: Instant,
    pub(crate) window_ready: Option<Duration>,
    pub(crate) first_pixel: Option<Duration>,
    pub(crate) max_navigation: Duration,
    pub(crate) navigation_started: Option<Instant>,
    pub(crate) navigation_target: Option<PathBuf>,
    pub(crate) navigation_targets: Option<VecDeque<usize>>,
    pub(crate) last_presented_path: Option<PathBuf>,
    pub(crate) idle_until: Option<Instant>,
    pub(crate) idle_redraws: u64,
    pub(crate) idle_non_redraw_events: u64,
    pub(crate) idle_event_repaint_requests: u64,
    pub(crate) idle_scheduled_egui_repaints: u64,
    pub(crate) peak_resident_bytes: u64,
    pub(crate) outcome: Option<Result<PerformanceReport, String>>,
}

impl PerformanceProbe {
    pub(crate) fn new(started_at: Instant) -> Self {
        Self {
            started_at,
            deadline: started_at + PERFORMANCE_PROBE_TIMEOUT,
            window_ready: None,
            first_pixel: None,
            max_navigation: Duration::ZERO,
            navigation_started: None,
            navigation_target: None,
            navigation_targets: None,
            last_presented_path: None,
            idle_until: None,
            idle_redraws: 0,
            idle_non_redraw_events: 0,
            idle_event_repaint_requests: 0,
            idle_scheduled_egui_repaints: 0,
            peak_resident_bytes: 0,
            outcome: None,
        }
    }

    pub(crate) fn record_window_ready(&mut self, now: Instant) {
        self.window_ready.get_or_insert(now - self.started_at);
    }

    pub(crate) fn record_presented_image(&mut self, path: &Path, now: Instant) {
        self.first_pixel.get_or_insert(now - self.started_at);
        self.last_presented_path = Some(path.to_owned());
        if self.navigation_target.as_deref() == Some(path)
            && let Some(started) = self.navigation_started.take()
        {
            self.max_navigation = self.max_navigation.max(now - started);
            self.navigation_target = None;
        }
    }

    pub(crate) fn reset_idle_observation(&mut self) {
        self.idle_until = None;
        self.idle_redraws = 0;
        self.idle_non_redraw_events = 0;
        self.idle_event_repaint_requests = 0;
        self.idle_scheduled_egui_repaints = 0;
    }
}

pub(crate) fn schedule_performance_wake(
    event_proxy: EventLoopProxy<crate::app::UserEvent>,
    thread_name: &str,
    deadline: Instant,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name(thread_name.into())
        .spawn(move || {
            std::thread::park_timeout(deadline.saturating_duration_since(Instant::now()));
            let _ = event_proxy.send_event(crate::app::UserEvent::Wake);
        })
        .map(|_| ())
        .map_err(|error| format!("could not start performance timer: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_json_is_stable_and_machine_readable() {
        let report = PerformanceReport {
            window_ready_us: 1,
            first_pixel_us: 2,
            max_navigation_us: 3,
            idle_redraws: 4,
            idle_non_redraw_events: 10,
            idle_event_repaint_requests: 11,
            idle_scheduled_egui_repaints: 12,
            idle_window_focused: true,
            idle_pointer_inside: false,
            peak_resident_bytes: 5,
            playlist_entries: 6,
            decoded_cache_entries: 7,
            decoded_cache_bytes: 9,
            thumbnail_texture_entries: 8,
        };
        assert_eq!(
            report.to_json(),
            "{\"window_ready_us\":1,\"first_pixel_us\":2,\"max_navigation_us\":3,\"idle_redraws\":4,\"idle_non_redraw_events\":10,\"idle_event_repaint_requests\":11,\"idle_scheduled_egui_repaints\":12,\"idle_window_focused\":true,\"idle_pointer_inside\":false,\"peak_resident_bytes\":5,\"playlist_entries\":6,\"decoded_cache_entries\":7,\"decoded_cache_bytes\":9,\"thumbnail_texture_entries\":8}"
        );
    }

    #[test]
    fn duration_conversion_preserves_microseconds() {
        assert_eq!(duration_us(Duration::from_micros(42)), 42);
    }

    #[test]
    fn process_peak_resident_set_is_available() {
        assert!(peak_resident_bytes().is_ok_and(|bytes| bytes > 0));
    }

    #[test]
    fn probe_records_first_events_once_and_navigation_latency() {
        let started = Instant::now();
        let mut probe = PerformanceProbe::new(started);
        probe.record_window_ready(started + Duration::from_millis(5));
        probe.record_window_ready(started + Duration::from_millis(8));
        assert_eq!(probe.window_ready, Some(Duration::from_millis(5)));

        let target = PathBuf::from("next.png");
        probe.navigation_started = Some(started + Duration::from_millis(10));
        probe.navigation_target = Some(target.clone());
        probe.record_presented_image(&target, started + Duration::from_millis(17));
        probe.record_presented_image(Path::new("later.png"), started + Duration::from_millis(30));

        assert_eq!(probe.first_pixel, Some(Duration::from_millis(17)));
        assert_eq!(probe.max_navigation, Duration::from_millis(7));
        assert_eq!(probe.last_presented_path, Some(PathBuf::from("later.png")));
        assert!(probe.navigation_started.is_none());
        assert!(probe.navigation_target.is_none());
    }

    #[test]
    fn probe_reset_clears_only_idle_observation() {
        let mut probe = PerformanceProbe::new(Instant::now());
        probe.idle_until = Some(Instant::now());
        probe.idle_redraws = 9;
        probe.idle_non_redraw_events = 8;
        probe.idle_event_repaint_requests = 7;
        probe.idle_scheduled_egui_repaints = 6;
        probe.peak_resident_bytes = 42;

        probe.reset_idle_observation();

        assert!(probe.idle_until.is_none());
        assert_eq!(probe.idle_redraws, 0);
        assert_eq!(probe.idle_non_redraw_events, 0);
        assert_eq!(probe.idle_event_repaint_requests, 0);
        assert_eq!(probe.idle_scheduled_egui_repaints, 0);
        assert_eq!(probe.peak_resident_bytes, 42);
    }
}
