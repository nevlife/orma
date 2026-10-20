//! `orma analyze <dir>`: runs a recording's event files through the offline
//! model window by window and writes the intervals per message, per callback and per DDS write,
//! and the chain steps, as CSV, with exact summaries (p50, p90, p99, max; the CSV also p99.9). The tail
//! samples get the host's state from the recording's windows (design 20.12).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::Path;

use orma_common::events::{ORMA_EVFILE_MAGIC, orma_evfile_header};
use orma_common::read;
use orma_common::shm::ORMA_SHM_VERSION;
use orma_proto::Ingest;
use orma_proto::msg::{
    ChainMode, ChainSpec, FlowKind, HostWindow, Method, RecordMeta, RecordMethod, Window,
    WindowLine,
};
use orma_proto::records;

use crate::model::{Model, Sample, chain};
use crate::sources::ormad::Sel;

/// Windows run after the last record, so pending pairs and callbacks resolve.
const DRAIN_WINDOWS: u64 = 3;
/// Samples per entity in `tails.csv`: the largest, down to its p99.
const TAIL_ROWS: usize = 10;

/// Nearest-rank quantile of sorted values.
pub fn quantile(sorted: &[u64], q: f64) -> u64 {
    let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

/// When a sample happened, and on which thread when it has one.
type When = (u64, Option<u32>);
type At = Option<When>;
type Lat = BTreeMap<String, Vec<(u64, At)>>;

#[derive(Default)]
struct Tables {
    /// Table title, then entity id, then values in ns.
    lat: BTreeMap<&'static str, Lat>,
    sub_takes: BTreeMap<String, (u64, Option<u64>)>,
    topic_ambiguous: BTreeMap<String, u64>,
    topic_drops: BTreeMap<String, (u64, u64)>,
    flows: BTreeMap<String, (u64, u64)>,
    blind: BTreeMap<String, u64>,
    node_cpu: BTreeMap<String, (String, f64, bool)>,
    /// Per chain: instances that reached the end, and publishes that did not reach the source.
    chains: BTreeMap<String, (u64, u64)>,
    duration_ns: u64,
}

const HOP: &str = "transport hop (publish -> take)";
const WAKE: &str = "wake latency (publish -> wake)";
const DISPATCH: &str = "dispatch wait (wake -> callback start)";
const CALLBACK: &str = "callback duration";
const ONCPU: &str = "callback on-CPU";
const WAIT: &str = "executor wait (wait -> wake)";
const CPU_WAIT: &str = "CPU wait (wakeup -> on CPU)";
const DWELL: &str = "flow dwell (take -> publish)";
const SPAN: &str = "span duration";
const CHAIN: &str = "chain step (source publish -> publish on the step's topic)";
const CUDA: &str = "callback CUDA driver calls";
const CUDA_SYNC: &str = "callback CUDA synchronize";
const FUTEX: &str = "callback futex wait";
const PRE_DDS: &str = "publish -> DDS write start";
const DDS: &str = "DDS write";
const TABLES: [&str; 15] = [
    HOP, WAKE, DISPATCH, CALLBACK, ONCPU, CUDA, CUDA_SYNC, FUTEX, WAIT, CPU_WAIT, PRE_DDS, DDS,
    DWELL, SPAN, CHAIN,
];

impl Tables {
    fn add(&mut self, table: &'static str, id: &str, ns: u64, at: At) {
        self.lat
            .entry(table)
            .or_default()
            .entry(id.to_string())
            .or_default()
            .push((ns, at));
    }

    fn window(&mut self, w: &Window) {
        self.duration_ns += w.window_ns;
        for s in &w.subscriptions {
            let e = self.sub_takes.entry(s.id.clone()).or_default();
            e.0 += (s.recv_hz.unwrap_or(0.0) * w.window_ns as f64 / 1e9).round() as u64;
            if let Some(l) = s.lost {
                *e.1.get_or_insert(0) += l;
            }
        }
        for t in &w.topics {
            if let Some(a) = t.drops.ambiguous {
                *self.topic_ambiguous.entry(t.id.clone()).or_default() += a;
            }
            if t.drops.publish.is_some() || t.drops.record.is_some() {
                let e = self.topic_drops.entry(t.id.clone()).or_default();
                e.0 += t.drops.publish.unwrap_or(0);
                e.1 += t.drops.record.unwrap_or(0);
            }
        }
        for f in &w.flows {
            if f.kind == FlowKind::Blind {
                *self.blind.entry(f.id.clone()).or_default() += f.count;
            } else {
                let e = self.flows.entry(f.id.clone()).or_default();
                e.0 += f.count;
                e.1 += f.ambiguous;
            }
        }
        for c in &w.chains {
            let e = self.chains.entry(c.name.clone()).or_default();
            e.0 += c.count;
            e.1 += c.broken;
        }
        for n in &w.nodes {
            let e = self.node_cpu.entry(n.id.clone()).or_default();
            e.0.clone_from(&n.fqn);
            e.1 += n.cpu_pct.unwrap_or(0.0) * w.window_ns as f64 / 100.0;
            e.2 = matches!(n.cpu_source, orma_proto::msg::CpuSource::Sched);
        }
    }
}

fn us(ns: u64) -> String {
    format!("{:.1}", ns as f64 / 1000.0)
}

fn csv_opt(v: Option<u64>) -> String {
    v.map(|v| v.to_string()).unwrap_or_default()
}

/// The recording's event files in order, each checked and without its 16-byte header.
fn event_files(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let events = dir.join("events");
    // An LTTng recording and a windows recording have none.
    if !events.is_dir() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = fs::read_dir(&events)
        .map_err(|e| format!("{}: {e}", events.display()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".ormaev") || n.ends_with(".ormaev.zst"))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let path = events.join(&name);
            let raw = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let bytes = if name.ends_with(".zst") {
                zstd::decode_all(raw.as_slice()).map_err(|e| format!("{name}: {e}"))?
            } else {
                raw
            };
            let header: orma_evfile_header =
                read(&bytes).ok_or_else(|| format!("{name}: shorter than its header"))?;
            if header.magic != ORMA_EVFILE_MAGIC || header.version != ORMA_SHM_VERSION {
                return Err(format!(
                    "{name}: not an orma event file of version {ORMA_SHM_VERSION}"
                ));
            }
            Ok((name, bytes[size_of::<orma_evfile_header>()..].to_vec()))
        })
        .collect()
}

