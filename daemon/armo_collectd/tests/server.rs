//! armo-collectd's client protocol and recording lifecycle, in process.

mod collectd;

use std::path::Path;
use std::time::{Duration, Instant};

use armo_collectd::server::Exit;
use armo_proto::frame;
use armo_proto::msg::*;
use collectd::*;

fn golden_requests() -> Vec<(String, ClientRequest)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../common/armo_proto/golden/client");
    let mut all: Vec<(String, ClientRequest)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| {
            let text = std::fs::read(&p).unwrap();
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            (name, parse_request(&text).unwrap())
        })
        .collect();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    all
}

#[test]
fn hello_first_then_every_op_is_answered() {
    let dir = scratch("protocol");
    let server = Server::start(&dir, config(&dir));

    let mut early = Client::raw(&server.socket);
    let id = early.send(ClientOp::RecordList);
    assert_eq!(early.answer(id).unwrap_err().code, code::BAD_REQUEST);
    frame::write(
        &mut early.stream,
        frame::Kind::Json,
        br#"{"op":"fly","id":4}"#,
    )
    .unwrap();
    assert_eq!(early.answer(4).unwrap_err().code, code::UNKNOWN_OP);

    let mut old = Client::raw(&server.socket);
    old.send(ClientOp::Hello(Hello {
        proto: [PROTO[0] + 1, 0],
        client: "old".into(),
        pid: 1,
    }));
    let ServerMessage::Error(e) = old.recv() else {
        panic!("no error")
    };
    assert_eq!(e.code, code::PROTO_MAJOR);
    assert!(frame::read(&mut old.stream).unwrap().is_none(), "closed");

    let mut c = server.client("golden");
    let mut answered = 0;
    for (name, request) in golden_requests() {
        if matches!(request.op, ClientOp::Hello(_) | ClientOp::Bye) {
            continue;
        }
        let id = c.send(request.op);
        // Paths in the goldens (/home/u) do not exist here: an error is still an answer.
        let _ = c.answer(id);
        answered += 1;
        assert!(!name.is_empty());
    }
    assert_eq!(answered, golden_requests().len() - 2);
    c.send(ClientOp::Bye);
    assert!(frame::read(&mut c.stream).unwrap().is_none(), "bye closes");
}

#[test]
fn select_and_configure_reach_every_client() {
    let dir = scratch("select");
    let server = Server::start(&dir, config(&dir));
    let mut gui = server.client("rqt_armo");
    let mut cli = server.client("armo");
    let mut methods = std::collections::BTreeMap::new();
    methods.insert("4021".to_string(), Method::Uprobe);
    let select = Select {
        selection: Selection {
            all: false,
            pids: vec![4021, 4022],
            ..Default::default()
        },
        method_by_pid: methods,
    };
    cli.request(ClientOp::Select(select)).unwrap();
    let s = gui.until(|m| matches!(m, ServerMessage::State(s) if !s.monitor.selection.all));
    let ServerMessage::State(s) = s else { panic!() };
    assert_eq!(s.monitor.selection.pids, [4021, 4022]);
    assert_eq!(s.monitor.method_by_pid["4021"], Method::Uprobe);

    let configure = Configure {
        window_s: Some(0.5),
        bandwidth: None,
        sched: Some(false),
        malloc: None,
        expected_hz: None,
    };
    gui.request(ClientOp::Configure(configure)).unwrap();
    cli.until(|m| matches!(m, ServerMessage::State(s) if s.monitor.window_s == 0.5));
    assert!(!cli.state.as_ref().unwrap().monitor.sched);

    let bad = Configure {
        window_s: Some(0.0),
        bandwidth: None,
        sched: None,
        malloc: None,
        expected_hz: None,
    };
    assert_eq!(
        gui.request(ClientOp::Configure(bad)).unwrap_err().code,
        code::BAD_REQUEST
    );

    gui.request(ClientOp::Subscribe {
        streams: vec![Stream::Window],
    })
    .unwrap();
    gui.until(|m| matches!(m, ServerMessage::Window(w) if !w.replay));
    let detail = gui.request(ClientOp::Detail {
        entity: "p:1:2".into(),
        what: DetailWhat::Hist,
    });
    assert!(matches!(detail, Ok(ReplyBody::Detail(_))));
}

