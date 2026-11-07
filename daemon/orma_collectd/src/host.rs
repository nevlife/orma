//! The host around a recording (design 14.9): the settings that change what a measurement means,
//! and what interfered with the run, from counters the kernel keeps anyway. Read at the start
//! and the end of a recording, never by the processes being watched.

use std::collections::BTreeMap;
use std::fs;

use orma_proto::msg::{Interference, ThreadCounters};

use crate::model::procfs;
use crate::sys;

const FILES: &[(&str, &str)] = &[
    ("kernel.release", "/proc/sys/kernel/osrelease"),
    ("kernel.version", "/proc/sys/kernel/version"),
    ("cmdline", "/proc/cmdline"),
    (
        "clocksource",
        "/sys/devices/system/clocksource/clocksource0/current_clocksource",
    ),
    ("sched.preempt", "/sys/kernel/debug/sched/preempt"),
    (
        "sched.base_slice_ns",
        "/sys/kernel/debug/sched/base_slice_ns",
    ),
    ("sched.features", "/sys/kernel/debug/sched/features"),
    (
        "sched.fair_server.cpu0.runtime",
        "/sys/kernel/debug/sched/fair_server/cpu0/runtime",
    ),
    (
        "sched.fair_server.cpu0.period",
        "/sys/kernel/debug/sched/fair_server/cpu0/period",
    ),
    ("sched_ext.state", "/sys/kernel/sched_ext/state"),
    ("sched_ext.ops", "/sys/kernel/sched_ext/root/ops"),
    (
        "sched_rt_runtime_us",
        "/proc/sys/kernel/sched_rt_runtime_us",
    ),
    ("sched_rt_period_us", "/proc/sys/kernel/sched_rt_period_us"),
    ("cpu.isolated", "/sys/devices/system/cpu/isolated"),
    ("cpu.nohz_full", "/sys/devices/system/cpu/nohz_full"),
    ("irq.default_affinity", "/proc/irq/default_smp_affinity"),
    ("cpufreq.boost", "/sys/devices/system/cpu/cpufreq/boost"),
    (
        "intel_pstate.no_turbo",
        "/sys/devices/system/cpu/intel_pstate/no_turbo",
    ),
    ("thp.enabled", "/sys/kernel/mm/transparent_hugepage/enabled"),
    ("thp.defrag", "/sys/kernel/mm/transparent_hugepage/defrag"),
    ("bpf_stats_enabled", "/proc/sys/kernel/bpf_stats_enabled"),
    ("lockdown", "/sys/kernel/security/lockdown"),
    ("hypervisor", "/sys/hypervisor/type"),
];

const CONFIG: &[&str] = &[
    "CONFIG_HZ",
    "CONFIG_PREEMPT_RT",
    "CONFIG_PREEMPT_DYNAMIC",
    "CONFIG_PREEMPT",
    "CONFIG_PREEMPT_VOLUNTARY",
    "CONFIG_NO_HZ_FULL",
];

const VMSTAT: &[&str] = &[
    "compact_stall",
    "pgmajfault",
    "pswpin",
    "pswpout",
    "thp_fault_fallback",
    "nr_tlb_remote_flush",
    "nr_tlb_remote_flush_received",
    "nr_tlb_local_flush_all",
    "nr_tlb_local_flush_one",
];

