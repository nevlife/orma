//! `armo analyze <dir>`: runs a recording's event files through the offline
//! model window by window and writes the intervals per message and per callback as CSV,
//! with exact tail-first summaries (p90, p99, p99.9, max).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::Path;

use armo_common::events::{ARMO_EVFILE_MAGIC, armo_evfile_header};
use armo_common::read;
use armo_common::shm::ARMO_SHM_VERSION;
use armo_proto::Ingest;
use armo_proto::msg::{FlowKind, Method, RecordMeta, RecordMethod, Window, WindowLine};
use armo_proto::records;

use crate::model::{Model, Sample};

/// Windows run after the last record, so pending pairs and callbacks resolve.
const DRAIN_WINDOWS: u64 = 3;

/// Nearest-rank quantile of sorted values.
pub fn quantile(sorted: &[u64], q: f64) -> u64 {
    let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

#[derive(Default)]
struct Tables {
    /// Table title, then entity id, then values in ns.
    lat: BTreeMap<&'static str, BTreeMap<String, Vec<u64>>>,
    sub_takes: BTreeMap<String, (u64, Option<u64>)>,
    topic_ambiguous: BTreeMap<String, u64>,
    topic_drops: BTreeMap<String, (u64, u64)>,
    flows: BTreeMap<String, (u64, u64)>,
    blind: BTreeMap<String, u64>,
    node_cpu: BTreeMap<String, (String, f64, bool)>,
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

impl Tables {
    fn add(&mut self, table: &'static str, id: &str, ns: u64) {
        self.lat
            .entry(table)
            .or_default()
            .entry(id.to_string())
            .or_default()
            .push(ns);
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
        for n in &w.nodes {
            let e = self.node_cpu.entry(n.id.clone()).or_default();
            e.0.clone_from(&n.fqn);
            e.1 += n.cpu_pct.unwrap_or(0.0) * w.window_ns as f64 / 100.0;
            e.2 = matches!(n.cpu_source, armo_proto::msg::CpuSource::Sched);
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
        .filter(|n| n.ends_with(".armoev") || n.ends_with(".armoev.zst"))
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
            let header: armo_evfile_header =
                read(&bytes).ok_or_else(|| format!("{name}: shorter than its header"))?;
            if header.magic != ARMO_EVFILE_MAGIC || header.version != ARMO_SHM_VERSION {
                return Err(format!(
                    "{name}: not an armo event file of version {ARMO_SHM_VERSION}"
                ));
            }
            Ok((name, bytes[size_of::<armo_evfile_header>()..].to_vec()))
        })
        .collect()
}

pub struct Report {
    pub text: String,
    pub records: u64,
    pub windows: u64,
}

/// Analyzes the recording in `dir` and writes `dir/analysis/{deliveries,callbacks,summary}.csv`.
pub fn run(dir: &Path) -> Result<Report, String> {
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
    let window_ns = fs::read_to_string(dir.join("windows.jsonl"))
        .ok()
        .and_then(|t| serde_json::from_str::<WindowLine>(t.lines().next()?).ok())
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

    let mut model = Model::offline();
    for p in &meta.ros {
        model.set_starttime(p.pid, p.starttime);
    }
    let out = dir.join("analysis");
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut deliveries = String::from(
        "topic,subscription,pub_pid,sub_pid,sub_tid,t_pub_ns,t_take_ns,hop_ns,wake_ns,dispatch_ns,callback,callback_ns\n",
    );
    let mut callbacks =
        String::from("callback,node,tid,t_start_ns,duration_ns,oncpu_ns,dispatch_ns\n");
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
                    if let Some(h) = d.hop_ns {
                        tables.add(HOP, &d.topic, h);
                    }
                    if let Some(v) = d.wake_ns {
                        tables.add(WAKE, &d.subscription, v);
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
                } => {
                    tables.add(CALLBACK, &id, duration_ns);
                    if let Some(v) = oncpu_ns {
                        tables.add(ONCPU, &id, v);
                    }
                    if let Some(v) = dispatch_ns {
                        tables.add(DISPATCH, &id, v);
                    }
                    let _ = writeln!(
                        callbacks,
                        "{id},{},{tid},{t_ns},{duration_ns},{},{}",
                        node.unwrap_or_default(),
                        csv_opt(oncpu_ns),
                        csv_opt(dispatch_ns),
                    );
                }
                Sample::Wait { executor, ns } => tables.add(WAIT, &executor, ns),
                Sample::CpuWait { thread, ns } => tables.add(CPU_WAIT, &thread, ns),
                Sample::Flow { id, dwell_ns } => tables.add(DWELL, &id, dwell_ns),
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

    let text = render(&tables, &mut out_summary(&out)?)?;
    write(&out.join("deliveries.csv"), &deliveries)?;
    write(&out.join("callbacks.csv"), &callbacks)?;
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
    writeln!(f, "table,entity,n,p90_ns,p99_ns,p999_ns,max_ns").map_err(|e| e.to_string())?;
    Ok(f)
}

fn render(t: &Tables, csv: &mut fs::File) -> Result<String, String> {
    let mut s = String::new();
    for table in [HOP, WAKE, DISPATCH, CALLBACK, ONCPU, WAIT, CPU_WAIT, DWELL] {
        let Some(rows) = t.lat.get(table) else {
            continue;
        };
        let mut rows: Vec<(&String, Vec<u64>)> = rows
            .iter()
            .map(|(id, v)| {
                let mut v = v.clone();
                v.sort_unstable();
                (id, v)
            })
            .collect();
        rows.sort_by_key(|(_, v)| std::cmp::Reverse(quantile(v, 0.99)));
        let _ = writeln!(
            s,
            "\n{table} [us]\n{:>10} {:>10} {:>10} {:>10} {:>10}  entity",
            "n", "p90", "p99", "p99.9", "max"
        );
        for (id, v) in rows {
            let q = [0.9, 0.99, 0.999].map(|q| quantile(&v, q));
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
                "{table},{id},{},{},{},{},{max}",
                v.len(),
                q[0],
                q[1],
                q[2]
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
            "\nflux2 drops\n{:>10} {:>10}  entity",
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
