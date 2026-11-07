//! Golden messages (design 9, 12): each file reads into its type and writes back unchanged, the
//! committed schema and frames are what the code makes, and Python reads the same values.
//! `ORMA_BLESS=1` rewrites schema/v1.json and golden/frames instead of comparing.

use std::path::{Path, PathBuf};
use std::process::Command;

use orma_common::events::*;
use orma_proto::frame::{self, Kind};
use orma_proto::msg::*;
use orma_proto::records;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

const DIRS: &[(&str, &str)] = &[
    ("client", "ClientRequest"),
    ("server", "ServerMessage"),
    ("ormad_request", "OrmadRequest"),
    ("ormad", "OrmadMessage"),
    ("window_line", "WindowLine"),
    ("meta", "RecordMeta"),
];

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn bless() -> bool {
    std::env::var_os("ORMA_BLESS").is_some()
}

fn check_file(path: &Path, made: &[u8]) {
    if bless() {
        std::fs::write(path, made).unwrap();
        return;
    }
    let committed = std::fs::read(path).unwrap_or_default();
    assert!(
        committed == made,
        "{} is stale: run scripts/gen_headers.sh",
        path.display()
    );
}

fn goldens() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for (sub, _) in DIRS {
        let mut files: Vec<_> = std::fs::read_dir(dir().join("golden").join(sub))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        for path in files {
            let name = format!("{sub}/{}", path.file_name().unwrap().to_str().unwrap());
            out.push((name, path));
        }
    }
    out
}

fn reread<T: Serialize + DeserializeOwned>(value: &Value) -> Value {
    let typed: T = serde_json::from_value(value.clone()).unwrap();
    serde_json::to_value(&typed).unwrap()
}