/// `NAME=t1,t2,...[:first]`, the `--chain` of `orma-analyze` and `orma analyze`.
pub fn parse_chain(spec: &str) -> Result<ChainSpec, String> {
    let (name, rest) = spec
        .split_once('=')
        .ok_or_else(|| format!("--chain {spec:?}: not NAME=t1,t2,...[:first]"))?;
    let (topics, mode) = match rest.strip_suffix(":first") {
        Some(t) => (t, ChainMode::First),
        None => (rest, ChainMode::Lineage),
    };
    let c = ChainSpec {
        name: name.into(),
        topics: topics.split(',').map(str::to_string).collect(),
        mode,
    };
    chain::check(std::slice::from_ref(&c))?;
    Ok(c)
}

pub struct Report {
    pub text: String,
    pub records: u64,
    pub windows: u64,
}

/// Analyzes the recording in `dir` and writes `dir/analysis/{deliveries,callbacks,summary}.csv`,
/// and `chains.csv` for `chains`, or for the recording's own chains when that is empty.
pub fn run(dir: &Path, chains: Vec<ChainSpec>) -> Result<Report, String> {
    let meta: RecordMeta = serde_json::from_slice(
        &fs::read(dir.join("meta.json")).map_err(|e| format!("meta.json: {e}"))?,
    )
    .map_err(|e| format!("meta.json: {e}"))?;
    let mut files = event_files(dir)?;
    let ctf = dir.join(crate::record::CTF);
    if files.is_empty() && ctf.is_dir() {
        let trace = crate::sources::lttng::ctf::read_trace(
            &ctf,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .map_err(|e| format!("{}: {e}", ctf.display()))?;
        files.push((crate::record::CTF.into(), trace.records));
    }
    if files.is_empty() {
        return Err("the recording has no event files (a windows-only recording)".into());
    }
    let lines: Vec<WindowLine> = fs::read_to_string(dir.join("windows.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let window_ns = lines
        .first()
        .ok_or("windows.jsonl has no window, so the window length is unknown")?
        .snap
        .window_ns
        .max(1);
    let offset = meta.started.real_ns as i64 - meta.started.mono_ns as i64;
    let methods: BTreeMap<u32, Method> = meta
        .methods_by_pid
        .iter()
        .filter_map(|(p, m)| Some((p.parse().ok()?, *m)))
        .collect();
    let every_pid_uprobe = meta.options.method == Some(RecordMethod::Uprobe);

    let o = &meta.options;
    let chains = match chains.is_empty() {
        true => o.chains.clone().unwrap_or_default(),
        false => chains,
    };
    chain::check(&chains)?;
    let latest = chain::latest_nodes(o.latest_nodes.as_deref().unwrap_or_default())?;
    let mut model = Model::offline();
    for p in &meta.ros {
        model.set_starttime(p.pid, p.starttime);
    }
    let futex = match o.futex == Some(true) {
        true => Sel::All,
        false => Sel::None,
    };
    model.set_probes(o.gpu == Some(true), futex);
    let has_chains = !chains.is_empty();
    model.set_flows(chains, latest);
    let out = dir.join("analysis");
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut deliveries = String::from(
        "topic,subscription,pub_pid,sub_pid,sub_tid,t_pub_ns,t_take_ns,hop_ns,wake_ns,dispatch_ns,callback,callback_ns\n",
    );
    let mut callbacks = String::from(
        "callback,node,tid,t_start_ns,duration_ns,oncpu_ns,dispatch_ns,cuda_ns,cuda_sync_ns,futex_ns\n",
    );
    let mut publishes = String::from("topic,pid,tid,t_write_end_ns,pub_to_dds_ns,dds_write_ns\n");
    let mut chain_rows = String::from("chain,mode,step,topic,t_source_ns,latency_ns\n");
    let mut tables = Tables::default();
    let mut windows = 0;
    let mut records = 0;
    let mut end: Option<u64> = None;

    let mut close = |model: &mut Model, t_end: u64, tables: &mut Tables| {
        let real = t_end.checked_add_signed(offset).unwrap_or(0);
        let w = model.window(t_end, real, window_ns, false);
        tables.window(&w);
        for s in model.take_samples() {
            match s {
                Sample::Delivery(d) => {
                    let at = Some((d.t_take_ns, Some(d.sub_tid)));
                    if let Some(h) = d.hop_ns {
                        tables.add(HOP, &d.topic, h, at);
                    }
                    if let Some(v) = d.wake_ns {
                        tables.add(WAKE, &d.subscription, v, at);
                    }
                    let _ = writeln!(
                        deliveries,
                        "{},{},{},{},{},{},{},{},{},{},{},{}",
                        d.topic,
                        d.subscription,
                        d.pub_pid.map(|p| p.to_string()).unwrap_or_default(),
                        d.sub_pid,
                        d.sub_tid,
                        csv_opt(d.t_pub_ns),
                        d.t_take_ns,
                        csv_opt(d.hop_ns),
                        csv_opt(d.wake_ns),
                        csv_opt(d.dispatch_ns),
                        d.callback.unwrap_or_default(),
                        csv_opt(d.callback_ns),
                    );
                }
                Sample::Callback {
                    id,
                    node,
                    tid,
                    t_ns,
                    duration_ns,
                    oncpu_ns,
                    dispatch_ns,
                    cuda_ns,
                    cuda_sync_ns,
                    futex_ns,
                } => {
                    let at = Some((t_ns, Some(tid)));
                    tables.add(CALLBACK, &id, duration_ns, at);
                    for (table, v) in [
                        (ONCPU, oncpu_ns),
                        (DISPATCH, dispatch_ns),
                        (CUDA, cuda_ns),
                        (CUDA_SYNC, cuda_sync_ns),
                        (FUTEX, futex_ns),
                    ] {
                        if let Some(v) = v {
                            tables.add(table, &id, v, at);
                        }
                    }
                    let _ = writeln!(
                        callbacks,
                        "{id},{},{tid},{t_ns},{duration_ns},{},{},{},{},{}",
                        node.unwrap_or_default(),
                        csv_opt(oncpu_ns),
                        csv_opt(dispatch_ns),
                        csv_opt(cuda_ns),
                        csv_opt(cuda_sync_ns),
                        csv_opt(futex_ns),
                    );
                }
                Sample::Publish {
                    topic,
                    pid,
                    tid,
                    t_ns,
                    pub_to_dds_ns,
                    dds_write_ns,
                } => {
                    let at = Some((t_ns, Some(tid)));
                    tables.add(DDS, &topic, dds_write_ns, at);
                    if let Some(v) = pub_to_dds_ns {
                        tables.add(PRE_DDS, &topic, v, at);
                    }
                    let _ = writeln!(
                        publishes,
                        "{topic},{pid},{tid},{t_ns},{},{dds_write_ns}",
                        csv_opt(pub_to_dds_ns)
                    );
                }
                Sample::Wait { executor, ns } => tables.add(WAIT, &executor, ns, None),
                Sample::CpuWait { thread, ns } => tables.add(CPU_WAIT, &thread, ns, None),
                Sample::Flow { id, dwell_ns } => tables.add(DWELL, &id, dwell_ns, None),
                Sample::Span { id, duration_ns } => tables.add(SPAN, &id, duration_ns, None),
                Sample::Chain {
                    name,
                    mode,
                    step,
                    topic,
                    t_source_ns,
                    latency_ns,
                } => {
                    let at = Some((t_source_ns + latency_ns, None));
                    tables.add(CHAIN, &format!("{name}:{step}:{topic}"), latency_ns, at);
                    let mode = match mode {
                        ChainMode::Lineage => "lineage",
                        ChainMode::First => "first",
                    };
                    let _ = writeln!(
                        chain_rows,
                        "{name},{mode},{step},{topic},{t_source_ns},{latency_ns}"
                    );
                }
            }
        }
    };

    for (name, bytes) in &files {
        for r in records::positioned(bytes) {
            let r = r.map_err(|e| format!("{name}: {e}"))?;
            let t = r.header.t_mono_ns;
            let e = *end.get_or_insert(t - t % window_ns + window_ns);
            let mut e = e;
            while t >= e + window_ns {
                close(&mut model, e, &mut tables);
                windows += 1;
                e += window_ns;
            }
            end = Some(e);
            // A uprobe recording of every process names no pids.
            let method = match methods.get(&r.pid) {
                Some(m) => *m,
                None if every_pid_uprobe => Method::Uprobe,
                None => return Err(format!("{name}: pid {} has no method in meta.json", r.pid)),
            };
            model.record(method, &r);
            records += 1;
        }
    }
    let mut e = end.expect("at least one file");
    for _ in 0..DRAIN_WINDOWS {
        close(&mut model, e, &mut tables);
        windows += 1;
        e += window_ns;
    }
    // The drain windows had no records in them.
    tables.duration_ns -= (DRAIN_WINDOWS - 1) * window_ns;

    let mut text = render(&tables, &mut out_summary(&out)?)?;
    let hosts = Hosts::new(&lines);
    if !hosts.0.is_empty() {
        let (tails, context) = tails(&tables, &hosts);
        write(&out.join("tails.csv"), &tails)?;
        text.push_str(&context);
    }
    write(&out.join("deliveries.csv"), &deliveries)?;
    write(&out.join("callbacks.csv"), &callbacks)?;
    if publishes.lines().count() > 1 {
        write(&out.join("publishes.csv"), &publishes)?;
    }
    if has_chains {
        write(&out.join("chains.csv"), &chain_rows)?;
    }
    Ok(Report {
        text,
        records,
        windows,
    })
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn out_summary(out: &Path) -> Result<fs::File, String> {
    let path = out.join("summary.csv");
    let mut f = fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    writeln!(f, "table,entity,n,p50_ns,p90_ns,p99_ns,p999_ns,max_ns").map_err(|e| e.to_string())?;
    Ok(f)
}

fn render(t: &Tables, csv: &mut fs::File) -> Result<String, String> {
    let mut s = String::new();
    for table in TABLES {
        let Some(rows) = t.lat.get(table) else {
            continue;
        };
        let mut rows: Vec<(&String, Vec<u64>)> = rows
            .iter()
            .map(|(id, v)| {
                let mut v: Vec<u64> = v.iter().map(|s| s.0).collect();
                v.sort_unstable();
                (id, v)
            })
            .collect();
        rows.sort_by_key(|(_, v)| std::cmp::Reverse(quantile(v, 0.99)));
        let _ = writeln!(
            s,
            "\n{table} [us]\n{:>10} {:>10} {:>10} {:>10} {:>10}  entity",
            "n", "p50", "p90", "p99", "max"
        );
        for (id, v) in rows {
            let q = [0.5, 0.9, 0.99, 0.999].map(|q| quantile(&v, q));
            let max = *v.last().expect("non-empty");
            let _ = writeln!(
                s,
                "{:>10} {:>10} {:>10} {:>10} {:>10}  {id}",
                v.len(),
                us(q[0]),
                us(q[1]),
                us(q[2]),
                us(max)
            );
            writeln!(
                csv,
                "{table},{id},{},{},{},{},{},{max}",
                v.len(),
                q[0],
                q[1],
                q[2],
                q[3]
            )
            .map_err(|e| e.to_string())?;
        }
    }
    if !t.sub_takes.is_empty() {
        let _ = writeln!(
            s,
            "\nsubscriptions\n{:>10} {:>10} {:>8}  entity",
            "taken", "lost", "loss %"
        );
        for (id, (taken, lost)) in &t.sub_takes {
            let (l, p) = match lost {
                Some(l) if taken + l > 0 => (
                    l.to_string(),
                    format!("{:.2}", *l as f64 * 100.0 / (taken + l) as f64),
                ),
                Some(l) => (l.to_string(), "-".into()),
                None => ("-".into(), "-".into()),
            };
            let _ = writeln!(s, "{taken:>10} {l:>10} {p:>8}  {id}");
        }
    }
    let ambiguous: Vec<_> = t.topic_ambiguous.iter().filter(|(_, n)| **n > 0).collect();
    if !ambiguous.is_empty() {
        let _ = writeln!(
            s,
            "\ntakes not paired: two publishers sent the same (topic, source_timestamp)"
        );
        for (id, n) in ambiguous {
            let _ = writeln!(s, "{n:>10}  {id}");
        }
    }
    if !t.topic_drops.is_empty() {
        let _ = writeln!(
            s,
            "\nflux drops\n{:>10} {:>10}  entity",
            "publish", "record"
        );
        for (id, (p, r)) in &t.topic_drops {
            let _ = writeln!(s, "{p:>10} {r:>10}  {id}");
        }
    }
    if !t.flows.is_empty() || !t.blind.is_empty() {
        let _ = writeln!(s, "\nflows\n{:>10} {:>10}  entity", "linked", "ambiguous");
        for (id, (c, a)) in &t.flows {
            let _ = writeln!(s, "{c:>10} {a:>10}  {id}");
        }
        for (id, c) in &t.blind {
            let _ = writeln!(s, "{c:>10} {:>10}  {id}", "-");
        }
    }
    if !t.chains.is_empty() {
        let _ = writeln!(s, "\nchains\n{:>10} {:>10}  chain", "complete", "broken");
        for (name, (count, broken)) in &t.chains {
            let _ = writeln!(s, "{count:>10} {broken:>10}  {name}");
        }
    }
    if !t.node_cpu.is_empty() {
        let _ = writeln!(
            s,
            "\nnode CPU over {:.1} s\n{:>10} {:>8} {:>10}  node",
            t.duration_ns as f64 / 1e9,
            "cpu s",
            "cpu %",
            "source"
        );
        let mut rows: Vec<_> = t.node_cpu.iter().collect();
        rows.sort_by(|a, b| b.1.1.total_cmp(&a.1.1));
        for (id, (fqn, ns, sched)) in rows {
            let _ = writeln!(
                s,
                "{:>10.3} {:>8.2} {:>10}  {fqn} {id}",
                ns / 1e9,
                ns * 100.0 / t.duration_ns.max(1) as f64,
                if *sched { "sched" } else { "wall_bound" }
            );
        }
    }
    Ok(s)
}

/// A recording window that carries the host's state.
struct HostAt<'a> {
    end: u64,
    len: u64,
    host: &'a HostWindow,
    cpus: BTreeMap<u32, &'a [u32]>,
}

struct Hosts<'a>(Vec<HostAt<'a>>);

impl<'a> Hosts<'a> {
    fn new(lines: &'a [WindowLine]) -> Self {
        Hosts(
            lines
                .iter()
                .filter_map(|l| {
                    Some(HostAt {
                        end: l.t_mono_ns,
                        len: l.snap.window_ns,
                        host: l.snap.host.as_ref()?,
                        cpus: l
                            .snap
                            .threads
                            .iter()
                            .map(|t| (t.tid, &t.cpus[..]))
                            .collect(),
                    })
                })
                .collect(),
        )
    }

    /// The state of the window around `t`: the lowest clock of the CPUs the thread ran on in it
    /// (all CPUs when it is not known), the hottest zone, the throttle events, and each GPU's
    /// clock, temperature and throttle reasons.
    fn context(&self, t: u64, tid: Option<u32>) -> Option<[String; 6]> {
        let w = self.0.iter().find(|w| t < w.end && t + w.len >= w.end)?;
        let h = w.host;
        let cpus = tid.and_then(|t| w.cpus.get(&t)).filter(|c| !c.is_empty());
        let mhz = match cpus {
            Some(c) => c.iter().filter_map(|&i| *h.cpu_mhz.get(i as usize)?).min(),
            None => h.cpu_mhz.iter().flatten().copied().min(),
        };
        let thermal = h.thermal.iter().map(|z| z.temp_c).reduce(f64::max);
        let gpus = |f: &dyn Fn(&orma_proto::msg::Gpu) -> String| {
            h.gpus.iter().map(f).collect::<Vec<_>>().join("|")
        };
        Some([
            mhz.map(|m| m.to_string()).unwrap_or_default(),
            thermal.map(|c| format!("{c:.1}")).unwrap_or_default(),
            h.throttle.values().sum::<u64>().to_string(),
            gpus(&|g| g.clock_mhz.map(|m| m.to_string()).unwrap_or_default()),
            gpus(&|g| g.temp_c.map(|c| format!("{c:.1}")).unwrap_or_default()),
            gpus(&|g| g.throttle.join("+")),
        ])
    }
}

/// `tails.csv`, and for the summary the max of each table's worst entity with its context.
fn tails(t: &Tables, hosts: &Hosts) -> (String, String) {
    let mut csv = String::from(
        "table,entity,rank,ns,t_ns,cpu_mhz,thermal_max_c,throttle,gpu_clock_mhz,gpu_temp_c,gpu_throttle\n",
    );
    let mut text = String::from(
        "\ntail context (the worst entity's max; CPU MHz of its thread's CPUs, hottest zone, throttle events, GPU clock/temp/throttle)\n",
    );
    let mut any = false;
    for table in TABLES {
        let Some(rows) = t.lat.get(table) else {
            continue;
        };
        let mut worst: Option<(u64, &String, u64, When)> = None;
        for (id, v) in rows {
            let mut s: Vec<(u64, When)> = v
                .iter()
                .filter_map(|(ns, at)| Some((*ns, (*at)?)))
                .collect();
            if s.is_empty() {
                continue;
            }
            let mut sorted: Vec<u64> = v.iter().map(|x| x.0).collect();
            sorted.sort_unstable();
            let p99 = quantile(&sorted, 0.99);
            s.sort_by_key(|x| std::cmp::Reverse(x.0));
            if worst.is_none_or(|w| p99 > w.0) {
                worst = Some((p99, id, s[0].0, s[0].1));
            }
            for (rank, (ns, (at, tid))) in s
                .iter()
                .take_while(|x| x.0 >= p99)
                .take(TAIL_ROWS)
                .enumerate()
            {
                let Some(c) = hosts.context(*at, *tid) else {
                    continue;
                };
                let _ = writeln!(csv, "{table},{id},{},{ns},{at},{}", rank + 1, c.join(","));
            }
        }
        if let Some((_, id, ns, (at, tid))) = worst
            && let Some(c) = hosts.context(at, tid)
        {
            any = true;
            let _ = writeln!(
                text,
                "{table}: {id} max {} us | cpu {} MHz, {} C, throttle {} | gpu {} MHz, {} C, {}",
                us(ns),
                dash(&c[0]),
                dash(&c[1]),
                c[2],
                dash(&c[3]),
                dash(&c[4]),
                dash(&c[5])
            );
        }
    }
    (csv, if any { text } else { String::new() })
}

fn dash(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}
