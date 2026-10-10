//! armo-collectd as processes: clients and armo-collectd killed with
//! SIGKILL, SIGTERM, and the guard. Child roles (a shim writer, a client) run this test binary
//! again with `ARMO_TEST_ROLE`.

mod collectd;

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::mem::offset_of;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use armo_collectd::guard::Cleanup;
use armo_collectd::record::show;
use armo_common::events::*;
use armo_common::shm::*;
use armo_proto::fake::shm::{FakeShm, Geometry};
use armo_proto::msg::*;
use armo_proto::records;
use collectd::*;

const EXE: &str = env!("CARGO_BIN_EXE_armo-collectd");

fn fake_lttng(dir: &Path) {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("lttng");
    std::fs::write(
        &script,
        "#!/bin/sh\n[ \"$1\" = --no-sessiond ] && shift\n\
         echo \"$LTTNG_HOME $*\" >> \"$ARMO_TEST_LTTNG_LOG\"\n\
         [ \"$1\" = list ] && [ -f \"$ARMO_TEST_LTTNG_LIST\" ] && cat \"$ARMO_TEST_LTTNG_LIST\"\n\
         exit 0\n",
    )
    .unwrap();
    // A real session daemon still starting up loses the SIGTERM that ends it.
    let sessiond = bin.join("lttng-sessiond");
    std::fs::write(&sessiond, "#!/bin/sh\nexec sleep infinity\n").unwrap();
    Command::new("chmod")
        .arg("+x")
        .arg(&script)
        .arg(&sessiond)
        .status()
        .unwrap();
}

fn lttng_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("lttng.log")).unwrap_or_default()
}

fn env(cmd: &mut Command, dir: &Path) {
    let path = format!(
        "{}:{}",
        dir.join("bin").display(),
        std::env::var("PATH").unwrap()
    );
    cmd.env("HOME", dir.join("home"))
        .env("XDG_RUNTIME_DIR", dir.join("xdg"))
        .env("ARMO_TEST_SHM_DIR", dir.join("shm"))
        .env("ARMO_TEST_LTTNG_LOG", dir.join("lttng.log"))
        .env("ARMO_TEST_LTTNG_LIST", dir.join("lttng.list"))
        .env("PATH", path);
}

fn setup(name: &str) -> Scratch {
    let dir = scratch(name);
    for d in ["home", "xdg", "shm"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    fake_lttng(&dir);
    dir
}

fn socket(dir: &Path) -> PathBuf {
    dir.join("xdg/armo/collectd.sock")
}

fn daemon(dir: &Path) -> Child {
    let _ = std::fs::remove_file(socket(dir));
    let mut cmd = Command::new(EXE);
    env(&mut cmd, dir);
    let child = cmd.spawn().unwrap();
    until(|| std::os::unix::net::UnixStream::connect(socket(dir)).is_ok());
    child
}

fn signal(pid: u32, sig: &str) {
    let ok = Command::new("kill")
        .args([sig, &pid.to_string()])
        .status()
        .unwrap();
    assert!(ok.success());
}

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .is_ok_and(|s| !s[s.rfind(')').unwrap() + 2..].starts_with('Z'))
}

/// The guard: armo-collectd's child that runs `--guard`.
fn guard_of(pid: u32) -> u32 {
    let mut found = None;
    until(|| {
        found = std::fs::read_dir("/proc").unwrap().flatten().find_map(|e| {
            let child: u32 = e.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
            let ppid: u32 = stat[stat.rfind(')')? + 2..]
                .split(' ')
                .nth(1)?
                .parse()
                .ok()?;
            (ppid == pid).then_some(child)
        });
        found.is_some()
    });
    found.unwrap()
}

/// Runs `role` in a copy of this test binary and returns it with the value it printed.
fn role(role: &str, vars: &[(&str, &Path)]) -> (Child, String) {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "child_role", "--nocapture", "--test-threads=1"])
        .env("ARMO_TEST_ROLE", role)
        .stdout(Stdio::piped());
    for (k, v) in vars {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let value = loop {
        let line = lines.next().expect("the role reports").unwrap();
        // The harness prints "test child_role ... " on the same line.
        if let Some((_, v)) = line.split_once("ROLE ") {
            break v.to_string();
        }
    };
    (child, value)
}

const SMALL: Geometry = Geometry {
    n_threads: 4,
    table_entries: 8,
    ring_bytes: 65536,
};

