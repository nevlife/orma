//! The LD_PRELOAD source against `FakeShm`: arming, rings into the model and into
//! recordings, table differences, losses, and files of dead writers.

mod collectd;

use std::fs::File;
use std::mem::offset_of;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::time::{Duration, Instant};

use armo_common::events::*;
use armo_common::shm::*;
use armo_proto::fake::shm::{FakeShm, Geometry};
use armo_proto::msg::*;
use armo_proto::records;
use collectd::*;

const SMALL: Geometry = Geometry {
    n_threads: 4,
    table_entries: 8,
    ring_bytes: 4096,
};

fn writer(dir: &Path) -> FakeShm {
    std::fs::create_dir_all(dir.join("shm")).unwrap();
    FakeShm::create(
        &dir.join("shm"),
        std::process::id(),
        own_starttime(),
        ARMO_BACKEND_SHM,
        SMALL,
    )
    .unwrap()
}

fn header_u32(path: &Path, field: usize) -> u32 {
    let mut word = [0u8; 4];
    File::open(path)
        .unwrap()
        .read_exact_at(&mut word, field as u64)
        .unwrap();
    u32::from_le_bytes(word)
}

fn armed(shm: &FakeShm) -> bool {
    header_u32(shm.path(), offset_of!(armo_shm_header, armed)) != 0
}

fn callback_start(callback: u64) -> armo_ev_callback_start {
    armo_ev_callback_start {
        callback,
        is_intra_process: 0,
        pad: 0,
    }
}

fn entity() -> String {
    format!("p:{}:{}", std::process::id(), own_starttime())
}

fn pid_only(dir: &Path, method: RecordMethod) -> RecordOptions {
    RecordOptions {
        method: Some(method),
        targets: Some(Targets {
            all: false,
            pids: vec![std::process::id()],
            node: None,
            topic: None,
            exclude: None,
        }),
        ..options(dir)
    }
}

#[test]
fn armed_follows_clients_selection_and_recordings() {
    let dir = scratch("arm");
    let shm = writer(&dir);
    let server = Server::start(&dir, config(&dir));
    std::thread::sleep(Duration::from_millis(300));
    assert!(!armed(&shm), "nobody watches yet");

    let mut gui = server.client("rqt_armo");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!armed(&shm), "connected but watching no windows");
    gui.request(ClientOp::Subscribe {
        streams: vec![Stream::Window],
    })
    .unwrap();
    until(|| armed(&shm));
    assert_eq!(gui.state.as_ref().unwrap().sources.preload_processes, 1);
    gui.request(ClientOp::Configure(Configure {
        window_s: None,
        bandwidth: None,
        sched: None,
        malloc: Some(true),
        pss: None,
        expected_hz: None,
        gpu: None,
        dds: None,
        futex: None,
        chains: None,
        latest_nodes: None,
    }))
    .unwrap();
    let options = || header_u32(shm.path(), offset_of!(armo_shm_header, options));
    until(|| options() == ARMO_OPTION_MALLOC);
    gui.request(ClientOp::Configure(Configure {
        window_s: None,
        bandwidth: None,
        sched: None,
        malloc: None,
        pss: None,
        expected_hz: None,
        gpu: Some(true),
        dds: Some(true),
        futex: None,
        chains: None,
        latest_nodes: None,
    }))
    .unwrap();
    until(|| options() == ARMO_OPTION_MALLOC | ARMO_OPTION_GPU | ARMO_OPTION_DDS);
    gui.request(ClientOp::Select(Select {
        selection: Selection {
            all: false,
            pids: vec![1],
            ..Default::default()
        },
        method_by_pid: Default::default(),
    }))
    .unwrap();
    until(|| !armed(&shm));
    gui.request(ClientOp::Select(Select {
        selection: Selection {
            all: true,
            pids: Vec::new(),
            ..Default::default()
        },
        method_by_pid: Default::default(),
    }))
    .unwrap();
    until(|| armed(&shm));
    drop(gui);
    until(|| !armed(&shm));

    let mut cli = server.client("armo");
    let (id, _) = cli.start(pid_only(&dir, RecordMethod::Preload));
    drop(cli);
    std::thread::sleep(Duration::from_millis(300));
    assert!(armed(&shm), "the recording keeps it armed");
    let mut cli = server.client("armo");
    cli.stop(&id);
    drop(cli);
    until(|| !armed(&shm));
    assert!(header_u32(shm.path(), offset_of!(armo_shm_header, reader_gen)) >= 3);
}

