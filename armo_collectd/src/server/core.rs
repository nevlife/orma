//! The one state of armo-collectd: clients, the monitoring selection, the
//! recordings and the sources. Client threads, the armod thread and the tick share it under one
//! lock.

use std::collections::{BTreeMap, VecDeque};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use std::time::Duration;

use armo_common::catalog;
use armo_common::events::{ARMO_EV_SCHED_SWITCH, ARMO_EV_SCHED_WAKEUP};
use armo_common::shm::ARMO_OPTION_MALLOC;
use armo_proto::Ingest;
use armo_proto::frame;
use armo_proto::msg::*;
use armo_proto::records::{self, RecordError};

use super::{Config, Exit, Model};
use crate::guard::{self, Cleanup, Guard};
use crate::model::procfs;
use crate::names::{Known, Names};
use crate::record::replay::Replay;
use crate::record::{self, Active, Context, show};
use crate::sources::armod::{self, Plan, Sel};
use crate::sources::shm::{Change, Shm, ShmFile};
use crate::sys;

mod lttng;

const SEC: u64 = 1_000_000_000;
const SCAN_EVERY_NS: u64 = SEC;
const RECENT: usize = 16;
const DRAIN_AGAIN: Duration = Duration::from_millis(20);
/// The longest an ending recording waits for armod's records.
const FLUSH_WAIT_NS: u64 = 2_000_000_000;

/// armod's records of ending recordings: asked for, then in or given up on.
#[derive(Clone, Copy, PartialEq)]
enum Flush {
    Idle,
    Waiting { id: u64, until_ns: u64 },
    Done,
}

/// A client's `selfcheck` whose reply waits for armod's answer to request `armod_id`.
struct Selfcheck {
    armod_id: u64,
    client: u64,
    id: u64,
}

pub(super) struct Client {
    tx: SyncSender<Arc<Vec<u8>>>,
    stream: UnixStream,
    hello: Option<Hello>,
    window: bool,
    event_line: bool,
    replays: BTreeMap<String, Replay>,
}

pub struct Core {
    config: Config,
    ingest: Box<dyn Model>,
    guard: Option<Guard>,
    clients: BTreeMap<u64, Client>,
    next_client: u64,
    seq: u64,

    window_s: f64,
    selection: Selection,
    /// The selection's name targets, and the pids it named itself.
    names: Option<(Names, Vec<u32>)>,
    method_by_pid: BTreeMap<String, Method>,
    bandwidth: bool,
    sched: bool,
    malloc: bool,

    active: Vec<Active>,
    finished: VecDeque<Recording>,
    next_record: u64,
    next_replay: u64,

    shm: Shm,
    armod: ArmodLink,
    armod_stream: Option<UnixStream>,
    armod_sent: Plan,
    armod_window: Option<ArmodWindow>,
    armod_flush: Flush,
    armod_selfchecks: Vec<Selfcheck>,
    next_armod_id: u64,
    lttng: LttngState,
    sessions: Vec<String>,
    next_session: u64,
    lt: lttng::Src,

    window_seq: u64,
    window_start: u64,
    last_window: Option<Window>,
    last_hists: BTreeMap<String, BTreeMap<String, Hist>>,
    cpu_prev: (u64, u64),
    scanned_at: u64,
    idle_since: Option<u64>,
    uid: u32,
    host: Host,
}

fn reply(id: u64, body: ReplyBody) -> ServerMessage {
    ServerMessage::Reply(Reply { id, ok: true, body })
}

fn ok(id: u64) -> ServerMessage {
    reply(id, ReplyBody::Empty {})
}

fn bad(id: u64, message: impl Into<String>) -> ServerMessage {
    ServerMessage::Error(ErrorMessage::new(Some(id), code::BAD_REQUEST, message))
}

fn selection_sel(selection: &Selection) -> Sel {
    if selection.all {
        Sel::All
    } else {
        Sel::pids(selection.pids.iter().copied())
    }
}

/// Hands a batch of positioned records to the model and, in runs of one pid, to every recording
/// that takes that pid by `method`. Scheduler records go only to recordings that asked for them:
/// armod switches its scheduler probes on for a pid when any client wants them.
fn deliver(
    ingest: &mut dyn Ingest,
    active: &mut [Active],
    method: Method,
    entity: &mut dyn FnMut(u32) -> String,
    batch: &[u8],
    now_ns: u64,
) -> Result<(), RecordError> {
    let mut flush = |(pid, sched): (u32, bool), run: &[u8], n: u64| {
        let id = entity(pid);
        for r in active.iter_mut() {
            if (!sched || r.options().sched == Some(true)) && r.takes(pid, method) {
                r.append_events(&id, run, n, now_ns);
            }
        }
    };
    let (mut at, mut start, mut n, mut run) = (0, 0, 0, None);
    for record in records::positioned(batch) {
        let record = record?;
        ingest.record(method, &record);
        let sched = matches!(
            record.header.event,
            ARMO_EV_SCHED_SWITCH | ARMO_EV_SCHED_WAKEUP
        );
        if run != Some((record.pid, sched)) {
            if let Some(r) = run {
                flush(r, &batch[start..at], n);
            }
            (start, n, run) = (at, 0, Some((record.pid, sched)));
        }
        n += 1;
        at += size_of::<armo_common::events::armo_position>() + record.header.len as usize;
    }
    if let Some(r) = run {
        flush(r, &batch[start..at], n);
    }
    Ok(())
}

impl Core {
    pub(super) fn new(config: Config, ingest: Box<dyn Model>, guard: Option<Guard>) -> Core {
        let now = sys::mono_ns();
        let uid = sys::uid();
        let shm = Shm::new(config.shm_dir.clone(), uid);
        let lttng = if config.lttng {
            LttngState::Available
        } else {
            LttngState::Absent
        };
        Core {
            config,
            ingest,
            guard,
            clients: BTreeMap::new(),
            next_client: 1,
            seq: 0,
            window_s: 1.0,
            selection: Selection {
                all: true,
                pids: Vec::new(),
                ..Default::default()
            },
            names: None,
            method_by_pid: BTreeMap::new(),
            bandwidth: false,
            sched: true,
            malloc: false,
            active: Vec::new(),
            finished: VecDeque::new(),
            next_record: 1,
            next_replay: 1,
            shm,
            armod: ArmodLink::Absent,
            armod_stream: None,
            armod_sent: Plan::default(),
            armod_window: None,
            armod_flush: Flush::Idle,
            armod_selfchecks: Vec::new(),
            next_armod_id: 1,
            lttng,
            sessions: Vec::new(),
            next_session: 1,
            lt: lttng::Src::default(),
            window_seq: 0,
            window_start: now,
            last_window: None,
            last_hists: BTreeMap::new(),
            cpu_prev: (sys::cpu_ns(), now),
            scanned_at: now,
            idle_since: None,
            uid,
            host: Host {
                hostname: sys::hostname(),
                kernel: sys::proc_text("/proc/sys/kernel/osrelease"),
                arch: std::env::consts::ARCH.into(),
                cpus: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
            },
        }
    }