#[test]
fn a_recording_outlives_its_client_and_any_client_stops_it() {
    let dir = scratch("outlive");
    let server = Server::start(&dir, config(&dir));
    let records = dir.join("rec");

    let mut gui = server.client("rqt_armo");
    let (id, rec_dir) = gui.start(options(&records));
    assert!(rec_dir.starts_with(&records));
    drop(gui);
    std::thread::sleep(Duration::from_millis(300));

    let mut cli = server.client("armo");
    let r = cli
        .recording(&id)
        .expect("the new client sees the recording");
    assert_eq!(r.state, RecordState::Recording);
    assert_eq!(r.owner.client, "rqt_armo");
    assert_eq!(r.owner.pid, std::process::id());
    assert_eq!(r.limits.max_duration_s, 1800);
    assert_eq!(r.limits.max_bytes, 8 << 30);
    assert!(r.limits.disk_floor_bytes >= 2 << 30);

    let Ok(ReplyBody::Recordings { recordings }) = cli.request(ClientOp::RecordList) else {
        panic!("no list")
    };
    assert_eq!(recordings.len(), 1);

    cli.stop(&id);
    let r = cli.recording(&id).unwrap();
    assert_eq!(r.state, RecordState::Finalized);
    assert_eq!(r.end_reason, Some(EndReason::UserStop));
    let m = meta(&rec_dir);
    assert_eq!(m.state, RecordState::Finalized);
    assert_eq!(m.end_reason, Some(EndReason::UserStop));
    assert_eq!(m.owner.client, "rqt_armo");
    assert!(m.ended.is_some());
    assert_eq!(m.files, ["events/000.armoev", "windows.jsonl"]);
    let events = std::fs::read(rec_dir.join("events/000.armoev")).unwrap();
    assert_eq!(&events[..8], b"ARMOEV1\0");
    assert!(!rec_dir.join("meta.json.tmp").exists());

    let shown = cli.request(ClientOp::RecordShow {
        record_id: Some(id.clone()),
        dir: None,
    });
    let Ok(ReplyBody::RecordShow { table }) = shown else {
        panic!("{shown:?}")
    };
    assert_eq!(table.end_reason, Some(EndReason::UserStop));
    assert_eq!(
        cli.request(ClientOp::RecordStop { record_id: id })
            .unwrap_err()
            .code,
        code::BAD_REQUEST
    );
}

#[test]
fn at_most_four_recordings() {
    let dir = scratch("four");
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    let ids: Vec<String> = (0..4)
        .map(|_| c.start(options(&dir.join("rec"))).0)
        .collect();
    let fifth = c.request(ClientOp::RecordStart(options(&dir.join("rec"))));
    assert!(fifth.unwrap_err().message.contains("4 recordings"));
    c.stop(&ids[0]);
    c.start(options(&dir.join("rec")));
}

#[test]
fn limits_always_exist() {
    let dir = scratch("nolimit");
    let mut cfg = config(&dir);
    cfg.caps.max_duration_s = 60;
    let server = Server::start(&dir, cfg);
    let mut c = server.client("armo");
    for o in [
        RecordOptions {
            max_duration_s: Some(0),
            ..options(&dir)
        },
        RecordOptions {
            max_bytes: Some(0),
            ..options(&dir)
        },
        RecordOptions {
            max_duration_s: Some(61),
            ..options(&dir)
        },
        RecordOptions {
            max_bytes: Some(u64::MAX),
            ..options(&dir)
        },
        RecordOptions {
            method: Some(RecordMethod::Lttng),
            ..options(&dir)
        },
    ] {
        let e = c.request(ClientOp::RecordStart(o)).unwrap_err();
        assert_eq!(e.code, code::BAD_REQUEST, "{}", e.message);
    }
    let named = RecordOptions {
        name: Some("run1".into()),
        kind: Some(RecordKind::Windows),
        ..options(&dir)
    };
    let (id, rec) = c.start(named.clone());
    assert_eq!(rec, dir.join("run1"));
    assert_eq!(c.recording(&id).unwrap().limits.max_duration_s, 60);
    assert!(
        c.request(ClientOp::RecordStart(named)).is_err(),
        "same name"
    );
}

