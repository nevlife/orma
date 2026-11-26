//! The calls ormad makes outside std: `CLOCK_MONOTONIC` in nanoseconds, the clock of
//! `bpf_ktime_get_ns` and of every record (design 3.1), and ormad's own CPU time.
#![allow(unsafe_code)]

fn clock_ns(clock: libc::clockid_t) -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid out pointer for the call.
    unsafe { libc::clock_gettime(clock, &raw mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

pub fn mono_ns() -> u64 {
    clock_ns(libc::CLOCK_MONOTONIC)
}

/// The CPU time of every ormad thread, those that ended included.
pub fn cpu_ns() -> u64 {
    clock_ns(libc::CLOCK_PROCESS_CPUTIME_ID)
}

/// Resident set size from `/proc/self/statm`.
pub fn rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf has no preconditions.
    Some(pages * unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64)
}

pub fn real_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// A Jetson's EMC activity counter and clock (debugfs, read through `CAP_DAC_READ_SEARCH`).
/// `mc_all` is in kHz of the EMC clock, so its share of the clock is the utilization.
pub fn memory_bus() -> Option<orma_proto::msg::MemoryBus> {
    let read = |p: &str| -> Option<u64> { std::fs::read_to_string(p).ok()?.trim().parse().ok() };
    let active_khz = read("/sys/kernel/debug/cactmon/mc_all")?;
    let clock_hz = read("/sys/kernel/debug/bpmp/debug/clk/emc/rate")?;
    Some(orma_proto::msg::MemoryBus {
        util_pct: active_khz as f64 * 100.0 / (clock_hz / 1000).max(1) as f64,
        clock_mhz: (clock_hz / 1_000_000) as u32,
    })
}
