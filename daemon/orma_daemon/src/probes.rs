//! The loaded programs and maps, and the attachments that follow what the clients ask for
//! (design 6.1, 12.8). Attachments are computed as a whole from the wanted sets and the
//! processes' maps, and applied as a difference.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    error::Error,
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    num::NonZeroU32,
    os::fd::{AsFd, AsRawFd, OwnedFd},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Receiver, Sender},
    },
};

use aya::{
    Ebpf, include_bytes_aligned,
    maps::{Array, HashMap as BpfHashMap, MapData, RingBuf},
    programs::{
        TracePoint, UProbe,
        trace_point::TracePointLinkId,
        uprobe::{UProbeAttachLocation, UProbeAttachPoint, UProbeLink, UProbeScope},
    },
    sys::{Stats, enable_stats},
};
use orma_common::bpf::*;
use orma_proto::msg::{ExecutorProbes, ProbeChange};

use crate::{
    libs::{self, FileKey, Lib, LibFile},
    procs::{self, DistroFrom, Mapped},
    progs::{Cond, FUTEX_PROGS, PROGS, Prog, SCHED_PROGS, Set, Sym},
    sel::Sel,
    sys,
};

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    All,
    Pid(u32),
}

/// The file as the kernel's uprobes see it: inode number and a hash of the build id. Containers
/// of one image map the same lower file through their own overlay devices, so their `FileKey`s
/// differ while their uprobes are the same (design 2.7.2).
type FileId = (u64, u64);

fn file_id(key: FileKey, f: &LibFile) -> FileId {
    let mut h = DefaultHasher::new();
    f.build_id.hash(&mut h);
    (key.1, h.finish())
}

type LinkKey = (usize, FileId, Scope);

/// Links of one program on one file that would both fire for the same process: one for every
/// process and one for a pid. Links for two pids never meet.
fn change(attach: bool, prog: &str, scope: Scope, points: usize, from_ns: u64) -> ProbeChange {
    ProbeChange {
        from_ns,
        to_ns: sys::mono_ns(),
        attach,
        prog: prog.to_string(),
        pid: match scope {
            Scope::All => None,
            Scope::Pid(p) => Some(p),
        },
        points: points as u32,
    }
}

fn overlap(a: &LinkKey, b: &LinkKey) -> bool {
    (a.0, a.1) == (b.0, b.1) && a.2 != b.2 && (a.2 == Scope::All || b.2 == Scope::All)
}

/// Links on their way out. The kernel removes uprobes one at a time, 1.1 s each on this RT kernel
/// (13 s for one process's hot set), so another thread drops them while the loop keeps draining
/// the ring buffer. A link fires until it is dropped: `take_back` keeps an attach from overlapping
/// one, which would emit every event of that point twice.
#[derive(Clone, Default)]
struct Reaper(Arc<(Mutex<Reaping>, Condvar)>);

#[derive(Default)]
struct Reaping {
    changes: Vec<ProbeChange>,
    queue: VecDeque<(LinkKey, Vec<UProbeLink>)>,
    dropping: Option<LinkKey>,
    /// Attaches queued or running. The kernel takes uprobes on and off one at a time, so a drop
    /// in progress holds an attach up to 1.1 s; drops wait while a recording waits for probes.
    attaching: usize,
}

impl Reaper {
    fn start() -> Self {
        let r = Reaper::default();
        let shared = r.clone();
        std::thread::spawn(move || {
            let (lock, cv) = &*shared.0;
            let mut s = lock.lock().expect("reaper lock");
            loop {
                if s.attaching > 0 {
                    s = cv.wait(s).expect("reaper lock");
                    continue;
                }
                let Some((key, links)) = s.queue.pop_front() else {
                    s = cv.wait(s).expect("reaper lock");
                    continue;
                };
                s.dropping = Some(key);
                drop(s);
                let (from, n) = (sys::mono_ns(), links.len());
                drop(links);
                s = lock.lock().expect("reaper lock");
                s.changes
                    .push(change(false, PROGS[key.0].name, key.2, n, from));
                s.dropping = None;
                cv.notify_all();
            }
        });
        r
    }

    fn attaching(&self, change: isize) {
        let (lock, cv) = &*self.0;
        let mut s = lock.lock().expect("reaper lock");
        s.attaching = s
            .attaching
            .checked_add_signed(change)
            .expect("as many done as queued");
        cv.notify_all();
    }

    fn log(&self, c: ProbeChange) {
        self.0.0.lock().expect("reaper lock").changes.push(c);
    }

    fn take_changes(&self) -> Vec<ProbeChange> {
        std::mem::take(&mut self.0.0.lock().expect("reaper lock").changes)
    }

    fn send(&self, key: LinkKey, links: Vec<UProbeLink>) {
        let (lock, cv) = &*self.0;
        lock.lock()
            .expect("reaper lock")
            .queue
            .push_back((key, links));
        cv.notify_all();
    }

    /// The queued links of keys wanted again, before the reaper gets to them. One that a queued
    /// or dropping link would `overlap` stays for `take_back`, which drops that one first so
    /// that no event fires twice.
    fn reclaim(&self, wanted: impl Fn(&LinkKey) -> bool) -> Vec<(LinkKey, Vec<UProbeLink>)> {
        let (lock, _) = &*self.0;
        let mut s = lock.lock().expect("reaper lock");
        let mut out = Vec::new();
        let mut i = 0;
        while i < s.queue.len() {
            let k = s.queue[i].0;
            let clash = s
                .dropping
                .iter()
                .chain(s.queue.iter().map(|q| &q.0))
                .any(|o| overlap(o, &k));
            if wanted(&k) && !clash {
                out.push(s.queue.remove(i).expect("in range"));
            } else {
                i += 1;
            }
        }
        out
    }