#[test]
fn duration_and_size_limits_end_recordings() {
    let dir = scratch("limits");
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    c.request(ClientOp::Subscribe {
        streams: vec![Stream::EventLine],
    })
    .unwrap();
    c.request(ClientOp::Configure(Configure {
        window_s: Some(0.05),
        bandwidth: None,
        sched: None,
        malloc: None,
        expected_hz: None,
    }))
    .unwrap();

    let started = Instant::now();
    let (short, short_dir) = c.start(RecordOptions {
        max_duration_s: Some(1),
        ..options(&dir)
    });
    let (small, small_dir) = c.start(RecordOptions {
        kind: Some(RecordKind::Windows),
        max_bytes: Some(4000),
        ..options(&dir)
    });
    let r = c.ended(&small);
    assert_eq!(r.end_reason, Some(EndReason::MaxSize));
    assert!(r.bytes >= 4000);
    let r = c.ended(&short);
    let took = started.elapsed();
    assert_eq!(r.end_reason, Some(EndReason::MaxDuration));
    assert!(
        took >= Duration::from_secs(1) && took < Duration::from_millis(1500),
        "{took:?}"
    );
    assert_eq!(meta(&short_dir).end_reason, Some(EndReason::MaxDuration));
    assert_eq!(meta(&small_dir).end_reason, Some(EndReason::MaxSize));
    let limit_lines = c.lines.iter().filter(|l| l.kind == "record_limit").count();
    assert_eq!(limit_lines, 2);
    let lines = std::fs::read_to_string(small_dir.join("windows.jsonl")).unwrap();
    assert!(lines.lines().count() >= 2);
    for line in lines.lines() {
        let w: WindowLine = serde_json::from_str(line).unwrap();
        assert!(!w.snap.replay);
    }
}

#[test]
fn idle_ends_only_without_clients_and_recordings() {
    let dir = scratch("idle");
    let mut cfg = config(&dir);
    cfg.idle = Duration::from_millis(300);
    let mut server = Server::start(&dir, cfg);
    let mut c = server.client("armo");
    let (id, _) = c.start(RecordOptions {
        kind: Some(RecordKind::Windows),
        ..options(&dir)
    });
    drop(c);
    std::thread::sleep(Duration::from_millis(800));
    assert!(!server.finished(), "a recording keeps it");
    let mut c = server.client("armo");
    c.stop(&id);
    std::thread::sleep(Duration::from_millis(600));
    assert!(!server.finished(), "a client keeps it");
    drop(c);
    until(|| server.finished());
    assert_eq!(server.join(), Exit::Idle);
    assert!(!dir.join("run/collectd.sock").exists());
}

#[test]
fn a_second_instance_ends_at_once() {
    let dir = scratch("twice");
    let _first = Server::start(&dir, config(&dir));
    let stop = std::sync::atomic::AtomicBool::new(false);
    let second = armo_collectd::server::run(config(&dir), Box::new(Counting::default()), &stop);
    assert_eq!(second.unwrap(), Exit::Running);
}