fn text(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn dir_names(path: &str) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(path)
        .map(|d| {
            d.filter_map(|e| Some(e.ok()?.file_name().to_str()?.to_string()))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The settings by name. Values root alone may read (debugfs) are left out under a user.
pub fn system(pids: &[u32]) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for (key, path) in FILES {
        if let Some(v) = text(path) {
            m.insert(key.to_string(), v);
        }
    }
    let release = m.get("kernel.release").cloned().unwrap_or_default();
    if let Some(config) = text(&format!("/boot/config-{release}")) {
        for l in config.lines() {
            if let Some((k, v)) = l.split_once('=')
                && CONFIG.contains(&k)
            {
                m.insert(format!("config.{k}"), v.to_string());
            }
        }
    }
    let vulns = "/sys/devices/system/cpu/vulnerabilities";
    for name in dir_names(vulns) {
        if let Some(v) = text(&format!("{vulns}/{name}")) {
            m.insert(format!("vulnerabilities.{name}"), v);
        }
    }
    let cpufreq = "/sys/devices/system/cpu/cpufreq";
    for policy in dir_names(cpufreq)
        .iter()
        .filter(|p| p.starts_with("policy"))
    {
        for file in [
            "scaling_governor",
            "energy_performance_preference",
            "scaling_max_freq",
        ] {
            if let Some(v) = text(&format!("{cpufreq}/{policy}/{file}")) {
                m.insert(format!("cpufreq.{policy}.{file}"), v);
            }
        }
    }
    let mut idle_off = Vec::new();
    for cpu in dir_names("/sys/devices/system/cpu") {
        let states = format!("/sys/devices/system/cpu/{cpu}/cpuidle");
        for state in dir_names(&states) {
            if text(&format!("{states}/{state}/disable")).as_deref() == Some("1") {
                let name = text(&format!("{states}/{state}/name")).unwrap_or(state);
                idle_off.push(format!("{cpu}:{name}"));
            }
        }
    }
    m.insert("cpuidle.disabled".into(), idle_off.join(" "));
    if fs::read_to_string("/proc/cpuinfo").is_ok_and(|c| {
        c.lines()
            .any(|l| l.starts_with("flags") && l.split_whitespace().any(|f| f == "hypervisor"))
    }) {
        m.insert("cpu.flag_hypervisor".into(), "1".into());
    }
    let auditd = fs::read_dir("/proc").is_ok_and(|d| {
        d.filter_map(std::result::Result::ok).any(|e| {
            text(&format!("/proc/{}/comm", e.file_name().to_string_lossy())).as_deref()
                == Some("auditd")
        })
    });
    m.insert(
        "auditd".into(),
        if auditd { "running" } else { "absent" }.into(),
    );
    if let Some((state, status)) = sys::adjtimex() {
        m.insert("time.state".into(), state.to_string());
        m.insert("time.status".into(), format!("{status:#x}"));
    }
    m.insert("clock_gettime_ns".into(), format!("{:.1}", clock_cost_ns()));
    for &pid in pids {
        let status = fs::read_to_string(procfs::proc_path(pid, "status")).unwrap_or_default();
        if let Some(v) = status.lines().find_map(|l| l.strip_prefix("Seccomp:")) {
            m.insert(format!("pid.{pid}.seccomp"), v.trim().to_string());
        }
        let cgroup = fs::read_to_string(procfs::proc_path(pid, "cgroup")).unwrap_or_default();
        if let Some(path) = cgroup.lines().find_map(|l| l.strip_prefix("0::"))
            && let Some(v) = text(&format!("/sys/fs/cgroup{path}/cpu.max"))
        {
            m.insert(format!("pid.{pid}.cpu.max"), v);
        }
    }
    m
}

/// What one `CLOCK_MONOTONIC` read costs here: KPTI, the clocksource and a VM change it.
fn clock_cost_ns() -> f64 {
    const N: u64 = 100_000;
    let t0 = sys::mono_ns();
    for _ in 0..N {
        std::hint::black_box(sys::mono_ns());
    }
    (sys::mono_ns() - t0) as f64 / N as f64
}

/// Rows of a per CPU table (`/proc/interrupts`, `/proc/softirqs`). Numbered rows are summed into
/// `dev`; rows with fewer values (`ERR`, `MIS`) keep what they have.
fn per_cpu(text: &str) -> BTreeMap<String, Vec<u64>> {
    let mut lines = text.lines();
    let cpus = lines.next().map_or(0, |h| h.split_whitespace().count());
    let mut out: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for l in lines {
        let Some((label, rest)) = l.split_once(':') else {
            continue;
        };
        let label = label.trim();
        let values: Vec<u64> = rest
            .split_whitespace()
            .take(cpus)
            .map_while(|v| v.parse().ok())
            .collect();
        if label.chars().all(|c| c.is_ascii_digit()) {
            let dev = out.entry("dev".into()).or_insert_with(|| vec![0; cpus]);
            for (d, v) in dev.iter_mut().zip(&values) {
                *d += v;
            }
        } else {
            out.insert(label.to_string(), values);
        }
    }
    out
}

fn vmstat(text: &str) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    for l in text.lines() {
        let Some((k, v)) = l.split_once(' ') else {
            continue;
        };
        let Ok(v) = v.trim().parse::<u64>() else {
            continue;
        };
        if k.starts_with("allocstall_") {
            *out.entry("allocstall".to_string()).or_default() += v;
        } else if VMSTAT.contains(&k) {
            out.insert(k.to_string(), v);
        }
    }
    out
}

fn pressure() -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    for res in ["cpu", "memory", "io", "irq"] {
        let Some(t) = text(&format!("/proc/pressure/{res}")) else {
            continue;
        };
        for l in t.lines() {
            let mut f = l.split_whitespace();
            let kind = f.next().unwrap_or_default();
            if let Some(total) = f.find_map(|x| x.strip_prefix("total="))
                && let Ok(v) = total.parse()
            {
                out.insert(format!("{res}.{kind}"), v);
            }
        }
    }
    out
}