    /// Marks recordings a dead armo-collectd left as truncated, destroys its LTTng sessions,
    /// and opens the shared memory files, which lowers their `armed`.
    pub(super) fn startup(&mut self) {
        let list = std::fs::read_to_string(self.config.active_list()).unwrap_or_default();
        for dir in list.lines().filter(|l| !l.is_empty()) {
            match show::recover(Path::new(dir)) {
                Ok(meta) => {
                    let limits = Limits {
                        max_duration_s: meta.options.max_duration_s.unwrap_or(0),
                        max_bytes: meta.options.max_bytes.unwrap_or(0),
                        disk_floor_bytes: 0,
                    };
                    let summary = record::summary(&meta, Path::new(dir), limits, meta.bytes, 0);
                    self.finished.push_front(summary);
                }
                Err(e) => eprintln!("armo-collectd: {dir}: {e}"),
            }
        }
        self.write_active_list();
        if self.lttng == LttngState::Available {
            for name in guard::sweep_sessions(&self.config.lttng_home()) {
                eprintln!("armo-collectd: destroyed LTTng session {name} of a dead armo-collectd");
            }
        }
        self.lttng_start();
        self.scan(sys::mono_ns());
        self.refresh();
    }

    pub(super) fn client_add(&mut self, tx: SyncSender<Arc<Vec<u8>>>, stream: UnixStream) -> u64 {
        let id = self.next_client;
        self.next_client += 1;
        self.clients.insert(
            id,
            Client {
                tx,
                stream,
                hello: None,
                window: false,
                event_line: false,
                replays: BTreeMap::new(),
            },
        );
        id
    }

    pub(super) fn client_remove(&mut self, id: u64) {
        if let Some(c) = self.clients.remove(&id) {
            let _ = c.stream.shutdown(Shutdown::Read);
            if c.hello.is_some() {
                self.refresh();
            }
        }
    }

    pub(super) fn said_hello(&self, id: u64) -> bool {
        self.clients.get(&id).is_some_and(|c| c.hello.is_some())
    }

    pub(super) fn sender(&self, id: u64) -> Option<SyncSender<Arc<Vec<u8>>>> {
        self.clients.get(&id).map(|c| c.tx.clone())
    }

    fn push(&mut self, id: u64, bytes: Arc<Vec<u8>>) {
        let Some(c) = self.clients.get(&id) else {
            return;
        };
        // A client that does not keep up is dropped rather than stalling the recorder.
        if c.tx.try_send(bytes).is_err() {
            let _ = c.stream.shutdown(Shutdown::Both);
            self.client_remove(id);
        }
    }

    pub(super) fn send(&mut self, id: u64, message: &ServerMessage) {
        self.push(id, Arc::new(frame::encode_json(message)));
    }