/// The golden as Rust reads it, written back: sorted keys, no spaces.
fn rust_reads(name: &str, value: &Value) -> Value {
    match name.split('/').next().unwrap() {
        "client" => reread::<ClientRequest>(value),
        "server" => reread::<ServerMessage>(value),
        "ormad_request" => reread::<OrmadRequest>(value),
        "ormad" => reread::<OrmadMessage>(value),
        "window_line" => reread::<WindowLine>(value),
        "meta" => reread::<RecordMeta>(value),
        other => panic!("no type for golden/{other}"),
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn every_golden_round_trips() {
    let goldens = goldens();
    assert!(goldens.len() > 40);
    for (name, path) in goldens {
        let value = read_json(&path);
        assert_eq!(
            rust_reads(&name, &value),
            value,
            "{name} changes through its type"
        );
    }
}

#[test]
fn every_client_op_has_a_golden() {
    for op in CLIENT_OPS {
        let found = goldens()
            .iter()
            .any(|(name, path)| name.starts_with("client/") && read_json(path)["op"] == *op);
        assert!(found, "no golden for op {op}");
    }
}

#[test]
fn schema_is_generated() {
    let mut text = serde_json::to_string_pretty(&schema()).unwrap();
    text.push('\n');
    check_file(&dir().join("schema/v1.json"), text.as_bytes());
}

/// One record of each string count, a cut string, and a payload-less event.
fn golden_batch() -> Vec<u8> {
    let mut b = Vec::new();
    let node = orma_ev_rcl_node_init {
        node: 0x55a0_0010,
        rmw_node: 0x55a0_0020,
    };
    records::encode_positioned(
        &mut b,
        4021,
        4021,
        ORMA_EV_RCL_NODE_INIT,
        100,
        &node,
        &[b"driver", b"/camera"],
    );
    let take = orma_ev_rmw_take {
        rmw_subscription: 0x55a0_0100,
        message: 0x7f00_0000_1000,
        source_timestamp: 1_791_473_415_000_000_123,
        stamp: (500u64 << 32) | 1_791_473_415,
        taken: 1,
        pad: 0,
    };
    records::encode_positioned(&mut b, 4021, 4025, ORMA_EV_RMW_TAKE, 200, &take, &[]);
    records::encode_positioned(
        &mut b,
        4021,
        4025,
        ORMA_EV_RCLCPP_EXECUTOR_GET_NEXT_READY,
        250,
        &(),
        &[],
    );
    let ep = orma_ev_flux_endpoint_init {
        endpoint: 0x55b0_0000,
        fingerprint: 0x9f3a_0c11_d2e4_b5a6,
        role: ORMA_FLUX_ROLE_SUBSCRIPTION,
        lang: ORMA_FLUX_LANG_CPP,
    };
    let long = [b'k'; 300];
    records::encode_positioned(
        &mut b,
        4022,
        4030,
        ORMA_EV_FLUX_ENDPOINT_INIT,
        300,
        &ep,
        &[b"/camera/driver", &long, b"/flux.v6.image"],
    );
    b
}

#[test]
fn frames_are_generated() {
    let hello: ClientRequest =
        serde_json::from_value(read_json(&dir().join("golden/client/hello.json"))).unwrap();
    check_file(
        &dir().join("golden/frames/hello.bin"),
        &frame::encode_json(&hello),
    );
    check_file(
        &dir().join("golden/frames/events.bin"),
        &frame::encode(Kind::Events, &golden_batch()),
    );
}

/// Header fields and the body in hex: what a reader without the catalog sees.
fn record_lines(batch: &[u8]) -> String {
    records::positioned(batch)
        .map(|r| {
            let r = r.unwrap();
            let h = r.header;
            let body: String = r.body.iter().map(|b| format!("{b:02x}")).collect();
            format!(
                "{} {} {} {} {} {} {body}\n",
                r.pid, r.tid, h.event, h.len, h.flags, h.t_mono_ns
            )
        })
        .collect()
}

#[test]
fn golden_records_decode() {
    let batch = golden_batch();
    let lines = record_lines(&batch);
    assert_eq!(lines.lines().count(), 4);
    let mut records = records::positioned(&batch).map(Result::unwrap);
    let node = records.next().unwrap();
    assert_eq!(
        node.payload::<orma_ev_rcl_node_init>().rmw_node,
        0x55a0_0020
    );
    assert_eq!(
        node.strings().collect::<Vec<_>>(),
        [&b"driver"[..], b"/camera"]
    );
    let take = records.next().unwrap().payload::<orma_ev_rmw_take>();
    assert_eq!(take.stamp & 0xffff_ffff, 1_791_473_415);
    assert_eq!(records.next().unwrap().header.len, 16);
    let ep = records.next().unwrap();
    assert!(ep.truncated());
    assert_eq!(ep.strings().nth(1).unwrap().len(), ORMA_STRING_MAX - 1);
}

#[test]
fn bad_records_stop_the_run() {
    let batch = golden_batch();
    let mut cut = batch.clone();
    cut[8 + 2] = 9;
    let r: Vec<_> = records::positioned(&cut).collect();
    assert_eq!(r.len(), 1);
    assert!(r[0].as_ref().unwrap_err().reason.contains("len 9"));
    let mut unknown = batch.clone();
    unknown[8] = 250;
    assert!(records::positioned(&unknown).next().unwrap().is_err());
    let mut unterminated = batch;
    let first_len = u16::from_le_bytes([unterminated[10], unterminated[11]]) as usize;
    for b in &mut unterminated[8 + 16 + 16..8 + first_len] {
        *b = b'x';
    }
    assert!(records::positioned(&unterminated).next().unwrap().is_err());
}

fn canonical(v: &Value) -> String {
    serde_json::to_string(v).unwrap()
}

/// `test/read_golden.py` validates every golden against the schema and prints what it read; Rust
/// must have read the same.
#[test]
fn python_reads_the_same_values() {
    let out = Command::new("python3")
        .arg(dir().join("test/read_golden.py"))
        .arg(dir())
        .output()
        .expect("python3 runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut want = String::new();
    for (name, path) in goldens() {
        want += &format!(
            "{name}\t{}\n",
            canonical(&rust_reads(&name, &read_json(&path)))
        );
    }
    let hello =
        frame::read(&mut &std::fs::read(dir().join("golden/frames/hello.bin")).unwrap()[..]);
    let hello: Value = serde_json::from_slice(&hello.unwrap().unwrap().payload).unwrap();
    want += &format!("frames/hello.bin\t{}\n", canonical(&hello));
    let events = std::fs::read(dir().join("golden/frames/events.bin")).unwrap();
    let (events, _) = frame::decode(&events).unwrap().unwrap();
    want += &record_lines(&events.payload);
    assert_eq!(String::from_utf8(out.stdout).unwrap(), want);
}

#[test]
fn frames_split_and_refuse() {
    let bytes = frame::encode(Kind::Json, b"{}");
    assert_eq!(frame::decode(&bytes[..4]).unwrap(), None);
    assert_eq!(frame::decode(&bytes[..6]).unwrap(), None);
    let (f, used) = frame::decode(&bytes).unwrap().unwrap();
    assert_eq!((f.kind, &f.payload[..], used), (Kind::Json, &b"{}"[..], 7));
    let mut big = bytes.clone();
    big[..4].copy_from_slice(&(frame::MAX_LENGTH + 1).to_le_bytes());
    assert!(frame::decode(&big).is_err());
    let mut kind = bytes.clone();
    kind[4] = 3;
    assert!(frame::decode(&kind).is_err());
    assert!(frame::decode(&[0, 0, 0, 0, 1]).is_err());
    assert!(frame::read(&mut &bytes[..3]).is_err());
    assert_eq!(frame::read(&mut &[][..]).unwrap(), None);
}

#[test]
fn requests_fail_with_their_codes() {
    let unknown = parse_request(br#"{"op":"fly","id":22}"#).unwrap_err();
    assert_eq!(
        (unknown.id, unknown.code.as_str()),
        (Some(22), code::UNKNOWN_OP)
    );
    let bad = parse_request(br#"{"op":"record_stop","id":3}"#).unwrap_err();
    assert_eq!((bad.id, bad.code.as_str()), (Some(3), code::BAD_REQUEST));
    let junk = parse_request(b"{").unwrap_err();
    assert_eq!((junk.id, junk.code.as_str()), (None, code::BAD_REQUEST));
    let newer = parse_request(br#"{"op":"bye","id":4,"later_field":1}"#).unwrap();
    assert_eq!(newer.op, ClientOp::Bye);
    assert!(check_proto(1, [1, 7]).is_ok());
    assert_eq!(check_proto(1, [2, 0]).unwrap_err().code, code::PROTO_MAJOR);
}