    /// Before `key` is attached: its own queued links come back for reuse, ones that `overlap` it
    /// are dropped now, and one being dropped is waited for.
    fn take_back(&self, key: LinkKey) -> Option<Vec<UProbeLink>> {
        let (lock, cv) = &*self.0;
        let same = |k: &LinkKey| *k == key || overlap(k, &key);
        let mut s = lock.lock().expect("reaper lock");
        let (mut mine, mut other) = (None, Vec::new());
        let mut i = 0;
        while i < s.queue.len() {
            if !same(&s.queue[i].0) {
                i += 1;
                continue;
            }
            let (k, links) = s.queue.remove(i).expect("in range");
            if k == key {
                mine = Some(links);
            } else {
                other.push((k, links));
            }
        }
        while s.dropping.as_ref().is_some_and(same) {
            s = cv.wait(s).expect("reaper lock");
        }
        drop(s);
        for (k, links) in other {
            let (from, n) = (sys::mono_ns(), links.len());
            drop(links);
            self.log(change(false, PROGS[k.0].name, k.2, n, from));
        }
        mine
    }
}

/// One link for the attacher thread to make.
struct Job {
    key: LinkKey,
    path: PathBuf,
    offsets: Vec<u64>,
}

type Done = (LinkKey, std::result::Result<Vec<UProbeLink>, String>);

/// Attaches on its own thread. An attach takes from milliseconds to seconds on this RT kernel
/// (seconds while the reaper drops links), and the loop must keep draining the ring buffer and
/// closing windows on time meanwhile (design 8.6).
fn attacher(ebpf: Arc<Mutex<Ebpf>>, reaper: Reaper, jobs: Receiver<Job>, done: Sender<Done>) {
    for j in jobs {
        let result = match reaper.take_back(j.key) {
            Some(links) => Ok(links),
            None => {
                let from = sys::mono_ns();
                let name = PROGS[j.key.0].name;
                let r = attach(&mut ebpf.lock().expect("ebpf lock"), &j)
                    .map_err(|e| format!("attach {name}: {e:?}"));
                if let Ok(links) = &r {
                    reaper.log(change(true, name, j.key.2, links.len(), from));
                }
                r
            }
        };
        reaper.attaching(-1);
        if done.send((j.key, result)).is_err() {
            return;
        }
    }
}

/// Dropping the returned links detaches them, so a failure part way detaches the others.
fn attach(ebpf: &mut Ebpf, j: &Job) -> Result<Vec<UProbeLink>> {
    // The pid is also the cookie the return programs compare (orma_ebpf maps.rs `mine`).
    let (scope, cookie) = match j.key.2 {
        Scope::All => (UProbeScope::AllProcesses, 0),
        Scope::Pid(p) => (
            UProbeScope::OneProcess(NonZeroU32::new(p).ok_or("pid 0")?),
            p as u64,
        ),
    };
    let u: &mut UProbe = ebpf
        .program_mut(PROGS[j.key.0].name)
        .ok_or("program missing")?
        .try_into()?;
    let mut links = Vec::new();
    for off in &j.offsets {
        let point = UProbeAttachPoint {
            location: UProbeAttachLocation::AbsoluteOffset(*off),
            cookie: Some(cookie),
        };
        let id = u.attach(point, &j.path, scope)?;
        links.push(u.take_link(id)?);
    }
    Ok(links)
}

/// A process that maps at least one library ormad attaches to.
#[derive(Clone, Debug)]
pub struct Proc {
    pub start: u64,
    pub comm: String,
    pub distro: String,
    pub distro_from: DistroFrom,
    pub files: Vec<FileKey>,
    /// `RMW_GID_STORAGE_SIZE` of its distro: 24 on Humble, 16 after (design 13.4).
    pub gid_len: u32,
    /// Its distro has the rcl publish ABI (design 2.7.2).
    pub rcl_abi: bool,
    /// Its librclcpp calls `rclcpp_callback_register` only through the gate (2.4).
    pub gated: Option<bool>,
    pub tracetools_lttng: Option<bool>,
    /// It maps librclcpp.so: the rcl_executor client that claims is rclcpp's.
    pub rclcpp: bool,
}

/// What a set's attachments could not cover: `pid` None for a file every process shares.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Missing {
    pub pid: Option<u32>,
    pub point: &'static str,
    pub reason: String,
}

/// A note for the operator: a library attached or refused.
pub struct Note {
    pub warn: bool,
    pub text: String,
}

/// One program's cumulative counters.
pub struct ProgStat {
    pub name: &'static str,
    pub run_time_ns: u64,
    pub run_cnt: u64,
    /// Runs skipped because a BPF program was already running on that CPU (5.12+).
    pub recursion_misses: u64,
    /// Instructions the verifier processed at load (5.16+).
    pub verified_insns: u64,
    pub attached: bool,
}

/// The `name: value` counters of a BPF object's fdinfo. Unlike a lookup by id this needs no
/// CAP_SYS_ADMIN.
fn fdinfo(fd: &OwnedFd) -> HashMap<String, u64> {
    parse_fdinfo(
        &fs::read_to_string(format!("/proc/self/fdinfo/{}", fd.as_raw_fd())).unwrap_or_default(),
    )
}

