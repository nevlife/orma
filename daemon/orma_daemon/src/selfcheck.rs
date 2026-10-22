//! ormad's part of `orma selfcheck` (design 6.7): over a window it attaches discovery, hot,
//! rclcpp symbol, bandwidth and dds probes to the target, watches the records, and reports
//! whether the assumptions behind them hold on this machine.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::{Duration, Instant},
};

use orma_common::{bpf::*, events::*};
use orma_proto::{
    msg::{Check, CheckStatus, ExecutorProbes},
    records,
};

use crate::{
    hostcheck,
    libs::Lib,
    probes::{Kernel, Scope},
    procs,
    procs::DistroFrom,
    progs::{PROGS, Set, Sym},
    ring::Head,
    sel::Sel,
    sys,
};

const NS: i64 = 1_000_000_000;

pub struct Session {
    pub client: u64,
    pub request: u64,
    pub duration: Duration,
    /// The `apply` that attached this check's probes.
    pub applied: u64,
    pub until: Option<Instant>,
    pub target: Sel,
    progs: HashMap<&'static str, u64>,
    samples: [u64; SAMPLE_SLOTS as usize],
    /// realtime - monotonic, for the stamp check.
    offset: i64,
    init_topic: HashMap<(u32, u64), String>,
    read_topic: HashMap<(u32, u64), String>,
    /// Per process: the `rmw_publisher_init` handles.
    rmw_pubs: HashMap<u32, HashSet<u64>>,
    /// Per process with a publisher init: `rmw_publish` whose first argument was an init handle,
    /// and those whose was not.
    abi: BTreeMap<u32, (u64, u64)>,
    rcl_abi_seen: HashSet<u32>,
    published: HashMap<(String, i64), u32>,
    rmw_topics: HashSet<String>,
    /// Takes of an rmw ABI topic: matched a publish of another process, all.
    takes: (u64, u64),
    rcl_pub_topic: HashMap<(u32, u64), String>,
    /// Per thread: the topic and message of the last `rcl_publish`, for the `rmw_publish` after.
    rcl_publish: HashMap<(u32, u32), (String, u64)>,
    /// rcl ABI publishes by (topic, header.stamp): the process and the realtime ns.
    rcl_published: HashMap<(String, u64), (u32, i64)>,
    /// Takes paired with another process's rcl ABI publish: source_timestamp in order, all.
    rcl_takes: (u64, u64),
    /// source_timestamp - publish of the takes in order, ns.
    rcl_lag: Vec<i64>,
    tids: HashMap<u32, HashSet<u32>>,
    stamps: BTreeMap<String, (u64, u64)>,
    rcl_subs: HashSet<(u32, u64)>,
    rclcpp_subs: Vec<(u32, u64)>,
    cxx: BTreeMap<u32, BTreeMap<u16, u64>>,
    /// Per thread: `rcl_wait` entries and returns. A return the uretprobe missed (the kernel's
    /// nesting limit, a full return instance pool) leaves an entry unmatched.
    waits: BTreeMap<(u32, u32), (u64, u64)>,
    /// Per process, from the rcl_executor probes: waits, claims, claims with an rcl handle,
    /// completions.
    core: BTreeMap<u32, [u64; 4]>,
    /// Per thread: the last record's event, and whether it was paired as below.
    last: HashMap<(u32, u32), (u16, bool)>,
    /// Per process: `rmw_publish` records, and those next to a DDS write on their thread, the
    /// pairing orma-collectd relies on (design 20.2).
    dds: BTreeMap<u32, (u64, u64)>,
}

impl Session {
    pub fn start(client: u64, request: u64, d: Duration, target: Sel, k: &Kernel) -> Session {
        Session {
            client,
            request,
            duration: d,
            applied: u64::MAX,
            until: None,
            target,
            progs: k
                .prog_stats()
                .into_iter()
                .map(|p| (p.name, p.run_cnt))
                .collect(),
            samples: k.samples(),
            offset: sys::real_ns() as i64 - sys::mono_ns() as i64,
            init_topic: HashMap::new(),
            read_topic: HashMap::new(),
            rmw_pubs: HashMap::new(),
            abi: BTreeMap::new(),
            rcl_abi_seen: HashSet::new(),
            published: HashMap::new(),
            rmw_topics: HashSet::new(),
            takes: (0, 0),
            rcl_pub_topic: HashMap::new(),
            rcl_publish: HashMap::new(),
            rcl_published: HashMap::new(),
            rcl_takes: (0, 0),
            rcl_lag: Vec::new(),
            tids: HashMap::new(),
            stamps: BTreeMap::new(),
            rcl_subs: HashSet::new(),
            rclcpp_subs: Vec::new(),
            cxx: BTreeMap::new(),
            waits: BTreeMap::new(),
            core: BTreeMap::new(),
            last: HashMap::new(),
            dds: BTreeMap::new(),
        }
    }

