//! The armod source against a fake armod: hello, the requests that follow the
//! selection and recordings, event batches into recordings, windows into the model, a lost
//! armod leaving a gap, `selfcheck` replies that carry armod's checks, and the current callbacks
//! of libarmo_lttng.so's file handed to armod.

mod collectd;

use std::os::unix::fs::FileExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use armo_collectd::sources::armod::{Plan, Sel, requests};
use armo_common::events::*;
use armo_common::shm::ARMO_BACKEND_LTTNG;
use armo_proto::fake::shm::{FakeShm, Geometry};
use armo_proto::frame::{self, Kind};
use armo_proto::msg::*;
use armo_proto::records;
use collectd::*;

#[derive(Clone, Default)]
struct FakeArmod {
    received: Arc<Mutex<Vec<ArmodOp>>>,
    conn: Arc<Mutex<Option<UnixStream>>>,
    connections: Arc<Mutex<u32>>,
    /// Whether a selfcheck gets its answer; without, it waits until the connection drops.
    answers_selfcheck: Arc<AtomicBool>,
    /// Bumped by each `window` request; the ticker of an older one stops.
    ticker: Arc<AtomicU64>,
    /// A period of its own in ms, instead of the requested `window_s`; 0 for none.
    period_ms: Arc<AtomicU64>,
}

fn empty_armod_window(seq: u64) -> ArmodMessage {
    ArmodMessage::Window(ArmodWindow {
        seq,
        t_mono_ns: 0,
        window_ns: 0,
        aggregates: Vec::new(),
        bpf: Vec::new(),
        maps: Vec::new(),
        lost: [("ring".to_string(), 0)].into(),
        probes_missing: Vec::new(),
        usage: None,
        probe_changes: Vec::new(),
    })
}

const ARMOD_CHECK: &str = "tracepoint layout";

impl FakeArmod {
    fn start(path: &Path) -> FakeArmod {
        FakeArmod::default().listen(path)
    }

    fn listen(self, path: &Path) -> FakeArmod {
        let fake = self;
        let listener = UnixListener::bind(path).unwrap();
        let f = fake.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let hello = frame::read(&mut stream).unwrap().unwrap();
                let hello: ArmodRequest = serde_json::from_slice(&hello.payload).unwrap();
                assert!(matches!(hello.op, ArmodOp::Hello(_)));
                let reply = ArmodMessage::Hello(ServerHello {
                    proto: PROTO,
                    server: "armod".into(),
                    version: "0".into(),
                });
                // Counted before the reply, which is what lets armo-collectd report connected.
                *f.conn.lock().unwrap() = Some(stream.try_clone().unwrap());
                *f.connections.lock().unwrap() += 1;
                frame::write_json(&mut stream, &reply).unwrap();
                while let Ok(Some(m)) = frame::read(&mut stream) {
                    let r: ArmodRequest = serde_json::from_slice(&m.payload).unwrap();
                    let body = match &r.op {
                        ArmodOp::Flush { .. } => Some(ReplyBody::Empty {}),
                        ArmodOp::Selfcheck { .. }
                            if f.answers_selfcheck.load(Ordering::Relaxed) =>
                        {
                            Some(ReplyBody::Selfcheck {
                                checks: vec![Check {
                                    item: ARMOD_CHECK.into(),
                                    status: CheckStatus::Pass,
                                    detail: "at the built offsets".into(),
                                }],
                            })
                        }
                        _ => None,
                    };
                    if let Some(body) = body {
                        let reply = ArmodMessage::Reply(Reply {
                            id: r.id,
                            ok: true,
                            body,
                        });
                        f.try_send(&reply);
                    }
                    if let ArmodOp::Window { window_s } = r.op {
                        let own = f.period_ms.load(Ordering::Relaxed);
                        f.tick(match own {
                            0 => Duration::from_secs_f64(window_s),
                            ms => Duration::from_millis(ms),
                        });
                    }
                    f.received.lock().unwrap().push(r.op);
                }
            }
        });
        fake
    }

    /// Sends a window every `every`, as armod does after a `window` request.
    fn tick(&self, every: Duration) {
        let generation = self.ticker.fetch_add(1, Ordering::Relaxed) + 1;
        let f = self.clone();
        thread::spawn(move || {
            for seq in 1.. {
                thread::sleep(every);
                if f.ticker.load(Ordering::Relaxed) != generation
                    || !f.try_send(&empty_armod_window(seq))
                {
                    break;
                }
            }
        });
    }

    fn try_send(&self, m: &ArmodMessage) -> bool {
        let mut conn = self.conn.lock().unwrap();
        conn.as_mut()
            .is_some_and(|s| frame::write_json(s, m).is_ok())
    }

    fn ops(&self) -> Vec<ArmodOp> {
        self.received.lock().unwrap().clone()
    }

    fn has(&self, want: impl Fn(&ArmodOp) -> bool) -> bool {
        self.ops().iter().any(want)
    }

    fn send(&self, kind: Kind, payload: &[u8]) {
        let mut conn = self.conn.lock().unwrap();
        frame::write(conn.as_mut().unwrap(), kind, payload).unwrap();
    }

    fn drop_connection(&self) {
        let conn = self.conn.lock().unwrap().take().unwrap();
        conn.shutdown(std::net::Shutdown::Both).unwrap();
        self.received.lock().unwrap().clear();
    }
}