fn parse_fdinfo(text: &str) -> HashMap<String, u64> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.to_string(), v.trim().parse().ok()?))
        })
        .collect()
}

/// A tracepoint field orma_ebpf reads: event, field, offset, size.
type Field = (&'static str, &'static str, usize, usize);

const SCHED_FIELDS: [Field; 5] = [
    ("sched_switch", "prev_pid", 24, 4),
    ("sched_switch", "prev_state", 32, 8),
    ("sched_switch", "next_pid", 56, 4),
    ("sched_wakeup", "pid", 24, 4),
    ("sched_wakeup", "target_cpu", 32, 4),
];

const FUTEX_FIELDS: [Field; 5] = [
    ("sys_enter_futex", "uaddr", 16, 8),
    ("sys_enter_futex", "op", 24, 8),
    ("sys_exit_futex", "ret", 16, 8),
    ("sys_enter_futex_waitv", "waiters", 16, 8),
    ("sys_exit_futex_waitv", "ret", 16, 8),
];

/// The tracepoint programs of one probe set, attached together while the set is wanted.
pub struct Tracepoints {
    system: &'static str,
    progs: &'static [&'static str],
    /// The collection points `probes_missing` names when the layout differs.
    points: &'static [&'static str],
    /// This kernel's layout against the offsets the programs were built with (design 4.2).
    pub layout: std::result::Result<(), String>,
    links: Vec<(&'static str, TracePointLinkId)>,
}

impl Tracepoints {
    fn new(
        system: &'static str,
        progs: &'static [&'static str],
        points: &'static [&'static str],
        fields: &[Field],
    ) -> Self {
        Tracepoints {
            system,
            progs,
            points,
            layout: check_layout(system, fields),
            links: Vec::new(),
        }
    }
}

pub struct Kernel {
    pub events: RingBuf<MapData>,
    config: Array<MapData, u64>,
    stream: BpfHashMap<MapData, u32, u8>,
    sched: BpfHashMap<MapData, u32, u8>,
    futex: BpfHashMap<MapData, u32, u8>,
    tid_tgid: BpfHashMap<MapData, u32, u32>,
    user_words: BpfHashMap<MapData, u32, u64>,
    agg: BpfHashMap<MapData, AggKey, AggVal>,
    seen: BpfHashMap<MapData, SeenKey, u8>,
    lost: Array<MapData, u64>,
    sample: Array<MapData, u64>,
    /// name, fd, max entries of every map.
    maps: Vec<(String, OwnedFd, u32)>,
    /// The BPF_ENABLE_STATS fd, or why the kernel refused it.
    stats: std::result::Result<OwnedFd, String>,
    generation: u64,
    pub files: HashMap<FileKey, LibFile>,
    pub procs: BTreeMap<u32, Proc>,
    /// Each link with the file it was attached through.
    links: HashMap<LinkKey, (FileKey, Vec<UProbeLink>)>,
    reaper: Reaper,
    desired: BTreeMap<LinkKey, FileKey>,
    jobs: Sender<Job>,
    done: Receiver<Done>,
    /// Links sent to the attacher, by the `apply` that sent them.
    in_flight: HashMap<LinkKey, u64>,
    /// Why a wanted link failed, until an attach of it succeeds.
    failed: HashMap<LinkKey, Missing>,
    applied: u64,
    /// Program fds by name, for the run statistics without the lock the attacher holds.
    prog_fds: Vec<(&'static str, OwnedFd)>,
    pub sched_tp: Tracepoints,
    pub futex_tp: Tracepoints,
    pub missing: BTreeSet<Missing>,
    deleted: HashSet<(u32, u64)>,
    pub notes: Vec<Note>,
    // Dropping this detaches the probes, so it lives as long as the maps are read.
    ebpf: Arc<Mutex<Ebpf>>,
}

fn take<T: TryFrom<aya::maps::Map>>(ebpf: &mut Ebpf, name: &str) -> Result<T>
where
    <T as TryFrom<aya::maps::Map>>::Error: Error + Send + Sync + 'static,
{
    let map = ebpf
        .take_map(name)
        .ok_or_else(|| format!("map {name} missing from the object"))?;
    Ok(T::try_from(map)?)
}

fn check_layout(system: &str, fields: &[Field]) -> std::result::Result<(), String> {
    for (event, field, offset, size) in fields {
        let path = format!("/sys/kernel/tracing/events/{system}/{event}/format");
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        let want = format!(" {field};\toffset:{offset};\tsize:{size};");
        if !text.lines().any(|l| l.contains(&want)) {
            return Err(format!(
                "{event}.{field} is not at offset {offset} size {size} in this kernel's {path}"
            ));
        }
    }
    Ok(())
}

impl Kernel {
    /// Load every program; nothing fires until something is attached.
    pub fn load() -> Result<Self> {
        let mut ebpf = Ebpf::load(include_bytes_aligned!(concat!(env!("OUT_DIR"), "/orma")))?;
        for p in PROGS {
            let u: &mut UProbe = ebpf
                .program_mut(p.name)
                .ok_or_else(|| format!("program {} missing from the object", p.name))?
                .try_into()?;
            u.load().map_err(|e| format!("load {}: {e}", p.name))?;
        }
        for name in SCHED_PROGS.iter().chain(&FUTEX_PROGS) {
            let t: &mut TracePoint = ebpf
                .program_mut(name)
                .ok_or_else(|| format!("program {name} missing from the object"))?
                .try_into()?;
            t.load().map_err(|e| format!("load {name}: {e}"))?;
        }
        let maps = ebpf
            .maps()
            .filter_map(|(name, m)| {
                let d = match m {
                    aya::maps::Map::RingBuf(d)
                    | aya::maps::Map::Array(d)
                    | aya::maps::Map::HashMap(d)
                    | aya::maps::Map::LruHashMap(d) => d,
                    _ => return None,
                };
                let max = d.info().ok()?.max_entries();
                Some((
                    name.to_string(),
                    d.fd().as_fd().try_clone_to_owned().ok()?,
                    max,
                ))
            })
            .collect();
        let prog_fds = PROGS
            .iter()
            .map(|p| p.name)
            .chain(SCHED_PROGS)
            .chain(FUTEX_PROGS)
            .map(|name| {
                let p = ebpf.program(name).ok_or("program missing")?;
                Ok((name, p.fd()?.as_fd().try_clone_to_owned()?))
            })
            .collect::<Result<_>>()?;
        // Held for ormad's life: the kernel counts run time only while some fd asks for it.
        let stats = enable_stats(Stats::RunTime).map_err(|e| {
            format!(
                "BPF_ENABLE_STATS refused ({}): eBPF run time and count stay 0",
                e.io_error
            )
        });
        let reaper = Reaper::start();
        let (jobs, rx) = mpsc::channel();
        let (tx, done) = mpsc::channel();
        let r = reaper.clone();
        let mut k = Kernel {
            events: take(&mut ebpf, "EVENTS")?,
            config: take(&mut ebpf, "CONFIG")?,
            stream: take(&mut ebpf, "STREAM")?,
            sched: take(&mut ebpf, "SCHED")?,
            futex: take(&mut ebpf, "FUTEX")?,
            tid_tgid: take(&mut ebpf, "TID_TGID")?,
            user_words: take(&mut ebpf, "USER_WORDS")?,
            agg: take(&mut ebpf, "AGG")?,
            seen: take(&mut ebpf, "SEEN")?,
            lost: take(&mut ebpf, "LOST")?,
            sample: take(&mut ebpf, "SAMPLE")?,
            maps,
            stats,
            generation: 1,
            files: HashMap::new(),
            procs: BTreeMap::new(),
            links: HashMap::new(),
            reaper,
            desired: BTreeMap::new(),
            jobs,
            done,
            in_flight: HashMap::new(),
            failed: HashMap::new(),
            applied: 0,
            prog_fds,
            sched_tp: Tracepoints::new(
                "sched",
                &SCHED_PROGS,
                &["sched_switch", "sched_wakeup"],
                &SCHED_FIELDS,
            ),
            futex_tp: Tracepoints::new("syscalls", &FUTEX_PROGS, &["futex_wait"], &FUTEX_FIELDS),
            missing: BTreeSet::new(),
            deleted: HashSet::new(),
            notes: Vec::new(),
            ebpf: {
                let e = Arc::new(Mutex::new(ebpf));
                let shared = e.clone();
                std::thread::spawn(move || attacher(shared, r, rx, tx));
                e
            },
        };
        k.config.set(CONFIG_GEN, k.generation, 0)?;
        // Loading is reported apart from attaching, which the notes and probes_missing report.
        eprintln!("ormad: loaded and verified {} programs", k.prog_fds.len());
        if let Err(why) = &k.stats {
            k.note(true, why.clone());
        }
        Ok(k)
    }

