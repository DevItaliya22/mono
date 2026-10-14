// SPDX-License-Identifier: MPL-2.0

//! **The machine facts a run header has to carry, and the two counters that are
//! read from the OS rather than sampled.**
//!
//! The paced pass measures wall clock, so the machine's state is part of the
//! measurement. **A run whose header is incomplete is a run whose numbers cannot
//! be compared**, which is why every field here is printed even when it is
//! unknown — an absent line reads as "nothing to say", and `unknown` reads as
//! what it is.
//!
//! ## Peak resident memory is the OS's own peak, never a sample
//!
//! A sampled maximum misses a load-time spike between samples, and a model load
//! is exactly a load-time spike. Windows has
//! `PROCESS_MEMORY_COUNTERS.PeakWorkingSetSize`; Unix has `getrusage`'s
//! `ru_maxrss`, which on macOS is the same kernel counter as
//! `mach_task_basic_info.resident_size_max` and is reported **in bytes** there
//! while Linux reports it **in kibibytes** — a units difference that is exactly
//! the kind of thing that produces a plausible wrong number, so it is handled
//! per platform rather than by one constant.
//!
//! ## CPU is the process's own CPU time, differenced
//!
//! Sampled at 100 ms during the paced pass and normalised so that one saturated
//! core reads 100%. Only the paced pass gives a meaningful figure: unpaced,
//! every viable candidate saturates its threads by construction.

use std::time::Duration;

/// What the run header says about this machine.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize)]
pub struct HostFacts {
    pub os: String,
    pub arch: String,
    /// Physical cores where the platform will say, logical otherwise, and the
    /// string says which — a core count that might be either is not a number
    /// anybody can compare.
    pub cores: String,
    /// SIMD level detected at run time.
    pub simd: String,
}

impl HostFacts {
    pub fn detect() -> HostFacts {
        HostFacts {
            os: os_description(),
            arch: std::env::consts::ARCH.to_owned(),
            cores: cores(),
            simd: simd(),
        }
    }

    /// The one line the header prints.
    pub fn line(&self) -> String {
        format!(
            "{} / {} / {} / {}",
            self.os, self.arch, self.cores, self.simd
        )
    }
}

fn os_description() -> String {
    // Deliberately not a build-number lookup: `koru-agent`'s `build_stamp`
    // already owns that on Windows, through `RtlGetVersion`, and a second
    // implementation here would be a second answer to what the OS version is.
    // The header's job is to identify the machine well enough to compare two
    // runs, and the class column carries the rest.
    std::env::consts::OS.to_owned()
}

fn cores() -> String {
    match std::thread::available_parallelism() {
        Ok(n) => format!("{n} logical cores"),
        Err(_) => "unknown cores".to_owned(),
    }
}

fn simd() -> String {
    #[cfg(target_arch = "x86_64")]
    {
        let mut levels: Vec<&str> = Vec::new();
        if is_x86_feature_detected!("avx2") {
            levels.push("AVX2");
        }
        if is_x86_feature_detected!("avx512f") {
            levels.push("AVX512");
        }
        if levels.is_empty() {
            return "no AVX2".to_owned();
        }
        return levels.join(" ");
    }
    #[cfg(target_arch = "aarch64")]
    {
        // NEON is mandatory on aarch64, so there is nothing to detect and
        // saying so is more useful than an empty field.
        return "NEON".to_owned();
    }
    #[allow(unreachable_code)]
    "unknown".to_owned()
}

/// Peak resident set size in bytes, from the OS's own counter.
///
/// `None` where the platform will not say, which is reported as `unknown`
/// rather than as zero — a peak RSS of 0 would pass the `M-arm` 2 GiB gate.
pub fn peak_rss_bytes() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: the struct is zeroed and its `cb` is set to its own size,
        // which is the whole of this API's contract; `GetCurrentProcess`
        // returns a pseudo-handle that needs no closing.
        unsafe {
            let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            if GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) != 0 {
                return Some(counters.PeakWorkingSetSize as u64);
            }
        }
        None
    }
    #[cfg(unix)]
    {
        // SAFETY: `getrusage` writes into a zeroed struct we own.
        unsafe {
            let mut usage: libc::rusage = std::mem::zeroed();
            if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
                return None;
            }
            let raw = usage.ru_maxrss as u64;
            // See the module note: macOS reports bytes, Linux kibibytes.
            if cfg!(target_os = "macos") {
                Some(raw)
            } else {
                Some(raw * 1024)
            }
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

/// This process's CPU time so far — user plus system.
///
/// `None` where the platform will not say. Differencing two of these across a
/// wall-clock interval and dividing by the interval gives the figure the
/// protocol reports, normalised so one saturated core reads 100%.
pub fn cpu_time() -> Option<Duration> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        // SAFETY: four owned, zeroed FILETIMEs, and a pseudo-handle.
        unsafe {
            let mut creation: FILETIME = std::mem::zeroed();
            let mut exit: FILETIME = std::mem::zeroed();
            let mut kernel: FILETIME = std::mem::zeroed();
            let mut user: FILETIME = std::mem::zeroed();
            if GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            ) == 0
            {
                return None;
            }
            // FILETIME is 100-nanosecond intervals in two 32-bit halves, and it
            // is *not* aligned to 8 bytes — reading it as a u64 through a
            // pointer cast is undefined behaviour on some targets, which is why
            // this recombines the halves.
            let to_ns = |t: FILETIME| {
                ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)) * 100
            };
            Some(Duration::from_nanos(to_ns(kernel) + to_ns(user)))
        }
    }
    #[cfg(unix)]
    {
        // SAFETY: as above.
        unsafe {
            let mut usage: libc::rusage = std::mem::zeroed();
            if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
                return None;
            }
            let seconds = (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as u64;
            let micros = (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as u64;
            Some(Duration::from_secs(seconds) + Duration::from_micros(micros))
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

/// A CPU percentage over one interval, where 100 means one saturated core.
///
/// `None` when either endpoint is unknown or no time passed — a percentage over
/// a zero interval is not zero.
pub fn cpu_percent(
    before: Option<Duration>,
    after: Option<Duration>,
    wall: Duration,
) -> Option<f64> {
    let (before, after) = (before?, after?);
    if wall.is_zero() {
        return None;
    }
    let used = after.checked_sub(before)?;
    Some(100.0 * used.as_secs_f64() / wall.as_secs_f64())
}

#[cfg(test)]
mod tests;
