//! The test doubles behave as design 12 and 13 say a server and a shim do.

use std::io::Write;
use std::mem::offset_of;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use orma_common::events::*;
use orma_common::read;
use orma_common::shm::*;
use orma_proto::fake::server::FakeServer;
use orma_proto::fake::shm::{FakeShm, Geometry};
use orma_proto::frame;
use orma_proto::msg::*;
use orma_proto::records;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("orma_fake_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn send(s: &mut UnixStream, id: u64, op: ClientOp) {
    frame::write_json(s, &ClientRequest { id, op }).unwrap();
}

fn recv(s: &mut UnixStream) -> ServerMessage {
    let f = frame::read(s).unwrap().expect("a message");
    serde_json::from_slice(&f.payload).unwrap()
}

fn hello(s: &mut UnixStream, proto: Version) {
    send(
        s,
        1,
        ClientOp::Hello(Hello {
            proto,
            client: "test".into(),
            pid: 1,
        }),
    );
}

fn state(m: ServerMessage) -> State {
    match m {
        ServerMessage::State(s) => s,
        other => panic!("not a state: {other:?}"),
    }
}

#[test]
fn a_recording_outlives_its_client() {
    let dir = scratch("server");
    let path = dir.join("collectd.sock");
    let log = Log::default();
    let server = FakeServer::start(&path, Box::new(log.clone())).unwrap();

    let mut gui = UnixStream::connect(&path).unwrap();
    hello(&mut gui, PROTO);
    assert!(matches!(recv(&mut gui), ServerMessage::Hello(_)));
    let first = state(recv(&mut gui));
    assert_eq!(first.recordings[0].state, RecordState::Recording);

    let options = RecordOptions {
        kind: Some(RecordKind::Windows),
        ..Default::default()
    };
    send(&mut gui, 2, ClientOp::RecordStart(options));
    let broadcast = state(recv(&mut gui));
    let ServerMessage::Reply(Reply {
        id: 2,
        body: ReplyBody::RecordStarted { record_id, .. },
        ..
    }) = recv(&mut gui)
    else {
        panic!("no record_start reply")
    };
    assert_eq!(broadcast.recordings.last().unwrap().id, record_id);
    drop(gui);

    let mut cli = UnixStream::connect(&path).unwrap();
    hello(&mut cli, [1, 3]);
    recv(&mut cli);
    let again = state(recv(&mut cli));
    assert!(again.recordings.iter().any(|r| r.id == record_id));
    send(
        &mut cli,
        3,
        ClientOp::RecordStop {
            record_id: record_id.clone(),
        },
    );
    let stopped = state(recv(&mut cli));
    let r = stopped
        .recordings
        .iter()
        .find(|r| r.id == record_id)
        .unwrap();
    assert_eq!(
        (r.state, r.end_reason),
        (RecordState::Finalized, Some(EndReason::UserStop))
    );
    assert!(stopped.seq > again.seq);
    assert!(matches!(
        recv(&mut cli),
        ServerMessage::Reply(Reply { id: 3, .. })
    ));
    assert_eq!(server.state().recordings.len(), 2);

    let logged = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let ops: Vec<String> = logged
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["op"].to_string())
        .collect();
    assert_eq!(
        ops,
        [
            "\"hello\"",
            "\"record_start\"",
            "\"hello\"",
            "\"record_stop\""
        ]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_server_refuses_what_it_should() {
    let dir = scratch("refuse");
    let path = dir.join("collectd.sock");
    let _server = FakeServer::start(&path, Box::new(std::io::sink())).unwrap();

    let mut early = UnixStream::connect(&path).unwrap();
    send(&mut early, 5, ClientOp::RecordList);
    assert!(matches!(recv(&mut early), ServerMessage::Error(e) if e.code == code::BAD_REQUEST));
    frame::write(&mut early, frame::Kind::Json, br#"{"op":"fly","id":6}"#).unwrap();
    assert!(matches!(recv(&mut early), ServerMessage::Error(e) if e.code == code::UNKNOWN_OP));

    let mut old = UnixStream::connect(&path).unwrap();
    hello(&mut old, [2, 0]);
    assert!(matches!(recv(&mut old), ServerMessage::Error(e) if e.code == code::PROTO_MAJOR));
    assert!(frame::read(&mut old).unwrap().is_none());

    let mut sub = UnixStream::connect(&path).unwrap();
    hello(&mut sub, PROTO);
    recv(&mut sub);
    recv(&mut sub);
    send(
        &mut sub,
        7,
        ClientOp::Subscribe {
            streams: vec![Stream::Window],
        },
    );
    assert!(matches!(recv(&mut sub), ServerMessage::Reply(_)));
    assert!(matches!(recv(&mut sub), ServerMessage::Window(_)));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Reads the file as orma-collectd would, through the file rather than a mapping.
struct Reader {
    file: std::fs::File,
    geometry: Geometry,
}

impl Reader {
    fn header(&self) -> orma_shm_header {
        let mut b = vec![0; ORMA_SHM_HEADER_SIZE];
        self.file.read_exact_at(&mut b, 0).unwrap();
        read(&b).unwrap()
    }

    fn arm(&self) {
        self.file
            .write_all_at(
                &1u32.to_le_bytes(),
                offset_of!(orma_shm_header, armed) as u64,
            )
            .unwrap();
    }

    fn slot_at(&self, slot: usize) -> u64 {
        (ORMA_SHM_HEADER_SIZE + slot * self.geometry.thread_size()) as u64
    }

    fn slot(&self, slot: usize) -> orma_thread_slot {
        let mut b = vec![0; size_of::<orma_thread_slot>()];
        self.file.read_exact_at(&mut b, self.slot_at(slot)).unwrap();
        read(&b).unwrap()
    }

    fn row(&self, slot: usize, i: usize) -> orma_table_entry {
        let mut b = vec![0; size_of::<orma_table_entry>()];
        let at = self.slot_at(slot) as usize + size_of::<orma_thread_slot>() + i * b.len();
        self.file.read_exact_at(&mut b, at as u64).unwrap();
        read(&b).unwrap()
    }

    /// Every record from tail to head, skipping PAD, then the tail moved to head.
    fn drain(&self, slot: usize) -> Vec<(u16, u64)> {
        let s = self.slot(slot);
        let size = u64::from(self.geometry.ring_bytes);
        let ring = self.slot_at(slot) + orma_ring_offset(self.geometry.table_entries) as u64;
        let mut data = vec![0; size as usize];
        self.file.read_exact_at(&mut data, ring).unwrap();
        let mut out = Vec::new();
        let mut at = s.ring_tail;
        while at < s.ring_head {
            let pos = (at % size) as usize;
            let len = u64::from(u16::from_le_bytes([data[pos + 2], data[pos + 3]]));
            if u16::from_le_bytes([data[pos], data[pos + 1]]) != ORMA_EV_PAD {
                let (h, _) = records::check(&data[pos..], pos).unwrap();
                out.push((h.event, h.t_mono_ns));
            }
            at += len;
        }
        assert_eq!(at, s.ring_head);
        let tail = self.slot_at(slot) + offset_of!(orma_thread_slot, ring_tail) as u64;
        self.file.write_all_at(&at.to_le_bytes(), tail).unwrap();
        out
    }
}

fn alive_lock_held(file: &std::fs::File) -> bool {
    let mut fl = libc::flock {
        l_type: libc::F_WRLCK as libc::c_short,
        l_whence: libc::SEEK_SET as libc::c_short,
        l_start: libc::off_t::from(ORMA_LOCK_ALIVE),
        l_len: 1,
        l_pid: 0,
    };
    assert_eq!(fcntl_getlk(file.as_raw_fd(), &mut fl), 0);
    fl.l_type == libc::F_RDLCK as libc::c_short
}

#[allow(unsafe_code)]
fn fcntl_getlk(fd: i32, fl: &mut libc::flock) -> i32 {
    // SAFETY: fl is a valid flock for the duration of the call.
    unsafe { libc::fcntl(fd, libc::F_OFD_GETLK, std::ptr::from_mut::<libc::flock>(fl)) }
}

#[test]
fn the_writer_follows_the_shared_memory_rules() {
    let dir = scratch("shm");
    let geometry = Geometry {
        n_threads: 2,
        table_entries: 2,
        ring_bytes: 256,
    };
    let shm = FakeShm::create(&dir, 4021, 771234, ORMA_BACKEND_SHM, geometry).unwrap();
    assert_eq!(shm.path(), dir.join("orma.4021.771234"));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(shm.path())
        .unwrap();
    assert_eq!(
        file.metadata().unwrap().len() as usize,
        geometry.file_size()
    );
    let reader = Reader { file, geometry };
    let h = reader.header();
    assert_eq!(
        (h.magic, h.version, h.pid, h.starttime),
        (ORMA_SHM_MAGIC, ORMA_SHM_VERSION, 4021, 771234)
    );
    assert_eq!(h.thread_size as usize, orma_thread_size(2, 256));
    assert!(alive_lock_held(&reader.file));

    let a = shm.claim(4025).unwrap();
    let b = shm.claim(4026).unwrap();
    assert_eq!(shm.claim(4027), None);
    assert_eq!(reader.header().threads_overflow, 1);
    assert_eq!(
        (
            reader.slot(a).tid,
            reader.slot(a).generation,
            reader.slot(b).state
        ),
        (4025, 1, ORMA_THREAD_LIVE)
    );

    let start = orma_ev_callback_start {
        callback: 0x55a0,
        is_intra_process: 0,
        pad: 0,
    };
    let take = orma_ev_rmw_take {
        taken: 1,
        ..Default::default()
    };
    // 32 and 56 byte records, so some wrap with a PAD before them.
    let push = |t: u64| match t % 2 {
        0 => shm.push(a, ORMA_EV_CALLBACK_START, t, &start, &[]),
        _ => shm.push(a, ORMA_EV_RMW_TAKE, t, &take, &[]),
    };
    assert!(!push(0));
    reader.arm();
    let mut pushed = Vec::new();
    for t in 0..40u64 {
        if push(t) {
            pushed.push(t);
        }
        if t % 3 == 2 {
            let got: Vec<u64> = reader.drain(a).into_iter().map(|(_, t)| t).collect();
            assert_eq!(got, pushed);
            pushed.clear();
        }
    }
    assert_eq!(reader.slot(a).ring_drops, 0);
    for t in 0..20 {
        push(t);
    }
    assert!(reader.slot(a).ring_drops > 0);

    assert!(shm.add(a, ORMA_AGG_CALLBACK, ORMA_SRC_ROS, 0, 0x55a0, 1500));
    assert!(shm.add(a, ORMA_AGG_CALLBACK, ORMA_SRC_ROS, 0, 0x55a0, 3000));
    assert!(shm.add(a, ORMA_AGG_DISPATCH, ORMA_SRC_ROS, 0, 0x55a0, 10));
    assert!(!shm.add(a, ORMA_AGG_WAIT, ORMA_SRC_ROS, 0, 0x99, 10));
    let row = reader.row(a, 0);
    assert_eq!(
        (row.kind, row.key, row.count, row.sum, row.max),
        (ORMA_AGG_CALLBACK, 0x55a0, 2, 4500, 3000)
    );
    assert_eq!(row.hist.iter().sum::<u64>(), 2);
    assert_eq!(reader.slot(a).table_overflow, 1);

    shm.set_current(b, 0x7000, ORMA_SRC_FLUX, 3);
    let cur = reader.slot(b);
    assert_eq!(
        (cur.cur_cb, cur.cur_src, cur.cur_entry),
        (0x7000, u32::from(ORMA_SRC_FLUX), 3)
    );
    shm.set_missing(ORMA_EV_RCLCPP_INTRA_PUBLISH);
    shm.set_missing(ORMA_EV_FLUX_WAKE);
    let missing = reader.header().missing;
    assert_eq!(missing[0], 1 << ORMA_EV_RCLCPP_INTRA_PUBLISH);
    assert_eq!(missing[2], 1 << (ORMA_EV_FLUX_WAKE - 128));
    shm.exit(b);
    assert_eq!(reader.slot(b).state, ORMA_THREAD_EXITED);

    drop(shm);
    assert!(!alive_lock_held(&reader.file));
    assert!(dir.join("orma.4021.771234").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