    pub fn bpf_stats(&self) -> bool {
        self.stats.is_ok()
    }

    fn note(&mut self, warn: bool, text: String) {
        self.notes.push(Note { warn, text });
    }

    /// Reads the maps of `pids` (or of every process), learning new processes and files.
    pub fn scan(&mut self, pids: Option<&BTreeSet<u32>>) {
        let all;
        let list: Vec<u32> = match pids {
            Some(p) => p.iter().copied().collect(),
            None => {
                all = procs::pids();
                all
            }
        };
        for pid in list {
            let Some(maps) = procs::mapped(pid) else {
                continue;
            };
            if maps.is_empty() {
                continue;
            }
            let Some(start) = procs::starttime(pid) else {
                continue;
            };
            self.learn(pid, start, &maps);
        }
    }

    /// Adds a library named on the command line, for the text mode's `--lib`.
    pub fn add_lib(&mut self, path: &Path, pid: Option<u32>) -> Result<()> {
        let mapped = path.to_str().ok_or("path is not UTF-8")?;
        let lib = Lib::of(mapped).ok_or_else(|| {
            format!("{mapped}: not a library ormad attaches to (libtracetools.so, ...)")
        })?;
        let (key, file) = libs::read(lib, Path::new("/"), mapped)?;
        self.files.entry(key).or_insert(file);
        if let Some(pid) = pid
            && let Some(p) = self.procs.get_mut(&pid)
            && !p.files.contains(&key)
        {
            p.files.push(key);
        }
        Ok(())
    }