#[test]
fn records_reach_the_model_and_the_recording() {
    let dir = scratch("rings");
    let shm = writer(&dir);
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    let (id, rec) = c.start(pid_only(&dir, RecordMethod::Auto));
    until(|| armed(&shm));
    let slot = shm.claim(4321).unwrap();
    for i in 0..50 {
        assert!(shm.push(
            slot,
            ARMO_EV_CALLBACK_START,
            1000 + i,
            &callback_start(7),
            &[]
        ));
        if i % 10 == 9 {
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    let preload = |s: &Seen| {
        s.records
            .iter()
            .filter(|(m, p, e)| {
                *m == Method::Preload && *p == std::process::id() && *e == ARMO_EV_CALLBACK_START
            })
            .count()
    };
    until(|| preload(&server.seen.0.lock().unwrap()) == 50);

    for v in [1000, 3000, 2000] {
        assert!(shm.add(slot, ARMO_AGG_CALLBACK, ARMO_SRC_ROS, 0, 7, v));
    }
    let rows = |s: &Seen| -> Vec<Aggregate> {
        s.aggregates
            .iter()
            .filter(|(m, a)| *m == Method::Preload && a.key == 7)
            .map(|(_, a)| a.clone())
            .collect()
    };
    until(|| !rows(&server.seen.0.lock().unwrap()).is_empty());
    assert!(shm.add(slot, ARMO_AGG_CALLBACK, ARMO_SRC_ROS, 0, 7, 500));
    until(|| rows(&server.seen.0.lock().unwrap()).len() == 2);
    let r = rows(&server.seen.0.lock().unwrap());
    assert_eq!(
        (r[0].tid, r[0].kind, r[0].count, r[0].sum, r[0].max),
        (4321, ARMO_AGG_CALLBACK, 3, 6000, 3000)
    );
    assert_eq!(
        (r[1].count, r[1].sum, r[1].max),
        (1, 500, 500),
        "a window's own max"
    );
    assert_eq!(r[1].hist.iter().sum::<u64>(), 1);

    c.stop(&id);
    let m = meta(&rec);
    assert_eq!(
        m.methods_by_pid[&std::process::id().to_string()],
        Method::Preload
    );
    assert_eq!(m.counts[&entity()].events, 50);
    let bytes = std::fs::read(rec.join("events/000.armoev")).unwrap();
    let in_file: Vec<_> = records::positioned(&bytes[16..])
        .map(Result::unwrap)
        .collect();
    assert_eq!(in_file.len(), 50);
    assert!(in_file.iter().all(|r| r.tid == 4321));
    assert_eq!(in_file[49].header.t_mono_ns, 1049);
}

#[test]
fn init_records_from_before_a_recording_open_it() {
    let dir = scratch("init");
    let shm = writer(&dir);
    let slot = shm.claim(4321).unwrap();
    let node = armo_ev_rcl_node_init {
        node: 0x10,
        rmw_node: 0x11,
    };
    assert!(shm.push(slot, ARMO_EV_RCL_NODE_INIT, 900, &node, &[b"talker", b"/"]));
    assert!(!shm.push(slot, ARMO_EV_CALLBACK_START, 901, &callback_start(7), &[]));
    let server = Server::start(&dir, config(&dir));
    let init = |s: &Seen| s.records.iter().any(|r| r.2 == ARMO_EV_RCL_NODE_INIT);
    until(|| init(&server.seen.0.lock().unwrap()));

    let mut c = server.client("armo");
    let (id, rec) = c.start(pid_only(&dir, RecordMethod::Preload));
    until(|| armed(&shm));
    assert!(shm.push(slot, ARMO_EV_CALLBACK_START, 1000, &callback_start(7), &[]));
    let started = |s: &Seen| s.records.iter().any(|r| r.2 == ARMO_EV_CALLBACK_START);
    until(|| started(&server.seen.0.lock().unwrap()));
    c.stop(&id);
    let bytes = std::fs::read(rec.join("events/000.armoev")).unwrap();
    let events: Vec<u16> = records::positioned(&bytes[16..])
        .map(|r| r.unwrap().header.event)
        .collect();
    assert_eq!(events, [ARMO_EV_RCL_NODE_INIT, ARMO_EV_CALLBACK_START]);
    assert_eq!(meta(&rec).counts[&entity()].events, 2);
}

#[test]
fn table_rows_anywhere_and_a_reused_slot_from_zero() {
    let dir = scratch("tables");
    let shm = writer(&dir);
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    let (id, _) = c.start(pid_only(&dir, RecordMethod::Preload));
    until(|| armed(&shm));
    let rows = |s: &Seen, tid: u32| -> Vec<(u64, u64)> {
        let mut r: Vec<_> = s
            .aggregates
            .iter()
            .filter(|(m, a)| *m == Method::Preload && a.tid == tid)
            .map(|(_, a)| (a.key, a.count))
            .collect();
        r.sort();
        r
    };

    let slot = shm.claim(11).unwrap();
    for key in 1..=6 {
        for _ in 0..3 {
            assert!(shm.add(slot, ARMO_AGG_CALLBACK, ARMO_SRC_ROS, 0, key, 100));
        }
    }
    let want: Vec<(u64, u64)> = (1..=6).map(|k| (k, 3)).collect();
    until(|| rows(&server.seen.0.lock().unwrap(), 11) == want);

    shm.exit(slot);
    until(|| shm.claim(12) == Some(slot));
    for _ in 0..5 {
        assert!(shm.add(slot, ARMO_AGG_CALLBACK, ARMO_SRC_ROS, 0, 1, 100));
    }
    until(|| !rows(&server.seen.0.lock().unwrap(), 12).is_empty());
    assert_eq!(rows(&server.seen.0.lock().unwrap(), 12), vec![(1, 5)]);
    c.stop(&id);
}

fn process(id: &str, pid: u32) -> Process {
    Process {
        id: id.into(),
        pid,
        ns_pid: None,
        comm: "talker".into(),
        ros: None,
        methods_available: vec![Method::Preload],
        method: Some(Method::Preload),
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
        cuda: None,
        gpu_mem_bytes: None,
    }
}

#[test]
fn overflow_counts_reach_the_window_and_outlive_a_reused_slot() {
    let dir = scratch("overflow");
    let shm = writer(&dir);
    let server = Server::start(&dir, config(&dir));
    server.seen.0.lock().unwrap().processes =
        vec![process(&entity(), std::process::id()), process("p:1:1", 1)];
    let mut c = server.client("rqt_armo");
    c.request(ClientOp::Subscribe {
        streams: vec![Stream::Window],
    })
    .unwrap();
    let mut shows = |threads, table, depth| {
        let want = Some(Overflow {
            threads,
            table,
            depth,
        });
        let start = Instant::now();
        let ServerMessage::Window(w) = c.until(|m| {
            assert!(start.elapsed() < WAIT, "no window with {want:?}");
            matches!(m, ServerMessage::Window(w) if w.processes[0].overflow == want)
        }) else {
            unreachable!()
        };
        assert_eq!(w.processes[1].overflow, None, "not a preload process");
    };
    shows(0, 0, 0);

    let slots: Vec<usize> = (0..4).map(|tid| shm.claim(tid).unwrap()).collect();
    assert!(shm.claim(9).is_none());
    let taken = (1..=SMALL.table_entries as u64 + 2)
        .filter(|key| shm.add(slots[0], ARMO_AGG_CALLBACK, ARMO_SRC_ROS, 0, *key, 100))
        .count();
    assert_eq!(taken, SMALL.table_entries as usize);
    shm.overflow_depth(slots[1]);
    shm.overflow_depth(slots[1]);
    shows(1, 2, 2);

    shm.exit(slots[1]);
    // Each claim before the reader frees the slot is one more thread without a slot.
    let mut refused = 0;
    until(|| {
        let slot = shm.claim(20);
        refused += u64::from(slot.is_none());
        slot == Some(slots[1])
    });
    shm.overflow_depth(slots[1]);
    shows(1 + refused, 2, 3);
}

#[test]
fn ring_drops_count_as_lost() {
    let dir = scratch("drops");
    let shm = writer(&dir);
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    c.request(ClientOp::Subscribe {
        streams: vec![Stream::Window, Stream::EventLine],
    })
    .unwrap();
    let (id, rec) = c.start(pid_only(&dir, RecordMethod::Preload));
    until(|| armed(&shm));
    let slot = shm.claim(99).unwrap();
    let pushed = (0..2000)
        .filter(|i| shm.push(slot, ARMO_EV_CALLBACK_START, *i, &callback_start(1), &[]))
        .count() as u64;
    assert!(pushed < 2000);
    c.until(|m| matches!(m, ServerMessage::EventLine(l) if l.kind == "loss"));
    c.stop(&id);
    let counts = &meta(&rec).counts[&entity()];
    assert_eq!(counts.events, pushed);
    assert_eq!(counts.lost, 2000 - pushed);
    assert_eq!(meta(&rec).drops.shm_ring, 2000 - pushed);
}

#[test]
fn exited_slots_are_freed_and_dead_files_removed() {
    let dir = scratch("dead");
    std::fs::create_dir_all(dir.join("shm")).unwrap();
    let stale = FakeShm::create(&dir.join("shm"), 999_999, 1, ARMO_BACKEND_SHM, SMALL).unwrap();
    let stale_path = stale.path().to_path_buf();
    drop(stale);
    let shm = writer(&dir);
    shm.set_missing(ARMO_EV_RCL_WAIT_ENTER);
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    c.request(ClientOp::Subscribe {
        streams: vec![Stream::Window, Stream::EventLine],
    })
    .unwrap();
    until(|| !stale_path.exists());

    let checks = c.request(ClientOp::Selfcheck { duration_s: 0.0 });
    let Ok(ReplyBody::Selfcheck { checks }) = checks else {
        panic!("{checks:?}")
    };
    let intercept = checks
        .iter()
        .find(|k| k.item == format!("intercept {}", entity()))
        .unwrap();
    assert_eq!(intercept.status, CheckStatus::Fail);
    assert!(intercept.detail.contains("rcl_wait_enter"));

    let slots: Vec<usize> = (0..4).map(|tid| shm.claim(tid).unwrap()).collect();
    assert!(shm.claim(9).is_none());
    shm.exit(slots[0]);
    until(|| shm.claim(10) == Some(slots[0]));

    let path = shm.path().to_path_buf();
    drop(shm);
    c.until(|m| matches!(m, ServerMessage::EventLine(l) if l.kind == "process_exit"));
    assert!(!path.exists());
    until(|| {
        c.state.as_ref().unwrap().sources.preload_processes == 0 || {
            c.recv();
            false
        }
    });
}

#[test]
fn a_damaged_ring_stops_that_file_only() {
    let dir = scratch("damaged");
    let shm = writer(&dir);
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    c.request(ClientOp::Subscribe {
        streams: vec![Stream::Window, Stream::EventLine],
    })
    .unwrap();
    until(|| armed(&shm));
    let slot = shm.claim(5).unwrap();
    let head =
        ARMO_SHM_HEADER_SIZE + slot * SMALL.thread_size() + offset_of!(armo_thread_slot, ring_head);
    File::options()
        .write(true)
        .open(shm.path())
        .unwrap()
        .write_all_at(&(1u64 << 40).to_le_bytes(), head as u64)
        .unwrap();
    c.until(|m| matches!(m, ServerMessage::EventLine(l) if l.kind == "shm_corrupt"));
    until(|| !armed(&shm));
}