/// The eighth value of each `cpuN` line of `/proc/stat`.
fn steal(text: &str) -> Vec<u64> {
    text.lines()
        .filter(|l| l.starts_with("cpu") && !l.starts_with("cpu "))
        .map(|l| {
            l.split_whitespace()
                .nth(8)
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        })
        .collect()
}

/// The counters at one instant.
#[derive(Default)]
pub struct Counters {
    interrupts: BTreeMap<String, Vec<u64>>,
    softirqs: BTreeMap<String, Vec<u64>>,
    vmstat: BTreeMap<String, u64>,
    pressure: BTreeMap<String, u64>,
    steal: Vec<u64>,
    threads: BTreeMap<(u32, u32), ThreadCounters>,
}

fn thread(pid: u32, tid: u32) -> Option<ThreadCounters> {
    let stat = procfs::task_stat(pid, tid)?;
    let sched =
        fs::read_to_string(procfs::proc_path(pid, &format!("task/{tid}/schedstat"))).ok()?;
    let mut s = sched
        .split_whitespace()
        .map(|v| v.parse::<u64>().unwrap_or(0));
    Some(ThreadCounters {
        pid,
        tid,
        comm: stat.comm.clone(),
        minflt: stat.minflt(),
        majflt: stat.majflt(),
        run_ns: s.next()?,
        wait_ns: s.next()?,
        slices: s.next()?,
    })
}

impl Counters {
    pub fn take(pids: &[u32]) -> Counters {
        let read = |p: &str| fs::read_to_string(p).unwrap_or_default();
        let mut c = Counters {
            interrupts: per_cpu(&read("/proc/interrupts")),
            softirqs: per_cpu(&read("/proc/softirqs")),
            vmstat: vmstat(&read("/proc/vmstat")),
            pressure: pressure(),
            steal: steal(&read("/proc/stat")),
            threads: BTreeMap::new(),
        };
        for &pid in pids {
            c.add_threads(pid);
        }
        c
    }

    /// Takes in a process that joined the targets after the start.
    pub fn add_threads(&mut self, pid: u32) {
        for tid in procfs::tids(pid) {
            if let Some(t) = thread(pid, tid) {
                self.threads.insert((pid, tid), t);
            }
        }
    }