    fn learn(&mut self, pid: u32, start: u64, maps: &[Mapped]) {
        let root = procs::root(pid);
        let mut keys = Vec::new();
        for m in maps {
            if m.deleted {
                if self.deleted.insert((pid, m.inode)) {
                    self.note(
                        true,
                        format!(
                            "not attached to {} (pid {pid}): replaced on disk since it was \
                             loaded; restart the process to trace it",
                            m.path
                        ),
                    );
                }
                continue;
            }
            let path = root.join(m.path.trim_start_matches('/'));
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            use std::os::unix::fs::MetadataExt;
            let key = (meta.dev(), meta.ino());
            // An inode number is reused once its file is gone, so a known key may be a new file.
            if !self.files.get(&key).is_some_and(|f| opens(f, key)) {
                match libs::read(m.lib, &root, &m.path) {
                    Ok((k, mut file)) => {
                        // The host's own path outlives the process, when it is the same file.
                        if fs::metadata(&m.path).is_ok_and(|h| (h.dev(), h.ino()) == k) {
                            file.path = PathBuf::from(&m.path);
                        }
                        if let Some(why) = &file.refused {
                            self.note(
                                true,
                                format!("not attached to {} (pid {pid}): {why}", m.path),
                            );
                        }
                        self.files.insert(k, file);
                    }
                    Err(e) => {
                        self.note(true, format!("not attached to {} (pid {pid}): {e}", m.path));
                        continue;
                    }
                }
            }
            keys.push(key);
        }
        let fresh = self.procs.get(&pid).is_none_or(|p| p.start != start);
        if fresh {
            let (distro, from) = procs::distro(pid, maps);
            let tracetools = keys
                .iter()
                .filter_map(|k| self.files.get(k))
                .find(|f| f.lib == Lib::Tracetools);
            let rclcpp = keys
                .iter()
                .filter_map(|k| self.files.get(k))
                .find(|f| f.lib == Lib::Rclcpp);
            let rcl_abi = procs::rcl_abi_distro(&distro);
            let gid_len = if rcl_abi { 24 } else { 16 };
            let gated = rclcpp.map(|f| {
                f.imports
                    .iter()
                    .any(|i| i == "ros_trace_do_rclcpp_callback_register")
            });
            let proc = Proc {
                start,
                comm: procs::comm(pid),
                distro,
                distro_from: from,
                files: keys,
                gid_len,
                rcl_abi,
                gated,
                tracetools_lttng: tracetools.map(|f| f.needs_lttng),
                rclcpp: rclcpp.is_some(),
            };
            self.procs.insert(pid, proc);
        } else if let Some(p) = self.procs.get_mut(&pid) {
            for k in keys {
                if !p.files.contains(&k) {
                    p.rclcpp |= self.files.get(&k).is_some_and(|f| f.lib == Lib::Rclcpp);
                    p.files.push(k);
                }
            }
        }
    }

    /// The executor probes `pid` gets, from the files it maps (docs/instrumentation.md 2.12).
    pub fn executor_probes(&self, pid: u32) -> Option<ExecutorProbes> {
        let p = self.procs.get(&pid)?;
        Some(libs::executor_probes(
            p.files.iter().filter_map(|k| self.files.get(k)),
        ))
    }

    /// Forgets exited processes and their kernel entries.
    pub fn prune(&mut self) -> Vec<u32> {
        let dead: Vec<u32> = self
            .procs
            .iter()
            .filter(|(pid, p)| !procs::alive(**pid, p.start))
            .map(|(pid, _)| *pid)
            .collect();
        for pid in &dead {
            self.procs.remove(pid);
        }
        // A file no process maps any more whose path no longer opens it (deleted): every attach to
        // it would fail. One a live process maps keeps its links; the next scan of that process
        // finds it a path again.
        let mapped: HashSet<FileKey> = self
            .procs
            .values()
            .flat_map(|p| p.files.iter().copied())
            .collect();
        self.files
            .retain(|key, f| mapped.contains(key) || opens(f, *key));
        let gone = |tgid: u32| -> bool {
            dead.contains(&tgid) || !Path::new("/proc").join(tgid.to_string()).exists()
        };
        let agg: Vec<AggKey> = self
            .agg
            .keys()
            .filter_map(|k| k.ok())
            .filter(|k| gone(k.tgid))
            .collect();
        for k in &agg {
            let _ = self.agg.remove(k);
        }
        let seen: Vec<SeenKey> = self
            .seen
            .keys()
            .filter_map(|k| k.ok())
            .filter(|k| gone(k.tgid))
            .collect();
        for k in &seen {
            let _ = self.seen.remove(k);
        }
        dead
    }

    /// Clears what a process showed once (code words, topics, callback owners), so a new client
    /// gets it again.
    pub fn forget_seen(&mut self) {
        let keys: Vec<SeenKey> = self
            .seen
            .keys()
            .filter_map(|k| k.ok())
            .filter(|k| {
                matches!(
                    k.what,
                    SEEN_CODE | SEEN_SUB_TOPIC | SEEN_PUB_TOPIC | SEEN_CALLBACK_OWNER
                )
            })
            .collect();
        for k in &keys {
            let _ = self.seen.remove(k);
        }
    }