    fn broadcast(&mut self, message: &ServerMessage, to: fn(&Client) -> bool) {
        let bytes = Arc::new(frame::encode_json(message));
        let ids: Vec<u64> = self
            .clients
            .iter()
            .filter(|(_, c)| c.hello.is_some() && to(c))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.push(id, Arc::clone(&bytes));
        }
    }

    fn state(&mut self) -> State {
        self.seq += 1;
        let now = sys::mono_ns();
        let mut recordings: Vec<Recording> = self.active.iter().map(|r| r.summary(now)).collect();
        recordings.extend(self.finished.iter().cloned());
        State {
            seq: self.seq,
            sources: Sources {
                armod: self.armod,
                lttng: self.lttng,
                preload_processes: self.shm.files.values().filter(|f| f.preload()).count() as u32,
            },
            monitor: Monitor {
                running: self.monitoring() || !self.active.is_empty(),
                window_s: self.window_s,
                selection: self.selection.clone(),
                method_by_pid: self.method_by_pid.clone(),
                bandwidth: self.bandwidth,
                sched: self.sched,
                malloc: self.malloc,
            },
            recordings,
        }
    }

    fn broadcast_state(&mut self) {
        let state = ServerMessage::State(self.state());
        self.broadcast(&state, |_| true);
    }

    fn event_line(&mut self, level: Level, kind: &str, text: String, ids: Vec<String>) {
        let line = ServerMessage::EventLine(EventLine {
            t_real_ns: sys::real_ns(),
            level,
            kind: kind.into(),
            text,
            ids,
        });
        self.broadcast(&line, |c| c.event_line);
    }

    /// Someone watches windows: a client that only asks for state or recordings costs the
    /// observed processes nothing.
    fn monitoring(&self) -> bool {
        self.clients.values().any(|c| c.hello.is_some() && c.window)
    }

    fn selected(&self, pid: u32) -> bool {
        self.selection.all || self.selection.pids.contains(&pid)
    }

    /// Brings the sources in line with clients, selection and recordings: `armed` per shared
    /// memory file, armod's probes and streams, and the guard's list.
    fn refresh(&mut self) {
        let monitor = self.monitoring();
        let Core {
            shm,
            active,
            method_by_pid,
            ..
        } = self;
        let malloc = self.malloc;
        let selection = &self.selection;
        let want = |pid: u32| -> Option<u32> {
            let mon = monitor
                && (selection.all || selection.pids.contains(&pid))
                && method_by_pid
                    .get(&pid.to_string())
                    .is_none_or(|m| *m == Method::Preload);
            let recs: Vec<&Active> = active.iter().filter(|r| r.wants_preload(pid)).collect();
            if !mon && recs.is_empty() {
                return None;
            }
            let malloc = (mon && malloc) || recs.iter().any(|r| r.options().malloc == Some(true));
            Some(if malloc { ARMO_OPTION_MALLOC } else { 0 })
        };
        shm.arm(&want);

        let mut plan = Plan::default();
        if monitor || !self.active.is_empty() {
            plan.window_s = Some(self.window_s);
        }
        // Init events only, from every process: kept while a client is connected, so that a
        // recording it starts does not wait for them to come off and on again (1.1 s per probe).
        plan.discovery =
            !self.active.is_empty() || self.clients.values().any(|c| c.hello.is_some());
        if monitor {
            let uprobe: Vec<u32> = self
                .method_by_pid
                .iter()
                .filter(|(_, m)| **m == Method::Uprobe)
                .filter_map(|(p, _)| p.parse().ok())
                .filter(|p| self.selected(*p))
                .collect();
            plan.hot.add(Sel::pids(uprobe.iter().copied()));
            // Rates, hops and losses come from individual publish and take records.
            plan.stream.add(Sel::pids(uprobe));
            if self.sched {
                plan.sched.add(selection_sel(&self.selection));
            }
            if self.bandwidth {
                plan.bandwidth.add(selection_sel(&self.selection));
            }
        }
        // An ending recording keeps its probes and stream until armod has flushed to it.
        for r in &self.active {
            let uprobe = r.uprobe_pids().map_or(Sel::All, Sel::pids);
            plan.hot.add(uprobe.clone());
            if r.kind() == RecordKind::Events {
                plan.stream.add(uprobe);
            }
            let t = r.targets();
            let targets = if record::every_process(t) {
                Sel::All
            } else {
                Sel::pids(t.pids.iter().copied())
            };
            if r.options().sched == Some(true) {
                plan.sched.add(targets.clone());
            }
            if r.options().bandwidth == Some(true) {
                plan.bandwidth.add(targets);
            }
        }
        let mut known = plan.hot.clone();
        if plan.sched != Sel::None {
            let lttng: Vec<u32> = self
                .shm
                .files
                .values()
                .filter(|f| !f.preload() && self.lttng_takes(f.pid))
                .map(|f| f.pid)
                .collect();
            for f in self.shm.files.values_mut() {
                let taken = if f.preload() {
                    f.armed
                } else {
                    lttng.contains(&f.pid)
                };
                if taken && plan.sched.contains(f.pid) {
                    plan.user_words.extend(f.cur_cb_words());
                    known.add(Sel::pids([f.pid]));
                }
            }
        }
        // Off-CPU is known where armod switches threads and knows their current callback: its
        // uprobe stack, or a shared memory file's words.
        let sched = match (self.armod == ArmodLink::Connected, &plan.sched, known) {
            (false, _, _) => Sel::None,
            (true, Sel::All, k) => k,
            (true, s, Sel::All) => s.clone(),
            (true, Sel::Pids(a), Sel::Pids(b)) => Sel::pids(a.intersection(&b).copied()),
            _ => Sel::None,
        };
        self.ingest.set_sched(sched);
        let hot = self.hot(&plan.hot);
        self.ingest.set_hot(hot);
        self.send_plan(plan);
        self.lttng_refresh();
        self.update_guard();
    }

    /// The processes whose hot events reach the model: armod's `uprobe` ones while it is there,
    /// and the shared memory files that are armed or that the LTTng source takes.
    fn hot(&self, uprobe: &Sel) -> Sel {
        let mut hot = match self.armod {
            ArmodLink::Connected => uprobe.clone(),
            _ => Sel::None,
        };
        hot.add(Sel::pids(
            self.shm
                .files
                .values()
                .filter(|f| match f.preload() {
                    true => f.armed,
                    false => self.lttng_takes(f.pid),
                })
                .map(|f| f.pid),
        ));
        hot
    }

    fn send_plan(&mut self, plan: Plan) {
        if let Some(stream) = self.armod_stream.as_mut() {
            for op in armod::requests(&self.armod_sent, &plan) {
                if armod::send(stream, 0, op).is_err() {
                    let _ = stream.shutdown(Shutdown::Both);
                    break;
                }
            }
        }
        self.armod_sent = plan;
    }

    fn update_guard(&mut self) {
        let list = Cleanup {
            lttng_home: self.config.lttng_home(),
            sessions: self.sessions.clone(),
            armed: self.shm.armed_paths(),
            recordings: self.active.iter().map(|r| r.dir.clone()).collect(),
        };
        if let Some(g) = self.guard.as_mut()
            && let Err(e) = g.update(&list)
        {
            eprintln!("armo-collectd: the guard is gone: {e}");
            self.guard = None;
        }
    }

    /// A new LTTng session name armo-collectd owns: the guard destroys it if armo-collectd dies,
    /// and `shutdown` destroys it. For the LTTng source (WP5).
    pub fn own_lttng_session(&mut self) -> String {
        let name = guard::session_name(self.next_session);
        self.next_session += 1;
        self.sessions.push(name.clone());
        self.update_guard();
        name
    }

    pub fn release_lttng_session(&mut self, name: &str) {
        self.sessions.retain(|s| s != name);
        self.update_guard();
    }

    fn write_active_list(&self) {
        let mut text = String::new();
        for r in &self.active {
            text.push_str(&r.dir.display().to_string());
            text.push('\n');
        }
        let path = self.config.active_list();
        let tmp = path.with_extension("tmp");
        let written = std::fs::create_dir_all(&self.config.data_dir)
            .and_then(|()| std::fs::write(&tmp, text))
            .and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(e) = written {
            eprintln!("armo-collectd: {}: {e}", path.display());
        }
    }

    fn entity_of(pid: u32) -> String {
        record::entity(pid, sys::starttime(pid).unwrap_or(0))
    }

    fn drain(&mut self, now: u64) {
        let Core {
            shm,
            ingest,
            active,
            ..
        } = self;
        let changes = shm.drain(&mut |f: &ShmFile, batch: &[u8], _| {
            let entity = f.entity();
            let _ = deliver(
                ingest.as_mut(),
                active,
                Method::Preload,
                &mut |_| entity.clone(),
                batch,
                now,
            );
        });
        self.changes(changes);
    }

    fn scan(&mut self, now: u64) {
        let Core {
            shm,
            ingest,
            active,
            ..
        } = self;
        let changes = shm.scan(&mut |f: &ShmFile, batch: &[u8], _| {
            let entity = f.entity();
            let _ = deliver(
                ingest.as_mut(),
                active,
                Method::Preload,
                &mut |_| entity.clone(),
                batch,
                now,
            );
        });
        self.changes(changes);
    }

    fn changes(&mut self, changes: Vec<Change>) {
        if changes.is_empty() {
            return;
        }
        for change in changes {
            match change {
                Change::Opened { pid, starttime } => {
                    let id = record::entity(pid, starttime);
                    let text = format!("process {pid} writes armo shared memory");
                    self.event_line(Level::Info, "process_start", text, vec![id.clone()]);
                    let missing = self
                        .shm
                        .files
                        .values()
                        .find(|f| f.pid == pid)
                        .map(|f| f.missing.clone())
                        .unwrap_or_default();
                    for event in missing {
                        let name = catalog::info(event).map_or("unknown", |i| i.name);
                        let text = format!("{name}: the preload shim found no original function");
                        self.event_line(Level::Warn, "probe_missing", text, vec![id.clone()]);
                    }
                }
                Change::Closed { pid, starttime } => {
                    let text = format!("process {pid} ended");
                    let ids = vec![record::entity(pid, starttime)];
                    self.event_line(Level::Info, "process_exit", text, ids);
                }
                Change::Corrupt {
                    pid,
                    starttime,
                    reason,
                } => {
                    let text = format!("shared memory of process {pid} is damaged: {reason}");
                    let ids = vec![record::entity(pid, starttime)];
                    self.event_line(Level::Error, "shm_corrupt", text, ids);
                }
                Change::Rejected { path, reason } => {
                    let text = format!("{}: {reason}", path.display());
                    self.event_line(Level::Warn, "shm_rejected", text, Vec::new());
                }
            }
        }
        self.refresh();
        self.broadcast_state();
    }

    /// A request whose answer `armod_message` matches by the id returned; `None` without armod.
    fn armod_request(&mut self, op: ArmodOp) -> Option<u64> {
        let stream = self.armod_stream.as_mut()?;
        let id = self.next_armod_id;
        self.next_armod_id += 1;
        armod::send(stream, id, op).ok().map(|()| id)
    }

    /// Whether armod has sent what it held for the ending recordings; asks it the first time.
    fn armod_flushed(&mut self) -> bool {
        match self.armod_flush {
            Flush::Done => true,
            Flush::Waiting { until_ns, .. } => sys::mono_ns() >= until_ns,
            Flush::Idle => {
                let uprobe = self
                    .active
                    .iter()
                    .any(|r| r.end.is_some() && r.methods().contains(&Method::Uprobe));
                let Some(id) = uprobe
                    .then(|| self.armod_request(ArmodOp::Flush { drain_only: true }))
                    .flatten()
                else {
                    return true;
                };
                self.armod_flush = Flush::Waiting {
                    id,
                    until_ns: sys::mono_ns() + FLUSH_WAIT_NS,
                };
                false
            }
        }
    }

    /// Finalizes the recordings whose `end` is set.
    fn finish_ended(&mut self) {
        if self.active.iter().all(|r| r.end.is_none()) || !self.armod_flushed() {
            return;
        }
        self.armod_flush = Flush::Idle;
        self.refresh();
        self.lttng_flush();
        let now = sys::mono_ns();
        self.drain(now);
        std::thread::sleep(DRAIN_AGAIN);
        self.drain(now);
        let (ended, kept): (Vec<Active>, Vec<Active>) = std::mem::take(&mut self.active)
            .into_iter()
            .partition(|r| r.end.is_some());
        self.active = kept;
        for r in ended {
            let (reason, detail) = r.end.clone().expect("partitioned");
            let summary = r.finalize(sys::mono_ns(), sys::real_ns());
            let ids = vec![summary.id.clone()];
            let detail = detail.map(|d| format!(": {d}")).unwrap_or_default();
            let text = format!("recording {} ended, {reason:?}{detail}", summary.id);
            if matches!(
                reason,
                EndReason::MaxDuration | EndReason::MaxSize | EndReason::DiskFloor
            ) {
                self.event_line(Level::Warn, "record_limit", text.clone(), ids.clone());
            }
            let level = match reason {
                EndReason::Error => Level::Error,
                _ => Level::Info,
            };
            self.event_line(level, "record_end", text, ids);
            self.finished.push_front(summary);
            self.finished.truncate(RECENT);
        }
        self.write_active_list();
        self.refresh();
        self.broadcast_state();
    }

    fn window(&mut self, now: u64) {
        let t0 = sys::mono_ns();
        let window_ns = now - self.window_start;
        self.window_start = now;
        let real = sys::real_ns();
        let mut rows = Vec::new();
        let mut lost = Vec::new();
        for f in self.shm.files.values_mut() {
            let mut n = 0;
            f.tables(&mut rows, &mut |_, d| n += d);
            if n > 0 {
                lost.push((f.pid, f.entity(), n));
            }
        }
        if !rows.is_empty() {
            self.ingest.aggregates(Method::Preload, &rows);
        }
        let mut drops = 0;
        for (pid, entity, n) in lost {
            drops += n;
            for r in &mut self.active {
                if r.takes(pid, Method::Preload) {
                    r.add_lost(&entity, n, Method::Preload);
                }
            }
            let text = format!("process {pid} dropped {n} records: ring full");
            self.event_line(Level::Warn, "loss", text, vec![entity]);
        }
        let missing: Vec<ProbeMissing> = self
            .shm
            .files
            .values()
            .flat_map(|f| {
                f.missing.iter().map(|e| ProbeMissing {
                    pid: Some(f.pid),
                    method: Method::Preload,
                    point: catalog::info(*e).map_or("unknown", |i| i.name).into(),
                    reason: "the preload shim found no original function".into(),
                })
            })
            .collect();
        for r in &mut self.active {
            let mine: Vec<ProbeMissing> = missing
                .iter()
                .filter(|m| m.pid.is_some_and(|p| r.wants_preload(p)))
                .cloned()
                .collect();
            r.add_probes_missing(&mine);
        }

        self.ingest
            .set_lttng_span(std::mem::take(&mut self.lt.span_ns));
        let mut w = self.ingest.window(now, real, window_ns, true);
        for f in self.shm.files.values().filter(|f| f.preload()) {
            let id = f.entity();
            if let Some(p) = w.processes.iter_mut().find(|p| p.id == id) {
                p.overflow = Some(f.overflow());
            }
        }
        self.window_seq += 1;
        w.seq = self.window_seq;
        w.replay = false;
        let cpu_ns = sys::cpu_ns();
        let (prev_cpu, prev_at) = self.cpu_prev;
        self.cpu_prev = (cpu_ns, now);
        w.agent.collectd.cpu_pct =
            Some((cpu_ns - prev_cpu) as f64 * 100.0 / (now - prev_at).max(1) as f64);
        w.agent.shm = ShmUse {
            files: self.shm.files.len() as u32,
            bytes: self.shm.bytes(),
            ring_drops: drops,
        };
        w.agent.probes_missing.extend(missing);
        self.lttng_window(&mut w);
        self.lttng_rotate();
        if let Some(a) = &self.armod_window {
            w.agent.armod = a.usage.clone();
            w.agent.bpf = a.bpf.clone();
            w.agent.maps = a.maps.clone();
            w.agent
                .probes_missing
                .extend(a.probes_missing.iter().cloned());
        }
        w.behind = sys::mono_ns() - t0 >= window_ns;

        let mut line = WindowLine {
            t_mono_ns: now,
            t_real_ns: real,
            snap: w,
        };
        if !self.active.is_empty() {
            let bytes = serde_json::to_vec(&line).expect("windows serialize");
            for r in &mut self.active {
                r.append_window(&bytes);
            }
        }
        self.last_hists = line.snap.hists.take().unwrap_or_default();
        let message = ServerMessage::Window(Box::new(line.snap));
        self.broadcast(&message, |c| c.window);
        let ServerMessage::Window(w) = message else {
            unreachable!()
        };
        self.last_window = Some(*w);
        if !self.active.is_empty() {
            self.broadcast_state();
        }
        self.follow_names();
    }

    fn known(&self) -> Known {
        self.last_window.as_ref().map(Known::of).unwrap_or_default()
    }

    /// The method monitoring takes `pid` by when the selection lists it without one: as
    /// `record_start`'s `auto` settles.
    fn monitor_method(&self, pid: u32) -> Option<Method> {
        if self.shm.has_pid(pid) {
            Some(Method::Preload)
        } else if self.lt.on() && self.lt.apps.contains(&pid) {
            Some(Method::Lttng)
        } else if self.armod == ArmodLink::Connected {
            Some(Method::Uprobe)
        } else {
            None
        }
    }

    fn settle_selection(&mut self) {
        for pid in self.selection.pids.clone() {
            if let Some(m) = self.monitor_method(pid) {
                self.method_by_pid.entry(pid.to_string()).or_insert(m);
            }
        }
    }

    /// Matches the name targets of the selection and of the recordings against the last window:
    /// a node or topic that appears takes its processes in.
    fn follow_names(&mut self) {
        let known = self.known();
        let mut changed = false;
        if let Some((names, listed)) = &self.names {
            let mut pids = names.pids(&known);
            pids.extend(listed);
            let pids: Vec<u32> = pids.into_iter().collect();
            if pids != self.selection.pids {
                self.selection.pids = pids;
                self.settle_selection();
                changed = true;
            }
        }
        let Core {
            active, shm, lt, ..
        } = self;
        let armod = self.armod == ArmodLink::Connected;
        let apps = &lt.apps;
        let registered = |pid: u32| apps.contains(&pid);
        let lttng = lt.on().then_some(&registered as &dyn Fn(u32) -> bool);
        for r in active.iter_mut() {
            if let Some(names) = r.names.clone() {
                changed |= r.add_pids(&names.pids(&known), armod, &|p| shm.has_pid(p), lttng);
            }
        }
        if changed {
            self.refresh();
            self.broadcast_state();
        }
    }

    /// Whether armod's windows end armo-collectd's: two clocks of one period would now and then
    /// put two of armod's windows into one of armo-collectd's.
    fn armod_clock(&self) -> bool {
        self.armod == ArmodLink::Connected && self.armod_sent.window_s.is_some()
    }

    /// One step of the main loop; `Some` when armo-collectd should end.
    /// `Exit::Signal` once a stop has ended every recording.
    pub(super) fn tick(&mut self, stop: bool) -> Option<Exit> {
        if stop {
            for r in self.active.iter_mut().filter(|r| r.end.is_none()) {
                r.end = Some((
                    EndReason::CollectorExit,
                    Some("armo-collectd ended by SIGTERM or SIGINT".into()),
                ));
            }
            self.finish_ended();
            if self.active.is_empty() {
                return Some(Exit::Signal);
            }
        }
        let now = sys::mono_ns();
        if now - self.scanned_at >= SCAN_EVERY_NS {
            self.scanned_at = now;
            self.scan(now);
            self.refresh();
        }
        self.drain(now);
        self.lttng_poll();
        let disk = self.config.disk;
        for r in &mut self.active {
            r.check(now, disk);
        }
        self.finish_ended();
        let busy = self.monitoring() || !self.active.is_empty();
        if !busy {
            self.window_start = now;
        } else if !self.armod_clock() && now - self.window_start >= (self.window_s * 1e9) as u64 {
            self.window(now);
        }
        if self.clients.is_empty() && self.active.is_empty() {
            let since = *self.idle_since.get_or_insert(now);
            if now - since >= self.config.idle.as_nanos() as u64 {
                return Some(Exit::Idle);
            }
        } else {
            self.idle_since = None;
        }
        None
    }

    /// Ends what armo-collectd owns, so the guard has nothing left to do.
    pub(super) fn shutdown(&mut self) {
        self.lttng_close();
        self.shm.arm(&|_| None);
        if let Some(s) = self.armod_stream.take() {
            let _ = s.shutdown(Shutdown::Both);
        }
        let home = self.config.lttng_home();
        for name in std::mem::take(&mut self.sessions) {
            if let Err(e) = guard::destroy_session(&home, &name) {
                eprintln!("armo-collectd: {e}");
            }
        }
        if let Some(g) = self.guard.take()
            && let Err(e) = g.close()
        {
            eprintln!("armo-collectd: closing the guard: {e}");
        }
        for (_, c) in std::mem::take(&mut self.clients) {
            let _ = c.stream.shutdown(Shutdown::Both);
        }
    }

    pub(super) fn replay_add(&mut self, client: u64, request: u64, replay: Replay) {
        let Some(c) = self.clients.get_mut(&client) else {
            return;
        };
        let replay_id = format!("rp-{}", self.next_replay);
        self.next_replay += 1;
        c.replays.insert(replay_id.clone(), replay);
        self.send(
            client,
            &reply(
                request,
                ReplyBody::ReplayOpened {
                    replay_id: replay_id.clone(),
                },
            ),
        );
        if let Some(r) = self
            .clients
            .get(&client)
            .and_then(|c| c.replays.get(&replay_id))
        {
            r.play(1.0).expect("rate 1 is valid");
        }
    }

    fn replay(
        &mut self,
        client: u64,
        id: u64,
        replay_id: &str,
        act: impl FnOnce(&Replay) -> Result<(), String>,
    ) -> ServerMessage {
        match self
            .clients
            .get(&client)
            .and_then(|c| c.replays.get(replay_id))
        {
            Some(r) => match act(r) {
                Ok(()) => ok(id),
                Err(e) => bad(id, e),
            },
            None => bad(id, format!("no replay {replay_id} on this connection")),
        }
    }

    fn select(&mut self, id: u64, select: Select) -> ServerMessage {
        if let Some(k) = select
            .method_by_pid
            .keys()
            .find(|k| k.parse::<u32>().is_err())
        {
            return bad(id, format!("method_by_pid key {k:?} is not a pid"));
        }
        if !self.lt.on() && select.method_by_pid.values().any(|m| *m == Method::Lttng) {
            return bad(
                id,
                "method lttng needs lttng, lttng-sessiond, babeltrace and setpriv on PATH",
            );
        }
        let s = &select.selection;
        let names = match Names::new(s.node.as_ref(), s.topic.as_ref(), s.exclude.as_ref()) {
            Err(e) => return bad(id, e),
            Ok(Some(_)) if s.all => return bad(id, "all with node, topic or exclude"),
            Ok(names) => names,
        };
        let mut selection = select.selection;
        // Every process but some is still every process: no uprobe on all of them (design
        // 6.1); the clients leave the excluded ones out of view.
        let names = names.filter(|n| !(n.only_excludes() && selection.pids.is_empty()));
        selection.all |= names.is_none() && selection.exclude.is_some();
        self.names = names.map(|n| {
            let listed = selection.pids.clone();
            let mut pids = n.pids(&self.known());
            pids.extend(&listed);
            selection.pids = pids.into_iter().collect();
            (n, listed)
        });
        self.selection = selection;
        // A listed pid without a method settles as record_start's auto does.
        self.method_by_pid = select.method_by_pid;
        self.settle_selection();
        self.refresh();
        self.broadcast_state();
        if self.selection.all {
            let text = "uprobe attaches only to processes selected by pid: the others show rates \
                        only when started with preload or lttng";
            self.event_line(Level::Info, "select_all", text.into(), Vec::new());
        }
        ok(id)
    }

    fn record_start(&mut self, client: u64, id: u64, mut options: RecordOptions) -> ServerMessage {
        let known = self.known();
        let ongoing = self.active.len();
        if ongoing >= record::MAX_ACTIVE {
            return bad(
                id,
                format!("{ongoing} recordings in progress, the most there can be"),
            );
        }
        let hello = self.clients[&client].hello.clone().expect("after hello");
        let real = sys::real_ns();
        let shm = &self.shm;
        let apps = &self.lt.apps;
        let registered = |pid: u32| apps.contains(&pid);
        let records_dir = self.config.records_dir();
        let cx = Context {
            id: format!("r-{}-{}", record::utc_stamp(real), self.next_record),
            records_dir: &records_dir,
            caps: &self.config.caps,
            disk: self.config.disk,
            owner: Owner {
                uid: self.uid,
                client: hello.client,
                pid: hello.pid,
                host: self.host.hostname.clone(),
            },
            host: self.host.clone(),
            armod: self.armod == ArmodLink::Connected,
            preload: &|pid| shm.has_pid(pid),
            lttng: self.lt.on().then_some(&registered as &dyn Fn(u32) -> bool),
            mono_ns: sys::mono_ns(),
            real_ns: real,
        };
        let names = match options
            .targets
            .as_ref()
            .map(|t| Names::new(t.node.as_ref(), t.topic.as_ref(), t.exclude.as_ref()))
        {
            Some(Err(e)) => return bad(id, e),
            Some(Ok(Some(_))) if options.targets.as_ref().is_some_and(|t| t.all) => {
                return bad(id, "all with node, topic or exclude");
            }
            Some(Ok(names)) => names,
            None => None,
        };
        if let (Some(n), Some(t)) = (&names, options.targets.as_mut()) {
            let mut pids = n.pids(&known);
            pids.extend(&t.pids);
            t.pids = pids.into_iter().collect();
        }
        let mut active = match record::start(options, cx) {
            Ok(a) => a,
            Err(e) => return bad(id, e),
        };
        active.names = names;
        self.next_record += 1;
        let (record_id, dir) = (active.id.clone(), active.dir.display().to_string());
        // Entities made before the recording are named by the init records the files kept.
        let now = sys::mono_ns();
        for f in self.shm.files.values().filter(|f| f.init_n > 0) {
            if active.wants_preload(f.pid) && active.takes(f.pid, Method::Preload) {
                active.append_events(&f.entity(), &f.init, f.init_n, now);
            }
        }
        self.active.push(active);
        self.write_active_list();
        self.refresh();
        let text = format!("recording {record_id} started in {dir}");
        self.event_line(Level::Info, "record_start", text, vec![record_id.clone()]);
        self.broadcast_state();
        reply(id, ReplyBody::RecordStarted { record_id, dir })
    }

    fn record_dir(&mut self, record_id: &str) -> Option<PathBuf> {
        if let Some(r) = self.active.iter_mut().find(|r| r.id == record_id) {
            let _ = r.write_meta();
            return Some(r.dir.clone());
        }
        self.finished
            .iter()
            .find(|r| r.id == record_id)
            .map(|r| PathBuf::from(&r.dir))
    }

    fn detail(&self, id: u64, entity: String, what: DetailWhat) -> ServerMessage {
        let Some(w) = &self.last_window else {
            return bad(id, "no window yet");
        };
        let mut detail = Detail {
            entity: entity.clone(),
            what,
            hists: None,
            flows: None,
            threads: None,
        };
        match what {
            DetailWhat::Hist => {
                detail.hists = Some(self.last_hists.get(&entity).cloned().unwrap_or_default());
            }
            DetailWhat::Flows => {
                let mine = |f: &&Flow| [&f.id, &f.node, &f.input, &f.output].contains(&&entity);
                detail.flows = Some(w.flows.iter().filter(mine).cloned().collect());
            }
            DetailWhat::Threads => {
                let prefix = entity.strip_prefix("p:").map(|p| format!("t:{p}:"));
                let mine = |t: &&Thread| {
                    t.id == entity
                        || prefix.as_ref().is_some_and(|p| t.id.starts_with(p))
                        || t.bucket.split_once(':').is_some_and(|(_, e)| e == entity)
                };
                detail.threads = Some(w.threads.iter().filter(mine).cloned().collect());
            }
        }
        reply(id, ReplyBody::Detail(detail))
    }

    /// Asks armod for its items over the selection; the reply waits for its answer.
    /// Without armod, collectd's own items at once.
    fn selfcheck(&mut self, client: u64, id: u64, duration_s: f64) -> Option<ServerMessage> {
        let pids = match self.selection.all {
            true => Vec::new(),
            false => self.selection.pids.clone(),
        };
        if let Some(armod_id) = self.armod_request(ArmodOp::Selfcheck { duration_s, pids }) {
            self.armod_selfchecks.push(Selfcheck {
                armod_id,
                client,
                id,
            });
            return None;
        }
        let link = format!("{:?}", self.armod).to_lowercase();
        Some(self.selfcheck_reply(id, CheckStatus::Unknown, link, Vec::new()))
    }

    /// The pending `selfcheck` armod's message `armod_id` answers, answered with its checks.
    fn selfcheck_answered(
        &mut self,
        armod_id: u64,
        status: CheckStatus,
        detail: String,
        from: Vec<Check>,
    ) -> bool {
        let Some(i) = self
            .armod_selfchecks
            .iter()
            .position(|s| s.armod_id == armod_id)
        else {
            return false;
        };
        let s = self.armod_selfchecks.remove(i);
        let answer = self.selfcheck_reply(s.id, status, detail, from);
        self.send(s.client, &answer);
        true
    }

    fn selfcheck_reply(
        &self,
        id: u64,
        armod: CheckStatus,
        detail: String,
        from_armod: Vec<Check>,
    ) -> ServerMessage {
        let mut checks = vec![
            Check {
                item: "armod".into(),
                status: armod,
                detail,
            },
            Check {
                item: "lttng".into(),
                status: match self.lttng {
                    LttngState::Available => CheckStatus::Pass,
                    LttngState::Absent => CheckStatus::Unknown,
                },
                detail: "lttng on PATH".into(),
            },
        ];
        for f in self.shm.files.values() {
            let names: Vec<&str> = f
                .missing
                .iter()
                .map(|e| catalog::info(*e).map_or("unknown", |i| i.name))
                .collect();
            checks.push(Check {
                item: format!("intercept {}", f.entity()),
                status: if names.is_empty() {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                detail: if names.is_empty() {
                    "every intercept point found its original function".into()
                } else {
                    format!("no original function: {}", names.join(", "))
                },
            });
        }
        for f in self.shm.files.values().filter(|f| f.preload()) {
            let (found, mapped, live) = f.host_ids();
            if found && f.ns_pid == f.pid && procfs::ns_pid(f.pid).is_none() {
                continue;
            }
            checks.push(Check {
                item: format!("container pid {} (preload)", f.pid),
                status: match (found, mapped == live, live) {
                    (false, _, _) | (_, false, _) => CheckStatus::Fail,
                    (_, _, 0) => CheckStatus::Unknown,
                    _ => CheckStatus::Pass,
                },
                detail: format!(
                    "file pid {} is host pid {} ({}); {mapped} of {live} live slot tids are host \
                     threads of it",
                    f.ns_pid,
                    f.pid,
                    if found {
                        "same starttime"
                    } else {
                        "no such process with its starttime"
                    }
                ),
            });
        }
        checks.extend(from_armod);
        reply(id, ReplyBody::Selfcheck { checks })
    }

    /// Answers one request from client `client`; false closes the connection.
    pub(super) fn handle(&mut self, client: u64, request: ClientRequest) -> bool {
        let id = request.id;
        let hello = self.said_hello(client);
        let answer = match request.op {
            ClientOp::Hello(h) => {
                if let Err(e) = check_proto(id, h.proto) {
                    self.send(client, &ServerMessage::Error(e));
                    return false;
                }
                if let Some(c) = self.clients.get_mut(&client) {
                    c.hello = Some(h);
                }
                let server = ServerHello {
                    proto: PROTO,
                    server: "armo-collectd".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                };
                self.send(client, &ServerMessage::Hello(server));
                let state = ServerMessage::State(self.state());
                self.send(client, &state);
                self.refresh();
                return true;
            }
            _ if !hello => bad(id, "hello first"),
            ClientOp::Subscribe { streams } => {
                if let Some(c) = self.clients.get_mut(&client) {
                    c.window = streams.contains(&Stream::Window);
                    c.event_line = streams.contains(&Stream::EventLine);
                }
                // A first window watcher arms the sources now rather than at the next scan.
                self.refresh();
                ok(id)
            }
            ClientOp::Select(select) => self.select(id, select),
            ClientOp::Configure(c) => {
                if let Some(w) = c
                    .window_s
                    .filter(|w| !(*w > 0.0 && Duration::try_from_secs_f64(*w).is_ok()))
                {
                    bad(
                        id,
                        format!("window_s {w} is not a positive number of seconds"),
                    )
                } else {
                    self.window_s = c.window_s.unwrap_or(self.window_s);
                    self.bandwidth = c.bandwidth.unwrap_or(self.bandwidth);
                    self.sched = c.sched.unwrap_or(self.sched);
                    self.malloc = c.malloc.unwrap_or(self.malloc);
                    if let Some(hz) = c.expected_hz {
                        self.ingest.set_expected_hz(hz);
                    }
                    self.refresh();
                    self.broadcast_state();
                    ok(id)
                }
            }
            ClientOp::RecordStart(options) => self.record_start(client, id, options),
            ClientOp::RecordStop { record_id } => {
                match self.active.iter_mut().find(|r| r.id == record_id) {
                    Some(r) => {
                        r.end = Some((EndReason::UserStop, None));
                        self.finish_ended();
                        ok(id)
                    }
                    None => bad(id, format!("no recording {record_id} in progress")),
                }
            }
            ClientOp::RecordList => {
                let now = sys::mono_ns();
                let mut recordings: Vec<Recording> =
                    self.active.iter().map(|r| r.summary(now)).collect();
                recordings.extend(self.finished.iter().cloned());
                reply(id, ReplyBody::Recordings { recordings })
            }
            ClientOp::RecordShow { record_id, dir } => {
                let dir = match (record_id, dir) {
                    (_, Some(dir)) => Some(PathBuf::from(dir)),
                    (Some(r), None) => self.record_dir(&r),
                    (None, None) => None,
                };
                match dir.map(|d| show::recover(&d).map_err(|e| format!("{}: {e}", d.display()))) {
                    Some(Ok(meta)) => reply(
                        id,
                        ReplyBody::RecordShow {
                            table: show::table(&meta, sys::mono_ns()),
                        },
                    ),
                    Some(Err(e)) => bad(id, e),
                    None => bad(id, "record_show needs a known record_id or a dir"),
                }
            }
            ClientOp::Detail { entity, what } => self.detail(id, entity, what),
            ClientOp::ReplaySeek { replay_id, frame } => {
                self.replay(client, id, &replay_id, |r| r.seek(frame))
            }
            ClientOp::ReplayPlay { replay_id, rate } => {
                self.replay(client, id, &replay_id, |r| r.play(rate))
            }
            ClientOp::ReplayPause { replay_id } => self.replay(client, id, &replay_id, |r| {
                r.pause();
                Ok(())
            }),
            ClientOp::ReplayClose { replay_id } => {
                match self
                    .clients
                    .get_mut(&client)
                    .and_then(|c| c.replays.remove(&replay_id))
                {
                    Some(_) => ok(id),
                    None => bad(id, format!("no replay {replay_id} on this connection")),
                }
            }
            ClientOp::Selfcheck { duration_s } => match self.selfcheck(client, id, duration_s) {
                Some(answer) => answer,
                None => return true,
            },
            ClientOp::Export { .. } | ClientOp::ReplayOpen { .. } | ClientOp::Bye => {
                unreachable!("the connection's thread answers these")
            }
        };
        self.send(client, &answer);
        true
    }
}