fn attached(op: &ArmodOp, set: ProbeSet, all: bool, pids: &[u32]) -> bool {
    matches!(op, ArmodOp::Attach(a) if a.sets == [set] && a.selection.all == all
        && a.selection.pids == pids)
}

fn batch(pid: u32, n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..n {
        let ev = armo_ev_callback_start {
            callback: 0x55,
            is_intra_process: 0,
            pad: 0,
        };
        records::encode_positioned(&mut out, pid, pid + 1, ARMO_EV_CALLBACK_START, i, &ev, &[]);
    }
    out
}

const TRACED: u32 = 4_000_042;
const OTHER: u32 = 4_000_077;

#[test]
fn armod_follows_selection_and_recordings_and_its_loss_leaves_a_gap() {
    let dir = scratch("armod");
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    c.request(ClientOp::Subscribe {
        streams: vec![Stream::Window, Stream::EventLine],
    })
    .unwrap();
    assert_eq!(c.state.as_ref().unwrap().sources.armod, ArmodLink::Absent);

    let fake = FakeArmod::start(&dir.join("armod.sock"));
    c.until(|m| matches!(m, ServerMessage::State(s) if s.sources.armod == ArmodLink::Connected));
    until(|| fake.has(|o| matches!(o, ArmodOp::Window { window_s } if *window_s == 1.0)));
    until(|| fake.has(|o| attached(o, ProbeSet::Discovery, true, &[])));
    until(|| fake.has(|o| attached(o, ProbeSet::Sched, true, &[])));

    let (id, rec) = c.start(RecordOptions {
        method: Some(RecordMethod::Uprobe),
        targets: Some(Targets {
            all: false,
            pids: vec![TRACED],
            node: None,
            topic: None,
            exclude: None,
        }),
        ..options(&dir)
    });
    until(|| fake.has(|o| attached(o, ProbeSet::Hot, false, &[TRACED])));
    until(|| fake.has(|o| matches!(o, ArmodOp::Stream { on: true, pids } if *pids == [TRACED])));

    let usage = ProcUse {
        cpu_pct: Some(0.4),
        rss_bytes: Some(20 << 20),
    };
    let mut events = batch(TRACED, 3);
    events.extend(batch(OTHER, 2));
    events.extend(batch(TRACED, 1));
    fake.send(Kind::Events, &events);
    let window = ArmodMessage::Window(ArmodWindow {
        seq: 1,
        t_mono_ns: 5,
        window_ns: 1_000_000_000,
        aggregates: vec![Aggregate {
            pid: TRACED,
            tid: 0,
            kind: 1,
            src: 1,
            entry: 0,
            key: 0x55,
            count: 4,
            sum: 400,
            max: 200,
            hist: vec![0; 40],
        }],
        bpf: Vec::new(),
        maps: Vec::new(),
        lost: [("ring".to_string(), 2), ("recursion".to_string(), 1)].into(),
        probes_missing: Vec::new(),
        usage: Some(usage.clone()),
        probe_changes: vec![ProbeChange {
            from_ns: 0,
            to_ns: armo_collectd::sys::mono_ns(),
            attach: true,
            prog: "rcl_publish".into(),
            pid: Some(TRACED),
            points: 1,
        }],
    });
    fake.send(Kind::Json, &serde_json::to_vec(&window).unwrap());
    until(|| {
        let seen = server.seen.0.lock().unwrap();
        seen.records
            .iter()
            .filter(|r| r.0 == Method::Uprobe)
            .count()
            == 6
            && seen
                .aggregates
                .iter()
                .any(|(m, a)| *m == Method::Uprobe && a.count == 4)
    });

    c.until(|m| matches!(m, ServerMessage::Window(w) if w.agent.armod.as_ref() == Some(&usage)));

    fake.drop_connection();
    c.until(|m| matches!(m, ServerMessage::EventLine(l) if l.kind == "armod_lost"));
    c.until(|m| matches!(m, ServerMessage::State(s) if s.sources.armod == ArmodLink::Lost));
    c.until(|m| matches!(m, ServerMessage::State(s) if s.sources.armod == ArmodLink::Connected));
    // The usage of the lost armod does not outlive it.
    c.until(|m| matches!(m, ServerMessage::Window(w) if w.agent.armod.is_none()));
    assert_eq!(*fake.connections.lock().unwrap(), 2);
    until(|| fake.has(|o| attached(o, ProbeSet::Hot, false, &[TRACED])));

    c.stop(&id);
    until(|| fake.has(|o| *o == ArmodOp::Flush { drain_only: true }));
    until(|| fake.has(|o| matches!(o, ArmodOp::Stream { on: false, .. })));
    let m = meta(&rec);
    assert_eq!(m.methods_by_pid[&TRACED.to_string()], Method::Uprobe);
    assert_eq!(m.counts[&format!("p:{TRACED}:0")].events, 4);
    assert!(!m.counts.keys().any(|k| k.contains(&OTHER.to_string())));
    assert_eq!(m.drops.armod_ring, 3);
    assert_eq!(m.counts["armod"].lost, 3);
    assert_eq!(m.probe_changes.len(), 1);
    assert_eq!(m.gaps.len(), 1);
    assert_eq!(
        (m.gaps[0].source.as_str(), m.gaps[0].reason.as_str()),
        ("armod", "armod_lost")
    );
    assert!(m.gaps[0].to_ns > m.gaps[0].from_ns);
    let bytes = std::fs::read(rec.join("events/000.armoev")).unwrap();
    let pids: Vec<u32> = records::positioned(&bytes[16..])
        .map(|r| r.unwrap().pid)
        .collect();
    assert_eq!(pids, [TRACED; 4]);
}