    fn topic(&self, pid: u32, rmw: u64) -> Option<&String> {
        self.init_topic
            .get(&(pid, rmw))
            .or_else(|| self.read_topic.get(&(pid, rmw)))
    }

    pub fn observe(&mut self, head: Head, rec: &[u8]) {
        if !self.target.contains(head.pid) && self.target != Sel::All {
            return;
        }
        let Some(Ok(r)) = records::positioned(rec).next() else {
            return;
        };
        let pid = head.pid;
        self.tids.entry(pid).or_default().insert(head.tid);
        // The bandwidth probe's size falls inside Fast DDS's write.
        if head.event != ORMA_EV_DDS_SERIALIZED_SIZE {
            self.adjacent(pid, head.tid, head.event);
        }
        let first = |r: &records::Record| -> String {
            String::from_utf8_lossy(r.strings().next().unwrap_or_default()).into_owned()
        };
        match head.event {
            ORMA_EV_RCL_PUBLISHER_INIT | ORMA_EV_RCL_SUBSCRIPTION_INIT => {
                let e: orma_ev_rcl_topic_init = r.payload();
                self.init_topic.insert((pid, e.rmw_handle), first(&r));
                if head.event == ORMA_EV_RCL_SUBSCRIPTION_INIT {
                    self.rcl_subs.insert((pid, e.handle));
                } else {
                    self.rcl_pub_topic.insert((pid, e.handle), first(&r));
                }
            }
            ORMA_EV_RMW_SUBSCRIPTION_TOPIC | ORMA_EV_RMW_PUBLISHER_TOPIC => {
                let e: orma_ev_rmw_topic = r.payload();
                self.read_topic.insert((pid, e.rmw_handle), first(&r));
            }
            ORMA_EV_RMW_PUBLISHER_INIT => {
                let e: orma_ev_rmw_endpoint_init = r.payload();
                self.rmw_pubs.entry(pid).or_default().insert(e.rmw_handle);
            }
            ORMA_EV_RMW_PUBLISH => {
                let e: orma_ev_rmw_publish = r.payload();
                // A process started before the probes went on has no init to compare with.
                if let Some(handles) = self.rmw_pubs.get(&pid) {
                    let a = self.abi.entry(pid).or_default();
                    if handles.contains(&e.rmw_publisher) {
                        a.0 += 1;
                    } else {
                        a.1 += 1;
                    }
                }
                if let Some(t) = self.topic(pid, e.rmw_publisher).cloned() {
                    self.rmw_topics.insert(t.clone());
                    self.published.insert((t.clone(), e.timestamp), pid);
                    self.stamp(t, e.stamp, r.header.t_mono_ns);
                }
            }
            ORMA_EV_RCL_PUBLISH => {
                let e: orma_ev_publish = r.payload();
                if let Some(t) = self.rcl_pub_topic.get(&(pid, e.publisher)) {
                    self.rcl_publish
                        .insert((pid, head.tid), (t.clone(), e.message));
                }
            }
            // The rcl ABI's rmw_publish has no handle and no timestamp: the topic comes from
            // the rcl_publish before it on the thread, the pairing from header.stamp (2.7.1).
            ORMA_EV_RMW_PUBLISH_RCL => {
                self.rcl_abi_seen.insert(pid);
                let e: orma_ev_rmw_publish_rcl = r.payload();
                if let Some((t, message)) = self.rcl_publish.remove(&(pid, head.tid))
                    && message == e.message
                {
                    let at = r.header.t_mono_ns as i64 + self.offset;
                    self.rcl_published.insert((t.clone(), e.stamp), (pid, at));
                    self.stamp(t, e.stamp, r.header.t_mono_ns);
                }
            }
            ORMA_EV_RMW_TAKE => {
                let e: orma_ev_rmw_take = r.payload();
                if e.taken == 0 {
                    return;
                }
                let Some(t) = self.topic(pid, e.rmw_subscription).cloned() else {
                    return;
                };
                if let Some((p, at)) = self.rcl_published.get(&(t.clone(), e.stamp))
                    && *p != pid
                {
                    let s = e.source_timestamp;
                    self.rcl_takes.1 += 1;
                    if *at <= s && s <= r.header.t_mono_ns as i64 + self.offset {
                        self.rcl_takes.0 += 1;
                        self.rcl_lag.push(s - at);
                    }
                } else if self.rmw_topics.contains(&t) {
                    self.takes.1 += 1;
                    if self
                        .published
                        .get(&(t, e.source_timestamp))
                        .is_some_and(|p| *p != pid)
                    {
                        self.takes.0 += 1;
                    }
                }
            }
            ORMA_EV_RCLCPP_SUBSCRIPTION_INIT => {
                let e: orma_ev_rclcpp_subscription_init = r.payload();
                self.rclcpp_subs.push((pid, e.subscription_handle));
            }
            ORMA_EV_RCL_WAIT_ENTER | ORMA_EV_RCL_WAIT_EXIT
                if r.header.flags & ORMA_RECORD_RCL_EXECUTOR != 0 =>
            {
                if head.event == ORMA_EV_RCL_WAIT_ENTER {
                    self.core.entry(pid).or_default()[0] += 1;
                }
            }
            ORMA_EV_RCL_WAIT_ENTER => self.waits.entry((pid, head.tid)).or_default().0 += 1,
            ORMA_EV_RCL_WAIT_EXIT => self.waits.entry((pid, head.tid)).or_default().1 += 1,
            ORMA_EV_RCL_EXECUTOR_CLAIM => {
                let e: orma_ev_rcl_executor_claim = r.payload();
                let c = self.core.entry(pid).or_default();
                c[1] += 1;
                c[2] += u64::from(e.entity != 0);
            }
            ORMA_EV_RCL_EXECUTOR_COMPLETE => self.core.entry(pid).or_default()[3] += 1,
            ev @ ORMA_EV_RCLCPP_PUBLISHER_BASE..=ORMA_EV_RCLCPP_COLLECTOR_ADD_GROUP
            | ev @ ORMA_EV_RCLCPP_EXECUTOR_ADD_GROUP => {
                *self.cxx.entry(pid).or_default().entry(ev).or_default() += 1;
            }
            _ => {}
        }
    }