#[test]
fn child_role() {
    let Ok(role) = std::env::var("ARMO_TEST_ROLE") else {
        return;
    };
    let var = |k: &str| PathBuf::from(std::env::var_os(k).unwrap());
    // A role ends with the test that started it, even one that failed before killing it.
    let parent = std::os::unix::process::parent_id();
    let orphaned = || std::os::unix::process::parent_id() != parent;
    match role.as_str() {
        "writer" => {
            let pid = std::process::id();
            let shm =
                FakeShm::create(&var("ARMO_TEST_SHM_DIR"), pid, own_starttime(), 1, SMALL).unwrap();
            println!("ROLE {}", shm.path().display());
            let slot = shm.claim(pid).unwrap();
            let ev = armo_ev_handle { handle: 1 };
            let mut t = 0;
            while !orphaned() {
                shm.push(slot, ARMO_EV_CALLBACK_END, t, &ev, &[]);
                t += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        "client" => {
            let mut c = Client::connect(&var("ARMO_TEST_SOCKET"), "child");
            let (id, _) = c.start(options(&var("ARMO_TEST_REC")));
            println!("ROLE {id}");
            while !orphaned() {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        other => panic!("no role {other}"),
    }
}

fn armed(path: &Path) -> bool {
    let mut word = [0u8; 4];
    File::open(path)
        .unwrap()
        .read_exact_at(&mut word, offset_of!(armo_shm_header, armed) as u64)
        .unwrap();
    word != [0; 4]
}

fn targeting(pid: u32, dir: &Path) -> RecordOptions {
    RecordOptions {
        targets: Some(Targets {
            all: false,
            pids: vec![pid],
            node: None,
            topic: None,
            exclude: None,
        }),
        ..options(dir)
    }
}

#[test]
fn a_killed_client_leaves_its_recording_running() {
    let dir = scratch("killed_client");
    let server = Server::start(&dir, config(&dir));
    let rec = dir.join("rec");
    let (mut client, id) = role(
        "client",
        &[
            ("ARMO_TEST_SOCKET", &server.socket),
            ("ARMO_TEST_REC", &rec),
        ],
    );
    let child_pid = client.id();
    client.kill().unwrap();
    client.wait().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let mut c = server.client("rqt_armo");
    let r = c.recording(&id).unwrap();
    assert_eq!(r.state, RecordState::Recording);
    assert_eq!((r.owner.client.as_str(), r.owner.pid), ("child", child_pid));
    c.stop(&id);
}

#[test]
fn sigterm_finalizes_and_ends_the_guard() {
    let dir = setup("sigterm_proc");
    let mut d = daemon(&dir);
    let guard = guard_of(d.id());
    let mut c = Client::connect(&socket(&dir), "armo");
    let (_, rec) = c.start(options(&dir.join("rec")));
    signal(d.id(), "-TERM");
    assert!(d.wait().unwrap().success());
    let m = meta(&rec);
    assert_eq!(m.state, RecordState::Finalized);
    assert_eq!(m.end_reason, Some(EndReason::CollectorExit));
    until(|| !alive(guard));
    assert!(!lttng_log(&dir).contains("destroy"));
    assert!(!socket(&dir).exists());
}

#[test]
fn after_sigkill_the_guard_disarms_and_truncates() {
    let dir = setup("sigkill");
    let (mut writer, shm) = role("writer", &[("ARMO_TEST_SHM_DIR", &dir.join("shm"))]);
    let shm = PathBuf::from(shm);
    let mut d = daemon(&dir);
    let guard = guard_of(d.id());
    let mut c = Client::connect(&socket(&dir), "armo");
    let (id, rec) = c.start(targeting(writer.id(), &dir.join("rec")));
    until(|| armed(&shm));
    until(|| {
        c.recv();
        c.recording(&id).is_some_and(|r| r.totals.events > 0)
    });
    drop(c);

    d.kill().unwrap();
    let killed = Instant::now();
    d.wait().unwrap();
    until(|| !armed(&shm) && meta(&rec).state == RecordState::Truncated);
    assert!(
        killed.elapsed() < Duration::from_secs(2),
        "{:?}",
        killed.elapsed()
    );
    until(|| !alive(guard));
    let m = meta(&rec);
    assert_eq!(m.end_reason, Some(EndReason::CollectorExit));
    let key = format!(
        "p:{}:{}",
        writer.id(),
        armo_collectd::sys::starttime(writer.id()).unwrap()
    );
    assert!(m.counts[&key].events > 0);

    let mut d = daemon(&dir);
    let c = Client::connect(&socket(&dir), "armo");
    let r = c.recording(&id).expect("listed after the restart");
    assert_eq!(
        (r.state, r.end_reason),
        (RecordState::Truncated, Some(EndReason::CollectorExit))
    );
    signal(d.id(), "-TERM");
    d.wait().unwrap();
    writer.kill().unwrap();
    writer.wait().unwrap();
}

#[test]
fn with_the_guard_dead_too_the_next_start_cleans_up() {
    let dir = setup("sigkill_both");
    let me = format!("armo-{}-{}-1", std::process::id(), own_starttime());
    std::fs::write(
        dir.join("lttng.list"),
        format!("Recording sessions:\n  1) armo-999999-1-1 [inactive]\n  2) {me} [active]\n"),
    )
    .unwrap();
    let (mut writer, shm) = role("writer", &[("ARMO_TEST_SHM_DIR", &dir.join("shm"))]);
    let shm = PathBuf::from(shm);
    let mut d = daemon(&dir);
    let guard = guard_of(d.id());
    let mut c = Client::connect(&socket(&dir), "armo");
    let (id, rec) = c.start(targeting(writer.id(), &dir.join("rec")));
    until(|| armed(&shm));
    drop(c);
    signal(guard, "-KILL");
    until(|| !alive(guard));
    d.kill().unwrap();
    d.wait().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(armed(&shm), "nobody cleaned up");
    assert_eq!(meta(&rec).state, RecordState::Recording);

    let mut d = daemon(&dir);
    until(|| !armed(&shm));
    assert_eq!(meta(&rec).state, RecordState::Truncated);
    let log = lttng_log(&dir);
    assert!(log.contains("destroy armo-999999-1-1"), "{log}");
    assert!(!log.contains(&format!("destroy {me}")), "{log}");
    assert!(log.starts_with(&dir.join("xdg/armo/lttng").display().to_string()));
    let mut c = Client::connect(&socket(&dir), "armo");
    let shown = c.request(ClientOp::RecordShow {
        record_id: Some(id),
        dir: None,
    });
    let Ok(ReplyBody::RecordShow { table }) = shown else {
        panic!("{shown:?}")
    };
    assert_eq!(table.end_reason, Some(EndReason::CollectorExit));
    signal(d.id(), "-TERM");
    d.wait().unwrap();
    writer.kill().unwrap();
    writer.wait().unwrap();
}

/// A recording directory as a killed armo-collectd leaves it: `recording`, lock free, and
/// three records of pid 4021 in the event file.
fn abandoned(dir: &Path) -> PathBuf {
    let rec = dir.join("abandoned");
    std::fs::create_dir_all(rec.join("events")).unwrap();
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("../armo_proto/golden/meta/meta.json");
    let mut m: RecordMeta = serde_json::from_slice(&std::fs::read(golden).unwrap()).unwrap();
    m.state = RecordState::Recording;
    m.end_reason = None;
    m.ended = None;
    show::write_meta(&rec, &m).unwrap();
    File::create(rec.join("lock")).unwrap();
    let mut bytes = armo_common::bytes_of(&armo_evfile_header {
        magic: ARMO_EVFILE_MAGIC,
        version: ARMO_SHM_VERSION,
        pad: 0,
    })
    .to_vec();
    for t in 0..3 {
        let ev = armo_ev_handle { handle: 9 };
        let at = m.started.mono_ns + t * 1_000_000;
        records::encode_positioned(&mut bytes, 4021, 4022, ARMO_EV_CALLBACK_END, at, &ev, &[]);
    }
    bytes.extend_from_slice(&[1, 2, 3]);
    std::fs::write(rec.join("events/000.armoev"), bytes).unwrap();
    rec
}

#[test]
fn the_guard_cleans_the_last_list_at_end_of_input() {
    let dir = setup("guard");
    let shm = FakeShm::create(&dir.join("shm"), 4021, 1, 1, SMALL).unwrap();
    File::options()
        .write(true)
        .open(shm.path())
        .unwrap()
        .write_all_at(
            &1u32.to_le_bytes(),
            offset_of!(armo_shm_header, armed) as u64,
        )
        .unwrap();
    assert!(shm.armed());
    let rec = abandoned(&dir);
    let list = Cleanup {
        lttng_home: dir.join("lh"),
        sessions: vec!["armo-1-2-3".into()],
        armed: vec![shm.path().to_path_buf()],
        recordings: vec![rec.clone()],
    };
    let run = |lists: &[&Cleanup]| {
        let mut cmd = Command::new(EXE);
        env(&mut cmd, &dir);
        let mut g = cmd.arg("--guard").stdin(Stdio::piped()).spawn().unwrap();
        let mut stdin = g.stdin.take().unwrap();
        for l in lists {
            writeln!(stdin, "{}", serde_json::to_string(l).unwrap()).unwrap();
        }
        drop(stdin);
        assert!(g.wait().unwrap().success());
    };

    run(&[&list, &Cleanup::default()]);
    assert!(shm.armed());
    assert_eq!(meta(&rec).state, RecordState::Recording);
    assert!(lttng_log(&dir).is_empty());

    run(&[&Cleanup::default(), &list]);
    assert!(!shm.armed());
    assert_eq!(
        lttng_log(&dir).trim(),
        format!("{} destroy armo-1-2-3", dir.join("lh").display())
    );
    let m = meta(&rec);
    assert_eq!(m.state, RecordState::Truncated);
    assert_eq!(m.end_reason, Some(EndReason::CollectorExit));
    assert_eq!(m.counts["p:4021:0"].events, 3);
    let ended = m.ended.as_ref().unwrap();
    assert_eq!(ended.mono_ns, m.started.mono_ns + 2_000_000);
    assert_eq!(m.files, ["events/000.armoev"]);
    let table = show::table(&m, 0);
    assert_eq!(table.totals.events, 4200 + 3);
    assert_eq!(table.end_reason, Some(EndReason::CollectorExit));
}
