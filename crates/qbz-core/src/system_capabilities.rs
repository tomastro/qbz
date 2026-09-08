//! System capability detection.
//!
//! Probes the host environment at startup to derive a runtime profile that
//! tunes resource-heavy behaviors (prefetch depth, streaming buffer size,
//! prefetch quality cap) for memory-constrained machines like the
//! Raspberry Pi 3B (1 GB RAM, issue #331).
//!
//! Detection is one-shot, cached in a `OnceLock`, and pure once given the
//! `/proc/meminfo` contents — making it trivial to test by passing
//! synthetic input.

use std::sync::OnceLock;

/// Memory class bucket the runtime adapts behavior to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryClass {
    /// >= 2 GB RAM. Default behavior, no caps applied.
    Normal,
    /// < 2 GB RAM. Reduces prefetch and buffer footprints to keep room
    /// for the WebView and avoid swap thrash on Raspberry Pi-class
    /// devices.
    LowMemory,
}

/// Derived runtime profile applied to memory-sensitive subsystems.
#[derive(Debug, Clone, Copy)]
pub struct MemoryProfile {
    pub class: MemoryClass,
    pub mem_total_kb: u64,
    /// How many upcoming Qobuz tracks to prefetch. Hi-Res tracks are
    /// ~60 MB each held in memory, so this is the dominant source of
    /// RSS growth during normal playback.
    pub prefetch_count: usize,
    /// Maximum allowed initial streaming buffer size in bytes. Caps the
    /// dynamic-buffer growth that `from_speed_mbps` would otherwise
    /// inflate to 2 MB on slow connections — exactly the wrong direction
    /// on a memory-pressured Pi where slow downloads are themselves a
    /// symptom of swap thrash.
    pub max_initial_buffer_bytes: usize,
    /// Concurrency cap for prefetch downloads.
    pub max_concurrent_prefetch: usize,
    /// When false, prefetch downgrades from HiRes/UltraHiRes to Lossless
    /// (44.1 kHz / 16-bit FLAC) so each cached track stays under ~15 MB
    /// instead of ~60 MB.
    pub allow_hires_prefetch: bool,
    /// Upper bound for the L1 (in-memory) audio cache. The default
    /// 400 MB cap is sized for Normal-class desktops; on a Pi 3B (1 GB
    /// total) that single subsystem could consume 40 % of RAM, which
    /// guarantees swap thrash before the watchdog can react.
    pub audio_cache_l1_max_bytes: usize,
}

impl MemoryProfile {
    /// Derive the profile from a total-memory figure (KB).
    fn from_total_kb(mem_total_kb: u64) -> Self {
        // Threshold: 2 GiB. Anything with at least 2 GB physical RAM is
        // assumed to have enough headroom for the WebView (~150 MB) plus
        // 5 cached HiRes tracks (~300 MB) plus typical app overhead.
        const NORMAL_FLOOR_KB: u64 = 2 * 1024 * 1024;

        if mem_total_kb >= NORMAL_FLOOR_KB {
            Self {
                class: MemoryClass::Normal,
                mem_total_kb,
                prefetch_count: 5,
                max_initial_buffer_bytes: 2 * 1024 * 1024,
                max_concurrent_prefetch: 2,
                allow_hires_prefetch: true,
                audio_cache_l1_max_bytes: 400 * 1024 * 1024,
            }
        } else {
            Self {
                class: MemoryClass::LowMemory,
                mem_total_kb,
                prefetch_count: 1,
                max_initial_buffer_bytes: 256 * 1024,
                max_concurrent_prefetch: 1,
                allow_hires_prefetch: false,
                audio_cache_l1_max_bytes: 50 * 1024 * 1024,
            }
        }
    }
}

/// Parse the `MemTotal:` line out of `/proc/meminfo` content.
/// Returns None if the field is missing or unparseable.
pub fn parse_meminfo_total_kb(content: &str) -> Option<u64> {
    parse_meminfo_field_kb(content, "MemTotal:")
}

