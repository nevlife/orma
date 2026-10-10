//! What armo-collectd reads from `/proc`. Files of a process are opened
//! through `/proc/<pid>/root`, so a container's libraries are its own.

use std::fs;
use std::path::PathBuf;

use armo_proto::msg::{PublishAbi, Ros};
use object::read::elf::ElfFile64;
use object::{Object, elf};

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

fn parse_stat(text: &str) -> Option<Stat> {
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

/// Cumulative on-CPU nanoseconds of a thread (the first field of `schedstat`).
pub fn schedstat(pid: u32, tid: u32) -> Option<u64> {
    fs::read_to_string(proc_path(pid, &format!("task/{tid}/schedstat")))
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// RSS and PSS in bytes from `smaps_rollup`.
pub fn memory(pid: u32) -> Option<(u64, u64)> {
    let text = fs::read_to_string(proc_path(pid, "smaps_rollup")).ok()?;
    let kb = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(name))?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()
            .map(|v| v * 1024)
    };
    Some((kb("Rss:")?, kb("Pss:")?))
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

/// The host pid of the process a container knows as `ns_pid` and that started at `starttime`
///: an LD_PRELOAD file names it with the values the process sees.
pub fn host_pid(ns_pid: u32, starttime: u64) -> Option<u32> {
    fs::read_dir("/proc").ok()?.find_map(|e| {
        let pid: u32 = e.ok()?.file_name().to_str()?.parse().ok()?;
        (stat(pid)?.starttime() == starttime).then_some(())?;
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
    })
}
