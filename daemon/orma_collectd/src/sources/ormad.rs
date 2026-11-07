//! The uprobe and kernel source: orma-collectd's connection to ormad. It connects,
//! says hello, sends what to attach and stream as `Plan` changes, passes ormad's windows and
//! event batches on, and reconnects after a loss.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use orma_proto::frame::{self, Kind};
use orma_proto::msg::*;

use crate::server::Core;

const RETRY: Duration = Duration::from_secs(1);

/// Which processes a probe set covers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Sel {
    #[default]
    None,
    All,
    Pids(BTreeSet<u32>),
}

impl Sel {
    pub fn add(&mut self, other: Sel) {
        *self = match (std::mem::take(self), other) {
            (Sel::All, _) | (_, Sel::All) => Sel::All,
            (Sel::None, o) | (o, Sel::None) => o,
            (Sel::Pids(mut a), Sel::Pids(b)) => {
                a.extend(b);
                Sel::Pids(a)
            }
        };
    }

    pub fn pids<I: IntoIterator<Item = u32>>(pids: I) -> Sel {
        let set: BTreeSet<u32> = pids.into_iter().collect();
        if set.is_empty() {
            Sel::None
        } else {
            Sel::Pids(set)
        }
    }

    pub fn contains(&self, pid: u32) -> bool {
        match self {
            Sel::None => false,
            Sel::All => true,
            Sel::Pids(p) => p.contains(&pid),
        }
    }

    fn list(&self) -> Vec<u32> {
        match self {
            Sel::Pids(p) => p.iter().copied().collect(),
            _ => Vec::new(),
        }
    }
}

/// What orma-collectd wants from ormad at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub window_s: Option<f64>,
    pub discovery: bool,
    pub hot: Sel,
    pub sched: Sel,
    pub bandwidth: Sel,
    pub gpu: Sel,
    pub dds: Sel,
    pub futex: Sel,
    pub names: Sel,
    pub user_words: BTreeMap<String, u64>,
    pub stream: Sel,
}

fn attach(sel: &Sel, sets: Vec<ProbeSet>, user_words: BTreeMap<String, u64>) -> OrmadOp {
    OrmadOp::Attach(Attach {
        selection: Selection {
            all: *sel == Sel::All,
            pids: sel.list(),
            ..Default::default()
        },
        sets,
        user_words,
    })
}

/// The requests that take ormad from `old` to `new`. An empty `pids` in `detach` means every
/// process of those sets, and each `stream` replaces the streamed set (empty with `on`: all).
pub fn requests(old: &Plan, new: &Plan) -> Vec<OrmadOp> {
    let mut ops = Vec::new();
    if new.window_s != old.window_s
        && let Some(window_s) = new.window_s
    {
        ops.push(OrmadOp::Window { window_s });
    }
    if new.discovery != old.discovery {
        ops.push(if new.discovery {
            attach(&Sel::All, vec![ProbeSet::Discovery], BTreeMap::new())
        } else {
            OrmadOp::Detach {
                pids: Vec::new(),
                sets: vec![ProbeSet::Discovery],
            }
        });
    }
    let sets = [
        (ProbeSet::Hot, &old.hot, &new.hot),
        (ProbeSet::RclcppSymbols, &old.hot, &new.hot),
        (ProbeSet::Sched, &old.sched, &new.sched),
        (ProbeSet::Bandwidth, &old.bandwidth, &new.bandwidth),
        (ProbeSet::Gpu, &old.gpu, &new.gpu),
        (ProbeSet::Dds, &old.dds, &new.dds),
        (ProbeSet::Futex, &old.futex, &new.futex),
        (ProbeSet::Names, &old.names, &new.names),
    ];
    for (set, o, n) in sets {
        let words = || match set {
            ProbeSet::Sched => new.user_words.clone(),
            _ => BTreeMap::new(),
        };
        let words_changed = set == ProbeSet::Sched && new.user_words != old.user_words;
        if o == n && !words_changed {
            continue;
        }
        let detach = match (o, n) {
            (Sel::None, _) | (Sel::All | Sel::Pids(_), Sel::All) => None,
            (Sel::All, _) => Some(Vec::new()),
            (Sel::Pids(a), Sel::None) => Some(a.iter().copied().collect()),
            (Sel::Pids(a), Sel::Pids(b)) => {
                Some(a.difference(b).copied().collect::<Vec<_>>()).filter(|gone| !gone.is_empty())
            }
        };
        if let Some(pids) = detach {
            ops.push(OrmadOp::Detach {
                pids,
                sets: vec![set],
            });
        }
        if *n != Sel::None {
            ops.push(attach(n, vec![set], words()));
        }
    }
    if new.stream != old.stream {
        ops.push(OrmadOp::Stream {
            on: new.stream != Sel::None,
            pids: new.stream.list(),
        });
    }
    ops
}

/// Sends one request; ormad's answers come back on the reader thread.
pub fn send(stream: &mut UnixStream, id: u64, op: OrmadOp) -> io::Result<()> {
    stream.write_all(&frame::encode_json(&OrmadRequest { id, op }))
}

fn hello(stream: &mut UnixStream) -> io::Result<bool> {
    let op = OrmadOp::Hello(Hello {
        proto: PROTO,
        client: "orma-collectd".into(),
        pid: std::process::id(),
    });
    send(stream, 0, op)?;
    stream.set_read_timeout(Some(RETRY))?;
    let reply = frame::read(stream)?;
    stream.set_read_timeout(None)?;
    let Some(reply) = reply.filter(|f| f.kind == Kind::Json) else {
        return Ok(false);
    };
    Ok(matches!(
        serde_json::from_slice(&reply.payload),
        Ok(OrmadMessage::Hello(h)) if h.proto[0] == PROTO[0]
    ))
}

/// Connects to `path` until `stop`, and serves each connection into `sink`.
pub fn spawn(sink: Arc<Mutex<Core>>, path: PathBuf, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let mut stream = match UnixStream::connect(&path) {
                Ok(s) => s,
                Err(e) => {
                    let link = match e.kind() {
                        io::ErrorKind::PermissionDenied => OrmadLink::Denied,
                        _ => OrmadLink::Absent,
                    };
                    sink.lock().unwrap().ormad_down(link);
                    thread::sleep(RETRY);
                    continue;
                }
            };
            if !hello(&mut stream).unwrap_or(false) {
                sink.lock().unwrap().ormad_down(OrmadLink::Absent);
                thread::sleep(RETRY);
                continue;
            }
            sink.lock()
                .unwrap()
                .ormad_up(stream.try_clone().expect("dup of a connected socket"));
            while let Ok(Some(f)) = frame::read(&mut stream) {
                let mut sink = sink.lock().unwrap();
                match f.kind {
                    Kind::Json => match serde_json::from_slice(&f.payload) {
                        Ok(m) => sink.ormad_message(m),
                        Err(e) => {
                            let text = format!("ormad sent an unreadable message: {e}");
                            sink.ormad_message(OrmadMessage::Error(ErrorMessage::new(
                                None,
                                code::BAD_REQUEST,
                                text,
                            )));
                        }
                    },
                    Kind::Events => sink.ormad_events(&f.payload),
                }
            }
            sink.lock().unwrap().ormad_down(OrmadLink::Lost);
            thread::sleep(RETRY);
        }
    });
}