/// Parse the `MemAvailable:` line out of `/proc/meminfo` content.
/// Returns None if the field is missing or unparseable.
///
/// `MemAvailable` is the kernel's estimate of how much memory can be
/// allocated to a new workload without swapping — i.e. the right
/// metric for memory pressure (more accurate than `MemFree`, which
/// doesn't account for reclaimable page cache).
pub fn parse_meminfo_available_kb(content: &str) -> Option<u64> {
    parse_meminfo_field_kb(content, "MemAvailable:")
}

/// Shared parser for `<Field>: <number> kB` style /proc/meminfo lines.
fn parse_meminfo_field_kb(content: &str, field_prefix: &str) -> Option<u64> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix(field_prefix) {
            let kb_str = rest.split_whitespace().next()?;
            return kb_str.parse::<u64>().ok();
        }
    }
    None
}

/// Snapshot of current memory pressure relative to the host's
/// MemoryProfile, used by the runtime memory watchdog to decide
/// whether to evict caches and abort prefetches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryPressure {
    pub mem_available_kb: u64,
    pub mem_total_kb: u64,
    /// Available memory as a percentage of total (0.0–100.0).
    pub available_pct: f64,
    /// True when available memory falls below 15 % of total.
    pub is_low: bool,
    /// True when available memory falls below 5 % of total. The
    /// watchdog should drop caches at this point.
    pub is_critical: bool,
}

/// Build a pressure snapshot from raw figures.
///
/// Pure for testing; the real entry point is [`read_memory_pressure`].
pub fn pressure_from_figures(mem_available_kb: u64, mem_total_kb: u64) -> MemoryPressure {
    let total = mem_total_kb.max(1);
    let pct = (mem_available_kb as f64 / total as f64) * 100.0;
    MemoryPressure {
        mem_available_kb,
        mem_total_kb,
        available_pct: pct,
        is_low: pct < 15.0,
        is_critical: pct < 5.0,
    }
}

/// Windows' answer to `/proc/meminfo`: total and available physical memory,
/// both in KiB, from `GlobalMemoryStatusEx`.
///
/// W14. Without this the profile falls back to the top tier and
/// `read_memory_pressure` returns `None` forever, so the memory watchdog is
/// blind on Windows and the prefetch/buffer tiers never step down on a small
/// machine. `ullAvailPhys` is the closest analogue to `MemAvailable`: both
/// mean "what a new allocation can expect without swapping".
#[cfg(windows)]
fn global_memory_status_kb() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::SystemInformation::{
        GlobalMemoryStatusEx, MEMORYSTATUSEX,
    };

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..unsafe { std::mem::zeroed() }
    };

    // SAFETY: `status` is a live, correctly sized MEMORYSTATUSEX whose
    // `dwLength` is set as the API requires; the call only writes into it.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    if ok == 0 {
        return None;
    }

    // The fields are bytes; the rest of this module speaks KiB.
    Some((status.ullTotalPhys / 1024, status.ullAvailPhys / 1024))
}

/// Read the platform's memory figures and return a pressure snapshot relative
/// to the detected MemoryProfile. Returns None on platforms with neither
/// `/proc/meminfo` nor `GlobalMemoryStatusEx`, or when the source is
/// unreadable.
pub fn read_memory_pressure() -> Option<MemoryPressure> {
    #[cfg(windows)]
    {
        let (_total_kb, avail_kb) = global_memory_status_kb()?;
        let profile = memory_profile();
        return Some(pressure_from_figures(avail_kb, profile.mem_total_kb));
    }

    #[cfg(not(windows))]
    {
        let content = std::fs::read_to_string("/proc/meminfo").ok()?;
        let mem_available_kb = parse_meminfo_available_kb(&content)?;
        let profile = memory_profile();
        Some(pressure_from_figures(mem_available_kb, profile.mem_total_kb))
    }
}

/// Pure detection given `/proc/meminfo` content. Falls back to Normal
/// when MemTotal is missing or unparseable so we never accidentally
/// throttle a system whose meminfo we couldn't read.
pub fn detect_profile_from_meminfo(content: &str) -> MemoryProfile {
    parse_meminfo_total_kb(content)
        .map(MemoryProfile::from_total_kb)
        .unwrap_or_else(|| MemoryProfile::from_total_kb(u64::MAX))
}

