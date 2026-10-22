//! JSON messages (design 12) and the files a recording writes (14.4). `null` means not measured,
//! 0 a measured zero; a field that is absent when unset says so. Maps keyed by pid or tid key by
//! its decimal string, as JSON objects do; serde cannot read integer keys inside tagged enums.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[major, minor]`.
pub type Version = [u16; 2];
pub const PROTO: Version = [1, 9];
pub const META_SCHEMA: u32 = 1;
pub const HIST_LOG2: &str = "log2";

macro_rules! types {
    ($($item:item)*) => {
        $(
            #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
            $item
        )*
    };
}

macro_rules! enums {
    ($($item:item)*) => {
        $(
            #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
                JsonSchema)]
            #[serde(rename_all = "snake_case")]
            $item
        )*
    };
}

enums! {
    pub enum Method { Uprobe, Preload, Lttng }
    pub enum RecordMethod { Auto, Uprobe, Preload, Lttng }
    pub enum Stream { State, Window, EventLine }
    pub enum DetailWhat { Hist, Flows, Threads }
    pub enum RecordKind { Events, Windows }
    pub enum Compression { None, Zstd }
    pub enum RecordState { Recording, Finalized, Truncated }
    pub enum EndReason { UserStop, MaxDuration, MaxSize, DiskFloor, CollectorExit, Error }
    pub enum OrmadLink { Connected, Absent, Denied, Lost }
    pub enum LttngState { Available, Absent }
    pub enum Level { Info, Warn, Error }
    pub enum CheckStatus { Pass, Fail, Unknown }
    pub enum ProbeSet { Discovery, Hot, Sched, Bandwidth, RclcppSymbols, Names, Gpu, Dds, Futex }
    pub enum PublishAbi { Rmw, Rcl }
    /// The uprobes on a process's executor: `rcl_wait` and the rclcpp executor tracepoints, and
    /// with `rcl_executor` also the anchors of the rcl_executor core it maps.
    pub enum ExecutorProbes { Stock, RclExecutor }
    pub enum CpuSource { Sched, WallBound }
    pub enum ExecutorKind { Rclcpp, Rclpy, #[serde(alias = "flux2")] Flux }
    pub enum CallbackKind { Sub, Timer, Service, #[serde(alias = "flux2")] Flux, Unknown }
    pub enum Transport { Ros, #[serde(alias = "flux2")] Flux }
    pub enum FlowKind { Sync, Stamp, Blind, Latest }
    pub enum ChainMode { Lineage, First }
}

types! {
    pub struct ClientRequest {
        pub id: u64,
        #[serde(flatten)]
        pub op: ClientOp,
    }

    #[serde(tag = "op", rename_all = "snake_case")]
    pub enum ClientOp {
        Hello(Hello),
        Subscribe { streams: Vec<Stream> },
        Select(Select),
        Configure(Configure),
        RecordStart(RecordOptions),
        RecordStop { record_id: String },
        RecordList,
        RecordShow {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            record_id: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            dir: Option<String>,
        },
        Detail { entity: String, what: DetailWhat },
        ReplayOpen { dir: String },
        ReplaySeek { replay_id: String, frame: u64 },
        ReplayPlay { replay_id: String, rate: f64 },
        ReplayPause { replay_id: String },
        ReplayClose { replay_id: String },
        Export { dir: String, out: String, metrics: Vec<String> },
        Selfcheck { duration_s: f64 },
        Bye,
    }

    pub struct Hello {
        pub proto: Version,
        pub client: String,
        pub pid: u32,
    }

    /// `node`, `topic` and `exclude` are regular expressions over whole names (design 14.3). In
    /// `state`, `pids` also holds the processes they matched. Proto 1.5.
    #[derive(Default)]
    pub struct Selection {
        #[serde(default)]
        pub all: bool,
        #[serde(default)]
        pub pids: Vec<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub node: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub topic: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub exclude: Option<String>,
    }

    pub struct Select {
        #[serde(flatten)]
        pub selection: Selection,
        #[serde(default)]
        pub method_by_pid: BTreeMap<String, Method>,
    }

    /// Only the fields given change.
    pub struct Configure {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub window_s: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub bandwidth: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub sched: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub malloc: Option<bool>,
        /// Read each process's PSS: a walk of its page tables under its mmap_lock. Proto 1.7.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub pss: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub expected_hz: Option<BTreeMap<String, f64>>,
        /// CUDA driver calls (design 20.1). Proto 1.8.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub gpu: Option<bool>,
        /// DDS write calls (design 20.2). Proto 1.8.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub dds: Option<bool>,
        /// Futex waits of the selected threads (design 20.4). Proto 1.8.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub futex: Option<bool>,
        /// Replaces the chains measured end to end (design 20.6). Proto 1.8.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub chains: Option<Vec<ChainSpec>>,
        /// Regular expressions over node names whose outputs link to their latest inputs
        /// (design 20.7). Proto 1.8.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub latest_nodes: Option<Vec<String>>,
    }

    /// `topics` in order from the source; at least two. Proto 1.8.
    pub struct ChainSpec {
        pub name: String,
        pub topics: Vec<String>,
        pub mode: ChainMode,
    }

    pub struct Targets {
        #[serde(default)]
        pub all: bool,
        #[serde(default)]
        pub pids: Vec<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub node: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub topic: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub exclude: Option<String>,
    }

    /// The options of design 14.3, one field each. An absent field takes the default there.
    #[derive(Default)]
    pub struct RecordOptions {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub method: Option<RecordMethod>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub targets: Option<Targets>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub kind: Option<RecordKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub dir: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub max_duration_s: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub max_bytes: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub split_bytes: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub split_duration_s: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cache_bytes: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub compression: Option<Compression>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub bandwidth: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub sched: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub malloc: Option<bool>,
        /// Proto 1.8, as in `Configure`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub gpu: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub dds: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub futex: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub chains: Option<Vec<ChainSpec>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub latest_nodes: Option<Vec<String>>,
    }

    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum ServerMessage {
        Hello(ServerHello),
        State(State),
        Window(Box<Window>),
        EventLine(EventLine),
        Reply(Reply),
        Error(ErrorMessage),
    }

    pub struct ServerHello {
        pub proto: Version,
        pub server: String,
        pub version: String,
    }

    pub struct Reply {
        pub id: u64,
        pub ok: bool,
        #[serde(flatten)]
        pub body: ReplyBody,
    }

    /// Told apart by their fields; the request's `id` says which op it answers.
    #[serde(untagged)]
    pub enum ReplyBody {
        RecordStarted { record_id: String, dir: String },
        Recordings { recordings: Vec<Recording> },
        RecordShow { table: VerifyTable },
        Detail(Detail),
        ReplayOpened { replay_id: String },
        Selfcheck { checks: Vec<Check> },
        Empty {},
    }

    pub struct ErrorMessage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub id: Option<u64>,
        pub code: String,
        pub message: String,
    }

    pub struct State {
        pub seq: u64,
        pub sources: Sources,
        pub monitor: Monitor,
        pub recordings: Vec<Recording>,
    }

    pub struct Sources {
        pub ormad: OrmadLink,
        pub lttng: LttngState,
        pub preload_processes: u32,
    }

    pub struct Monitor {
        pub running: bool,
        pub window_s: f64,
        pub selection: Selection,
        pub method_by_pid: BTreeMap<String, Method>,
        pub bandwidth: bool,
        pub sched: bool,
        pub malloc: bool,
        /// Proto 1.7.
        #[serde(default)]
        pub pss: bool,
        /// Proto 1.8.
        #[serde(default)]
        pub gpu: bool,
        #[serde(default)]
        pub dds: bool,
        #[serde(default)]
        pub futex: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub chains: Vec<ChainSpec>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub latest_nodes: Vec<String>,
    }

    pub struct Recording {
        pub id: String,
        pub state: RecordState,
        pub dir: String,
        pub kind: RecordKind,
        pub methods: Vec<Method>,
        pub targets: Targets,
        pub started_real_ns: u64,
        pub elapsed_s: f64,
        pub bytes: u64,
        pub limits: Limits,
        pub owner: Owner,
        pub totals: Counts,
        pub per_entity: Vec<EntityCount>,
        pub per_entity_truncated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub end_reason: Option<EndReason>,
    }

    pub struct Limits {
        pub max_duration_s: u64,
        pub max_bytes: u64,
        pub disk_floor_bytes: u64,
    }

    pub struct Owner {
        pub uid: u32,
        pub client: String,
        pub pid: u32,
        pub host: String,
    }

    pub struct Counts {
        pub events: u64,
        pub lost: u64,
    }

    pub struct EntityCount {
        pub id: String,
        pub events: u64,
        pub lost: u64,
    }

    pub struct EventLine {
        pub t_real_ns: u64,
        pub level: Level,
        pub kind: String,
        pub text: String,
        pub ids: Vec<String>,
    }

    pub struct Detail {
        pub entity: String,
        pub what: DetailWhat,
        /// By the name of the entity's latency field.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub hists: Option<BTreeMap<String, Hist>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub flows: Option<Vec<Flow>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub threads: Option<Vec<Thread>>,
    }

    /// For `log2`, bucket `i` counts `[2^i, 2^(i+1))` ns over `orma_common::BUCKETS` buckets.
    pub struct Hist {
        pub scheme: String,
        pub counts: Vec<u64>,
    }

    pub struct VerifyTable {
        pub entities: Vec<VerifyRow>,
        pub warnings: Vec<Warning>,
        pub totals: VerifyTotals,
        pub end_reason: Option<EndReason>,
        pub end_detail: Option<String>,
    }

    pub struct VerifyRow {
        pub id: String,
        pub events: u64,
        pub lost: u64,
        pub hz: Option<f64>,
    }

    pub struct Warning {
        pub kind: String,
        pub text: String,
        pub ids: Vec<String>,
    }

    pub struct VerifyTotals {
        pub events: u64,
        pub lost: u64,
        pub duration_s: f64,
        pub bytes: u64,
    }

    pub struct Check {
        pub item: String,
        pub status: CheckStatus,
        pub detail: String,
    }

    pub struct Window {
        pub seq: u64,
        pub replay: bool,
        pub t_mono_ns: u64,
        pub t_real_ns: u64,
        pub window_ns: u64,
        pub behind: bool,
        pub processes: Vec<Process>,
        pub threads: Vec<Thread>,
        pub nodes: Vec<Node>,
        pub executors: Vec<Executor>,
        pub callbacks: Vec<Callback>,
        pub topics: Vec<Topic>,
        pub subscriptions: Vec<Subscription>,
        pub flows: Vec<Flow>,
        /// Proto 1.8 (design 20.5).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub spans: Vec<Span>,
        /// Proto 1.8 (design 20.6).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub chains: Vec<Chain>,
        /// The host's GPUs, network, temperatures and clocks over the window. Proto 1.8 (design
        /// 20.3).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub host: Option<HostWindow>,
        pub agent: Agent,
        /// Window records only (14.4): entity id, then latency field name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub hists: Option<BTreeMap<String, BTreeMap<String, Hist>>>,
    }

    pub struct Lat {
        pub n: u64,
        pub mean_ns: u64,
        pub p50_ns: u64,
        /// Proto 1.8; recordings from before read 0.
        #[serde(default)]
        pub p90_ns: u64,
        pub p99_ns: u64,
        pub p999_ns: u64,
        pub max_ns: u64,
    }

    pub struct Process {
        pub id: String,
        pub pid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub ns_pid: Option<u32>,
        pub comm: String,
        pub ros: Option<Ros>,
        pub methods_available: Vec<Method>,
        pub method: Option<Method>,
        pub cpu_pct: Option<f64>,
        pub rss_bytes: Option<u64>,
        pub pss_bytes: Option<u64>,
        pub shared_cpu_pct: Option<f64>,
        pub threads: Vec<String>,
        pub nodes: Vec<String>,
        pub exited: bool,
        pub cold: bool,
        /// LD_PRELOAD processes only (design 13.2).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub overflow: Option<Overflow>,
        /// The `CUresult` that wedged the process's GPU (`flux_trace_gpu_wedged`); its GPU
        /// channels refuse claims, loans and takes from then on. Proto 1.7.
        #[serde(default, alias = "flux2_gpu_wedged", skip_serializing_if = "Option::is_none")]
        pub flux_gpu_wedged: Option<i32>,
        /// Proto 1.8 (design 20.1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cuda: Option<ProcCuda>,
        /// Device memory the process holds, from the GPU driver. Proto 1.8.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub gpu_mem_bytes: Option<u64>,
    }

    /// The process's CUDA driver calls over the window.
    pub struct ProcCuda {
        pub calls_hz: f64,
        pub launch_hz: f64,
        pub memcpy_bps: f64,
        pub alloc_hz: f64,
        /// Calls that returned an error.
        pub errors: u64,
        /// Time blocked in the synchronize calls.
        pub sync: Option<Lat>,
    }

    /// What the preload shim could not take, counted since it made its file.
    pub struct Overflow {
        pub threads: u64,
        pub table: u64,
        pub depth: u64,
    }

    pub struct Ros {
        pub distro: String,
        pub publish_abi: PublishAbi,
        pub tracetools_lttng: bool,
        pub gated_register: bool,
        /// Proto 1.9.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub executor_probes: Option<ExecutorProbes>,
    }

    pub struct Thread {
        pub id: String,
        pub tid: u32,
        pub name: String,
        pub policy: String,
        pub prio: i32,
        pub cpu_pct: Option<f64>,
        pub in_callback_cpu_pct: Option<f64>,
        pub cpu_wait: Option<Lat>,
        /// Proto 1.8 (design 20.4).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub futex: Option<Lat>,
        pub bucket: String,
        pub cpus: Vec<u32>,
    }

    pub struct Node {
        pub id: String,
        pub fqn: String,
        pub pid: u32,
        pub cpu_pct: Option<f64>,
        pub cpu_source: CpuSource,
        pub busy_pct: Option<f64>,
        pub cb_hz: Option<f64>,
        pub callbacks: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub malloc: Option<Malloc>,
    }

    pub struct Malloc {
        pub alloc_bps: f64,
        pub free_bps: f64,
        pub alloc_hz: f64,
    }

    pub struct Executor {
        pub id: String,
        pub kind: ExecutorKind,
        pub nodes: Vec<String>,
        pub groups: Vec<String>,
        pub callbacks: Vec<String>,
        pub busy_pct: Option<f64>,
        pub oncpu_pct: Option<f64>,
        pub wait: Option<Lat>,
        pub dispatch: Option<Lat>,
    }

    pub struct Callback {
        pub id: String,
        pub node: Option<String>,
        pub kind: CallbackKind,
        pub topic: Option<String>,
        /// The service name of a service callback. Proto 1.5.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub service: Option<String>,
        pub symbol: Option<String>,
        pub group: Option<String>,
        pub executor: Option<String>,
        pub hz: Option<f64>,
        pub expected_hz: Option<f64>,
        pub duration: Option<Lat>,
        pub oncpu_mean_ns: Option<u64>,
        pub dispatch: Option<Lat>,
        pub age: Option<Lat>,
        pub intra_pct: Option<f64>,
        pub nested: bool,
        /// Proto 1.8 (design 20.1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cuda: Option<CudaUse>,
        /// Mean futex wait per run. Proto 1.8 (design 20.4).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub futex_mean_ns: Option<u64>,
    }

    /// A callback's CUDA driver calls over the window; the means are per run of the callback.
    pub struct CudaUse {
        pub calls: u64,
        pub launches: u64,
        pub memcpy_bytes: u64,
        pub api_mean_ns: u64,
        pub sync_mean_ns: u64,
    }

    pub struct Topic {
        pub id: String,
        pub transport: Transport,
        pub name: String,
        pub publishers: Vec<String>,
        pub subscribers: Vec<String>,
        pub pub_hz: Option<f64>,
        pub expected_hz: Option<f64>,
        pub bw_bps: Option<f64>,
        pub hop: Option<Lat>,
        pub age_pub: Option<Lat>,
        pub age_take: Option<Lat>,
        pub drops: Drops,
        /// `rclcpp_publish` to the DDS write call, and the call itself. Proto 1.8 (design 20.2).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub pub_to_dds: Option<Lat>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub dds_write: Option<Lat>,
    }

    pub struct Drops {
        pub publish: Option<u64>,
        pub record: Option<u64>,
        pub ambiguous: Option<u64>,
    }

    pub struct Subscription {
        pub id: String,
        pub topic: String,
        pub node: String,
        pub recv_hz: Option<f64>,
        pub loss_pct: Option<f64>,
        pub lost: Option<u64>,
        pub intra_pct: Option<f64>,
        pub hop: Option<Lat>,
        pub wake: Option<Lat>,
    }

    pub struct Flow {
        pub id: String,
        pub kind: FlowKind,
        pub node: String,
        pub input: String,
        pub output: String,
        pub count: u64,
        pub dwell: Option<Lat>,
        pub ambiguous: u64,
    }

    /// One `liborma_span.so` span (design 20.5). `parent` is the span it ran inside.
    pub struct Span {
        pub id: String,
        pub name: String,
        pub pid: u32,
        pub node: Option<String>,
        pub callback: Option<String>,
        pub parent: Option<String>,
        pub hz: f64,
        pub duration: Lat,
    }

    /// One configured chain (design 20.6). `steps[k]` is the time from the source's publish to
    /// the publish on `topics[k + 1]`; the last is `e2e`. `broken` counts the publishes on a
    /// later topic that did not reach the source.
    pub struct Chain {
        pub name: String,
        pub mode: ChainMode,
        pub topics: Vec<String>,
        pub count: u64,
        pub e2e: Option<Lat>,
        pub steps: Vec<Option<Lat>>,
        pub broken: u64,
    }

    pub struct HostWindow {
        /// `scaling_cur_freq` by CPU number at the window's end.
        pub cpu_mhz: Vec<Option<u32>>,
        pub thermal: Vec<Thermal>,
        /// Throttle counters' increase over the window, by name.
        pub throttle: BTreeMap<String, u64>,
        pub net: Vec<NetIf>,
        pub udp: Option<Udp>,
        pub gpus: Vec<Gpu>,
    }

    pub struct Thermal {
        pub name: String,
        pub temp_c: f64,
    }

    /// `/proc/net/dev` over the window.
    pub struct NetIf {
        pub name: String,
        pub rx_bps: f64,
        pub tx_bps: f64,
        pub rx_pps: f64,
        pub tx_pps: f64,
        pub rx_drops: u64,
        pub tx_drops: u64,
        pub rx_errors: u64,
        pub tx_errors: u64,
    }

    /// The UDP rows of `/proc/net/snmp`, increase over the window.
    pub struct Udp {
        pub in_errors: u64,
        pub rcvbuf_errors: u64,
        pub sndbuf_errors: u64,
        pub no_ports: u64,
    }

    pub struct Gpu {
        pub index: u32,
        pub name: String,
        pub util_pct: Option<f64>,
        pub mem_used_bytes: Option<u64>,
        pub mem_total_bytes: Option<u64>,
        pub clock_mhz: Option<u32>,
        pub temp_c: Option<f64>,
        pub power_w: Option<f64>,
        /// Why the clock is held down now, empty when it is not.
        pub throttle: Vec<String>,
    }

    pub struct Agent {
        pub ormad: Option<ProcUse>,
        pub collectd: ProcUse,
        pub bpf: Vec<BpfProg>,
        pub maps: Vec<MapUse>,
        pub shm: ShmUse,
        pub lttng_discarded: Option<u64>,
        pub collect_ns: u64,
        pub probes_missing: Vec<ProbeMissing>,
    }

    pub struct ProcUse {
        pub cpu_pct: Option<f64>,
        pub rss_bytes: Option<u64>,
    }

    /// Differences over the window. `pid` is null for a program shared by every process.
    pub struct BpfProg {
        pub prog: String,
        pub pid: Option<u32>,
        pub run_time_ns: u64,
        pub run_cnt: u64,
    }

    pub struct MapUse {
        pub name: String,
        pub used: u64,
        pub max: u64,
        pub memlock_bytes: u64,
    }

    pub struct ShmUse {
        pub files: u32,
        pub bytes: u64,
        pub ring_drops: u64,
    }

    pub struct ProbeMissing {
        pub pid: Option<u32>,
        pub method: Method,
        pub point: String,
        pub reason: String,
    }

    pub struct OrmadRequest {
        pub id: u64,
        #[serde(flatten)]
        pub op: OrmadOp,
    }

    #[serde(tag = "op", rename_all = "snake_case")]
    pub enum OrmadOp {
        Hello(Hello),
        Attach(Attach),
        Detach { pids: Vec<u32>, sets: Vec<ProbeSet> },
        Stream { on: bool, pids: Vec<u32> },
        Window { window_s: f64 },
        /// ormad's items of design 6.7 over `duration_s`, answered with a `selfcheck` reply.
        /// `pids` is orma-collectd's selection; empty: every process.
        Selfcheck { duration_s: f64, pids: Vec<u32> },
        /// Sends the records ormad holds, then answers: every record from before the request
        /// arrives ahead of the reply. Without `drain_only` the answer also waits until the
        /// attaches asked for before are in; an ending recording needs only the records.
        Flush {
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            drain_only: bool,
        },
        Bye,
    }

    pub struct Attach {
        #[serde(flatten)]
        pub selection: Selection,
        pub sets: Vec<ProbeSet>,
        /// Thread id to the address of its slot's `cur_cb` (design 15.2), host ids.
        #[serde(default)]
        pub user_words: BTreeMap<String, u64>,
    }

    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum OrmadMessage {
        Hello(ServerHello),
        Window(OrmadWindow),
        EventLine(EventLine),
        Reply(Reply),
        Error(ErrorMessage),
    }

    /// The kernel maps' differences over one window.
    pub struct OrmadWindow {
        pub seq: u64,
        pub t_mono_ns: u64,
        pub window_ns: u64,
        pub aggregates: Vec<Aggregate>,
        pub bpf: Vec<BpfProg>,
        pub maps: Vec<MapUse>,
        pub lost: BTreeMap<String, u64>,
        pub probes_missing: Vec<ProbeMissing>,
        /// ormad's own CPU over the window and its RSS (design 6.8). Proto 1.4.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub usage: Option<ProcUse>,
        /// The attaches and detaches that ended since the last window. Proto 1.7.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub probe_changes: Vec<ProbeChange>,
    }

    /// One program attached to or detached from the points of one file (design 6.8): the
    /// kernel patched code and flushed TLBs on the CPUs then, monotonic ns.
    pub struct ProbeChange {
        pub from_ns: u64,
        pub to_ns: u64,
        pub attach: bool,
        pub prog: String,
        /// `None`: every process mapping the file.
        pub pid: Option<u32>,
        pub points: u32,
    }

    /// One table row's difference over a window (`orma_table_entry`, kinds `ORMA_AGG_*`). `tid`
    /// is 0 for a row a kernel map keeps per process.
    #[derive(Eq)]
    pub struct Aggregate {
        pub pid: u32,
        pub tid: u32,
        pub kind: u16,
        pub src: u16,
        pub entry: u32,
        pub key: u64,
        pub count: u64,
        pub sum: u64,
        pub max: u64,
        pub hist: Vec<u64>,
    }

    pub struct WindowLine {
        pub t_mono_ns: u64,
        pub t_real_ns: u64,
        pub snap: Window,
    }

    pub struct RecordMeta {
        pub schema: u32,
        pub orma_version: String,
        pub id: String,
        pub host: Host,
        pub ros: Vec<ProcessRos>,
        #[serde(alias = "flux2_contract")]
        pub flux_contract: Option<String>,
        pub options: RecordOptions,
        pub methods_by_pid: BTreeMap<String, Method>,
        pub owner: Owner,
        pub started: Instant,
        pub ended: Option<Instant>,
        pub state: RecordState,
        pub end_reason: Option<EndReason>,
        pub end_detail: Option<String>,
        pub probes_missing: Vec<ProbeMissing>,
        pub gaps: Vec<Gap>,
        pub counts: BTreeMap<String, Counts>,
        pub drops: RecordDrops,
        pub files: Vec<String>,
        pub bytes: u64,
        /// The host's settings that change what a measurement means, read at the start and at
        /// the end; an unreadable one is left out (design 14.9). Proto 1.7.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub system_start: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub system_end: BTreeMap<String, String>,
        /// Proto 1.7.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub interference: Option<Interference>,
        /// When CLOCK_REALTIME stepped during the recording, monotonic ns. Wall-clock ages and
        /// timestamp pairings near these are not to be trusted. Proto 1.7.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub realtime_steps: Vec<u64>,
        /// ormad's attaches and detaches during the recording. Proto 1.7.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub probe_changes: Vec<ProbeChange>,
    }

    /// Kernel counters over a recording, end minus start (design 14.9). Per CPU vectors are
    /// indexed by CPU number.
    pub struct Interference {
        /// `/proc/interrupts` by row; the numbered (device) rows are summed into `dev`.
        pub interrupts: BTreeMap<String, Vec<u64>>,
        pub softirqs: BTreeMap<String, Vec<u64>>,
        pub vmstat: BTreeMap<String, u64>,
        /// `total` of `/proc/pressure/<resource>`, by `<resource>.some` and `<resource>.full`.
        pub pressure_us: BTreeMap<String, u64>,
        /// Time a hypervisor ran something else, from `/proc/stat`.
        pub steal_ticks: Vec<u64>,
        pub threads: Vec<ThreadCounters>,
    }

    /// A target thread's faults and scheduler statistics; from its start when it began during
    /// the recording.
    pub struct ThreadCounters {
        pub pid: u32,
        pub tid: u32,
        pub comm: String,
        pub minflt: u64,
        pub majflt: u64,
        pub run_ns: u64,
        pub wait_ns: u64,
        pub slices: u64,
    }

    pub struct Host {
        pub hostname: String,
        pub kernel: String,
        pub arch: String,
        pub cpus: u32,
    }

    pub struct ProcessRos {
        pub pid: u32,
        pub starttime: u64,
        pub ros: Ros,
        pub rmw: Option<String>,
        pub libraries: Vec<Library>,
    }

    pub struct Library {
        pub path: String,
        pub build_id: Option<String>,
    }

    pub struct Instant {
        pub mono_ns: u64,
        pub real_ns: u64,
    }

    pub struct Gap {
        pub from_ns: u64,
        pub to_ns: u64,
        pub source: String,
        pub reason: String,
    }

    pub struct RecordDrops {
        pub shm_ring: u64,
        pub ormad_ring: u64,
        pub lttng: u64,
    }
}

/// Values of `ErrorMessage::code`; a reader keeps any other as text.
pub mod code {
    pub const PROTO_MAJOR: &str = "proto_major";
    pub const UNKNOWN_OP: &str = "unknown_op";
    pub const BAD_REQUEST: &str = "bad_request";
}

pub const CLIENT_OPS: &[&str] = &[
    "hello",
    "subscribe",
    "select",
    "configure",
    "record_start",
    "record_stop",
    "record_list",
    "record_show",
    "detail",
    "replay_open",
    "replay_seek",
    "replay_play",
    "replay_pause",
    "replay_close",
    "export",
    "selfcheck",
    "bye",
];

impl ErrorMessage {
    pub fn new(id: Option<u64>, code: &str, message: impl Into<String>) -> Self {
        ErrorMessage {
            id,
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// A client request from a JSON payload, or the error to answer it with (12.3).
pub fn parse_request(payload: &[u8]) -> Result<ClientRequest, ErrorMessage> {
    let value: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|e| ErrorMessage::new(None, code::BAD_REQUEST, e.to_string()))?;
    let id = value.get("id").and_then(serde_json::Value::as_u64);
    let op = value
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if !CLIENT_OPS.contains(&op) {
        return Err(ErrorMessage::new(
            id,
            code::UNKNOWN_OP,
            format!("unknown op {op:?}"),
        ));
    }
    serde_json::from_value(value)
        .map_err(|e| ErrorMessage::new(id, code::BAD_REQUEST, e.to_string()))
}

/// The client's `proto` against ours: majors must match (12.3).
pub fn check_proto(id: u64, client: Version) -> Result<(), ErrorMessage> {
    if client[0] == PROTO[0] {
        return Ok(());
    }
    Err(ErrorMessage::new(
        Some(id),
        code::PROTO_MAJOR,
        format!(
            "client speaks {}.{}, server {}.{}",
            client[0], client[1], PROTO[0], PROTO[1]
        ),
    ))
}

/// The JSON Schema `schema/v1.json`: every top-level type under `$defs`.
pub fn schema() -> serde_json::Value {
    let mut generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    let roots = [
        ("ClientRequest", generator.subschema_for::<ClientRequest>()),
        ("ServerMessage", generator.subschema_for::<ServerMessage>()),
        ("OrmadRequest", generator.subschema_for::<OrmadRequest>()),
        ("OrmadMessage", generator.subschema_for::<OrmadMessage>()),
        ("WindowLine", generator.subschema_for::<WindowLine>()),
        ("RecordMeta", generator.subschema_for::<RecordMeta>()),
    ];
    let defs = generator.take_definitions(true);
    for (name, root) in &roots {
        assert_eq!(
            root.get("$ref").and_then(|r| r.as_str()),
            Some(&*format!("#/$defs/{name}"))
        );
    }
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "orma protocol 1",
        "$defs": defs,
    })
}