/// armod's link, from its reader thread.
impl Core {
    pub(crate) fn armod_up(&mut self, stream: UnixStream) {
        let now = sys::mono_ns();
        self.armod_stream = Some(stream);
        self.armod_sent = Plan::default();
        let back = self.armod == ArmodLink::Lost;
        self.armod = ArmodLink::Connected;
        for r in &mut self.active {
            r.gap_close(now);
        }
        if back {
            self.event_line(
                Level::Info,
                "armod_back",
                "armod is back".into(),
                Vec::new(),
            );
        }
        self.refresh();
        self.broadcast_state();
    }

    /// `Absent`, `Denied` or `Lost`.
    pub(crate) fn armod_down(&mut self, link: ArmodLink) {
        let link = match (self.armod, link) {
            (ArmodLink::Lost, ArmodLink::Absent) => ArmodLink::Lost,
            (_, l) => l,
        };
        self.armod_stream = None;
        self.armod_window = None;
        if let Flush::Waiting { .. } = self.armod_flush {
            self.armod_flush = Flush::Done;
        }
        for armod_id in self
            .armod_selfchecks
            .iter()
            .map(|s| s.armod_id)
            .collect::<Vec<_>>()
        {
            let detail = "armod went away before it answered".to_string();
            self.selfcheck_answered(armod_id, CheckStatus::Unknown, detail, Vec::new());
        }
        if link == self.armod {
            return;
        }
        if self.armod == ArmodLink::Connected {
            let now = sys::mono_ns();
            for r in &mut self.active {
                r.gap_open("armod", "armod_lost", now);
            }
            let text = "lost armod: uprobe and kernel events stop until it is back".into();
            self.event_line(Level::Warn, "armod_lost", text, Vec::new());
        }
        self.armod = link;
        self.ingest.set_sched(Sel::None);
        let hot = self.hot(&Sel::None);
        self.ingest.set_hot(hot);
        self.broadcast_state();
    }