    fn adjacent(&mut self, pid: u32, tid: u32, event: u16) {
        let publish = |e: u16| matches!(e, ORMA_EV_RMW_PUBLISH | ORMA_EV_RMW_PUBLISH_RCL);
        let before = self.last.get(&(pid, tid)).copied();
        let paired = before.is_some_and(|(e, used)| {
            !used
                && ((publish(event) && e == ORMA_EV_DDS_WRITE)
                    || (event == ORMA_EV_DDS_WRITE && publish(e)))
        });
        self.last.insert((pid, tid), (event, paired));
        if publish(event) {
            self.dds.entry(pid).or_default().0 += 1;
        }
        if paired {
            self.dds.entry(pid).or_default().1 += 1;
        }
    }

    /// The age test of design 2.1.2 for a wall-clock stamp.
    fn stamp(&mut self, topic: String, stamp: u64, t_mono: u64) {
        let s = self.stamps.entry(topic).or_default();
        s.0 += 1;
        let sec = (stamp & 0xffff_ffff) as i64;
        let nsec = (stamp >> 32) as i64;
        if !(1_000_000_000..4_000_000_000).contains(&sec) || nsec >= NS {
            return;
        }
        let age = t_mono as i64 + self.offset - (sec * NS + nsec);
        if (-NS..600 * NS).contains(&age) {
            s.1 += 1;
        }
    }

    /// Which executor probes the process got, and what the rcl_executor ones saw.
    fn executor_probes(&self, k: &Kernel, pid: u32, comm: &str) -> Check {
        let item = format!("executor probes pid {pid} ({comm})");
        if k.executor_probes(pid) != Some(ExecutorProbes::RclExecutor) {
            return Check {
                item,
                status: CheckStatus::Pass,
                detail: "stock: rcl_wait and the rclcpp executor tracepoints".into(),
            };
        }
        let core = k
            .procs
            .get(&pid)
            .into_iter()
            .flat_map(|p| p.files.iter())
            .filter_map(|f| k.files.get(f))
            .find(|f| f.lib == Lib::RclExecutor)
            .map_or("", |f| f.mapped.as_str());
        let [waits, claims, handles, completions] =
            self.core.get(&pid).copied().unwrap_or_default();
        Check {
            item,
            status: if waits + claims > 0 {
                CheckStatus::Pass
            } else {
                CheckStatus::Unknown
            },
            detail: format!(
                "rcl_executor ({core}) and stock: {waits} core waits, {claims} claims \
                 ({handles} with an rcl handle), {completions} completions"
            ),
        }
    }