    /// The offsets `prog` attaches to in `file`, or why it does not.
    pub fn offsets(&self, prog: &Prog, file: &LibFile) -> std::result::Result<Vec<u64>, String> {
        let name = file
            .mapped
            .rsplit('/')
            .next()
            .unwrap_or(file.mapped.as_str());
        match prog.sym {
            Sym::Trace(tp) => file
                .symbols
                .get(&format!("ros_trace_{tp}"))
                .map(|o| vec![*o])
                .ok_or_else(|| format!("{name} does not export ros_trace_{tp}")),
            Sym::Name(n) => file
                .symbols
                .get(n)
                .map(|o| vec![*o])
                .ok_or_else(|| format!("{name} does not export {n}")),
            Sym::Prefix(prefix, overloads) => libs::by_prefix(&file.symbols, prefix, overloads)
                .map_err(|e| format!("{name}: {e}")),
            Sym::Names(names) => {
                let mut offsets: Vec<u64> = names
                    .iter()
                    .filter_map(|n| file.symbols.get(*n).copied())
                    .collect();
                offsets.sort_unstable();
                offsets.dedup();
                if offsets.is_empty() {
                    return Err(format!("{name} exports none of {}", names.join(" ")));
                }
                Ok(offsets)
            }
            Sym::Debug(prefix) => {
                let syms = file
                    .debug_symbols
                    .as_ref()
                    .map_err(|e| format!("{name}: {e}"))?;
                libs::by_prefix(syms, prefix, false).map_err(|e| format!("{name}: {e}"))
            }
        }
    }

    /// Whether `prog` belongs on `file` at all: the ABI and rclcpp major it was written for.
    fn applies(prog: &Prog, file: &LibFile) -> bool {
        let rcl_abi =
            procs::rcl_abi_distro(&libs::distro_of_path(&file.mapped).unwrap_or_default());
        match prog.cond {
            Cond::Always => true,
            Cond::RmwAbi => !rcl_abi,
            Cond::RclAbi => rcl_abi,
            Cond::Rclcpp16 => file.rclcpp_major == Some(16),
            Cond::Rclcpp28 => file.rclcpp_major.is_some_and(|m| m >= 28),
        }
    }

    /// Brings the attachments and the filter maps in line with `want` (design 12.8).
    pub fn apply(
        &mut self,
        want: &BTreeMap<Set, Sel>,
        stream: &Sel,
        user_words: &BTreeMap<u32, u64>,
    ) {
        let mut desired: BTreeMap<LinkKey, FileKey> = BTreeMap::new();
        let mut missing: BTreeSet<Missing> = (self.stats.as_ref().err())
            .map(|why| Missing {
                pid: None,
                point: "bpf_stats",
                reason: why.clone(),
            })
            .into_iter()
            .collect();
        for (set, sel) in want {
            if matches!(set, Set::Sched | Set::Futex) {
                continue;
            }
            let targets: Vec<(Scope, Vec<FileKey>)> = match sel {
                Sel::None => continue,
                Sel::All => vec![(Scope::All, self.distinct_files())],
                Sel::Pids(pids) => pids
                    .iter()
                    .filter_map(|p| Some((Scope::Pid(*p), self.procs.get(p)?.files.clone())))
                    .collect(),
            };
            for (scope, files) in targets {
                let pid = match scope {
                    Scope::All => None,
                    Scope::Pid(p) => Some(p),
                };
                for key in files {
                    let Some(file) = self.files.get(&key) else {
                        continue;
                    };
                    for (i, prog) in PROGS.iter().enumerate() {
                        if prog.set != *set || prog.lib != file.lib || !Self::applies(prog, file) {
                            continue;
                        }
                        if let Some(why) = &file.refused {
                            missing.insert(Missing {
                                pid,
                                point: prog.point,
                                reason: why.clone(),
                            });
                            continue;
                        }
                        match self.offsets(prog, file) {
                            Ok(_) => {
                                desired.insert((i, file_id(key, file), scope), key);
                            }
                            Err(reason) => {
                                missing.insert(Missing {
                                    pid,
                                    point: prog.point,
                                    reason,
                                });
                            }
                        }
                    }
                }
                if *set == Set::Hot
                    && let Some(p) = pid.and_then(|p| self.procs.get(&p))
                    && p.gated == Some(true)
                {
                    missing.insert(Missing {
                        pid,
                        point: "rclcpp_callback_register",
                        reason: "rclcpp calls it only when a gate says yes, which a uprobe cannot \
                                 do; callbacks are named from callback_code (design 2.4)"
                            .into(),
                    });
                }
            }
        }
        let stale: Vec<LinkKey> = self
            .links
            .keys()
            .filter(|k| !desired.contains_key(k))
            .copied()
            .collect();
        for k in stale {
            if let Some((_, links)) = self.links.remove(&k) {
                self.reaper.send(k, links);
            }
        }
        self.failed.retain(|k, _| desired.contains_key(k));
        missing.extend(self.failed.values().cloned());
        self.missing = missing;
        self.apply_maps(want, stream, user_words);
        self.applied += 1;
        // Links wanted again are still firing in the reaper's queue: they come back at once rather
        // than after the attaches queued ahead of them (design 8.6).
        let back = self
            .reaper
            .reclaim(|k| desired.contains_key(k) && !self.in_flight.contains_key(k));
        for (key, links) in back {
            self.links.insert(key, (desired[&key], links));
        }
        for (&key, file) in &desired {
            if self.links.contains_key(&key) || self.in_flight.contains_key(&key) {
                continue;
            }
            let file = &self.files[file];
            let job = Job {
                key,
                path: file.path.clone(),
                offsets: self.offsets(&PROGS[key.0], file).expect("checked above"),
            };
            self.reaper.attaching(1);
            self.jobs
                .send(job)
                .expect("the attacher lives as long as ormad");
            self.in_flight.insert(key, self.applied);
        }
        self.desired = desired;
    }