    pub(crate) fn armod_message(&mut self, message: ArmodMessage) {
        match message {
            ArmodMessage::Window(w) => {
                self.ingest.aggregates(Method::Uprobe, &w.aggregates);
                // The other kinds lose aggregate rows or attribution, not records.
                let lost = w.lost["ring"];
                if lost > 0 {
                    for r in &mut self.active {
                        if r.methods().contains(&Method::Uprobe) {
                            r.add_lost("armod", lost, Method::Uprobe);
                        }
                    }
                }
                self.armod_window = Some(w);
                if self.monitoring() || !self.active.is_empty() {
                    self.window(sys::mono_ns());
                }
            }
            ArmodMessage::EventLine(line) => {
                self.broadcast(&ServerMessage::EventLine(line), |c| c.event_line);
            }
            ArmodMessage::Error(e) => {
                let text = format!("armod: {}: {}", e.code, e.message);
                let answered = e.id.is_some_and(|id| {
                    self.selfcheck_answered(id, CheckStatus::Unknown, text.clone(), Vec::new())
                });
                if !answered {
                    self.event_line(Level::Warn, "armod_error", text, Vec::new());
                }
            }
            ArmodMessage::Reply(r) => {
                if matches!(self.armod_flush, Flush::Waiting { id, .. } if id == r.id) {
                    self.armod_flush = Flush::Done;
                    self.finish_ended();
                } else if let ReplyBody::Selfcheck { checks } = r.body {
                    let connected = "connected".to_string();
                    self.selfcheck_answered(r.id, CheckStatus::Pass, connected, checks);
                }
            }
            ArmodMessage::Hello(_) => {}
        }
    }

    pub(crate) fn armod_events(&mut self, batch: &[u8]) {
        let now = sys::mono_ns();
        let Core { ingest, active, .. } = self;
        let mut starttimes = BTreeMap::new();
        let delivered = deliver(
            ingest.as_mut(),
            active,
            Method::Uprobe,
            &mut |pid| {
                starttimes
                    .entry(pid)
                    .or_insert_with(|| Core::entity_of(pid))
                    .clone()
            },
            batch,
            now,
        );
        if let Err(e) = delivered {
            let text = format!("armod sent a bad event batch: {e}");
            self.event_line(Level::Error, "armod_error", text, Vec::new());
        }
    }
}
