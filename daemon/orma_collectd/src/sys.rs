//! What std does not give: the monotonic clock as event records carry it, file system free space,
//! a stop flag set by SIGTERM and SIGINT, and facts about processes from /proc.
#![allow(unsafe_code)]

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn mono_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: ts is a valid out pointer for the call.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

pub fn real_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_nanos() as u64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Disk {
    pub avail: u64,
    pub total: u64,
}

pub fn disk(path: &Path) -> io::Result<Disk> {
    let c = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: statvfs is plain data; zero is a valid value to overwrite.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: c is NUL-terminated and st a valid out pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &raw mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Disk {
        avail: st.f_bavail * st.f_frsize,
        total: st.f_blocks * st.f_frsize,
    })
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// The flag SIGTERM and SIGINT set from now on.
pub fn stop_on_signals() -> &'static AtomicBool {
    for signal in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
        unsafe { libc::signal(signal, on_stop as *const () as libc::sighandler_t) };
    }
    &STOP
}

pub fn uid() -> u32 {
    std::fs::metadata("/proc/self")
        .expect("/proc is mounted")
        .uid()
}

pub fn pid() -> u32 {
    std::process::id()
}

/// Field 22 of `/proc/<pid>/stat`, or `None` once the process is gone.
pub fn starttime(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = &stat[stat.rfind(')')? + 1..];
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

/// Raises the soft open file limit to the hard one: the model keeps a few `/proc` files open per
/// watched thread (`procfs::Kept`).
pub fn raise_nofile() {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: lim is a valid out pointer for the call.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut lim) } == 0 {
        lim.rlim_cur = lim.rlim_max;
        // SAFETY: lim is a valid rlimit.
        unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const lim) };
    }
}

/// The kernel clock discipline's state (`TIME_OK` 0 ... `TIME_ERROR` 5) and status bits.
pub fn adjtimex() -> Option<(i32, i32)> {
    // SAFETY: timex is plain data; modes 0 only reads.
    let mut t: libc::timex = unsafe { std::mem::zeroed() };
    // SAFETY: t is a valid timex for the call.
    let state = unsafe { libc::adjtimex(&raw mut t) };
    (state >= 0).then_some((state, t.status))
}

pub fn page_size() -> u64 {
    // SAFETY: sysconf has no preconditions.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 }
}

pub fn physical_memory() -> u64 {
    // SAFETY: sysconf has no preconditions.
    let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    pages as u64 * page_size()
}

/// Sends SIGTERM to `pid`.
pub fn terminate(pid: u32) {
    // SAFETY: kill has no memory preconditions.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
}

pub fn proc_text(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn hostname() -> String {
    proc_text("/proc/sys/kernel/hostname")
}

/// The unit of /proc's tick counts (`USER_HZ`).
pub fn clock_ticks() -> u64 {
    // SAFETY: sysconf has no preconditions.
    unsafe { libc::sysconf(libc::_SC_CLK_TCK) as u64 }
}

/// This process's CPU time in ns.
pub fn cpu_ns() -> u64 {
    let stat = crate::model::procfs::stat(pid()).expect("this process exists");
    stat.cpu_ticks() * 1_000_000_000 / clock_ticks()
}

/// A timerfd that a step of `CLOCK_REALTIME` cancels (settimeofday, `clock_settime`, an NTP step).
/// A slew moves `CLOCK_REALTIME` and `CLOCK_MONOTONIC` together and leaves it armed.
pub struct RealtimeSteps(OwnedFd);

impl RealtimeSteps {
    pub fn new() -> io::Result<RealtimeSteps> {
        // SAFETY: no pointer arguments.
        let fd = unsafe {
            libc::timerfd_create(libc::CLOCK_REALTIME, libc::TFD_NONBLOCK | libc::TFD_CLOEXEC)
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd is a new descriptor this value owns from now on.
        let s = RealtimeSteps(unsafe { OwnedFd::from_raw_fd(fd) });
        s.arm()?;
        Ok(s)
    }

    fn arm(&self) -> io::Result<()> {
        let never = libc::itimerspec {
            it_interval: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            it_value: libc::timespec {
                tv_sec: 1 << 33,
                tv_nsec: 0,
            },
        };
        let flags = libc::TFD_TIMER_ABSTIME | libc::TFD_TIMER_CANCEL_ON_SET;
        // SAFETY: never is a valid itimerspec; the old value is not asked for.
        let r = unsafe {
            libc::timerfd_settime(
                self.0.as_raw_fd(),
                flags,
                &raw const never,
                std::ptr::null_mut(),
            )
        };
        if r != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Whether `CLOCK_REALTIME` stepped since the last call.
    pub fn stepped(&self) -> bool {
        let mut expirations = 0u64;
        // SAFETY: reads 8 bytes into a u64.
        let n = unsafe {
            libc::read(
                self.0.as_raw_fd(),
                (&raw mut expirations).cast(),
                size_of::<u64>(),
            )
        };
        let stepped = n < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ECANCELED);
        if stepped {
            self.arm().expect("re-arming a timerfd that armed before");
        }
        stepped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_step_without_a_clock_set() {
        let s = RealtimeSteps::new().unwrap();
        assert!(!s.stepped());
        assert!(!s.stepped());
    }
}