    pub fn finish(self, k: &mut Kernel) -> Vec<Check> {
        let mut out = Vec::new();
        let check = |item: String, status: CheckStatus, detail: String| Check {
            item,
            status,
            detail,
        };
        out.push(match &k.sched_tp.layout {
            Ok(()) => check(
                "tracepoint layout".into(),
                CheckStatus::Pass,
                "sched_switch and sched_wakeup fields at the built offsets".into(),
            ),
            Err(e) => check("tracepoint layout".into(), CheckStatus::Fail, e.clone()),
        });
        out.push(match &k.futex_tp.layout {
            Ok(()) => check(
                "futex tracepoint layout".into(),
                CheckStatus::Pass,
                "sys_enter_futex, sys_exit_futex and the futex_waitv pair at the built offsets"
                    .into(),
            ),
            Err(e) => check(
                "futex tracepoint layout".into(),
                CheckStatus::Fail,
                e.clone(),
            ),
        });
        let stats = k.prog_stats();
        out.push(check(
            "bpf load".into(),
            CheckStatus::Pass,
            format!(
                "{} programs loaded; the verifier processed {} instructions",
                stats.len(),
                stats.iter().map(|p| p.verified_insns).sum::<u64>()
            ),
        ));
        out.extend(hostcheck::host());
        out.push(check(
            "bpf stats".into(),
            if k.bpf_stats() {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            },
            format!(
                "BPF_ENABLE_STATS {}; {} programs ran",
                if k.bpf_stats() { "held" } else { "refused" },
                stats
                    .iter()
                    .filter(|p| p.run_cnt > self.progs.get(p.name).copied().unwrap_or(0))
                    .count()
            ),
        ));
        for set in [
            Set::Discovery,
            Set::Hot,
            Set::RclcppSymbols,
            Set::Bandwidth,
            Set::Dds,
        ] {
            let attached: std::collections::BTreeSet<&str> = k
                .attachments()
                .filter(|(p, _, _)| p.set == set)
                .map(|(p, _, _)| p.name)
                .collect();
            let fired: Vec<&str> = attached
                .iter()
                .copied()
                .filter(|n| {
                    stats.iter().any(|p| {
                        p.name == *n && p.run_cnt > self.progs.get(n).copied().unwrap_or(0)
                    })
                })
                .collect();
            let idle: Vec<&str> = attached
                .iter()
                .copied()
                .filter(|n| !fired.contains(n))
                .collect();
            out.push(check(
                format!("probes {}", set_name(set)),
                if attached.is_empty() {
                    CheckStatus::Unknown
                } else {
                    CheckStatus::Pass
                },
                format!(
                    "{} attached; fired: {}; not fired: {}",
                    attached.len(),
                    fired.join(" "),
                    idle.join(" ")
                ),
            ));
        }
        let mut reasons: BTreeMap<(&str, &str), Vec<String>> = BTreeMap::new();
        for m in &k.missing {
            reasons
                .entry((m.point, m.reason.as_str()))
                .or_default()
                .push(m.pid.map_or("all".into(), |p| p.to_string()));
        }
        for ((point, reason), pids) in reasons {
            out.push(check(
                format!("missing {point}"),
                CheckStatus::Unknown,
                format!("{reason} (pids {})", pids.join(" ")),
            ));
        }
        for f in k.files.values() {
            let Some(contract) = f.lib.contract() else {
                continue;
            };
            let item = format!("flux contract {}", f.mapped);
            out.push(match &f.refused {
                None => check(item, CheckStatus::Pass, format!("exports {contract}")),
                Some(why) => check(item, CheckStatus::Fail, why.clone()),
            });
        }
        for (pid, p) in &k.procs {
            if !self.target.contains(*pid) {
                continue;
            }
            let from = match p.distro_from {
                DistroFrom::Environ => "ROS_DISTRO",
                DistroFrom::Path => "library path",
                DistroFrom::Unknown => "nothing",
            };
            let (good, bad) = self.abi.get(pid).copied().unwrap_or((0, 0));
            let saw_rcl = self.rcl_abi_seen.contains(pid);
            let (status, seen) = match (p.rcl_abi, good + bad > 0, saw_rcl) {
                // The rmw's own publishers (Cyclone's ros_discovery_info) have no init event.
                (false, true, _) if good > 0 => (CheckStatus::Pass, "rmw (handles match)"),
                (false, true, _) => (
                    CheckStatus::Fail,
                    "rmw_publish's first argument is not a publisher handle: rcl ABI?",
                ),
                (true, _, true) => (CheckStatus::Pass, "rcl"),
                (true, true, _) => (
                    CheckStatus::Fail,
                    "rmw_publish with a handle in an rcl ABI process",
                ),
                _ => (CheckStatus::Unknown, "no publish after a publisher init"),
            };
            out.push(check(
                format!("distro pid {pid} ({})", p.comm),
                status,
                format!(
                    "{} from {from}; publish ABI {} ({seen}); tracetools LTTng {}; gated \
                     callback_register {}; gid {} B",
                    p.distro,
                    if p.rcl_abi { "rcl" } else { "rmw" },
                    opt(p.tracetools_lttng),
                    opt(p.gated),
                    p.gid_len
                ),
            ));
            out.push(self.executor_probes(k, *pid, &p.comm));
        }
        out.extend(self.gpu(k));
        for (pid, p) in k.procs.iter().filter(|(p, _)| self.target.contains(**p)) {
            let (publishes, paired) = self.dds.get(pid).copied().unwrap_or((0, 0));
            let dds = k
                .attachments()
                .any(|(g, _, s)| g.set == Set::Dds && (s == Scope::All || s == Scope::Pid(*pid)));
            if dds && publishes > 0 {
                // A publish at the window's end may have its write after it.
                out.push(check(
                    format!("dds write pid {pid} ({})", p.comm),
                    if paired > 0 && paired + 1 >= publishes {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    },
                    format!(
                        "{paired} of {publishes} rmw_publish next to a DDS write on their thread"
                    ),
                ));
            }
            let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).unwrap_or_default();
            out.extend(hostcheck::preload(*pid, &p.comm, &maps));
            out.extend(hostcheck::timens(*pid, &p.comm));
            let waits: Vec<(u64, u64)> = self
                .waits
                .range((*pid, 0)..=(*pid, u32::MAX))
                .map(|(_, w)| *w)
                .collect();
            if !waits.is_empty() {
                // A thread may be inside rcl_wait at either end of the window.
                let unmatched = waits.iter().filter(|(e, r)| e.abs_diff(*r) > 1).count();
                let (e, r) = waits.iter().fold((0, 0), |a, w| (a.0 + w.0, a.1 + w.1));
                out.push(check(
                    format!("rcl_wait returns pid {pid} ({})", p.comm),
                    if unmatched == 0 {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    },
                    format!(
                        "{e} entries, {r} returns on {} threads; {unmatched} threads differ by \
                         more than one",
                        waits.len()
                    ),
                ));
            }
        }
        for pid in k.procs.keys().filter(|p| self.target.contains(**p)) {
            let ns = procs::nspid(*pid, *pid);
            if ns.len() < 2 {
                continue;
            }
            let host: HashSet<u32> = procs::threads(*pid).into_iter().collect();
            let inner: HashSet<u32> = host
                .iter()
                .filter_map(|t| procs::nspid(*pid, *t).last().copied())
                .filter(|t| !host.contains(t))
                .collect();
            let seen = self.tids.get(pid).cloned().unwrap_or_default();
            let ok = seen.intersection(&host).count();
            let wrong = seen.intersection(&inner).count();
            out.push(check(
                format!("container pid {pid} (uprobe)"),
                match (ok, wrong) {
                    (_, w) if w > 0 => CheckStatus::Fail,
                    (0, _) => CheckStatus::Unknown,
                    _ => CheckStatus::Pass,
                },
                format!(
                    "NSpid {}; record tids: {ok} host tids of the process, {wrong} its container \
                     tids, {} neither (exited)",
                    ns.iter().map(u32::to_string).collect::<Vec<_>>().join(" "),
                    seen.len() - ok - wrong
                ),
            ));
        }
        let (mut same, mut differ) = (0, Vec::new());
        for (key, read) in &self.read_topic {
            if let Some(init) = self.init_topic.get(key) {
                if init == read {
                    same += 1;
                } else {
                    differ.push(format!("{init} != {read}"));
                }
            }
        }
        out.push(check(
            "rmw topic_name offset 16".into(),
            match (same, differ.len()) {
                (_, d) if d > 0 => CheckStatus::Fail,
                (0, _) => CheckStatus::Unknown,
                _ => CheckStatus::Pass,
            },
            format!(
                "{same} equal to the init event's name; {}",
                differ.join(", ")
            ),
        ));
        if self.takes.1 > 0 || self.rcl_abi_seen.is_empty() {
            out.push(check(
                "rmw_publish timestamp = rmw_take source_timestamp".into(),
                match self.takes {
                    (_, 0) => CheckStatus::Unknown,
                    (m, _) if m > 0 => CheckStatus::Pass,
                    _ => CheckStatus::Fail,
                },
                format!(
                    "{} of {} takes matched a publish of another process",
                    self.takes.0, self.takes.1
                ),
            ));
        }
        if !self.rcl_abi_seen.is_empty() {
            let mut lag = self.rcl_lag.clone();
            lag.sort_unstable();
            let us = |q: usize| lag.get(q).map_or(0.0, |v| *v as f64 / 1000.0);
            out.push(check(
                "rmw_take source_timestamp after rmw_publish (rcl ABI)".into(),
                match self.rcl_takes {
                    (_, 0) => CheckStatus::Unknown,
                    (m, n) if m == n => CheckStatus::Pass,
                    _ => CheckStatus::Fail,
                },
                format!(
                    "{} of {} takes paired with another process's publish by header.stamp have \
                     source_timestamp between the publish and the take; it is {:.1} us (p50), \
                     {:.1} us (max) after the publish",
                    self.rcl_takes.0,
                    self.rcl_takes.1,
                    us(lag.len() / 2),
                    us(lag.len().wrapping_sub(1)),
                ),
            ));
        }
        for (topic, (n, ok)) in &self.stamps {
            out.push(check(
                format!("header.stamp {topic}"),
                CheckStatus::Pass,
                format!("{ok} of {n} publishes pass the clock test"),
            ));
        }
        let python: HashSet<u32> = k
            .procs
            .iter()
            .filter(|(_, p)| p.comm.starts_with("python"))
            .map(|(pid, _)| *pid)
            .collect();
        let py: Vec<&(u32, u64)> = self
            .rclcpp_subs
            .iter()
            .filter(|(p, _)| python.contains(p))
            .collect();
        if !py.is_empty() {
            let equal = py.iter().filter(|k| self.rcl_subs.contains(k)).count();
            out.push(check(
                "rclpy handle".into(),
                if equal == py.len() {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                format!("{equal}/{} handles equal", py.len()),
            ));
        }
        let known = k.seen_count(KNOWN_RCLCPP_SUB);
        let linked = k.seen_count(LINKED_RCLCPP_SUB);
        let unvalidated = k.lost()[LOST_UNVALIDATED as usize];
        // rclpy uses no rclcpp group: the row is for processes with librclcpp.
        let rclcpp = |pid: &u32| {
            k.procs.get(pid).is_some_and(|p| {
                p.files
                    .iter()
                    .any(|f| k.files.get(f).is_some_and(|f| f.lib == Lib::Rclcpp))
            })
        };
        for (pid, n) in &known {
            if !self.target.contains(*pid) || !rclcpp(pid) {
                continue;
            }
            let l = linked.get(pid).copied().unwrap_or(0);
            let events = self.cxx.get(pid).cloned().unwrap_or_default();
            let names: Vec<String> = events
                .iter()
                .map(|(ev, c)| {
                    let name = orma_common::catalog::info(*ev).map_or("?", |i| i.name);
                    format!("{name} {c}")
                })
                .collect();
            out.push(check(
                format!("rclcpp symbols pid {pid}"),
                if l > 0 {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                format!(
                    "{l}/{n} subscriptions linked to a group; {}; {unvalidated} arguments no \
                     tracepoint named",
                    names.join(", ")
                ),
            ));
        }
        let s = k.samples();
        let d = |i: u32| s[i as usize] - self.samples[i as usize];
        let n = d(SAMPLE_SERDATA_N);
        let has_cyclone = k.files.values().any(|f| f.lib == Lib::Cyclone);
        if has_cyclone {
            let (a, b) = (d(SAMPLE_SERDATA_72), d(SAMPLE_SERDATA_56));
            let item = "cyclone serdata_rmw::m_size offset".to_string();
            out.push(if n == 0 {
                let why = k
                    .files
                    .values()
                    .filter(|f| f.lib == Lib::Cyclone)
                    .find_map(|f| f.debug_symbols.as_ref().err().cloned())
                    .unwrap_or_else(|| "no Cyclone publish seen".into());
                check(item, CheckStatus::Unknown, why)
            } else {
                let off = if b > a { 56 } else { 72 };
                k.set_serdata_offset(off);
                check(
                    item,
                    if a.max(b) == n {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    },
                    format!("offset {off}: plausible at 72 {a}/{n}, at 56 {b}/{n}"),
                )
            });
        }
        out
    }
}

impl Session {
    /// For each libcuda.so the target maps: the driver functions the gpu programs would attach
    /// to, without attaching them (a CUDA process has some 100 of them, design 20.1).
    fn gpu(&self, k: &Kernel) -> Vec<Check> {
        let mut out = Vec::new();
        let mut files: Vec<_> = k
            .procs
            .iter()
            .filter(|(pid, _)| self.target.contains(**pid))
            .flat_map(|(_, p)| p.files.iter())
            .filter_map(|f| k.files.get(f))
            .filter(|f| f.lib == Lib::Cuda)
            .collect();
        files.sort_by(|a, b| a.mapped.cmp(&b.mapped));
        files.dedup_by(|a, b| a.mapped == b.mapped);
        for f in files {
            let entries = PROGS.iter().filter(|p| p.set == Set::Gpu && !p.ret);
            let mut none = Vec::new();
            let (mut have, mut absent) = (0, Vec::new());
            for p in entries {
                if k.offsets(p, f).is_err() {
                    none.push(p.name);
                }
                if let Sym::Names(names) = p.sym {
                    for n in names {
                        if f.symbols.contains_key(*n) {
                            have += 1;
                        } else {
                            absent.push(*n);
                        }
                    }
                }
            }
            out.push(Check {
                item: format!("gpu symbols {}", f.mapped),
                status: if none.is_empty() {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                detail: format!(
                    "{have} driver functions exported; not exported: {}; programs without one: {}",
                    absent.join(" "),
                    none.join(" ")
                ),
            });
        }
        out
    }
}

fn opt(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

pub fn set_name(s: Set) -> &'static str {
    match s {
        Set::Discovery => "discovery",
        Set::Hot => "hot",
        Set::Bandwidth => "bandwidth",
        Set::RclcppSymbols => "rclcpp_symbols",
        Set::Names => "names",
        Set::Sched => "sched",
        Set::Gpu => "gpu",
        Set::Dds => "dds",
        Set::Futex => "futex",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            client: 0,
            request: 0,
            duration: Duration::ZERO,
            applied: 0,
            until: None,
            target: Sel::All,
            progs: HashMap::new(),
            samples: [0; SAMPLE_SLOTS as usize],
            offset: 0,
            init_topic: HashMap::new(),
            read_topic: HashMap::new(),
            rmw_pubs: HashMap::new(),
            abi: BTreeMap::new(),
            rcl_abi_seen: HashSet::new(),
            published: HashMap::new(),
            rmw_topics: HashSet::new(),
            takes: (0, 0),
            rcl_pub_topic: HashMap::new(),
            rcl_publish: HashMap::new(),
            rcl_published: HashMap::new(),
            rcl_takes: (0, 0),
            rcl_lag: Vec::new(),
            tids: HashMap::new(),
            stamps: BTreeMap::new(),
            rcl_subs: HashSet::new(),
            rclcpp_subs: Vec::new(),
            cxx: BTreeMap::new(),
            waits: BTreeMap::new(),
            core: BTreeMap::new(),
            last: HashMap::new(),
            dds: BTreeMap::new(),
        }
    }

