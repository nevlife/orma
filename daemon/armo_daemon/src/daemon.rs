//! armod's loop: clients ask for probe sets, streams and windows (design 12.8); armod applies
//! the union of what every client wants, passes records and window differences to each, and
//! forgets a client's wishes when its connection closes.

use std::{
    collections::{BTreeMap, HashMap},
    io::{self, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    time::{Duration, Instant},
};

use armo_common::{BUCKETS, bpf::*, events::*};
use armo_proto::{
    frame::{self, Kind},
    msg::*,
};

use crate::{
    probes::{Kernel, Missing},
    procs,
    progs::{Set, naming},
    ring,
    sel::Sel,
    selfcheck::Session,
    sys,
};

/// How often every process's maps are read for new libraries, when some set covers all
/// processes. Reading all of them takes tens of ms on a busy host.
const SCAN_ALL: Duration = Duration::from_secs(5);
/// How often the selected processes' maps are read: a Python node loads rclpy late.
const SCAN_SELECTED: Duration = Duration::from_secs(1);
const TICK: Duration = Duration::from_millis(10);
/// A batch is sent when it reaches this, or at the end of a tick.
const BATCH: usize = 1 << 20;
const MAPS_EVERY: Duration = Duration::from_secs(30);

/// `LOST` slots by the name `window.lost` gives them. Unvalidated C++ arguments are not losses.
const LOST_NAMES: [(u32, &str); 5] = [
    (LOST_RING, "ring"),
    (LOST_AGG_FULL, "agg_full"),
    (LOST_DEPTH, "depth"),
    (LOST_USER_WORD, "user_word"),
    (LOST_THREAD_FULL, "thread_full"),
];

/// Where a client's messages go.
pub enum Sink {
    Socket {
        stream: UnixStream,
        input: Vec<u8>,
        output: Vec<u8>,
    },
    /// In-process: the text mode and `--selfcheck` read messages and batches here.
    Local {
        messages: Vec<ArmodMessage>,
        batches: Vec<Vec<u8>>,
    },
}

struct Window {
    every: Duration,
    due: Instant,
    last_ns: u64,
    cpu_ns: u64,
    seq: u64,
    agg: HashMap<AggKey, AggVal>,
    max: HashMap<AggKey, u64>,
    progs: HashMap<&'static str, (u64, u64)>,
    lost: [u64; LOST_SLOTS_ALL as usize],
}

pub struct Client {
    pub sink: Sink,
    hello: bool,
    pub sets: BTreeMap<Set, Sel>,
    pub stream: Sel,
    user_words: BTreeMap<u32, u64>,
    window: Option<Window>,
    batch: Vec<u8>,
    /// `flush` requests, answered after a drain once the links asked for before are tried:
    /// (request, apply number).
    flush: Vec<(u64, u64)>,
    closed: bool,
}

impl Client {
    pub fn new(sink: Sink) -> Client {
        Client {
            sink,
            hello: false,
            sets: BTreeMap::new(),
            stream: Sel::None,
            user_words: BTreeMap::new(),
            window: None,
            batch: Vec::new(),
            flush: Vec::new(),
            closed: false,
        }
    }

    fn covers(&self, pid: u32) -> bool {
        self.sets
            .iter()
            .any(|(s, sel)| *s != Set::Sched && sel.contains(pid))
    }

    fn send(&mut self, m: &ArmodMessage) {
        match &mut self.sink {
            Sink::Socket { output, .. } => output.extend_from_slice(&frame::encode_json(m)),
            Sink::Local { messages, .. } => messages.push(m.clone()),
        }
    }

    fn flush_batch(&mut self) {
        if self.batch.is_empty() {
            return;
        }
        let batch = std::mem::take(&mut self.batch);
        match &mut self.sink {
            Sink::Socket { output, .. } => {
                output.extend_from_slice(&frame::encode(Kind::Events, &batch))
            }
            Sink::Local { batches, .. } => batches.push(batch),
        }
    }
}

/// What every client wants together: sets, the stream, the user words.
type Want = (BTreeMap<Set, Sel>, Sel, BTreeMap<u32, u64>);

pub struct Daemon {
    pub k: Kernel,
    pub clients: BTreeMap<u64, Client>,
    next: u64,
    listener: Option<UnixListener>,
    scanned_all: Option<Instant>,
    scanned: Instant,
    sessions: Vec<Session>,
    /// `attach` replies held until the `apply` they caused has tried its links: (client,
    /// request, apply number).
    attach_replies: Vec<(u64, u64, u64)>,
    applied: Option<Want>,
    owners: BTreeMap<u32, u32>,
    maps: Option<(Instant, Vec<MapUse>)>,
    scratch: Vec<u8>,
}

impl Daemon {
    pub fn new(k: Kernel, listener: Option<UnixListener>) -> Daemon {
        Daemon {
            k,
            clients: BTreeMap::new(),
            next: 1,
            listener,
            scanned_all: None,
            scanned: Instant::now(),
            sessions: Vec::new(),
            attach_replies: Vec::new(),
            applied: None,
            owners: BTreeMap::new(),
            maps: None,
            scratch: Vec::new(),
        }
    }

    pub fn add(&mut self, c: Client) -> u64 {
        let id = self.next;
        self.next += 1;
        self.clients.insert(id, c);
        id
    }

    /// One pass: accept, read, act, drain the ring buffer, send.
    pub fn tick(&mut self) {
        self.accept();
        let mut requests = Vec::new();
        for (id, c) in &mut self.clients {
            if let Sink::Socket { stream, input, .. } = &mut c.sink {
                let mut buf = [0u8; 65536];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => {
                            c.closed = true;
                            break;
                        }
                        Ok(n) => input.extend_from_slice(&buf[..n]),
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(_) => {
                            c.closed = true;
                            break;
                        }
                    }
                }
                loop {
                    match frame::decode(input) {
                        Ok(Some((f, used))) => {
                            input.drain(..used);
                            requests.push((*id, f));
                        }
                        Ok(None) => break,
                        Err(_) => {
                            c.closed = true;
                            break;
                        }
                    }
                }
            }
        }
        for (id, f) in requests {
            self.request_frame(id, f);
        }
        self.housekeeping();
        self.k.take_attached();
        self.drain();
        let (_, attached) = self.k.applied();
        for c in self.clients.values_mut() {
            let done: Vec<u64> = c
                .flush
                .extract_if(.., |(_, at)| *at <= attached)
                .map(|(rid, _)| rid)
                .collect();
            if !done.is_empty() {
                c.flush_batch();
            }
            for rid in done {
                c.send(&ok(rid));
            }
        }
        for (id, rid, _) in self
            .attach_replies
            .extract_if(.., |(_, _, at)| *at <= attached)
        {
            if let Some(c) = self.clients.get_mut(&id) {
                c.send(&ok(rid));
            }
        }
        self.windows();
        self.finish_sessions();
        self.notes();
        for c in self.clients.values_mut() {
            c.flush_batch();
            if let Sink::Socket { stream, output, .. } = &mut c.sink {
                while !output.is_empty() {
                    match stream.write(output) {
                        Ok(0) => {
                            c.closed = true;
                            break;
                        }
                        Ok(n) => {
                            output.drain(..n);
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(_) => {
                            c.closed = true;
                            break;
                        }
                    }
                }
            }
        }
        let closed: Vec<u64> = self
            .clients
            .iter()
            .filter(|(_, c)| c.closed)
            .map(|(id, _)| *id)
            .collect();
        if !closed.is_empty() {
            for id in closed {
                self.clients.remove(&id);
                self.sessions.retain(|s| s.client != id);
                self.attach_replies.retain(|(c, _, _)| *c != id);
            }
            self.apply();
        }
    }

    pub fn run(&mut self) -> ! {
        loop {
            self.tick();
            std::thread::sleep(TICK);
        }
    }

    fn accept(&mut self) {
        while let Some(Ok((stream, _))) = self.listener.as_ref().map(UnixListener::accept) {
            if stream.set_nonblocking(true).is_err() {
                continue;
            }
            self.add(Client::new(Sink::Socket {
                stream,
                input: Vec::new(),
                output: Vec::new(),
            }));
        }
    }

    fn request_frame(&mut self, id: u64, f: frame::Frame) {
        let parsed = (f.kind == Kind::Json)
            .then(|| serde_json::from_slice::<ArmodRequest>(&f.payload))
            .transpose();
        match parsed {
            Ok(Some(r)) => self.request(id, r),
            Ok(None) => self.error(
                id,
                None,
                code::BAD_REQUEST,
                "event batches only go to clients",
            ),
            Err(e) => {
                let rid = serde_json::from_slice::<serde_json::Value>(&f.payload)
                    .ok()
                    .and_then(|v| v.get("id")?.as_u64());
                let op = serde_json::from_slice::<serde_json::Value>(&f.payload)
                    .ok()
                    .and_then(|v| v.get("op")?.as_str().map(str::to_string));
                let known = [
                    "hello",
                    "attach",
                    "detach",
                    "stream",
                    "window",
                    "selfcheck",
                    "flush",
                    "bye",
                ];
                let code = match op {
                    Some(op) if !known.contains(&op.as_str()) => code::UNKNOWN_OP,
                    _ => code::BAD_REQUEST,
                };
                self.error(id, rid, code, &e.to_string());
            }
        }
    }

    fn error(&mut self, id: u64, rid: Option<u64>, code: &str, text: &str) {
        if let Some(c) = self.clients.get_mut(&id) {
            c.send(&ArmodMessage::Error(ErrorMessage::new(rid, code, text)));
        }
    }

    /// Answers one request and applies what changed.
    pub fn request(&mut self, id: u64, r: ArmodRequest) {
        let Some(hello) = self.clients.get(&id).map(|c| c.hello) else {
            return;
        };
        let rid = r.id;
        let attach = matches!(r.op, ArmodOp::Attach(_));
        if !hello && !matches!(r.op, ArmodOp::Hello(_)) {
            return self.error(id, Some(rid), code::BAD_REQUEST, "hello first");
        }
        match r.op {
            ArmodOp::Hello(h) => {
                let c = self.clients.get_mut(&id).expect("checked");
                if let Err(e) = check_proto(rid, h.proto) {
                    c.send(&ArmodMessage::Error(e));
                    c.closed = true;
                    return;
                }
                c.hello = true;
                c.send(&ArmodMessage::Hello(ServerHello {
                    proto: PROTO,
                    server: "armod".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                }));
                // What processes showed once goes out again, for this client's names.
                self.k.forget_seen();
                return;
            }
            ArmodOp::Attach(a) => {
                let c = self.clients.get_mut(&id).expect("checked");
                let sel = Sel::of(&a.selection);
                for set in &a.sets {
                    c.sets.entry(set_of(*set)).or_default().add(&sel);
                    if *set == ProbeSet::Sched {
                        c.user_words = a
                            .user_words
                            .iter()
                            .filter_map(|(t, w)| Some((t.parse().ok()?, *w)))
                            .collect();
                    }
                }
            }
            ArmodOp::Detach { pids, sets } => {
                let c = self.clients.get_mut(&id).expect("checked");
                for set in &sets {
                    let s = c.sets.entry(set_of(*set)).or_default();
                    s.detach(&pids);
                    if *set == ProbeSet::Sched && *s == Sel::None {
                        c.user_words.clear();
                    }
                }
            }
            ArmodOp::Stream { on, pids } => {
                let c = self.clients.get_mut(&id).expect("checked");
                c.stream = match (on, pids.is_empty()) {
                    (false, _) => Sel::None,
                    (true, true) => Sel::All,
                    (true, false) => Sel::pids(pids),
                };
            }
            ArmodOp::Window { window_s } => {
                if !(window_s > 0.0 && window_s.is_finite()) {
                    return self.error(id, Some(rid), code::BAD_REQUEST, "window_s must be > 0");
                }
                let every = Duration::from_secs_f64(window_s);
                let (rows, _) = self.k.sample();
                let progs = self
                    .k
                    .prog_stats()
                    .into_iter()
                    .map(|(n, t, c, _)| (n, (t, c)))
                    .collect();
                let lost = self.k.lost();
                let c = self.clients.get_mut(&id).expect("checked");
                c.window = Some(Window {
                    every,
                    due: Instant::now() + every,
                    last_ns: sys::mono_ns(),
                    cpu_ns: sys::cpu_ns(),
                    seq: 0,
                    agg: rows.into_iter().collect(),
                    max: HashMap::new(),
                    progs,
                    lost,
                });
                c.send(&ok(rid));
                return;
            }
            ArmodOp::Selfcheck { duration_s, pids } => {
                if !(0.0..=3600.0).contains(&duration_s) {
                    return self.error(
                        id,
                        Some(rid),
                        code::BAD_REQUEST,
                        "duration_s must be in 0..=3600",
                    );
                }
                let d = Duration::from_secs_f64(duration_s);
                let target = if pids.is_empty() {
                    Sel::All
                } else {
                    Sel::pids(pids)
                };
                self.k.forget_seen();
                let s = Session::start(id, rid, d, target, &self.k);
                self.sessions.push(s);
                self.apply();
                let applied = self.k.applied().0;
                if let Some(s) = self.sessions.last_mut() {
                    s.applied = applied;
                }
                return;
            }
            ArmodOp::Flush { drain_only } => {
                let applied = if drain_only { 0 } else { self.k.applied().0 };
                if let Some(c) = self.clients.get_mut(&id) {
                    c.flush.push((rid, applied));
                }
                return;
            }
            ArmodOp::Bye => {
                if let Some(c) = self.clients.get_mut(&id) {
                    c.closed = true;
                }
                return;
            }
        }
        self.apply();
        if attach {
            self.attach_replies.push((id, rid, self.k.applied().0));
        } else if let Some(c) = self.clients.get_mut(&id) {
            c.send(&ok(rid));
        }
    }

    /// The union of every client's sets, stream and words, and of running selfchecks.
    fn want(&self) -> Want {
        let mut sets: BTreeMap<Set, Sel> = BTreeMap::new();
        let mut stream = Sel::None;
        let mut words = BTreeMap::new();
        for c in self.clients.values().filter(|c| c.hello) {
            for (s, sel) in &c.sets {
                sets.entry(*s).or_default().add(sel);
            }
            stream.add(&c.stream);
            words.extend(c.user_words.iter().map(|(k, v)| (*k, *v)));
        }
        for s in &self.sessions {
            sets.entry(Set::Discovery).or_default().add(&Sel::All);
            for set in [Set::Hot, Set::RclcppSymbols, Set::Bandwidth] {
                sets.entry(set).or_default().add(&s.target);
            }
            stream.add(&s.target);
        }
        (sets, stream, words)
    }

    fn apply(&mut self) {
        let want = self.want();
        let needs_all = want.0.values().any(|s| *s == Sel::All);
        if needs_all && self.scanned_all.is_none() {
            self.k.scan(None);
            self.scanned_all = Some(Instant::now());
        }
        let selected: std::collections::BTreeSet<u32> = want
            .0
            .values()
            .filter_map(|s| s.list())
            .flatten()
            .copied()
            .collect();
        self.k.scan(Some(&selected));
        self.k.apply(&want.0, &want.1, &want.2);
        if let Some(sched) = want.0.get(&Set::Sched) {
            let pids: Vec<u32> = match sched {
                Sel::All => self.k.procs.keys().copied().collect(),
                Sel::Pids(p) => p.iter().copied().collect(),
                Sel::None => Vec::new(),
            };
            self.owners = procs::owners(pids);
        }
        self.applied = Some(want);
    }

    fn housekeeping(&mut self) {
        let now = Instant::now();
        let mut rescan = false;
        if self.scanned_all.is_some_and(|t| now - t >= SCAN_ALL) {
            self.k.scan(None);
            let dead = self.k.prune();
            for c in self.clients.values_mut() {
                for s in c.sets.values_mut() {
                    s.retain(|p| !dead.contains(&p));
                }
                c.stream.retain(|p| !dead.contains(&p));
            }
            self.scanned_all = Some(now);
            rescan = true;
        }
        if now - self.scanned >= SCAN_SELECTED {
            self.scanned = now;
            rescan = true;
        }
        if rescan && self.applied.is_some() {
            let needs_all = self
                .applied
                .as_ref()
                .is_some_and(|w| w.0.values().any(|s| *s == Sel::All));
            if !needs_all {
                self.scanned_all = None;
            }
            self.apply();
        }
    }

    /// Moves records from the ring buffer to the clients that want them.
    fn drain(&mut self) {
        while let Some(item) = self.k.events.next() {
            self.scratch.clear();
            let procs = &self.k.procs;
            let gid_len = |pid: u32| procs.get(&pid).map_or(16, |p| p.gid_len);
            let Some(head) = ring::convert(&item, gid_len, &mut self.scratch) else {
                continue;
            };
            drop(item);
            let rec = &self.scratch[..];
            let other = match head.event {
                ARMO_EV_SCHED_SWITCH | ARMO_EV_SCHED_WAKEUP => {
                    let body = &rec[size_of::<armo_position>() + size_of::<armo_record>()..];
                    let tid = if head.event == ARMO_EV_SCHED_SWITCH {
                        armo_common::read::<armo_ev_sched_switch>(body).map(|e| e.next_tid)
                    } else {
                        armo_common::read::<armo_ev_sched_wakeup>(body).map(|e| e.tid)
                    };
                    tid.and_then(|t| self.owners.get(&t).copied())
                }
                _ => None,
            };
            for s in &mut self.sessions {
                s.observe(head, rec);
            }
            let named = naming(head.event);
            for c in self.clients.values_mut() {
                let wanted = c.stream.contains(head.pid)
                    || other.is_some_and(|p| c.stream.contains(p))
                    || (named && c.covers(head.pid));
                if wanted {
                    c.batch.extend_from_slice(rec);
                    if c.batch.len() >= BATCH {
                        c.flush_batch();
                    }
                }
            }
        }
    }

    fn windows(&mut self) {
        let now = Instant::now();
        let due: Vec<u64> = self
            .clients
            .iter()
            .filter(|(_, c)| c.window.as_ref().is_some_and(|w| now >= w.due))
            .map(|(id, _)| *id)
            .collect();
        if due.is_empty() {
            return;
        }
        let (rows, closed) = self.k.sample();
        for c in self.clients.values_mut() {
            if let Some(w) = &mut c.window {
                for (k, v) in &rows {
                    if v.max_gen == closed {
                        let m = w.max.entry(*k).or_default();
                        *m = (*m).max(v.max);
                    }
                }
            }
        }
        let progs = self.k.prog_stats();
        let lost = self.k.lost();
        let missing: Vec<ProbeMissing> = self.k.missing.iter().map(probe_missing).collect();
        let maps = match &self.maps {
            Some((t, m)) if now - *t < MAPS_EVERY => m.clone(),
            _ => {
                let m: Vec<MapUse> = self
                    .k
                    .map_use()
                    .into_iter()
                    .map(|(name, used, max, memlock)| MapUse {
                        name,
                        used,
                        max,
                        memlock_bytes: memlock,
                    })
                    .collect();
                self.maps = Some((now, m.clone()));
                m
            }
        };
        let t_mono_ns = sys::mono_ns();
        let (cpu_ns, rss_bytes) = (sys::cpu_ns(), sys::rss_bytes());
        for id in due {
            let c = self.clients.get_mut(&id).expect("listed");
            let w = c.window.as_mut().expect("due");
            let mut aggregates = Vec::new();
            for (k, v) in &rows {
                let before = w.agg.get(k);
                let count = v.count - before.map_or(0, |b| b.count);
                if count == 0 {
                    continue;
                }
                let sum = v.sum - before.map_or(0, |b| b.sum);
                let mut hist = vec![0u64; BUCKETS];
                for (i, h) in hist.iter_mut().enumerate() {
                    *h = v.hist[i] - before.map_or(0, |b| b.hist[i]);
                }
                let max = w.max.remove(k).unwrap_or(0);
                aggregates.push(Aggregate {
                    pid: k.tgid,
                    tid: k.tid,
                    kind: k.kind,
                    src: k.src,
                    entry: k.entry,
                    key: k.key,
                    count,
                    sum,
                    max,
                    hist,
                });
            }
            w.agg = rows.iter().copied().collect();
            w.max.clear();
            let mut bpf = Vec::new();
            for (name, t, n, attached) in &progs {
                let (t0, n0) = w.progs.get(name).copied().unwrap_or((0, 0));
                if *attached || *n != n0 {
                    bpf.push(BpfProg {
                        prog: name.to_string(),
                        pid: None,
                        run_time_ns: t.saturating_sub(t0),
                        run_cnt: n.saturating_sub(n0),
                    });
                }
                w.progs.insert(name, (*t, *n));
            }
            let mut lost_map = BTreeMap::new();
            for (slot, name) in LOST_NAMES {
                let d = lost[slot as usize] - w.lost[slot as usize];
                lost_map.insert(name.to_string(), d);
            }
            w.lost = lost;
            w.seq += 1;
            let window_ns = t_mono_ns - w.last_ns;
            w.last_ns = t_mono_ns;
            let usage = ProcUse {
                cpu_pct: Some((cpu_ns - w.cpu_ns) as f64 * 100.0 / window_ns.max(1) as f64),
                rss_bytes,
            };
            w.cpu_ns = cpu_ns;
            w.due += w.every;
            if w.due < now {
                w.due = now + w.every;
            }
            let m = ArmodMessage::Window(ArmodWindow {
                seq: w.seq,
                t_mono_ns,
                window_ns,
                aggregates,
                bpf,
                maps: maps.clone(),
                lost: lost_map,
                probes_missing: missing.clone(),
                usage: Some(usage),
            });
            c.send(&m);
        }
    }

    fn finish_sessions(&mut self) {
        let now = Instant::now();
        let (_, attached) = self.k.applied();
        // The window opens once the probes are on: attaching takes tens of seconds here.
        for s in self.sessions.iter_mut().filter(|s| s.applied <= attached) {
            s.until.get_or_insert(now + s.duration);
        }
        let (done, running): (Vec<Session>, Vec<Session>) = std::mem::take(&mut self.sessions)
            .into_iter()
            .partition(|s| s.until.is_some_and(|u| now >= u));
        self.sessions = running;
        if done.is_empty() {
            return;
        }
        for s in done {
            let (client, request) = (s.client, s.request);
            let checks = s.finish(&mut self.k);
            if let Some(c) = self.clients.get_mut(&client) {
                c.send(&ArmodMessage::Reply(Reply {
                    id: request,
                    ok: true,
                    body: ReplyBody::Selfcheck { checks },
                }));
            }
        }
        self.apply();
    }

    fn notes(&mut self) {
        for n in std::mem::take(&mut self.k.notes) {
            eprintln!("armod: {}", n.text);
            let line = ArmodMessage::EventLine(EventLine {
                t_real_ns: sys::real_ns(),
                level: if n.warn { Level::Warn } else { Level::Info },
                kind: "probe_missing".into(),
                text: n.text,
                ids: Vec::new(),
            });
            for c in self.clients.values_mut().filter(|c| c.hello) {
                c.send(&line);
            }
        }
    }
}

fn ok(id: u64) -> ArmodMessage {
    ArmodMessage::Reply(Reply {
        id,
        ok: true,
        body: ReplyBody::Empty {},
    })
}

fn set_of(s: ProbeSet) -> Set {
    match s {
        ProbeSet::Discovery => Set::Discovery,
        ProbeSet::Hot => Set::Hot,
        ProbeSet::Sched => Set::Sched,
        ProbeSet::Bandwidth => Set::Bandwidth,
        ProbeSet::RclcppSymbols => Set::RclcppSymbols,
        ProbeSet::Names => Set::Names,
    }
}

fn probe_missing(m: &Missing) -> ProbeMissing {
    ProbeMissing {
        pid: m.pid,
        method: Method::Uprobe,
        point: m.point.to_string(),
        reason: m.reason.clone(),
    }
}