    /// Takes the attacher's results. A link no longer wanted goes straight to the reaper.
    pub fn take_attached(&mut self) {
        while let Ok((key, result)) = self.done.try_recv() {
            self.in_flight.remove(&key);
            let file = self.desired.get(&key).copied();
            let wanted = file.is_some();
            match (result, file) {
                (Ok(links), Some(file)) => {
                    self.links.insert(key, (file, links));
                    if let Some(m) = self.failed.remove(&key) {
                        self.missing.remove(&m);
                    }
                }
                (Ok(links), None) => self.reaper.send(key, links),
                (Err(reason), _) if wanted => {
                    let m = Missing {
                        pid: match key.2 {
                            Scope::All => None,
                            Scope::Pid(p) => Some(p),
                        },
                        point: PROGS[key.0].point,
                        reason,
                    };
                    self.missing.insert(m.clone());
                    self.failed.insert(key, m);
                }
                (Err(_), _) => {}
            }
        }
    }

    /// The attaches and detaches that ended since the last call.
    pub fn take_changes(&self) -> Vec<ProbeChange> {
        self.reaper.take_changes()
    }

    /// The number of `apply` calls so far, and the last one whose links have all been tried.
    pub fn applied(&self) -> (u64, u64) {
        let attached = self
            .in_flight
            .values()
            .min()
            .map_or(self.applied, |a| a - 1);
        (self.applied, attached)
    }

    /// One key per file the kernel sees (`FileId`), preferring one whose path still opens: a
    /// stopped container's key lingers until the next prune and cannot be attached through.
    fn distinct_files(&self) -> Vec<FileKey> {
        let mut by_id: BTreeMap<FileId, FileKey> = BTreeMap::new();
        let mut keys: Vec<FileKey> = self.files.keys().copied().collect();
        keys.sort_unstable();
        for k in keys {
            let f = &self.files[&k];
            let kept = by_id.entry(file_id(k, f)).or_insert(k);
            if !opens(&self.files[kept], *kept) && opens(f, k) {
                *kept = k;
            }
        }
        by_id.into_values().collect()
    }

    /// Mapped library files with at least one attachment, for the operator's log.
    pub fn attached_files(&self) -> BTreeSet<(String, Scope)> {
        self.links
            .iter()
            .filter_map(|((_, _, s), (f, _))| Some((self.files.get(f)?.mapped.clone(), *s)))
            .collect()
    }

    pub fn attachments(&self) -> impl Iterator<Item = (&'static Prog, &LibFile, Scope)> {
        self.links
            .iter()
            .filter_map(|((i, _, s), (f, _))| Some((&PROGS[*i], self.files.get(f)?, *s)))
    }

    fn apply_maps(
        &mut self,
        want: &BTreeMap<Set, Sel>,
        stream: &Sel,
        user_words: &BTreeMap<u32, u64>,
    ) {
        let _ = self
            .config
            .set(CONFIG_STREAM_ALL, (*stream == Sel::All) as u64, 0);
        let stream_pids: BTreeSet<u32> = match stream {
            Sel::Pids(p) => p.clone(),
            _ => BTreeSet::new(),
        };
        sync(&mut self.stream, &stream_pids);

        let sched = want.get(&Set::Sched).cloned().unwrap_or_default();
        let sched_pids: BTreeSet<u32> = match &sched {
            Sel::None => BTreeSet::new(),
            Sel::All => self.procs.keys().copied().collect(),
            Sel::Pids(p) => p.clone(),
        };
        sync(&mut self.sched, &sched_pids);
        for (tid, pid) in procs::owners(sched_pids.iter().copied()) {
            let _ = self.tid_tgid.insert(tid, pid, 0);
        }
        let old: Vec<u32> = self.user_words.keys().filter_map(|k| k.ok()).collect();
        for tid in old {
            if !user_words.contains_key(&tid) {
                let _ = self.user_words.remove(&tid);
            }
        }
        for (tid, addr) in user_words {
            let _ = self.user_words.insert(tid, addr, 0);
        }

        let futex = want.get(&Set::Futex).cloned().unwrap_or_default();
        let futex_pids: BTreeSet<u32> = match &futex {
            Sel::None => BTreeSet::new(),
            Sel::All => self.procs.keys().copied().collect(),
            Sel::Pids(p) => p.clone(),
        };
        sync(&mut self.futex, &futex_pids);

        self.switch(false, sched != Sel::None);
        self.switch(true, futex != Sel::None);
    }

    /// Attaches the sched or futex tracepoints when `on`, detaches them when not. Waits for an
    /// attach in progress, only when they turn on or off.
    fn switch(&mut self, futex: bool, on: bool) {
        let tp = if futex {
            &mut self.futex_tp
        } else {
            &mut self.sched_tp
        };
        if on && tp.links.is_empty() {
            if let Err(why) = &tp.layout {
                for point in tp.points {
                    self.missing.insert(Missing {
                        pid: None,
                        point,
                        reason: why.clone(),
                    });
                }
                return;
            }
            let mut ebpf = self.ebpf.lock().expect("ebpf lock");
            for name in tp.progs {
                let from = sys::mono_ns();
                let t: &mut TracePoint = ebpf
                    .program_mut(name)
                    .expect("loaded")
                    .try_into()
                    .expect("a tracepoint");
                match t.attach(tp.system, name) {
                    Ok(id) => {
                        tp.links.push((name, id));
                        self.reaper.log(change(true, name, Scope::All, 1, from));
                    }
                    Err(e) => self.notes.push(Note {
                        warn: true,
                        text: format!("attach {name}: {e}"),
                    }),
                }
            }
        } else if !on && !tp.links.is_empty() {
            let mut ebpf = self.ebpf.lock().expect("ebpf lock");
            for (name, id) in std::mem::take(&mut tp.links) {
                let t: &mut TracePoint = ebpf
                    .program_mut(name)
                    .expect("loaded")
                    .try_into()
                    .expect("a tracepoint");
                let from = sys::mono_ns();
                let _ = t.detach(id);
                self.reaper.log(change(false, name, Scope::All, 1, from));
            }
        }
    }