    fn record<T: orma_common::Plain>(event: u16, t: u64, payload: &T, topic: &str) -> Vec<u8> {
        let mut body = orma_common::bytes_of(payload).to_vec();
        if !topic.is_empty() {
            body.extend_from_slice(topic.as_bytes());
            body.push(0);
        }
        let len = (size_of::<orma_record>() + body.len()).next_multiple_of(ORMA_RECORD_ALIGN);
        let header = orma_record {
            event,
            len: len as u16,
            flags: 0,
            t_mono_ns: t,
        };
        let mut out = orma_common::bytes_of(&orma_position { pid: 0, tid: 0 }).to_vec();
        out.extend_from_slice(orma_common::bytes_of(&header));
        out.extend_from_slice(&body);
        out.resize(size_of::<orma_position>() + len, 0);
        out
    }

    fn observe<T: orma_common::Plain>(s: &mut Session, pid: u32, event: u16, t: u64, p: &T) {
        let topic = if matches!(
            event,
            ORMA_EV_RCL_PUBLISHER_INIT | ORMA_EV_RCL_SUBSCRIPTION_INIT
        ) {
            "/t"
        } else {
            ""
        };
        let head = Head {
            pid,
            tid: pid + 1,
            event,
        };
        s.observe(head, &record(event, t, p, topic));
    }

