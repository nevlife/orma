//! armo-collectd's model: names from init events, the intervals,
//! message pairing across processes, flows inside nodes, per-node CPU, and the window
//! snapshot of 12.7, all behind `armo_proto::Ingest`.
//!
//! Records are buffered and handled in time order when a window closes, so sources may deliver
//! out of order up to one window. Pairing and flows hold their tables across windows.

mod decode;
mod handle;
pub mod lat;
mod pair;
pub mod procfs;
mod snapshot;
pub mod stamp;
pub mod symbols;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use armo_common::shm::{ARMO_SRC_FLUX2, ARMO_SRC_ROS};
use armo_proto::Ingest;
use armo_proto::msg::{
    Aggregate, CallbackKind, FlowKind, Method, PublishAbi, Ros, Transport, Window,
};
use armo_proto::records::Record;

use crate::sources::armod::Sel;

use decode::{Ev, decode};
use lat::LatAcc;
use stamp::Clock;

/// How long armod reads the executor for a callback no event named (design 2.1.4).
const NAMING_NS: u64 = 10_000_000_000;

/// One callback in one process, keyed as the aggregate tables key it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CbKey {
    pub pid: u32,
    pub src: u16,
    pub key: u64,
    pub entry: u32,
}

impl CbKey {
    fn ros(pid: u32, callback: u64) -> Self {
        CbKey {
            pid,
            src: ARMO_SRC_ROS,
            key: callback,
            entry: 0,
        }
    }
    fn flux2(pid: u32, executor: u64, entry: u32) -> Self {
        CbKey {
            pid,
            src: ARMO_SRC_FLUX2,
            key: executor,
            entry,
        }
    }
}

/// A ROS executor by the wait set its threads wait on, a flux2 executor by its control block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct ExecKey {
    pid: u32,
    src: u16,
    handle: u64,
}

type TopicIx = usize;
type NodeIx = usize;

