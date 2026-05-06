//! CPU load measurement via sysctl.
//!
//! Reads `kern.cp_time` which returns cumulative CPU ticks since boot in 5 categories:
//! CPU_STATE_USER, CPU_STATE_NICE, CPU_STATE_SYSTEM, CPU_STATE_IDLE, CPU_STATE_UNKNOWN.
//! By sampling twice and computing the delta, we get the system-wide CPU utilization.
//!
//! No shell-out, no external processes — a direct syscall via libc.

use std::ffi::CString;
use std::time::Duration;

// From <sys/sysctl.h> on macOS
const CPUSTATES: usize = 5;
const CP_USER: usize = 0;
const CP_NICE: usize = 1;
const CP_SYS: usize = 2;
const CP_IDLE: usize = 3;
// const CP_INTR: usize = 4;

/// Raw CPU tick counter for one sample.
#[derive(Debug, Clone, Copy)]
struct CpuTicks {
    user: u64,
    nice: u64,
    sys: u64,
    idle: u64,
}

impl CpuTicks {
    fn busy(&self) -> u64 {
        self.user.saturating_add(self.nice).saturating_add(self.sys)
    }

    fn total(&self) -> u64 {
        self.busy().saturating_add(self.idle)
    }

    fn sample() -> Option<Self> {
        let name = CString::new("kern.cp_time").ok()?;
        // kern.cp_time on macOS returns natural_t (uint32_t) × CPUSTATES,
        // but libc exposes it as the kernel-width equivalent. Use u64 for safety.
        let mut buf = [0u64; CPUSTATES];
        let mut size = std::mem::size_of::<[u64; CPUSTATES]>();
        let res = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                buf.as_mut_ptr() as *mut _,
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if res != 0 {
            return None;
        }
        // On some macOS versions cp_time is 5x uint32, on others 5x uint64.
        // Detect by size returned.
        if size == std::mem::size_of::<[u32; CPUSTATES]>() {
            // Reinterpret: the first 5 u32 values live in the low halves of the u64 array
            let raw = buf.as_ptr() as *const u32;
            unsafe {
                Some(Self {
                    user: *raw.add(CP_USER) as u64,
                    nice: *raw.add(CP_NICE) as u64,
                    sys: *raw.add(CP_SYS) as u64,
                    idle: *raw.add(CP_IDLE) as u64,
                })
            }
        } else {
            Some(Self {
                user: buf[CP_USER],
                nice: buf[CP_NICE],
                sys: buf[CP_SYS],
                idle: buf[CP_IDLE],
            })
        }
    }
}

/// Long-running CPU load monitor. Keeps a running sample for delta computation.
pub struct CpuLoadMonitor {
    last: Option<CpuTicks>,
}

impl CpuLoadMonitor {
    pub fn new() -> Self {
        Self {
            last: CpuTicks::sample(),
        }
    }

    /// Measure CPU utilization as a fraction 0.0..=1.0 since the last call.
    /// Returns None on the very first call or if sysctl fails.
    pub fn sample(&mut self) -> Option<f64> {
        let current = CpuTicks::sample()?;
        let prev = match self.last.take() {
            Some(p) => p,
            None => {
                self.last = Some(current);
                return None;
            }
        };
        self.last = Some(current);

        let busy_delta = current.busy().saturating_sub(prev.busy());
        let total_delta = current.total().saturating_sub(prev.total());
        if total_delta == 0 {
            return None;
        }
        Some(busy_delta as f64 / total_delta as f64)
    }

    /// Convenience: block for `wait` and then sample. Useful for one-shot queries.
    pub fn sample_blocking(wait: Duration) -> Option<f64> {
        let mut mon = Self::new();
        std::thread::sleep(wait);
        mon.sample()
    }
}
