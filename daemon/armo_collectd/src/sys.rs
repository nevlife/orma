//! What std does not give: the monotonic clock as event records carry it, file system free space,
//! a stop flag set by SIGTERM and SIGINT, and facts about processes from /proc.
#![allow(unsafe_code)]

use std::ffi::CString;
use std::io;
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
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
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
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
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

pub fn physical_memory() -> u64 {
    // SAFETY: sysconf has no preconditions.
    let (pages, size) = unsafe {
        (
            libc::sysconf(libc::_SC_PHYS_PAGES),
            libc::sysconf(libc::_SC_PAGESIZE),
        )
    };
    pages as u64 * size as u64
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

/// This process's CPU time in ns.
pub fn cpu_ns() -> u64 {
    // SAFETY: sysconf has no preconditions.
    let tick = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;
    let stat = crate::model::procfs::stat(pid()).expect("this process exists");
    stat.cpu_ticks() * 1_000_000_000 / tick
}
