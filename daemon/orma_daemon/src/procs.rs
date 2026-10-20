//! Processes and the libraries they map, and each one's ROS distro (design 2.7.2).

use std::{collections::BTreeMap, fs, path::Path};

use crate::libs::{Lib, distro_of_path};

/// One mapped library of interest, as `/proc/<pid>/maps` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapped {
    pub lib: Lib,
    pub path: String,
    pub inode: u64,
    /// Replaced on disk since it was mapped: the path names another file now.
    pub deleted: bool,
}

/// The libraries `pid` maps that ormad attaches to, each once.
pub fn mapped(pid: u32) -> Option<Vec<Mapped>> {
    let maps = fs::read_to_string(format!("/proc/{pid}/maps")).ok()?;
    let mut out: Vec<Mapped> = Vec::new();
    for line in maps.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (Some(&path), Some(inode)) = (f.get(5), f.get(4).and_then(|i| i.parse().ok())) else {
            continue;
        };
        let Some(lib) = Lib::of(path) else {
            continue;
        };
        if out.iter().any(|m| m.path == path && m.inode == inode) {
            continue;
        }
        out.push(Mapped {
            lib,
            path: path.to_string(),
            inode,
            deleted: f.get(6) == Some(&"(deleted)"),
        });
    }
    Some(out)
}

pub fn pids() -> Vec<u32> {
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect()
}

/// Field 22 of `/proc/<pid>/stat`: with the pid, a process's identity.
pub fn starttime(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

pub fn threads(pid: u32) -> Vec<u32> {
    fs::read_dir(format!("/proc/{pid}/task"))
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect()
}

/// `NSpid` of a thread: its id in each pid namespace, the host's first.
pub fn nspid(pid: u32, tid: u32) -> Vec<u32> {
    fs::read_to_string(format!("/proc/{pid}/task/{tid}/status"))
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("NSpid:").map(|v| {
                    v.split_whitespace()
                        .filter_map(|i| i.parse().ok())
                        .collect()
                })
            })
        })
        .unwrap_or_default()
}

pub fn comm(pid: u32) -> String {
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn environ(pid: u32, name: &str) -> Option<String> {
    let env = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let key = format!("{name}=");
    env.split(|b| *b == 0)
        .find_map(|kv| kv.strip_prefix(key.as_bytes()))
        .map(|v| String::from_utf8_lossy(v).into_owned())
}

/// How a process's distro was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistroFrom {
    Environ,
    Path,
    Unknown,
}

/// 1. `ROS_DISTRO` in the environment, 2. the mapped libtracetools.so's `/opt/ros/<distro>/`,
/// 3. `unknown` (design 2.7.2).
pub fn distro(pid: u32, maps: &[Mapped]) -> (String, DistroFrom) {
    if let Some(d) = environ(pid, "ROS_DISTRO").filter(|d| !d.is_empty()) {
        return (d, DistroFrom::Environ);
    }
    maps.iter()
        .filter(|m| m.lib == Lib::Tracetools)
        .find_map(|m| distro_of_path(&m.path))
        .map_or(("unknown".into(), DistroFrom::Unknown), |d| {
            (d, DistroFrom::Path)
        })
}

/// The rcl publish ABI, where `ros_trace_rmw_publish` has only the message: Humble's.
pub fn rcl_abi_distro(distro: &str) -> bool {
    distro == "humble"
}

pub fn alive(pid: u32, start: u64) -> bool {
    starttime(pid) == Some(start)
}

/// Threads of `pids` by thread id, for the scheduler records.
pub fn owners(pids: impl IntoIterator<Item = u32>) -> BTreeMap<u32, u32> {
    let mut out = BTreeMap::new();
    for pid in pids {
        for tid in threads(pid) {
            out.insert(tid, pid);
        }
    }
    out
}

pub fn root(pid: u32) -> std::path::PathBuf {
    Path::new("/proc").join(pid.to_string()).join("root")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_a_starttime_and_threads() {
        let me = std::process::id();
        assert!(starttime(me).is_some());
        assert!(threads(me).contains(&me));
        assert_eq!(nspid(me, me).first(), Some(&me));
        assert!(alive(me, starttime(me).unwrap()));
    }

    #[test]
    fn humble_is_the_rcl_abi() {
        assert!(rcl_abi_distro("humble"));
        assert!(!rcl_abi_distro("jazzy"));
        assert!(!rcl_abi_distro("unknown"));
    }
}
