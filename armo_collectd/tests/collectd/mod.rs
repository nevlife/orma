//! Helpers for WP3a's tests: an in-process armo-collectd with a counting model, and a client.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use armo_collectd::record::Caps;
use armo_collectd::server::{self, Config, Exit, Model};
use armo_collectd::sources::armod::Sel;
use armo_collectd::sys;
use armo_proto::Ingest;
use armo_proto::frame;
use armo_proto::msg::*;
use armo_proto::records::Record;

pub const WAIT: Duration = Duration::from_secs(10);

/// A directory removed when dropped; declared before the server, it outlives it.
pub struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("armo_collectd_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// What the model saw.
#[derive(Default)]
pub struct Seen {
    pub records: Vec<(Method, u32, u16)>,
    pub aggregates: Vec<(Method, Aggregate)>,
    pub windows: u64,
    pub sched: bool,
    pub expected_hz: BTreeMap<String, f64>,
    /// What each window lists as its processes.
    pub processes: Vec<Process>,
    pub nodes: Vec<Node>,
}

/// An `Ingest` that only counts, the stand-in for WP3b's model.
#[derive(Clone, Default)]
pub struct Counting(pub Arc<Mutex<Seen>>);

impl Ingest for Counting {
    fn record(&mut self, method: Method, record: &Record<'_>) {
        let mut seen = self.0.lock().unwrap();
        seen.records.push((method, record.pid, record.header.event));
    }
    fn aggregates(&mut self, method: Method, rows: &[Aggregate]) {
        let mut seen = self.0.lock().unwrap();
        seen.aggregates
            .extend(rows.iter().map(|r| (method, r.clone())));
    }
    fn window(&mut self, t_mono_ns: u64, t_real_ns: u64, window_ns: u64, _: bool) -> Window {
        let mut seen = self.0.lock().unwrap();
        seen.windows += 1;
        Window {
            processes: seen.processes.clone(),
            nodes: seen.nodes.clone(),
            ..empty_window(t_mono_ns, t_real_ns, window_ns)
        }
    }
}

impl Model for Counting {
    fn set_sched(&mut self, pids: Sel) {
        self.0.lock().unwrap().sched = pids != Sel::None;
    }
    fn set_hot(&mut self, _: Sel) {}
    fn set_lttng_span(&mut self, _: u64) {}
    fn set_expected_hz(&mut self, by_id: BTreeMap<String, f64>) {
        self.0.lock().unwrap().expected_hz = by_id;
    }
}

pub fn empty_window(t_mono_ns: u64, t_real_ns: u64, window_ns: u64) -> Window {
    Window {
        seq: 0,
        replay: false,
        t_mono_ns,
        t_real_ns,
        window_ns,
        behind: false,
        processes: Vec::new(),
        threads: Vec::new(),
        nodes: Vec::new(),
        executors: Vec::new(),
        callbacks: Vec::new(),
        topics: Vec::new(),
        subscriptions: Vec::new(),
        flows: Vec::new(),
        agent: Agent {
            armod: None,
            collectd: ProcUse {
                cpu_pct: None,
                rss_bytes: None,
            },
            bpf: Vec::new(),
            maps: Vec::new(),
            shm: ShmUse {
                files: 0,
                bytes: 0,
                ring_drops: 0,
            },
            lttng_discarded: None,
            collect_ns: 0,
            probes_missing: Vec::new(),
        },
        hists: None,
    }
}

pub fn config(dir: &Path) -> Config {
    for d in ["shm", "data"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    Config {
        runtime_dir: dir.join("run"),
        data_dir: dir.join("data"),
        shm_dir: dir.join("shm"),
        armod_socket: dir.join("armod.sock"),
        caps: Caps::default(),
        idle: Duration::from_secs(300),
        disk: sys::disk,
        guard: None,
        lttng: false,
    }
}

pub struct Server {
    pub socket: PathBuf,
    pub dir: PathBuf,
    pub seen: Counting,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<io::Result<Exit>>>,
}

impl Server {
    pub fn start(dir: &Path, config: Config) -> Server {
        let socket = config.socket();
        let seen = Counting::default();
        let model = seen.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = thread::spawn(move || server::run(config, Box::new(model), &flag));
        until(|| socket.exists());
        Server {
            socket,
            dir: dir.to_path_buf(),
            seen,
            stop,
            thread: Some(thread),
        }
    }

    pub fn client(&self, name: &str) -> Client {
        Client::connect(&self.socket, name)
    }

    pub fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// Stops as SIGTERM does.
    pub fn stop(mut self) -> Exit {
        self.stop.store(true, Ordering::Relaxed);
        self.join()
    }

    pub fn join(&mut self) -> Exit {
        self.thread.take().unwrap().join().unwrap().unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn until(mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < WAIT, "timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

pub struct Client {
    pub stream: UnixStream,
    next: u64,
    pub state: Option<State>,
    pub lines: Vec<EventLine>,
    pub windows: Vec<Window>,
}

impl Client {
    pub fn raw(socket: &Path) -> Client {
        let stream = UnixStream::connect(socket).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        Client {
            stream,
            next: 1,
            state: None,
            lines: Vec::new(),
            windows: Vec::new(),
        }
    }

    /// Connected, past hello and the first state.
    pub fn connect(socket: &Path, name: &str) -> Client {
        let mut c = Client::raw(socket);
        c.send(ClientOp::Hello(Hello {
            proto: PROTO,
            client: name.into(),
            pid: std::process::id(),
        }));
        assert!(matches!(c.recv(), ServerMessage::Hello(_)));
        assert!(matches!(c.recv(), ServerMessage::State(_)));
        c
    }

    pub fn send(&mut self, op: ClientOp) -> u64 {
        let id = self.next;
        self.next += 1;
        frame::write_json(&mut self.stream, &ClientRequest { id, op }).unwrap();
        id
    }

    pub fn recv(&mut self) -> ServerMessage {
        let f = frame::read(&mut self.stream).unwrap().expect("a message");
        let m: ServerMessage = serde_json::from_slice(&f.payload).unwrap();
        match &m {
            ServerMessage::State(s) => self.state = Some(s.clone()),
            ServerMessage::EventLine(l) => self.lines.push(l.clone()),
            ServerMessage::Window(w) => self.windows.push((**w).clone()),
            _ => {}
        }
        m
    }

    /// Reads until `done` holds for a message, keeping state, lines and windows on the way.
    pub fn until(&mut self, mut done: impl FnMut(&ServerMessage) -> bool) -> ServerMessage {
        let start = Instant::now();
        loop {
            assert!(start.elapsed() < WAIT, "timed out waiting for a message");
            let m = self.recv();
            if done(&m) {
                return m;
            }
        }
    }

    /// The reply or error to request `id`.
    pub fn answer(&mut self, id: u64) -> Result<ReplyBody, ErrorMessage> {
        match self.until(|m| match m {
            ServerMessage::Reply(r) => r.id == id,
            ServerMessage::Error(e) => e.id == Some(id),
            _ => false,
        }) {
            ServerMessage::Reply(r) => Ok(r.body),
            ServerMessage::Error(e) => Err(e),
            _ => unreachable!(),
        }
    }

    pub fn request(&mut self, op: ClientOp) -> Result<ReplyBody, ErrorMessage> {
        let id = self.send(op);
        self.answer(id)
    }

    pub fn start(&mut self, options: RecordOptions) -> (String, PathBuf) {
        match self.request(ClientOp::RecordStart(options)) {
            Ok(ReplyBody::RecordStarted { record_id, dir }) => (record_id, PathBuf::from(dir)),
            other => panic!("record_start: {other:?}"),
        }
    }

    pub fn stop(&mut self, record_id: &str) {
        let r = self.request(ClientOp::RecordStop {
            record_id: record_id.into(),
        });
        assert!(matches!(r, Ok(ReplyBody::Empty {})), "{r:?}");
    }

    pub fn recording(&self, id: &str) -> Option<Recording> {
        self.state
            .as_ref()?
            .recordings
            .iter()
            .find(|r| r.id == id)
            .cloned()
    }

    /// Reads states until recording `id` has ended.
    pub fn ended(&mut self, id: &str) -> Recording {
        loop {
            if let Some(r) = self
                .recording(id)
                .filter(|r| r.state != RecordState::Recording)
            {
                return r;
            }
            self.recv();
        }
    }
}

pub fn options(dir: &Path) -> RecordOptions {
    RecordOptions {
        dir: Some(dir.display().to_string()),
        ..RecordOptions::default()
    }
}

pub fn meta(dir: &Path) -> RecordMeta {
    armo_collectd::record::show::read_meta(dir).unwrap()
}

pub fn own_starttime() -> u64 {
    sys::starttime(std::process::id()).unwrap()
}
