//! A stand-in for orma-collectd's client socket, for client tests (WP7, WP8). It speaks the
//! protocol from the golden messages: it starts with the golden `state` (one recording in
//! progress), keeps recordings and the selection across clients, and logs each request.

use std::io::{self, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::frame::{self, Kind};
use crate::msg::*;

pub const GOLDEN_STATE: &str = include_str!("../../golden/server/state.json");
pub const GOLDEN_WINDOW: &str = include_str!("../../golden/server/window.json");
pub const GOLDEN_EVENT_LINE: &str = include_str!("../../golden/server/event_line.json");
pub const GOLDEN_SHOW: &str = include_str!("../../golden/server/reply_record_show.json");
pub const GOLDEN_DETAIL: &str = include_str!("../../golden/server/reply_detail.json");
pub const GOLDEN_SELFCHECK: &str = include_str!("../../golden/server/reply_selfcheck.json");

fn golden(text: &str) -> ServerMessage {
    serde_json::from_str(text).expect("golden messages parse")
}

fn golden_body(text: &str) -> ReplyBody {
    match golden(text) {
        ServerMessage::Reply(reply) => reply.body,
        other => panic!("golden reply is {other:?}"),
    }
}

struct Shared {
    state: State,
    clients: Vec<UnixStream>,
    started: u64,
    log: Box<dyn Write + Send>,
}

impl Shared {
    fn broadcast_state(&mut self) {
        self.state.seq += 1;
        let bytes = frame::encode_json(&ServerMessage::State(self.state.clone()));
        self.clients.retain_mut(|c| c.write_all(&bytes).is_ok());
    }
}

pub struct FakeServer {
    shared: Arc<Mutex<Shared>>,
}

impl FakeServer {
    /// Binds `path` and serves each connection on its own thread. `log` gets every request's
    /// JSON, one line each.
    pub fn start(path: &Path, log: Box<dyn Write + Send>) -> io::Result<FakeServer> {
        let listener = UnixListener::bind(path)?;
        let ServerMessage::State(state) = golden(GOLDEN_STATE) else {
            panic!("golden state is not a state")
        };
        let shared = Arc::new(Mutex::new(Shared {
            state,
            clients: Vec::new(),
            started: 0,
            log,
        }));
        let accept = Arc::clone(&shared);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = Arc::clone(&accept);
                thread::spawn(move || {
                    let _ = serve(stream, &shared);
                });
            }
        });
        Ok(FakeServer { shared })
    }

    pub fn state(&self) -> State {
        self.shared.lock().unwrap().state.clone()
    }
}

fn reply(id: u64, body: ReplyBody) -> ServerMessage {
    ServerMessage::Reply(Reply { id, ok: true, body })
}