fn gettid() -> u32 {
    std::fs::read_link("/proc/thread-self")
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn checks(c: &mut Client, duration_s: f64) -> Vec<Check> {
    match c.request(ClientOp::Selfcheck { duration_s }) {
        Ok(ReplyBody::Selfcheck { checks }) => checks,
        other => panic!("selfcheck: {other:?}"),
    }
}

fn status(checks: &[Check], item: &str) -> Option<CheckStatus> {
    checks.iter().find(|c| c.item == item).map(|c| c.status)
}

#[test]
fn selfcheck_waits_for_armod_and_carries_its_checks() {
    let dir = scratch("selfcheck");
    std::fs::create_dir_all(dir.join("shm")).unwrap();
    let pid = std::process::id();
    let shm = |starttime| {
        let backend = armo_common::shm::ARMO_BACKEND_SHM;
        FakeShm::create(
            &dir.join("shm"),
            pid,
            starttime,
            backend,
            Geometry::default(),
        )
        .unwrap()
    };
    // This process is no container's: its own file has no row, one of another start fails.
    let (own, stale) = (shm(own_starttime()), shm(1));
    own.claim(gettid()).unwrap();
    stale.claim(gettid()).unwrap();
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    let alone = checks(&mut c, 3.0);
    assert_eq!(status(&alone, "armod"), Some(CheckStatus::Unknown));
    assert_eq!(status(&alone, ARMOD_CHECK), None);
    let container = format!("container pid {pid} (preload)");
    let rows: Vec<&Check> = alone.iter().filter(|c| c.item == container).collect();
    assert_eq!(rows.len(), 1, "{alone:?}");
    assert_eq!(rows[0].status, CheckStatus::Fail);
    assert!(rows[0].detail.contains("no such process"));

    let fake = FakeArmod::start(&dir.join("armod.sock"));
    fake.answers_selfcheck.store(true, Ordering::Relaxed);
    c.until(|m| matches!(m, ServerMessage::State(s) if s.sources.armod == ArmodLink::Connected));
    c.request(ClientOp::Select(Select {
        selection: Selection {
            all: false,
            pids: vec![TRACED],
            ..Default::default()
        },
        method_by_pid: Default::default(),
    }))
    .unwrap();
    let merged = checks(&mut c, 3.0);
    assert_eq!(status(&merged, "armod"), Some(CheckStatus::Pass));
    assert_eq!(status(&merged, "lttng"), Some(CheckStatus::Unknown));
    assert_eq!(status(&merged, ARMOD_CHECK), Some(CheckStatus::Pass));
    until(|| {
        fake.has(|o| {
            *o == ArmodOp::Selfcheck {
                duration_s: 3.0,
                pids: vec![TRACED],
            }
        })
    });

    // armod goes away with a selfcheck open: the reply comes without its checks.
    fake.answers_selfcheck.store(false, Ordering::Relaxed);
    let id = c.send(ClientOp::Selfcheck { duration_s: 3.0 });
    until(|| fake.has(|o| matches!(o, ArmodOp::Selfcheck { .. })));
    fake.drop_connection();
    let Ok(ReplyBody::Selfcheck { checks }) = c.answer(id) else {
        panic!("no selfcheck reply after armod went away");
    };
    assert_eq!(status(&checks, "armod"), Some(CheckStatus::Unknown));
    assert_eq!(status(&checks, ARMOD_CHECK), None);
}

#[test]
fn scheduler_records_reach_only_recordings_with_sched() {
    let dir = scratch("armod_sched");
    let server = Server::start(&dir, config(&dir));
    let fake = FakeArmod::start(&dir.join("armod.sock"));
    let mut c = server.client("armo");
    c.until(|m| matches!(m, ServerMessage::State(s) if s.sources.armod == ArmodLink::Connected));
    let targets = || {
        Some(Targets {
            all: false,
            pids: vec![TRACED],
            node: None,
            topic: None,
            exclude: None,
        })
    };
    let mut recs = Vec::new();
    for sched in [false, true] {
        let (id, d) = c.start(RecordOptions {
            method: Some(RecordMethod::Uprobe),
            targets: targets(),
            name: Some(format!("sched_{sched}")),
            sched: Some(sched),
            ..options(&dir)
        });
        recs.push((id, d));
    }
    until(|| fake.has(|o| matches!(o, ArmodOp::Stream { on: true, .. })));
    let mut events = batch(TRACED, 2);
    let switch = armo_ev_sched_switch {
        prev_state: 1,
        next_tid: 0,
        cpu: 3,
    };
    for i in 0..3 {
        records::encode_positioned(
            &mut events,
            TRACED,
            TRACED,
            ARMO_EV_SCHED_SWITCH,
            i,
            &switch,
            &[],
        );
    }
    events.extend(batch(TRACED, 1));
    fake.send(Kind::Events, &events);
    until(|| server.seen.0.lock().unwrap().records.len() >= 6);
    for (id, _) in &recs {
        c.stop(id);
        c.ended(id);
    }
    let entity = format!("p:{TRACED}:0");
    assert_eq!(meta(&recs[0].1).counts[&entity].events, 3);
    assert_eq!(meta(&recs[1].1).counts[&entity].events, 6);
}

#[test]
fn armod_windows_end_the_windows() {
    let dir = scratch("armod_clock");
    let server = Server::start(&dir, config(&dir));
    let fake = FakeArmod::default();
    fake.period_ms.store(600, Ordering::Relaxed);
    let _armod = fake.listen(&dir.join("armod.sock"));
    let mut c = server.client("armo");
    c.until(|m| matches!(m, ServerMessage::State(s) if s.sources.armod == ArmodLink::Connected));
    c.request(ClientOp::Configure(Configure {
        window_s: Some(0.2),
        bandwidth: None,
        sched: None,
        malloc: None,
        pss: None,
        expected_hz: None,
    }))
    .unwrap();
    c.request(ClientOp::Subscribe {
        streams: vec![Stream::Window],
    })
    .unwrap();
    // The first window may start before armod's; the rest follow armod's 600 ms.
    for i in 0..4 {
        let ServerMessage::Window(w) = c.until(|m| matches!(m, ServerMessage::Window(_))) else {
            unreachable!()
        };
        if i > 0 {
            assert!(
                (500_000_000..700_000_000).contains(&w.window_ns),
                "window {i}: {} ns",
                w.window_ns
            );
        }
    }
}

#[test]
fn an_lttng_file_lends_its_current_callbacks_and_is_no_source() {
    let dir = scratch("lttng_words");
    std::fs::create_dir_all(dir.join("shm")).unwrap();
    let geometry = Geometry {
        n_threads: 4,
        table_entries: 0,
        ring_bytes: 0,
    };
    let pid = std::process::id();
    let shm = FakeShm::create(
        &dir.join("shm"),
        pid,
        own_starttime(),
        ARMO_BACKEND_LTTNG,
        geometry,
    )
    .unwrap();
    let tid = 4_242;
    let slot = shm.claim(tid).unwrap();
    shm.set_current(slot, 0x55, 1, 0);

    let server = Server::start(&dir, config(&dir));
    let fake = FakeArmod::start(&dir.join("armod.sock"));
    let mut c = server.client("armo");
    c.request(ClientOp::Subscribe {
        streams: vec![Stream::Window, Stream::EventLine],
    })
    .unwrap();
    // Rings and tables in a backend 2 file would count events next to the LTTng source. Made
    // after subscribing: the next scan reports it.
    let with_rings = FakeShm::create(
        &dir.join("shm"),
        OTHER,
        7,
        ARMO_BACKEND_LTTNG,
        Geometry::default(),
    )
    .unwrap();
    let rejected =
        c.until(|m| matches!(m, ServerMessage::EventLine(l) if l.kind == "shm_rejected"));
    let ServerMessage::EventLine(rejected) = rejected else {
        unreachable!()
    };
    assert!(
        rejected
            .text
            .contains(&with_rings.path().display().to_string())
    );
    let word = || {
        fake.ops().iter().rev().find_map(|o| match o {
            ArmodOp::Attach(a) if a.sets == [ProbeSet::Sched] => {
                a.user_words.get(&tid.to_string()).copied()
            }
            _ => None,
        })
    };
    until(|| word().is_some());
    // The address is the slot's cur_cb in this process, which maps the file.
    let mut cur = [0u8; 8];
    std::fs::File::open("/proc/self/mem")
        .unwrap()
        .read_exact_at(&mut cur, word().unwrap())
        .unwrap();
    assert_eq!(u64::from_le_bytes(cur), 0x55);
    assert_eq!(c.state.as_ref().unwrap().sources.preload_processes, 0);
    c.until(|m| matches!(m, ServerMessage::Window(_)));
    let seen = server.seen.0.lock().unwrap();
    assert!(!seen.records.iter().any(|r| r.0 == Method::Preload));
    assert!(!seen.aggregates.iter().any(|a| a.0 == Method::Preload));
}

#[test]
fn plan_changes_become_attach_detach_and_stream() {
    let pids = |p: &[u32]| Sel::pids(p.iter().copied());
    let old = Plan {
        window_s: Some(1.0),
        discovery: true,
        hot: pids(&[1, 2]),
        sched: Sel::All,
        ..Plan::default()
    };
    let new = Plan {
        window_s: Some(0.5),
        discovery: true,
        hot: pids(&[2, 3]),
        sched: pids(&[9]),
        stream: Sel::All,
        ..Plan::default()
    };
    let ops = requests(&old, &new);
    let json: Vec<String> = ops
        .iter()
        .map(|op| serde_json::to_string(op).unwrap())
        .collect();
    assert_eq!(
        json,
        [
            r#"{"op":"window","window_s":0.5}"#,
            r#"{"op":"detach","pids":[1],"sets":["hot"]}"#,
            r#"{"op":"attach","all":false,"pids":[2,3],"sets":["hot"],"user_words":{}}"#,
            r#"{"op":"detach","pids":[1],"sets":["rclcpp_symbols"]}"#,
            r#"{"op":"attach","all":false,"pids":[2,3],"sets":["rclcpp_symbols"],"user_words":{}}"#,
            r#"{"op":"detach","pids":[],"sets":["sched"]}"#,
            r#"{"op":"attach","all":false,"pids":[9],"sets":["sched"],"user_words":{}}"#,
            r#"{"op":"stream","on":true,"pids":[]}"#,
        ]
    );
    assert!(requests(&new, &new).is_empty());
    let off = requests(&new, &Plan::default());
    assert!(off.contains(&ArmodOp::Stream {
        on: false,
        pids: Vec::new()
    }));
    assert!(off.contains(&ArmodOp::Detach {
        pids: Vec::new(),
        sets: vec![ProbeSet::Discovery]
    }));
}

fn live(pid: u32, fqn: &str) -> (Process, Node) {
    let id = format!("p:{pid}:0");
    let process = Process {
        id: id.clone(),
        pid,
        ns_pid: None,
        comm: "node".into(),
        ros: None,
        methods_available: vec![Method::Uprobe],
        method: None,
        cpu_pct: None,
        rss_bytes: None,
        pss_bytes: None,
        shared_cpu_pct: None,
        threads: Vec::new(),
        nodes: Vec::new(),
        exited: false,
        cold: false,
        overflow: None,
        flux2_gpu_wedged: None,
    };
    let node = Node {
        id: format!("n:{pid}:0:a0"),
        fqn: fqn.into(),
        pid,
        cpu_pct: None,
        cpu_source: CpuSource::WallBound,
        busy_pct: None,
        cb_hz: None,
        callbacks: Vec::new(),
        malloc: None,
    };
    (process, node)
}

#[test]
fn a_node_name_selects_the_processes_of_its_nodes_as_they_appear() {
    let dir = scratch("names");
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    c.request(ClientOp::Subscribe {
        streams: vec![Stream::Window, Stream::State],
    })
    .unwrap();
    let fake = FakeArmod::start(&dir.join("armod.sock"));
    c.until(|m| matches!(m, ServerMessage::State(s) if s.sources.armod == ArmodLink::Connected));
    assert!(
        c.request(ClientOp::Select(Select {
            selection: Selection {
                node: Some("(".into()),
                ..Default::default()
            },
            method_by_pid: Default::default(),
        }))
        .is_err()
    );
    c.request(ClientOp::Select(Select {
        selection: Selection {
            node: Some("/cam/.*".into()),
            ..Default::default()
        },
        method_by_pid: Default::default(),
    }))
    .unwrap();
    {
        let mut seen = server.seen.0.lock().unwrap();
        let (p1, n1) = live(TRACED, "/cam/driver");
        let (p2, n2) = live(OTHER, "/talker");
        seen.processes = vec![p1, p2];
        seen.nodes = vec![n1, n2];
    }
    fake.send(
        Kind::Json,
        &serde_json::to_vec(&empty_armod_window(1)).unwrap(),
    );
    c.until(|m| {
        matches!(m, ServerMessage::State(s) if s.monitor.selection.pids == [TRACED]
            && s.monitor.selection.node.as_deref() == Some("/cam/.*"))
    });
    until(|| fake.has(|o| attached(o, ProbeSet::Hot, false, &[TRACED])));
    assert!(!fake.has(|o| attached(o, ProbeSet::Hot, false, &[OTHER])));

    c.request(ClientOp::Select(Select {
        selection: Selection {
            exclude: Some("/talker".into()),
            ..Default::default()
        },
        method_by_pid: Default::default(),
    }))
    .unwrap();
    let State { monitor, .. } = c.state.clone().unwrap();
    assert!(monitor.selection.all && monitor.selection.pids.is_empty());
    assert_eq!(monitor.selection.exclude.as_deref(), Some("/talker"));
}

#[test]
fn armod_reads_the_executor_only_of_hot_processes_with_unnamed_callbacks() {
    let dir = scratch("unowned");
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    server.seen.0.lock().unwrap().unowned = [TRACED, OTHER].into();
    let fake = FakeArmod::start(&dir.join("armod.sock"));
    c.until(|m| matches!(m, ServerMessage::State(s) if s.sources.armod == ArmodLink::Connected));
    c.start(RecordOptions {
        method: Some(RecordMethod::Uprobe),
        targets: Some(Targets {
            all: false,
            pids: vec![TRACED],
            node: None,
            topic: None,
            exclude: None,
        }),
        ..options(&dir)
    });
    until(|| fake.has(|o| attached(o, ProbeSet::Names, false, &[TRACED])));
    server.seen.0.lock().unwrap().unowned.clear();
    until(|| {
        fake.has(|o| {
            matches!(o, ArmodOp::Detach { pids, sets } if *pids == [TRACED]
            && *sets == [ProbeSet::Names])
        })
    });
    assert!(!fake.has(|o| attached(o, ProbeSet::Names, false, &[OTHER])));
}
