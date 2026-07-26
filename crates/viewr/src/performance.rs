//! Explicit, local-only GUI performance probe measurements.
//!
//! The normal viewer never records performance data. The opt-in probe reports a
//! single machine-readable line to its caller and exits after exercising a
//! bounded set of navigation and thumbnail-cache states.

#![allow(unsafe_code)] // read-only OS process-memory counters

use std::io;
use std::time::Duration;

/// Measurements produced by one explicit GUI performance probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerformanceReport {
    /// Application-entry to a visible initialized window, in microseconds.
    pub window_ready_us: u64,
    /// Application-entry to the first presented image frame, in microseconds.
    pub first_pixel_us: u64,
    /// Slowest sampled navigation-to-present interval, in microseconds.
    pub max_navigation_us: u64,
    /// Redraw requests observed during the settled 500 ms idle window.
    pub idle_redraws: u64,
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
            peak_resident_bytes: 5,
            playlist_entries: 6,
            decoded_cache_entries: 7,
            decoded_cache_bytes: 9,
            thumbnail_texture_entries: 8,
        };
        assert_eq!(
            report.to_json(),
            "{\"window_ready_us\":1,\"first_pixel_us\":2,\"max_navigation_us\":3,\"idle_redraws\":4,\"peak_resident_bytes\":5,\"playlist_entries\":6,\"decoded_cache_entries\":7,\"decoded_cache_bytes\":9,\"thumbnail_texture_entries\":8}"
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
}
