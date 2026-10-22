//! What orma-collectd reads from `/proc`. Files of a process are opened
//! through `/proc/<pid>/root`, so a container's libraries are its own.

use std::collections::{HashMap, hash_map::Entry};
use std::fs::{self, File};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;

use object::read::elf::ElfFile64;
use object::{Object, elf};
use orma_common::RCL_EXECUTOR_ANCHORS;
use orma_proto::msg::{ExecutorProbes, PublishAbi, Ros};

pub fn proc_path(pid: u32, rest: &str) -> PathBuf {
    PathBuf::from(format!("/proc/{pid}/{rest}"))
}

/// The fields after `comm`, numbered as proc(5) numbers them from 3.
pub struct Stat {
    pub comm: String,
    fields: Vec<u64>,
}

impl Stat {
    fn field(&self, n: usize) -> u64 {
        self.fields.get(n - 3).copied().unwrap_or(0)
    }
    pub fn starttime(&self) -> u64 {
        self.field(22)
    }
    pub fn minflt(&self) -> u64 {
        self.field(10)
    }
    pub fn majflt(&self) -> u64 {
        self.field(12)
    }
    /// utime plus stime, in clock ticks.
    pub fn cpu_ticks(&self) -> u64 {
        self.field(14) + self.field(15)
    }
    pub fn processor(&self) -> u32 {
        self.field(39) as u32
    }
    pub fn policy(&self) -> &'static str {
        match self.field(41) {
            0 => "other",
            1 => "fifo",
            2 => "rr",
            3 => "batch",
            5 => "idle",
            6 => "deadline",
            _ => "unknown",
        }
    }
    /// The real-time priority under `fifo` and `rr`, the nice value otherwise.
    pub fn prio(&self) -> i32 {
        match self.field(41) {
            1 | 2 => self.field(40) as i32,
            _ => self.field(19) as i64 as i32,
        }
    }
}

pub fn parse_stat(text: &str) -> Option<Stat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let numbers = text[close + 1..]
        .split_whitespace()
        .skip(1)
        .map(|f| f.parse::<i64>().map_or(0, |v| v as u64));
    let fields = std::iter::once(0).chain(numbers).collect();
    Some(Stat {
        comm: text[open + 1..close].to_string(),
        fields,
    })
}

pub fn stat(pid: u32) -> Option<Stat> {
    parse_stat(&fs::read_to_string(proc_path(pid, "stat")).ok()?)
}

pub fn task_stat(pid: u32, tid: u32) -> Option<Stat> {
    parse_stat(&fs::read_to_string(proc_path(pid, &format!("task/{tid}/stat"))).ok()?)
}

pub fn tids(pid: u32) -> Vec<u32> {
    let Ok(dir) = fs::read_dir(proc_path(pid, "task")) else {
        return Vec::new();
    };
    let mut tids: Vec<u32> = dir
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect();
    tids.sort_unstable();
    tids
}

/// Holds every file of /proc it reads, all small.
const KEPT_BUF: usize = 4096;

/// Files of `/proc` kept open across windows. A read is one pread from offset 0, which has the
/// kernel generate the whole file in one pass; the file of a thread or process that ended
/// answers ESRCH, also after its id is reused, and is let go.
#[derive(Default)]
pub struct Kept {
    files: HashMap<(u32, u32, &'static str), File>,
    buf: Vec<u8>,
}

impl Kept {
    /// `/proc/<pid>/task/<tid>/<name>`, or with `tid` 0 `/proc/<pid>/<name>`.
    pub fn read(&mut self, pid: u32, tid: u32, name: &'static str) -> Option<&str> {
        let key = (pid, tid, name);
        let file = match self.files.entry(key) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                let rest = if tid == 0 {
                    name.to_string()
                } else {
                    format!("task/{tid}/{name}")
                };
                e.insert(File::open(proc_path(pid, &rest)).ok()?)
            }
        };
        self.buf.resize(KEPT_BUF, 0);
        match file.read_at(&mut self.buf, 0) {
            Ok(n) => std::str::from_utf8(&self.buf[..n]).ok(),
            Err(_) => {
                self.files.remove(&key);
                None
            }
        }
    }

    /// Lets go of the files of a process's threads not in `tids` (sorted), or with `None` of
    /// all its files.
    pub fn retain(&mut self, pid: u32, tids: Option<&[u32]>) {
        self.files.retain(|&(p, t, _), _| {
            p != pid || tids.is_some_and(|ts| t == 0 || ts.binary_search(&t).is_ok())
        });
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.files.len()
    }
}

/// Cumulative on-CPU nanoseconds of a thread: the first field of `schedstat`.
pub fn schedstat_ns(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse().ok()
}