    #[test]
    fn an_rcl_abi_take_pairs_by_header_stamp_and_lies_between_publish_and_take() {
        let mut s = session();
        s.offset = 1_700_000_000 * NS;
        let stamp = 7 << 32 | 1_700_000_000;
        let init = |handle, rmw_handle| orma_ev_rcl_topic_init {
            handle,
            node: 0,
            rmw_handle,
            queue_depth: 10,
        };
        observe(&mut s, 10, ORMA_EV_RCL_PUBLISHER_INIT, 0, &init(0xa, 0xb));
        observe(
            &mut s,
            20,
            ORMA_EV_RCL_SUBSCRIPTION_INIT,
            0,
            &init(0xc, 0xd),
        );
        for (t, message) in [(1000, 0x100), (1900, 0x200)] {
            let publish = orma_ev_publish {
                publisher: 0xa,
                message,
                stamp,
            };
            observe(&mut s, 10, ORMA_EV_RCL_PUBLISH, t, &publish);
        }
        let rmw = orma_ev_rmw_publish_rcl {
            message: 0x200,
            stamp,
        };
        observe(&mut s, 10, ORMA_EV_RMW_PUBLISH_RCL, 2000, &rmw);
        for source_timestamp in [s.offset + 2500, s.offset + 1500] {
            let take = orma_ev_rmw_take {
                rmw_subscription: 0xd,
                message: 0x300,
                source_timestamp,
                stamp,
                taken: 1,
                pad: 0,
            };
            observe(&mut s, 20, ORMA_EV_RMW_TAKE, 5000, &take);
        }
        assert_eq!(s.rcl_takes, (1, 2));
        assert_eq!(s.rcl_lag, vec![500]);
        assert_eq!(s.takes, (0, 0));
        assert_eq!(s.stamps["/t"], (1, 1));
    }