#[test]
fn stop_signal_finalizes_with_collector_exit() {
    let dir = scratch("sigterm");
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    let (_, rec) = c.start(options(&dir));
    assert_eq!(server.stop(), Exit::Signal);
    let m = meta(&rec);
    assert_eq!(m.state, RecordState::Finalized);
    assert_eq!(m.end_reason, Some(EndReason::CollectorExit));
    assert!(
        std::fs::read_to_string(dir.join("data/active"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn replay_and_export_read_windows() {
    let dir = scratch("replay");
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    c.request(ClientOp::Configure(Configure {
        window_s: Some(0.05),
        bandwidth: None,
        sched: None,
        malloc: None,
        expected_hz: None,
    }))
    .unwrap();
    let (id, rec) = c.start(RecordOptions {
        kind: Some(RecordKind::Windows),
        ..options(&dir)
    });
    std::thread::sleep(Duration::from_millis(500));
    c.stop(&id);
    let frames = std::fs::read_to_string(rec.join("windows.jsonl"))
        .unwrap()
        .lines()
        .count();
    assert!(frames >= 5, "{frames}");

    let opened = c.request(ClientOp::ReplayOpen {
        dir: rec.display().to_string(),
    });
    let Ok(ReplyBody::ReplayOpened { replay_id }) = opened else {
        panic!("{opened:?}")
    };
    c.windows.clear();
    let before = Instant::now();
    while c.windows.len() < frames {
        c.recv();
    }
    assert!(c.windows.iter().all(|w| w.replay && w.hists.is_none()));
    assert!(before.elapsed() >= Duration::from_millis(150), "paced");
    let seqs: Vec<u64> = c.windows.iter().map(|w| w.seq).collect();
    assert!(seqs.windows(2).all(|p| p[0] < p[1]));

    c.request(ClientOp::ReplayPlay {
        replay_id: replay_id.clone(),
        rate: 4.0,
    })
    .unwrap();
    c.request(ClientOp::ReplayPause {
        replay_id: replay_id.clone(),
    })
    .unwrap();
    c.windows.clear();
    c.request(ClientOp::ReplaySeek {
        replay_id: replay_id.clone(),
        frame: 2,
    })
    .unwrap();
    c.until(|m| matches!(m, ServerMessage::Window(_)));
    assert_eq!(c.windows[0].seq, seqs[2]);
    assert!(
        c.request(ClientOp::ReplaySeek {
            replay_id: replay_id.clone(),
            frame: 10_000,
        })
        .is_err()
    );
    c.request(ClientOp::ReplayClose {
        replay_id: replay_id.clone(),
    })
    .unwrap();
    assert!(c.request(ClientOp::ReplayPause { replay_id }).is_err());

    let out = dir.join("run1.csv");
    c.request(ClientOp::Export {
        dir: rec.display().to_string(),
        out: out.display().to_string(),
        metrics: vec!["agent".into()],
    })
    .unwrap();
    let csv = std::fs::read_to_string(&out).unwrap();
    let mut rows = csv.lines();
    assert_eq!(
        rows.next(),
        Some("t_mono_ns,t_real_ns,metric,id,field,value")
    );
    assert!(rows.any(|r| r.contains(",agent,agent,collectd.rss_bytes,")));
    assert!(csv.contains(",agent,agent,armod,\n"), "null is empty");
    assert!(dir.join("run1.meta.json").exists());
    let bad = c.request(ClientOp::Export {
        dir: rec.display().to_string(),
        out: out.display().to_string(),
        metrics: vec!["nonsense".into()],
    });
    assert!(bad.is_err());
}

#[test]
fn a_slow_client_is_dropped_not_waited_for() {
    let dir = scratch("slow");
    let server = Server::start(&dir, config(&dir));
    let mut slow = server.client("slow");
    slow.request(ClientOp::Subscribe {
        streams: vec![Stream::Window, Stream::EventLine],
    })
    .unwrap();
    let mut c = server.client("armo");
    c.request(ClientOp::Configure(Configure {
        window_s: Some(0.01),
        bandwidth: None,
        sched: None,
        malloc: None,
        expected_hz: None,
    }))
    .unwrap();
    let (id, _) = c.start(RecordOptions {
        kind: Some(RecordKind::Windows),
        ..options(&dir)
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline && server.seen.0.lock().unwrap().windows < 800 {
        c.recv();
    }
    c.stop(&id);
    let end = loop {
        match frame::read(&mut slow.stream) {
            Ok(Some(_)) => continue,
            end => break end,
        }
    };
    assert!(matches!(end, Ok(None)), "dropped, not timed out: {end:?}");
}

#[test]
fn out_of_range_values_are_refused_and_the_server_lives_on() {
    let dir = scratch("ranges");
    let server = Server::start(&dir, config(&dir));
    let mut c = server.client("armo");
    let window = |w| {
        ClientOp::Configure(Configure {
            window_s: Some(w),
            bandwidth: None,
            sched: None,
            malloc: None,
            expected_hz: None,
        })
    };
    assert_eq!(
        c.request(window(1e300)).unwrap_err().code,
        code::BAD_REQUEST
    );
    for bad in [
        RecordOptions {
            cache_bytes: Some(1 << 60),
            ..options(&dir)
        },
        RecordOptions {
            split_duration_s: Some(u64::MAX),
            ..options(&dir)
        },
    ] {
        assert!(c.request(ClientOp::RecordStart(bad)).is_err());
    }

    c.request(window(0.05)).unwrap();
    let (id, rec) = c.start(RecordOptions {
        kind: Some(RecordKind::Windows),
        ..options(&dir)
    });
    std::thread::sleep(Duration::from_millis(300));
    c.stop(&id);
    let opened = c.request(ClientOp::ReplayOpen {
        dir: rec.display().to_string(),
    });
    let Ok(ReplyBody::ReplayOpened { replay_id }) = opened else {
        panic!("{opened:?}")
    };
    c.request(ClientOp::ReplayPlay {
        replay_id: replay_id.clone(),
        rate: 1e-300,
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    c.windows.clear();
    c.request(ClientOp::ReplaySeek {
        replay_id,
        frame: 1,
    })
    .unwrap();
    c.until(|m| matches!(m, ServerMessage::Window(_)));
    c.request(window(1.0)).unwrap();
}