/// RSS in bytes from `statm`: counters the kernel keeps, read without the target's mmap_lock.
pub fn statm_rss(text: &str) -> Option<u64> {
    let pages: u64 = text.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * crate::sys::page_size())
}

/// PSS in bytes from `smaps_rollup`, a walk of every page table of the target under its
/// mmap_lock: only when a client asks for PSS.
pub fn rollup_pss(text: &str) -> Option<u64> {
    let kb: u64 = text
        .lines()
        .find_map(|l| l.strip_prefix("Pss:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kb * 1024)
}

/// `NSpid` of a `status` file: the id in each pid namespace, host first.
fn nspids(status: &str) -> Vec<u32> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("NSpid:"))
        .map(|v| {
            v.split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The pid inside the process's own namespace when it differs from the host pid.
pub fn ns_pid(pid: u32) -> Option<u32> {
    let ids = nspids(&fs::read_to_string(proc_path(pid, "status")).ok()?);
    ids.last().copied().filter(|&ns| ns != pid)
}

/// The boottime offset of a time namespace's `timens_offsets`, in clock ticks.
fn boottime_ticks(offsets: &str, tick_hz: u64) -> i64 {
    offsets
        .lines()
        .find_map(|l| {
            let mut f = l.split_whitespace();
            (f.next()? == "boottime").then_some(())?;
            let ns =
                f.next()?.parse::<i128>().ok()? * 1_000_000_000 + f.next()?.parse::<i128>().ok()?;
            Some((ns * i128::from(tick_hz)).div_euclid(1_000_000_000) as i64)
        })
        .unwrap_or(0)
}

/// Whether `pid` started at `starttime` as the process itself reads it. /proc adds the reader's
/// time namespace boottime offset, and the shared memory file carries the process's own view.
pub fn started_at(pid: u32, starttime: u64) -> bool {
    let Some(host) = stat(pid).map(|s| s.starttime()) else {
        return false;
    };
    host == starttime
        || fs::read_to_string(proc_path(pid, "timens_offsets")).is_ok_and(|t| {
            host.wrapping_add_signed(boottime_ticks(&t, crate::sys::clock_ticks())) == starttime
        })
}

/// The host pid of the process a container knows as `ns_pid` and that started at `starttime`
///: an LD_PRELOAD file names it with the values the process sees.
pub fn host_pid(ns_pid: u32, starttime: u64) -> Option<u32> {
    fs::read_dir("/proc").ok()?.find_map(|e| {
        let pid: u32 = e.ok()?.file_name().to_str()?.parse().ok()?;
        started_at(pid, starttime).then_some(())?;
        let ids = nspids(&fs::read_to_string(proc_path(pid, "status")).ok()?);
        (ids.last() == Some(&ns_pid)).then_some(pid)
    })
}

/// The host tid of the thread a container knows as `ns_tid` in host process `pid`.
pub fn host_tid(pid: u32, ns_tid: u32) -> Option<u32> {
    tids(pid).into_iter().find(|&tid| {
        fs::read_to_string(proc_path(pid, &format!("task/{tid}/status")))
            .is_ok_and(|s| nspids(&s).last() == Some(&ns_tid))
    })
}

pub struct Mapping {
    pub start: u64,
    pub end: u64,
    pub exec: bool,
    pub offset: u64,
    pub path: String,
}

pub fn maps(pid: u32) -> Vec<Mapping> {
    let Ok(text) = fs::read_to_string(proc_path(pid, "maps")) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (start, end) = f.next()?.split_once('-')?;
            let perms = f.next()?;
            let offset = u64::from_str_radix(f.next()?, 16).ok()?;
            let path = f.nth(2)?;
            path.starts_with('/').then(|| Mapping {
                start: u64::from_str_radix(start, 16).unwrap_or(0),
                end: u64::from_str_radix(end, 16).unwrap_or(0),
                exec: perms.as_bytes().get(2) == Some(&b'x'),
                offset,
                path: path.to_string(),
            })
        })
        .collect()
}

/// A mapped file as the process sees it.
pub fn read_mapped(pid: u32, path: &str) -> Option<Vec<u8>> {
    fs::read(proc_path(pid, &format!("root{path}"))).ok()
}

fn environ_var(pid: u32, name: &str) -> Option<String> {
    let env = fs::read(proc_path(pid, "environ")).ok()?;
    env.split(|b| *b == 0).find_map(|kv| {
        let v = kv.strip_prefix(name.as_bytes())?.strip_prefix(b"=")?;
        Some(String::from_utf8_lossy(v).into_owned())
    })
}

fn needs(elf: &ElfFile64<'_>, prefix: &[u8]) -> bool {
    let Ok(table) = elf.elf_dynamic_table() else {
        return false;
    };
    table
        .iter()
        .any(|d| d.tag == elf::DT_NEEDED && table.string(d).is_ok_and(|s| s.starts_with(prefix)))
}

/// ormad's choice for the process (its `libs::executor_probes`): the rcl_executor probes when a
/// mapped librcl_executor.so exports every anchor.
fn executor_probes(pid: u32, maps: &[Mapping]) -> ExecutorProbes {
    let mut paths: Vec<&str> = maps
        .iter()
        .map(|m| m.path.as_str())
        .filter(|p| {
            p.rsplit('/').next().is_some_and(|n| {
                n.strip_prefix("librcl_executor.so")
                    .is_some_and(|v| v.is_empty() || v.starts_with('.'))
            })
        })
        .collect();
    paths.dedup();
    let core = paths.into_iter().any(|path| {
        let Some(data) = read_mapped(pid, path) else {
            return false;
        };
        let Ok(file) = object::File::parse(&*data) else {
            return false;
        };
        let Ok(exports) = file.exports() else {
            return false;
        };
        RCL_EXECUTOR_ANCHORS
            .iter()
            .all(|a| exports.iter().any(|e| e.name() == a.as_bytes()))
    });
    if core {
        ExecutorProbes::RclExecutor
    } else {
        ExecutorProbes::Stock
    }
}

/// The process's ROS as the distro check decides it; `None` when it maps no `libtracetools.so`.
pub fn ros(pid: u32) -> Option<Ros> {
    let maps = maps(pid);
    let lib = |name: &str| {
        maps.iter()
            .find(|m| m.path.rsplit('/').next() == Some(name))
            .map(|m| m.path.clone())
    };
    let tracetools = lib("libtracetools.so")?;
    let distro = environ_var(pid, "ROS_DISTRO")
        .or_else(|| {
            let rest = tracetools.strip_prefix("/opt/ros/")?;
            Some(rest.split('/').next()?.to_string())
        })
        .unwrap_or_else(|| "unknown".into());
    let tt = read_mapped(pid, &tracetools);
    let tracetools_lttng = tt
        .as_deref()
        .and_then(|d| ElfFile64::parse(d).ok())
        .is_some_and(|e| needs(&e, b"liblttng-ust"));
    let rclcpp = lib("librclcpp.so").and_then(|p| read_mapped(pid, &p));
    let gated_register = rclcpp
        .as_deref()
        .and_then(|d| object::File::parse(d).ok())
        .and_then(|f| f.imports().ok())
        .is_some_and(|imports| {
            imports
                .iter()
                .any(|i| i.name() == b"ros_trace_do_rclcpp_callback_register")
        });
    Some(Ros {
        publish_abi: if distro == "humble" {
            PublishAbi::Rcl
        } else {
            PublishAbi::Rmw
        },
        distro,
        tracetools_lttng,
        gated_register,
        executor_probes: Some(executor_probes(pid, &maps)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_files_reread_and_let_go() {
        let me = std::process::id();
        let mut k = Kept::default();
        let a = parse_stat(k.read(me, 0, "stat").unwrap()).unwrap();
        assert_eq!(a.starttime(), stat(me).unwrap().starttime());
        assert!(statm_rss(k.read(me, 0, "statm").unwrap()).unwrap() > 0);
        let child = std::thread::spawn(|| {
            let link = fs::read_link("/proc/thread-self").unwrap();
            let tid: u32 = link.file_name().unwrap().to_str().unwrap().parse().unwrap();
            let mut k = Kept::default();
            let me = std::process::id();
            assert!(schedstat_ns(k.read(me, tid, "schedstat").unwrap()).is_some());
            (tid, k)
        });
        let (tid, mut k2) = child.join().unwrap();
        // join returns when the thread clears its tid, a little before the kernel releases it.
        let gone = (0..1000).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(1));
            k2.read(me, tid, "schedstat").is_none()
        });
        assert!(gone, "ended thread");
        assert_eq!(k2.len(), 0);
        k.read(me, 0, "statm");
        k.retain(me, Some(&[]));
        assert_eq!(k.len(), 2);
        k.retain(me, None);
        assert_eq!(k.len(), 0);
    }

    #[test]
    fn starttime_in_a_time_namespace() {
        assert_eq!(boottime_ticks("monotonic 0 0\nboottime 0 0\n", 100), 0);
        assert_eq!(
            boottime_ticks("monotonic 5 0\nboottime 86400 500000000\n", 100),
            8_640_050
        );
        assert_eq!(boottime_ticks("boottime -1 0\n", 100), -100);
        let me = std::process::id();
        let st = stat(me).unwrap().starttime();
        assert!(started_at(me, st));
        assert!(!started_at(me, st + 1));
    }

    #[test]
    fn pss_from_rollup() {
        let text =
            "00400000-7fff [rollup]\nRss:                3700 kB\nPss:                1234 kB\n";
        assert_eq!(rollup_pss(text), Some(1234 * 1024));
    }
}