    #[test]
    fn core_waits_are_counted_apart_from_rcl_wait() {
        let mut s = session();
        let enter = orma_ev_rcl_wait_enter::default();
        let exit = orma_ev_rcl_wait_exit::default();
        let mut flagged = record(ORMA_EV_RCL_WAIT_ENTER, 1, &enter, "");
        flagged[size_of::<orma_position>() + 4] = ORMA_RECORD_RCL_EXECUTOR as u8;
        let head = |event| Head {
            pid: 10,
            tid: 11,
            event,
        };
        s.observe(head(ORMA_EV_RCL_WAIT_ENTER), &flagged);
        flagged = record(ORMA_EV_RCL_WAIT_EXIT, 2, &exit, "");
        flagged[size_of::<orma_position>() + 4] = ORMA_RECORD_RCL_EXECUTOR as u8;
        s.observe(head(ORMA_EV_RCL_WAIT_EXIT), &flagged);
        observe(&mut s, 10, ORMA_EV_RCL_WAIT_ENTER, 3, &enter);
        let claim = |entity| orma_ev_rcl_executor_claim {
            entity,
            ..Default::default()
        };
        observe(&mut s, 10, ORMA_EV_RCL_EXECUTOR_CLAIM, 4, &claim(0x20));
        observe(&mut s, 10, ORMA_EV_RCL_EXECUTOR_CLAIM, 5, &claim(0));
        let done = orma_ev_rcl_executor_complete::default();
        observe(&mut s, 10, ORMA_EV_RCL_EXECUTOR_COMPLETE, 6, &done);
        assert_eq!(s.core[&10], [1, 2, 1, 1]);
        assert_eq!(s.waits[&(10, 11)], (1, 0));
    }