/// Who holds a subscription or publisher: its node, or only its process when the node is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Owner {
    Node(NodeIx),
    Proc(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct SubKey {
    topic: TopicIx,
    owner: Owner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct PubKey {
    pid: u32,
    handle: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FlowKey {
    kind: FlowKind,
    node: Owner,
    input: Option<TopicIx>,
    output: TopicIx,
}

struct TopicInfo {
    transport: Transport,
    name: String,
    fingerprint: u64,
    publishers: BTreeSet<Owner>,
    subscribers: BTreeSet<Owner>,
    /// Periods of the timers whose callbacks published here (the expected rate).
    timers: BTreeSet<i64>,
    /// A publisher of the rcl publish ABI published here.
    rcl_abi: bool,
}

struct NodeInfo {
    pid: u32,
    handle: Option<u64>,
    fqn: String,
}

#[derive(Default)]
struct Proc {
    starttime: u64,
    methods: BTreeSet<Method>,
    method: Option<Method>,
    abi: Option<PublishAbi>,
    ros: Option<Ros>,
    ns_pid: Option<u32>,
    comm: String,
    cold: bool,
    exited: bool,
    probed: bool,
    rss: Option<u64>,
    pss: Option<u64>,
    pss_at: u64,
    nodes: BTreeSet<NodeIx>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CbOwner {
    Sub(u64),
    Timer(u64),
    Service(u64),
    Endpoint(u64),
}

#[derive(Default)]
struct Cb {
    owner: Option<CbOwner>,
    symbol: Option<String>,
    late_topic: Option<TopicIx>,
    executor: Option<ExecKey>,
    code_tried: bool,
    /// When it first started.
    first: u64,
}

#[derive(Clone, Copy)]
struct Ep {
    topic: TopicIx,
    node: u64,
}

#[derive(Default)]
struct Timer {
    period: Option<i64>,
    node: Option<u64>,
    group: Option<u64>,
}

struct Channel {
    dev: u64,
    ino: u64,
    signpost: String,
    publisher: bool,
    topic: TopicIx,
    endpoint: Option<u64>,
    /// flux2's `lost` at this subscriber's last take: it counts from the channel's start.
    lost: u64,
}

struct Endpoint {
    node: String,
    topic: TopicIx,
}

#[derive(Clone, Copy)]
struct Input {
    topic: TopicIx,
    sub: SubKey,
    t: u64,
    stamp: Option<u64>,
}

struct Out {
    topic: TopicIx,
    main: Vec<u64>,
    intra: Vec<u64>,
}

struct Frame {
    cb: CbKey,
    t: u64,
    input: Option<Input>,
    outputs: Vec<Out>,
    dispatch: Option<u64>,
    off: u64,
    delivery: Option<u64>,
}

#[derive(Default)]
struct Thread {
    stack: Vec<Frame>,
    last_wake: Option<u64>,
    wait_enter: Option<u64>,
    exec: Option<ExecKey>,
    last_take: Option<Input>,
    last_take_rmw: Option<u64>,
    last_cb_end: u64,
    pending_rcl: Option<(u64, u64, u64)>,
    last_pub_topic: Option<TopicIx>,
    pending_group_timer: Option<u64>,
    on_since: Option<u64>,
    off_since: Option<(u64, CbKey)>,
    woken_at: Option<u64>,
    schedstat: Option<u64>,
    history: VecDeque<BTreeSet<Option<NodeIx>>>,
    delivery: Option<u64>,
    rclcpp: bool,
}

#[derive(Default)]
struct CbWin {
    n: u64,
    starts: u64,
    dur: LatAcc,
    off: u64,
    dispatch: LatAcc,
    age: LatAcc,
    intra: u64,
    nested: bool,
    /// Wall time per thread from the per-thread aggregate tables.
    wall_by_tid: BTreeMap<u32, u64>,
}

#[derive(Default)]
struct TopicWin {
    /// Per publisher: publishes that went to rmw or flux2, and intra-process ones.
    pubs: BTreeMap<PubKey, [u64; 2]>,
    /// By publishing process, so that each scales by its own source.
    bytes: Option<BTreeMap<u32, u64>>,
    hop: LatAcc,
    age_pub: LatAcc,
    age_take: LatAcc,
    drop_publish: Option<u64>,
    drop_record: Option<u64>,
    ambiguous: u64,
}

#[derive(Default)]
struct SubWin {
    takes: u64,
    lost: u64,
    flux2_lost: Option<u64>,
    callbacks: u64,
    intra: u64,
    hop: LatAcc,
    wake: LatAcc,
}

#[derive(Default)]
struct ExecWin {
    wait: LatAcc,
    dispatch: LatAcc,
    busy: u64,
    off: u64,
}

#[derive(Default)]
struct ThreadWin {
    in_cb_wall: u64,
    in_cb_off: u64,
    cpu_wait: LatAcc,
    oncpu: Option<u64>,
    cpu_ns: Option<u64>,
    cpus: BTreeSet<u32>,
    owners: BTreeSet<Option<NodeIx>>,
}

#[derive(Default)]
struct FlowWin {
    count: u64,
    dwell: LatAcc,
    ambiguous: u64,
}

#[derive(Default)]
struct Malloc {
    alloc: u64,
    alloc_n: u64,
    free: u64,
}

#[derive(Default)]
struct Win {
    cbs: HashMap<CbKey, CbWin>,
    topics: HashMap<TopicIx, TopicWin>,
    subs: HashMap<SubKey, SubWin>,
    execs: HashMap<ExecKey, ExecWin>,
    threads: HashMap<(u32, u32), ThreadWin>,
    flows: BTreeMap<FlowKey, FlowWin>,
    malloc: HashMap<Owner, Malloc>,
}

/// Exact per-event values for `armo analyze`; the model keeps them only offline.
#[derive(Clone, Debug, PartialEq)]
pub enum Sample {
    /// One message from its publish to the end of the callback that handled it.
    Delivery(Delivery),
    Callback {
        id: String,
        node: Option<String>,
        tid: u32,
        t_ns: u64,
        duration_ns: u64,
        oncpu_ns: Option<u64>,
        dispatch_ns: Option<u64>,
    },
    Wait {
        executor: String,
        ns: u64,
    },
    CpuWait {
        thread: String,
        ns: u64,
    },
    Flow {
        id: String,
        dwell_ns: u64,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Delivery {
    pub topic: String,
    pub subscription: String,
    pub pub_pid: Option<u32>,
    pub sub_pid: u32,
    pub sub_tid: u32,
    pub t_pub_ns: Option<u64>,
    pub t_take_ns: u64,
    pub hop_ns: Option<u64>,
    pub wake_ns: Option<u64>,
    pub dispatch_ns: Option<u64>,
    pub callback: Option<String>,
    pub callback_ns: Option<u64>,
}

/// A delivery until both its pairing and its callback are known.
struct Open {
    row: Delivery,
    t_wake: Option<u64>,
    paired: bool,
    done: bool,
    windows: u8,
}

pub struct Model {
    live: bool,
    /// Whether to read PSS (`configure.pss`).
    pss: bool,
    kept: procfs::Kept,
    seq: u64,
    sched: Sel,
    /// The processes whose publish, take and callback events reach the model; the others have
    /// no rates (null), not zero ones.
    hot: Sel,
    /// The time this window's LTTng chunks cover; 0 when none came.
    lttng_span: u64,
    /// The end of the last window.
    now: u64,
    clock: Clock,
    buf: Vec<Ev>,
    rows: Vec<(Method, Aggregate)>,
    expected: BTreeMap<String, f64>,
    procs: BTreeMap<u32, Proc>,
    topics: Vec<TopicInfo>,
    topic_ix: BTreeMap<(Transport, String), TopicIx>,
    nodes: Vec<NodeInfo>,
    node_ix: HashMap<(u32, u64), NodeIx>,
    node_fqn: HashMap<(u32, String), Vec<NodeIx>>,
    pubs: HashMap<(u32, u64), Ep>,
    rmw_pubs: HashMap<(u32, u64), u64>,
    subs: HashMap<(u32, u64), Ep>,
    rmw_subs: HashMap<(u32, u64), u64>,
    rmw_topics: HashMap<(u32, u64), TopicIx>,
    rclcpp_subs: HashMap<(u32, u64), u64>,
    sub_groups: HashMap<(u32, u64), u64>,
    timers: HashMap<(u32, u64), Timer>,
    /// Node handle and service name.
    services: HashMap<(u32, u64), (u64, String)>,
    cbs: HashMap<CbKey, Cb>,
    known_subs: BTreeSet<SubKey>,
    /// The stamp each publisher last sent, so a dual (intra and rmw) publish links once.
    last_stamp: HashMap<PubKey, u64>,
    channels: HashMap<(u32, u64), Channel>,
    endpoints: HashMap<(u32, u64), Endpoint>,
    endpoint_signposts: HashMap<(u32, String, bool), Vec<u64>>,
    exec_rclcpp: HashSet<ExecKey>,
    exec_cbs: BTreeMap<ExecKey, BTreeSet<CbKey>>,
    threads: HashMap<(u32, u32), Thread>,
    tid_pid: HashMap<u32, u32>,
    pair: pair::Pairing,
    stamps: pair::StampIndex,
    resolver: symbols::Resolver,
    deliveries: BTreeMap<u64, Open>,
    next_delivery: u64,
    samples: Option<Vec<Sample>>,
    win: Win,
}

impl Model {
    fn new(live: bool) -> Self {
        Model {
            live,
            pss: false,
            kept: procfs::Kept::default(),
            seq: 0,
            sched: Sel::None,
            hot: Sel::All,
            lttng_span: 0,
            now: 0,
            clock: Clock::default(),
            buf: Vec::new(),
            rows: Vec::new(),
            expected: BTreeMap::new(),
            procs: BTreeMap::new(),
            topics: Vec::new(),
            topic_ix: BTreeMap::new(),
            nodes: Vec::new(),
            node_ix: HashMap::new(),
            node_fqn: HashMap::new(),
            pubs: HashMap::new(),
            rmw_pubs: HashMap::new(),
            subs: HashMap::new(),
            rmw_subs: HashMap::new(),
            rmw_topics: HashMap::new(),
            rclcpp_subs: HashMap::new(),
            sub_groups: HashMap::new(),
            timers: HashMap::new(),
            services: HashMap::new(),
            cbs: HashMap::new(),
            known_subs: BTreeSet::new(),
            last_stamp: HashMap::new(),
            channels: HashMap::new(),
            endpoints: HashMap::new(),
            endpoint_signposts: HashMap::new(),
            exec_rclcpp: HashSet::new(),
            exec_cbs: BTreeMap::new(),
            threads: HashMap::new(),
            tid_pid: HashMap::new(),
            pair: pair::Pairing::default(),
            stamps: pair::StampIndex::default(),
            resolver: symbols::Resolver::default(),
            deliveries: BTreeMap::new(),
            next_delivery: 0,
            samples: (!live).then(Vec::new),
            win: Win::default(),
        }
    }

    /// The model inside armo-collectd: reads `/proc` for thread CPU, memory, names and starttimes;
    /// in-process intervals of the uprobe and LD_PRELOAD methods come from their aggregate tables.
    pub fn live() -> Self {
        Model::new(true)
    }

    /// The model inside `armo analyze`: no `/proc`, every interval from records, exact samples kept.
    pub fn offline() -> Self {
        Model::new(false)
    }

    /// The processes whose callback CPU excludes off-CPU time: armod delivers scheduler events and
    /// knows each thread's current callback there.
    pub fn set_sched(&mut self, pids: Sel) {
        self.sched = pids;
    }

    pub fn set_hot(&mut self, pids: Sel) {
        self.hot = pids;
    }

    pub fn set_pss(&mut self, on: bool) {
        self.pss = on;
    }

    /// Processes with a ROS callback no event has named the entity of, first run in the last
    /// `NAMING_NS`: armod reads those from the executor for that long (design 2.1.4).
    pub fn unowned(&self) -> BTreeSet<u32> {
        self.cbs
            .iter()
            .filter(|(k, c)| {
                k.src == ARMO_SRC_ROS
                    && c.owner.is_none()
                    && c.first != 0
                    && c.first + NAMING_NS >= self.now
            })
            .map(|(k, _)| k.pid)
            .collect()
    }

    /// The time the LTTng chunks of the coming window cover. A chunk is a rotation of the
    /// session, so its counts are over that time rather than over the window.
    pub fn set_lttng_span(&mut self, ns: u64) {
        self.lttng_span = ns;
    }

    /// `configure.expected_hz`, by entity id.
    pub fn set_expected_hz(&mut self, by_id: BTreeMap<String, f64>) {
        self.expected = by_id;
    }

    /// A process's starttime when `/proc` cannot give it (a recording's `meta.json`).
    pub fn set_starttime(&mut self, pid: u32, starttime: u64) {
        self.procs.entry(pid).or_default().starttime = starttime;
    }

    /// A thread's cumulative on-CPU time, as `/proc/<pid>/task/<tid>/schedstat` gives it.
    pub fn thread_cpu(&mut self, pid: u32, tid: u32, cumulative_ns: u64) {
        let th = self.threads.entry((pid, tid)).or_default();
        if let Some(prev) = th.schedstat.replace(cumulative_ns) {
            self.win.threads.entry((pid, tid)).or_default().cpu_ns =
                Some(cumulative_ns.saturating_sub(prev));
        }
        self.tid_pid.insert(tid, pid);
    }

    pub fn take_samples(&mut self) -> Vec<Sample> {
        self.samples
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    fn records_intervals(&self, method: Method) -> bool {
        !self.live || method == Method::Lttng
    }

    fn proc_mut(&mut self, pid: u32) -> &mut Proc {
        let live = self.live;
        let p = self.procs.entry(pid).or_default();
        if live && !p.probed {
            p.probed = true;
            if let Some(stat) = procfs::stat(pid) {
                p.starttime = stat.starttime();
                p.comm = stat.comm;
            }
            p.ns_pid = procfs::ns_pid(pid);
            p.ros = procfs::ros(pid);
        }
        p
    }

    fn starttime(&self, pid: u32) -> u64 {
        self.procs.get(&pid).map_or(0, |p| p.starttime)
    }

    fn topic(&mut self, transport: Transport, name: &str) -> TopicIx {
        if let Some(&ix) = self.topic_ix.get(&(transport, name.to_string())) {
            return ix;
        }
        self.topics.push(TopicInfo {
            transport,
            name: name.to_string(),
            fingerprint: 0,
            publishers: BTreeSet::new(),
            subscribers: BTreeSet::new(),
            timers: BTreeSet::new(),
            rcl_abi: false,
        });
        self.topic_ix
            .insert((transport, name.to_string()), self.topics.len() - 1);
        self.topics.len() - 1
    }

    fn node(&mut self, pid: u32, handle: u64) -> NodeIx {
        if let Some(&ix) = self.node_ix.get(&(pid, handle)) {
            return ix;
        }
        self.nodes.push(NodeInfo {
            pid,
            handle: Some(handle),
            fqn: String::new(),
        });
        let ix = self.nodes.len() - 1;
        self.node_ix.insert((pid, handle), ix);
        self.proc_mut(pid).nodes.insert(ix);
        ix
    }

    /// A node known by name only (a flux2 endpoint): the ROS node of that name in the process
    /// when there is exactly one, else a node of its own.
    fn node_by_fqn(&mut self, pid: u32, fqn: &str) -> Option<NodeIx> {
        match self
            .node_fqn
            .get(&(pid, fqn.to_string()))
            .map(Vec::as_slice)
        {
            Some([ix]) => Some(*ix),
            Some(_) => None,
            None => {
                self.nodes.push(NodeInfo {
                    pid,
                    handle: None,
                    fqn: fqn.to_string(),
                });
                let ix = self.nodes.len() - 1;
                self.node_fqn.insert((pid, fqn.to_string()), vec![ix]);
                self.proc_mut(pid).nodes.insert(ix);
                Some(ix)
            }
        }
    }

    fn owner(&self, pid: u32, node: Option<u64>) -> Owner {
        node.and_then(|h| self.node_ix.get(&(pid, h)).copied())
            .map_or(Owner::Proc(pid), Owner::Node)
    }

    fn cb_kind(&self, key: &CbKey) -> CallbackKind {
        let cb = self.cbs.get(key);
        match cb.and_then(|c| c.owner) {
            Some(CbOwner::Sub(_)) => CallbackKind::Sub,
            Some(CbOwner::Timer(_)) => CallbackKind::Timer,
            Some(CbOwner::Service(_)) => CallbackKind::Service,
            Some(CbOwner::Endpoint(_)) => CallbackKind::Flux2,
            None if key.src == ARMO_SRC_FLUX2 => CallbackKind::Flux2,
            None if cb.is_some_and(|c| c.late_topic.is_some()) => CallbackKind::Sub,
            None => CallbackKind::Unknown,
        }
    }

    fn cb_node(&mut self, key: &CbKey) -> Option<NodeIx> {
        let pid = key.pid;
        let node = match self.cbs.get(key)?.owner? {
            CbOwner::Sub(obj) => {
                let rcl = self.rclcpp_subs.get(&(pid, obj))?;
                self.subs.get(&(pid, *rcl))?.node
            }
            CbOwner::Timer(t) => self.timers.get(&(pid, t))?.node?,
            CbOwner::Service(s) => self.services.get(&(pid, s))?.0,
            CbOwner::Endpoint(e) => {
                let fqn = self.endpoints.get(&(pid, e))?.node.clone();
                return self.node_by_fqn(pid, &fqn);
            }
        };
        self.node_ix.get(&(pid, node)).copied()
    }

    fn cb_topic(&self, key: &CbKey) -> Option<TopicIx> {
        let pid = key.pid;
        let cb = self.cbs.get(key)?;
        match cb.owner {
            Some(CbOwner::Sub(obj)) => {
                let rcl = self.rclcpp_subs.get(&(pid, obj))?;
                Some(self.subs.get(&(pid, *rcl))?.topic)
            }
            Some(CbOwner::Endpoint(e)) => Some(self.endpoints.get(&(pid, e))?.topic),
            Some(CbOwner::Timer(_) | CbOwner::Service(_)) => None,
            None => cb.late_topic,
        }
    }

    fn sample(&mut self, s: Sample) {
        if let Some(v) = self.samples.as_mut() {
            v.push(s);
        }
    }
}

impl Ingest for Model {
    fn record(&mut self, method: Method, record: &Record<'_>) {
        let Some(e) = decode(record) else {
            return;
        };
        self.buf.push(Ev {
            pid: record.pid,
            tid: record.tid,
            t: record.header.t_mono_ns,
            method,
            e,
        });
    }

    fn aggregates(&mut self, method: Method, rows: &[Aggregate]) {
        self.rows.extend(rows.iter().map(|r| (method, r.clone())));
    }

    fn window(&mut self, t_mono_ns: u64, t_real_ns: u64, window_ns: u64, hists: bool) -> Window {
        let started = std::time::Instant::now();
        self.clock.rt_offset = Some(t_real_ns as i64 - t_mono_ns as i64);
        self.now = t_mono_ns;
        let (mut now, later): (Vec<Ev>, Vec<Ev>) =
            self.buf.drain(..).partition(|ev| ev.t <= t_mono_ns);
        self.buf = later;
        now.sort_by_key(|ev| ev.t);
        for ev in now {
            self.handle(ev);
        }
        for (method, row) in std::mem::take(&mut self.rows) {
            self.aggregate(method, &row);
        }
        self.retry_pending();
        self.evict(t_mono_ns);
        if self.live {
            self.sample_proc(t_mono_ns);
        }
        let mut w = self.snapshot(t_mono_ns, t_real_ns, window_ns, hists);
        w.agent.collect_ns = started.elapsed().as_nanos() as u64;
        self.finish_deliveries();
        self.end_window();
        w
    }
}