fn serve(mut stream: UnixStream, shared: &Mutex<Shared>) -> io::Result<()> {
    let mut hello = false;
    while let Some(frame) = frame::read(&mut stream)? {
        if frame.kind != Kind::Json {
            return Ok(());
        }
        {
            let mut s = shared.lock().unwrap();
            s.log.write_all(&frame.payload)?;
            s.log.write_all(b"\n")?;
            s.log.flush()?;
        }
        let request = match parse_request(&frame.payload) {
            Ok(r) => r,
            Err(e) => {
                frame::write_json(&mut stream, &ServerMessage::Error(e))?;
                continue;
            }
        };
        let id = request.id;
        let mut s = shared.lock().unwrap();
        let out = match request.op {
            ClientOp::Hello(h) => {
                if let Err(e) = check_proto(id, h.proto) {
                    return frame::write_json(&mut stream, &ServerMessage::Error(e));
                }
                hello = true;
                let server = ServerHello {
                    proto: PROTO,
                    server: "orma-fake-server".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                };
                frame::write_json(&mut stream, &ServerMessage::Hello(server))?;
                s.clients.push(stream.try_clone()?);
                vec![ServerMessage::State(s.state.clone())]
            }
            _ if !hello => {
                let e = ErrorMessage::new(Some(id), code::BAD_REQUEST, "hello first");
                vec![ServerMessage::Error(e)]
            }
            ClientOp::Subscribe { streams } => {
                let mut out = vec![reply(id, ReplyBody::Empty {})];
                if streams.contains(&Stream::Window) {
                    out.push(golden(GOLDEN_WINDOW));
                }
                if streams.contains(&Stream::EventLine) {
                    out.push(golden(GOLDEN_EVENT_LINE));
                }
                out
            }
            ClientOp::Select(select) => {
                s.state.monitor.selection = select.selection;
                s.state.monitor.method_by_pid = select.method_by_pid;
                s.broadcast_state();
                vec![reply(id, ReplyBody::Empty {})]
            }
            ClientOp::Configure(c) => {
                let m = &mut s.state.monitor;
                m.window_s = c.window_s.unwrap_or(m.window_s);
                m.bandwidth = c.bandwidth.unwrap_or(m.bandwidth);
                m.sched = c.sched.unwrap_or(m.sched);
                m.malloc = c.malloc.unwrap_or(m.malloc);
                m.gpu = c.gpu.unwrap_or(m.gpu);
                m.dds = c.dds.unwrap_or(m.dds);
                m.futex = c.futex.unwrap_or(m.futex);
                m.chains = c.chains.unwrap_or(std::mem::take(&mut m.chains));
                m.latest_nodes = c
                    .latest_nodes
                    .unwrap_or(std::mem::take(&mut m.latest_nodes));
                s.broadcast_state();
                vec![reply(id, ReplyBody::Empty {})]
            }
            ClientOp::RecordStart(options) => {
                s.started += 1;
                let record_id = format!("r-fake-{}", s.started);
                // The real server answers with the recording's own directory, `dir/name`.
                let dir = format!(
                    "{}/{}",
                    options.dir.as_deref().unwrap_or("/tmp"),
                    options.name.as_deref().unwrap_or(&record_id)
                );
                let mut recording = s.state.recordings[0].clone();
                recording.id.clone_from(&record_id);
                recording.dir.clone_from(&dir);
                recording.kind = options.kind.unwrap_or(RecordKind::Events);
                recording.targets = options.targets.unwrap_or(Targets {
                    all: true,
                    pids: Vec::new(),
                    node: None,
                    topic: None,
                    exclude: None,
                });
                recording.elapsed_s = 0.0;
                recording.bytes = 0;
                s.state.recordings.push(recording);
                s.broadcast_state();
                vec![reply(id, ReplyBody::RecordStarted { record_id, dir })]
            }
            ClientOp::RecordStop { record_id } => {
                let found = s.state.recordings.iter_mut().find(|r| r.id == record_id);
                if let Some(r) = found {
                    r.state = RecordState::Finalized;
                    r.end_reason = Some(EndReason::UserStop);
                    s.broadcast_state();
                    vec![reply(id, ReplyBody::Empty {})]
                } else {
                    let text = format!("no recording {record_id}");
                    vec![ServerMessage::Error(ErrorMessage::new(
                        Some(id),
                        code::BAD_REQUEST,
                        text,
                    ))]
                }
            }
            ClientOp::RecordList => {
                let recordings = s.state.recordings.clone();
                vec![reply(id, ReplyBody::Recordings { recordings })]
            }
            ClientOp::RecordShow { .. } => vec![reply(id, golden_body(GOLDEN_SHOW))],
            ClientOp::Detail { .. } => vec![reply(id, golden_body(GOLDEN_DETAIL))],
            ClientOp::Selfcheck { .. } => vec![reply(id, golden_body(GOLDEN_SELFCHECK))],
            ClientOp::ReplayOpen { .. } => {
                vec![reply(
                    id,
                    ReplyBody::ReplayOpened {
                        replay_id: "rp-fake-1".into(),
                    },
                )]
            }
            ClientOp::Bye => return Ok(()),
            ClientOp::ReplaySeek { .. }
            | ClientOp::ReplayPlay { .. }
            | ClientOp::ReplayPause { .. }
            | ClientOp::ReplayClose { .. }
            | ClientOp::Export { .. } => vec![reply(id, ReplyBody::Empty {})],
        };
        // Under the lock, so a broadcast from another connection never splits a frame.
        for message in out {
            frame::write_json(&mut stream, &message)?;
        }
    }
    Ok(())
}