    /// `end` minus `self`, over the threads `end` has; one that began since counts from 0.
    pub fn until(&self, end: &Counters) -> Interference {
        let table = |a: &BTreeMap<String, Vec<u64>>, b: &BTreeMap<String, Vec<u64>>| {
            b.iter()
                .map(|(k, v)| (k.clone(), minus(v, a.get(k).map_or(&[][..], Vec::as_slice))))
                .collect()
        };
        let scalars = |a: &BTreeMap<String, u64>, b: &BTreeMap<String, u64>| {
            b.iter()
                .map(|(k, v)| (k.clone(), v.saturating_sub(a.get(k).copied().unwrap_or(0))))
                .collect()
        };
        Interference {
            interrupts: table(&self.interrupts, &end.interrupts),
            softirqs: table(&self.softirqs, &end.softirqs),
            vmstat: scalars(&self.vmstat, &end.vmstat),
            pressure_us: scalars(&self.pressure, &end.pressure),
            steal_ticks: minus(&end.steal, &self.steal),
            threads: end
                .threads
                .iter()
                .map(|(k, e)| {
                    let s = self.threads.get(k);
                    let d = |f: fn(&ThreadCounters) -> u64| f(e).saturating_sub(s.map_or(0, f));
                    ThreadCounters {
                        pid: e.pid,
                        tid: e.tid,
                        comm: e.comm.clone(),
                        minflt: d(|t| t.minflt),
                        majflt: d(|t| t.majflt),
                        run_ns: d(|t| t.run_ns),
                        wait_ns: d(|t| t.wait_ns),
                        slices: d(|t| t.slices),
                    }
                })
                .collect(),
        }
    }
}

fn minus(b: &[u64], a: &[u64]) -> Vec<u64> {
    b.iter()
        .enumerate()
        .map(|(i, v)| v.saturating_sub(a.get(i).copied().unwrap_or(0)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERRUPTS: &str = "           CPU0       CPU1
   0:         21          3   IO-APIC   2-edge      timer
   8:          1          2   IO-APIC   8-edge      rtc0
 LOC:    5996676    1130084   Local timer interrupts
 TLB:    1010045     455090   TLB shootdowns
 ERR:          0
";

    #[test]
    fn per_cpu_rows() {
        let t = per_cpu(INTERRUPTS);
        assert_eq!(t["dev"], [22, 5]);
        assert_eq!(t["LOC"], [5996676, 1130084]);
        assert_eq!(t["TLB"], [1010045, 455090]);
        assert_eq!(t["ERR"], [0]);
        let arm = "       CPU0  CPU1\nIPI0:   10  20  Rescheduling interrupts\n";
        assert_eq!(per_cpu(arm)["IPI0"], [10, 20]);
    }

    #[test]
    fn vmstat_and_steal() {
        let v = vmstat("allocstall_dma 1\nallocstall_normal 2\npgmajfault 7\nnr_free_pages 9\n");
        assert_eq!(v["allocstall"], 3);
        assert_eq!(v["pgmajfault"], 7);
        assert!(!v.contains_key("nr_free_pages"));
        let s = "cpu  1 2 3 4 5 6 7 99 0 0\ncpu0 1 2 3 4 5 6 7 40 0 0\ncpu1 1 2 3 4 5 6 7 59 0 0\n";
        assert_eq!(steal(s), [40, 59]);
    }

    #[test]
    fn this_host() {
        let me = std::process::id();
        let s = system(&[me]);
        for key in ["kernel.release", "cmdline", "clock_gettime_ns", "auditd"] {
            assert!(s.contains_key(key), "{key}");
        }
        assert!(s.contains_key(&format!("pid.{me}.seccomp")));
        let a = Counters::take(&[me]);
        let spin = std::time::Instant::now();
        while spin.elapsed() < std::time::Duration::from_millis(20) {}
        let b = Counters::take(&[me]);
        let i = a.until(&b);
        assert!(!i.interrupts.is_empty() && !i.softirqs.is_empty());
        let link = fs::read_link("/proc/thread-self").unwrap();
        let tid: u32 = link.file_name().unwrap().to_str().unwrap().parse().unwrap();
        let t = i
            .threads
            .iter()
            .find(|t| t.tid == tid)
            .expect("this thread");
        assert!(t.run_ns >= 10_000_000, "{}", t.run_ns);
    }
}
