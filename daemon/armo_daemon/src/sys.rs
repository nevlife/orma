//! The calls armod makes outside std: CLOCK_MONOTONIC in nanoseconds, the clock of
//! `bpf_ktime_get_ns` and of every record (design 3.1), armod's own CPU time, and giving up a
//! capability.
#![allow(unsafe_code)]

fn clock_ns(clock: libc::clockid_t) -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid out pointer for the call.
    unsafe { libc::clock_gettime(clock, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

pub fn mono_ns() -> u64 {
    clock_ns(libc::CLOCK_MONOTONIC)
}

/// The CPU time of every armod thread, those that ended included.
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

pub const CAP_SYS_ADMIN: u32 = 21;

#[repr(C)]
struct CapHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CapData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// Takes `cap` out of this thread's effective, permitted and inheritable sets for good; the
/// kernel then takes it out of the ambient set. Threads started later inherit the sets.
pub fn drop_cap(cap: u32) -> std::io::Result<()> {
    let mut header = CapHeader {
        version: 0x2008_0522,
        pid: 0,
    };
    let mut data = [CapData::default(); 2];
    // SAFETY: version 3 takes a header and two data words, both valid for the call.
    if unsafe { libc::syscall(libc::SYS_capget, &mut header, data.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let d = &mut data[(cap / 32) as usize];
    let keep = !(1u32 << (cap % 32));
    d.effective &= keep;
    d.permitted &= keep;
    d.inheritable &= keep;
    // SAFETY: as above.
    if unsafe { libc::syscall(libc::SYS_capset, &header, data.as_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(name: &str) -> u64 {
        let status = std::fs::read_to_string("/proc/thread-self/status").unwrap();
        let v = status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .unwrap()
            .trim();
        u64::from_str_radix(v, 16).unwrap()
    }

    #[test]
    fn the_dropped_capability_is_gone() {
        drop_cap(CAP_SYS_ADMIN).unwrap();
        for set in ["CapEff:", "CapPrm:", "CapInh:", "CapAmb:"] {
            assert_eq!(caps(set) & (1 << CAP_SYS_ADMIN), 0, "{set}");
        }
    }
}