/// Read `/proc/meminfo` and derive the profile. Returns the Normal-fallback
/// profile on platforms without `/proc/meminfo` (macOS, Windows) or when
/// the file is unreadable for any reason.
fn detect_profile() -> MemoryProfile {
    #[cfg(windows)]
    {
        // Same Normal-fallback contract as the meminfo path: a probe we could
        // not run must never throttle the machine.
        return match global_memory_status_kb() {
            Some((total_kb, _avail_kb)) => MemoryProfile::from_total_kb(total_kb),
            None => MemoryProfile::from_total_kb(u64::MAX),
        };
    }

    #[cfg(not(windows))]
    match std::fs::read_to_string("/proc/meminfo") {
        Ok(content) => detect_profile_from_meminfo(&content),
        Err(_) => MemoryProfile::from_total_kb(u64::MAX),
    }
}

/// Process-wide cached profile. Detection runs once on first access.
static PROFILE: OnceLock<MemoryProfile> = OnceLock::new();

/// Return the cached memory profile, running detection on first call.
/// Logs the resolved profile at info level on the initial detection.
pub fn memory_profile() -> &'static MemoryProfile {
    PROFILE.get_or_init(|| {
        let profile = detect_profile();
        match profile.class {
            MemoryClass::LowMemory => {
                log::info!(
                    "[system] Low-memory profile active: {} MB total RAM, prefetch={}, max_initial_buffer={}KB, hires_prefetch=disabled",
                    profile.mem_total_kb / 1024,
                    profile.prefetch_count,
                    profile.max_initial_buffer_bytes / 1024,
                );
            }
            MemoryClass::Normal => {
                log::info!(
                    "[system] Normal memory profile: {} MB total RAM",
                    profile.mem_total_kb / 1024
                );
            }
        }
        profile
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_meminfo_total_kb_extracts_value() {
        let sample = "\
MemTotal:         938196 kB
MemFree:          120000 kB
Buffers:           48000 kB
";
        assert_eq!(parse_meminfo_total_kb(sample), Some(938196));
    }

    #[test]
    fn parse_meminfo_total_kb_ignores_other_fields() {
        let sample = "\
MemFree:          120000 kB
MemTotal:        4194304 kB
SwapTotal:       2097152 kB
";
        assert_eq!(parse_meminfo_total_kb(sample), Some(4194304));
    }

    #[test]
    fn parse_meminfo_total_kb_handles_missing_field() {
        let sample = "\
MemFree:          120000 kB
SwapTotal:       2097152 kB
";
        assert_eq!(parse_meminfo_total_kb(sample), None);
    }

    #[test]
    fn parse_meminfo_total_kb_handles_empty_input() {
        assert_eq!(parse_meminfo_total_kb(""), None);
    }

    #[test]
    fn pi3b_with_1gb_resolves_to_low_memory() {
        // Raspberry Pi 3B = 1 GB RAM = ~938196 kB after kernel reservations.
        let profile = MemoryProfile::from_total_kb(938196);
        assert_eq!(profile.class, MemoryClass::LowMemory);
        assert_eq!(profile.prefetch_count, 1);
        assert_eq!(profile.max_concurrent_prefetch, 1);
        assert!(!profile.allow_hires_prefetch);
        assert!(profile.max_initial_buffer_bytes <= 256 * 1024);
        // L1 cap must be a small fraction of total RAM — at most ~10 %
        // of a Pi 3B, so we don't reserve four-tenths of memory for one
        // subsystem on a 1 GB host.
        assert!(profile.audio_cache_l1_max_bytes <= 100 * 1024 * 1024);
    }

    #[test]
    fn audio_cache_cap_is_significantly_smaller_on_low_memory() {
        let normal = MemoryProfile::from_total_kb(8 * 1024 * 1024);
        let low = MemoryProfile::from_total_kb(938196);
        assert!(low.audio_cache_l1_max_bytes < normal.audio_cache_l1_max_bytes);
    }

    #[test]
    fn pi_zero_2w_512mb_resolves_to_low_memory() {
        let profile = MemoryProfile::from_total_kb(500 * 1024);
        assert_eq!(profile.class, MemoryClass::LowMemory);
    }

    #[test]
    fn machine_with_2gb_resolves_to_normal() {
        // Exactly the threshold — Normal (>= NORMAL_FLOOR_KB).
        let profile = MemoryProfile::from_total_kb(2 * 1024 * 1024);
        assert_eq!(profile.class, MemoryClass::Normal);
        assert_eq!(profile.prefetch_count, 5);
        assert!(profile.allow_hires_prefetch);
    }

    #[test]
    fn machine_with_just_under_2gb_resolves_to_low_memory() {
        let profile = MemoryProfile::from_total_kb(2 * 1024 * 1024 - 1);
        assert_eq!(profile.class, MemoryClass::LowMemory);
    }

    #[test]
    fn detect_profile_from_meminfo_falls_back_to_normal_when_unparseable() {
        let profile = detect_profile_from_meminfo("garbage\nno memtotal here\n");
        assert_eq!(profile.class, MemoryClass::Normal);
    }

    #[test]
    fn detect_profile_from_meminfo_returns_low_memory_for_pi() {
        let pi_meminfo = "\
MemTotal:         938196 kB
MemFree:          250000 kB
";
        let profile = detect_profile_from_meminfo(pi_meminfo);
        assert_eq!(profile.class, MemoryClass::LowMemory);
        assert_eq!(profile.mem_total_kb, 938196);
    }

    #[test]
    fn parse_meminfo_available_kb_extracts_value() {
        let sample = "\
MemTotal:         938196 kB
MemFree:          120000 kB
MemAvailable:     180000 kB
";
        assert_eq!(parse_meminfo_available_kb(sample), Some(180000));
    }

    #[test]
    fn parse_meminfo_available_kb_distinguishes_from_memtotal() {
        let sample = "\
MemAvailable:     500000 kB
MemTotal:        4194304 kB
";
        assert_eq!(parse_meminfo_available_kb(sample), Some(500000));
        assert_eq!(parse_meminfo_total_kb(sample), Some(4194304));
    }

    #[test]
    fn parse_meminfo_available_kb_handles_missing_field() {
        let sample = "MemTotal: 938196 kB\nMemFree: 100000 kB\n";
        assert_eq!(parse_meminfo_available_kb(sample), None);
    }

    #[test]
    fn pressure_healthy_machine_is_neither_low_nor_critical() {
        // 4 GB total, 2 GB available => 50%
        let p = pressure_from_figures(2 * 1024 * 1024, 4 * 1024 * 1024);
        assert!((p.available_pct - 50.0).abs() < 0.001);
        assert!(!p.is_low);
        assert!(!p.is_critical);
    }

    #[test]
    fn pressure_at_14pct_is_low_not_critical() {
        // Exactly 14 % available — under 15 % low threshold but well
        // above the 5 % critical floor.
        let total = 1_000_000;
        let avail = 140_000;
        let p = pressure_from_figures(avail, total);
        assert!(p.is_low);
        assert!(!p.is_critical);
    }

    #[test]
    fn pressure_at_4pct_is_both_low_and_critical() {
        let total = 1_000_000;
        let avail = 40_000;
        let p = pressure_from_figures(avail, total);
        assert!(p.is_low);
        assert!(p.is_critical);
    }

    #[test]
    fn pressure_threshold_boundaries_inclusive_above() {
        // 15.0 % should not register as low (strict inequality).
        let p = pressure_from_figures(150_000, 1_000_000);
        assert!(!p.is_low);
        // Just under: 14.999 % -> low.
        let p = pressure_from_figures(149_990, 1_000_000);
        assert!(p.is_low);

        // 5.0 % should not register as critical.
        let p = pressure_from_figures(50_000, 1_000_000);
        assert!(!p.is_critical);
    }

    #[test]
    fn pressure_handles_zero_total_safely() {
        // The .max(1) guard prevents divide-by-zero. The resulting pct
        // is meaningless but we don't panic.
        let p = pressure_from_figures(100, 0);
        assert!(p.available_pct.is_finite());
    }

    #[test]
    fn pi3b_critical_pressure_matches_codehd7_scenario() {
        // codehd7's reported case (issue #331 comments): Pi 3B with
        // ~938 MB total, swap thrash drives MemAvailable below 30 MB.
        // The watchdog should treat this as critical.
        let p = pressure_from_figures(25 * 1024, 938 * 1024);
        assert!(p.is_critical);
    }
}