    #[test]
    fn the_stamp_test_takes_recent_wall_clock_stamps_only() {
        let mut s = session();
        let now = 1_700_000_000 * NS;
        s.offset = now - 5 * NS;
        let at = |sec: i64| (sec as u64) | (5u64 << 32);
        s.stamp("/t".into(), at(1_700_000_000), 5 * NS as u64);
        s.stamp("/t".into(), at(42), 5 * NS as u64);
        s.stamp("/t".into(), at(1_700_000_000 - 700), 5 * NS as u64);
        s.stamp("/t".into(), at(1_700_000_002), 5 * NS as u64);
        assert_eq!(s.stamps["/t"], (4, 1));
    }

    #[test]
    fn a_publish_pairs_with_the_dds_write_next_to_it_on_its_thread() {
        let mut s = session();
        let publish = orma_ev_rmw_publish::default();
        let write = orma_ev_dds_write::default();
        let size = orma_ev_dds_serialized_size { size: 18 };
        // Fast DDS: publish, (size), write. Cyclone with the tracepoint after the write.
        observe(&mut s, 10, ORMA_EV_RMW_PUBLISH, 1, &publish);
        observe(&mut s, 10, ORMA_EV_DDS_SERIALIZED_SIZE, 2, &size);
        observe(&mut s, 10, ORMA_EV_DDS_WRITE, 3, &write);
        observe(&mut s, 10, ORMA_EV_RMW_PUBLISH, 4, &publish);
        observe(&mut s, 20, ORMA_EV_DDS_WRITE, 1, &write);
        observe(&mut s, 20, ORMA_EV_RMW_PUBLISH, 2, &publish);
        // A service request's write after a publish whose write was not seen.
        let wait = orma_ev_rcl_wait_enter::default();
        observe(&mut s, 20, ORMA_EV_RMW_PUBLISH, 3, &publish);
        observe(&mut s, 20, ORMA_EV_RCL_WAIT_ENTER, 4, &wait);
        observe(&mut s, 20, ORMA_EV_DDS_WRITE, 5, &write);
        assert_eq!(s.dds[&10], (2, 1));
        assert_eq!(s.dds[&20], (2, 1));
    }

    #[test]
    fn publish_handles_are_judged_only_in_processes_whose_publisher_init_was_seen() {
        let mut s = session();
        let publish = |rmw_publisher| orma_ev_rmw_publish {
            rmw_publisher,
            ..Default::default()
        };
        observe(&mut s, 10, ORMA_EV_RMW_PUBLISH, 1, &publish(0x100));
        assert!(!s.abi.contains_key(&10));
        let init = orma_ev_rmw_endpoint_init {
            rmw_handle: 0x200,
            ..Default::default()
        };
        observe(&mut s, 20, ORMA_EV_RMW_PUBLISHER_INIT, 1, &init);
        observe(&mut s, 20, ORMA_EV_RMW_PUBLISH, 2, &publish(0x200));
        observe(&mut s, 20, ORMA_EV_RMW_PUBLISH, 3, &publish(0x300));
        assert_eq!(s.abi[&20], (1, 1));
    }
}