    /// Bumps the max generation and reads every aggregate row: the rows, and the generation
    /// whose `max` the rows now hold.
    pub fn sample(&mut self) -> (Vec<(AggKey, AggVal)>, u64) {
        let closed = self.generation;
        self.generation += 1;
        let _ = self.config.set(CONFIG_GEN, self.generation, 0);
        let rows = self.agg.iter().filter_map(|r| r.ok()).collect();
        (rows, closed)
    }

    pub fn lost(&self) -> [u64; LOST_SLOTS_ALL as usize] {
        let mut out = [0; LOST_SLOTS_ALL as usize];
        for (i, v) in out.iter_mut().enumerate() {
            *v = self.lost.get(&(i as u32), 0).unwrap_or(0);
        }
        out
    }

    pub fn samples(&self) -> [u64; SAMPLE_SLOTS as usize] {
        let mut out = [0; SAMPLE_SLOTS as usize];
        for (i, v) in out.iter_mut().enumerate() {
            *v = self.sample.get(&(i as u32), 0).unwrap_or(0);
        }
        out
    }

    pub fn set_serdata_offset(&mut self, offset: u64) {
        let _ = self.config.set(CONFIG_SERDATA_SIZE_OFFSET, offset, 0);
    }

    /// Seen entries of one kind, by process.
    pub fn seen_count(&self, what: u32) -> BTreeMap<u32, u64> {
        let mut out = BTreeMap::new();
        for k in self.seen.keys().filter_map(|k| k.ok()) {
            if k.what == what {
                *out.entry(k.tgid).or_default() += 1;
            }
        }
        out
    }

    /// Each program's cumulative counters (design 6.8), and whether it is attached. Read
    /// through a kept fd, as the attacher may hold the `Ebpf` for seconds.
    pub fn prog_stats(&self) -> Vec<ProgStat> {
        let attached: HashSet<&str> = self
            .links
            .keys()
            .map(|(i, _, _)| PROGS[*i].name)
            .chain(self.sched_tp.links.iter().map(|(n, _)| *n))
            .chain(self.futex_tp.links.iter().map(|(n, _)| *n))
            .collect();
        self.prog_fds
            .iter()
            .map(|(name, fd)| {
                let info = fdinfo(fd);
                let get = |k: &str| info.get(k).copied().unwrap_or(0);
                ProgStat {
                    name,
                    run_time_ns: get("run_time_ns"),
                    run_cnt: get("run_cnt"),
                    recursion_misses: get("recursion_misses"),
                    verified_insns: get("verified_insns"),
                    attached: attached.contains(name),
                }
            })
            .collect()
    }

    /// Entries used, the limit and locked memory of every map (design 6.8).
    pub fn map_use(&self) -> Vec<(String, u64, u64, u64)> {
        let used: HashMap<&str, u64> = [
            ("AGG", self.agg.keys().count() as u64),
            ("SEEN", self.seen.keys().count() as u64),
            ("STREAM", self.stream.keys().count() as u64),
            ("SCHED", self.sched.keys().count() as u64),
            ("FUTEX", self.futex.keys().count() as u64),
            ("TID_TGID", self.tid_tgid.keys().count() as u64),
            ("USER_WORDS", self.user_words.keys().count() as u64),
        ]
        .into();
        self.maps
            .iter()
            .map(|(name, fd, max)| {
                let memlock = fdinfo(fd).get("memlock").copied().unwrap_or(0);
                (
                    name.clone(),
                    used.get(name.as_str()).copied().unwrap_or(0),
                    *max as u64,
                    memlock,
                )
            })
            .collect()
    }
}

fn sync(map: &mut BpfHashMap<MapData, u32, u8>, want: &BTreeSet<u32>) {
    let have: Vec<u32> = map.keys().filter_map(|k| k.ok()).collect();
    for k in &have {
        if !want.contains(k) {
            let _ = map.remove(k);
        }
    }
    for k in want {
        if !have.contains(k) {
            let _ = map.insert(k, 1, 0);
        }
    }
}

/// Whether `f`'s path still opens the file of `key`.
fn opens(f: &LibFile, key: FileKey) -> bool {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(&f.path).is_ok_and(|m| (m.dev(), m.ino()) == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fdinfo_counters() {
        let prog = "pos:\t0\nflags:\t02000002\nprog_type:\t2\nprog_tag:\ta04f5eef06a7f555\n\
                    memlock:\t4096\nprog_id:\t73\nrun_time_ns:\t1520\nrun_cnt:\t7\n\
                    recursion_misses:\t2\nverified_insns:\t14\n";
        let m = parse_fdinfo(prog);
        assert_eq!(m["run_time_ns"], 1520);
        assert_eq!(m["run_cnt"], 7);
        assert_eq!(m["recursion_misses"], 2);
        assert_eq!(m["memlock"], 4096);
        assert!(!m.contains_key("prog_tag"));
    }
}
