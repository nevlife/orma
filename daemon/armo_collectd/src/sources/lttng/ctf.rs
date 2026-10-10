//! CTF from libarmo_lttng.so back into armo records: babeltrace 1.5
//! prints the trace as text and this module parses the lines. Provider armo carries a record's
//! payload as words and its strings, provider flux2 the flux2 function's arguments by name; ros2
//! events are for ros2_tracing's tools and skipped, as armo has each of them as an armo event.

use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use armo_common::bytes_of;
use armo_common::catalog::{self, EventInfo};
use armo_common::events::{
    ARMO_RECORD_ALIGN, ARMO_RECORD_TRUNCATED, ARMO_STRING_MAX, armo_position, armo_record,
};

/// The payload of each flux2 event as its fields in payload order, with their byte widths.
/// `pad` is not a field of the LTTng event.
const FLUX2: &[(&str, &[(&str, usize)])] = &[
    (
        "channel_init",
        &[
            ("channel", 8),
            ("dev", 8),
            ("ino", 8),
            ("publisher", 4),
            ("slot_count", 4),
            ("slot_size", 4),
            ("stamp_offset", 4),
        ],
    ),
    (
        "claim",
        &[
            ("channel", 8),
            ("slot", 4),
            ("result", 4),
            ("loan", 4),
            ("pad", 4),
        ],
    ),
    ("loan", &[("channel", 8), ("slot", 4), ("pad", 4)]),
    ("loan_end", &[("channel", 8), ("slot", 4), ("result", 4)]),
    (
        "commit",
        &[
            ("channel", 8),
            ("ticket", 8),
            ("nbytes", 8),
            ("stamp", 8),
            ("slot", 4),
            ("woke", 4),
        ],
    ),
    (
        "take",
        &[
            ("channel", 8),
            ("ticket", 8),
            ("lost", 8),
            ("stamp", 8),
            ("slot", 4),
            ("pad", 4),
        ],
    ),
    (
        "release",
        &[("channel", 8), ("ticket", 8), ("held", 4), ("pad", 4)],
    ),
    ("refused", &[("channel", 8), ("reason", 4), ("pad", 4)]),
    (
        "wait",
        &[
            ("executor", 8),
            ("timeout_ns", 8),
            ("sources", 4),
            ("pad", 4),
        ],
    ),
    ("wake", &[("executor", 8), ("result", 4), ("entry", 4)]),
    (
        "callback_start",
        &[("executor", 8), ("endpoint", 8), ("entry", 4), ("pad", 4)],
    ),
    (
        "callback_end",
        &[
            ("executor", 8),
            ("channel", 8),
            ("entry", 4),
            ("delivered", 4),
        ],
    ),
    (
        "endpoint_init",
        &[
            ("endpoint", 8),
            ("fingerprint", 8),
            ("role", 4),
            ("lang", 4),
        ],
    ),
    ("py_gil", &[("phase", 4), ("pad", 4)]),
    ("py_ready", &[("channel", 8), ("ticket", 8)]),
    (
        "record_drop",
        &[("channel", 8), ("ticket", 8), ("count", 4), ("reason", 4)],
    ),
];

/// The string fields of each flux2 event with strings, in record order.
const FLUX2_STRINGS: &[(&str, &[&str])] = &[
    ("channel_init", &["segment"]),
    ("callback_end", &["segment"]),
    ("endpoint_init", &["node", "key", "signpost"]),
];

pub fn flux2_fields(event: &str) -> Option<&'static [(&'static str, usize)]> {
    FLUX2.iter().find(|(n, _)| *n == event).map(|(_, f)| *f)
}

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Int(u64),
    Str(String),
    Words(Vec<u64>),
}

/// What one trace held.
#[derive(Debug, Default)]
pub struct Trace {
    /// Positioned records, in babeltrace's order: by time across streams.
    pub records: Vec<u8>,
    pub events: u64,
    /// Events of other providers (ros2).
    pub skipped: u64,
    /// `(pid, armo event)` of each `armo:missing`.
    pub missing: Vec<(u32, u16)>,
}

fn integer(text: &str) -> Option<u64> {
    if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else if text.starts_with('-') {
        text.parse::<i64>().ok().map(|v| v as u64)
    } else {
        text.parse().ok()
    }
}

/// `key = value` pairs of every `{ ... }` group after the event name, flattened: the packet
/// context (`cpu_id`), the event context (`vpid`, `vtid`) and the payload.
fn fields(mut s: &str) -> Result<Vec<(&str, Value)>, String> {
    let mut out = Vec::new();
    loop {
        s = s.trim_start_matches([' ', ',', '{', '}']);
        if s.is_empty() {
            return Ok(out);
        }
        let (key, rest) = s
            .split_once(" = ")
            .ok_or_else(|| format!("no `=` in {s:?}"))?;
        s = rest;
        let value = if let Some(rest) = s.strip_prefix('"') {
            // babeltrace 1.5 prints a string between quotes without escaping it.
            let end = rest
                .match_indices('"')
                .map(|(i, _)| i)
                .find(|i| matches!(rest[i + 1..].chars().next(), None | Some(',' | ' ')))
                .ok_or("unterminated string")?;
            s = &rest[end + 1..];
            Value::Str(rest[..end].to_string())
        } else if let Some(rest) = s.strip_prefix("[ ]") {
            s = rest;
            Value::Words(Vec::new())
        } else if let Some(rest) = s.strip_prefix("[ ") {
            let end = rest.find(" ]").ok_or("unterminated array")?;
            let words = rest[..end]
                .split(", ")
                .filter(|e| !e.is_empty())
                .map(|e| {
                    e.split_once(" = ")
                        .and_then(|(_, v)| integer(v))
                        .ok_or_else(|| format!("array element {e:?}"))
                })
                .collect::<Result<Vec<u64>, String>>()?;
            s = &rest[end + 2..];
            Value::Words(words)
        } else {
            let end = s.find([',', ' ']).unwrap_or(s.len());
            let v = integer(&s[..end]).ok_or_else(|| format!("integer {:?}", &s[..end]))?;
            s = &s[end..];
            Value::Int(v)
        };
        out.push((key, value));
    }
}

fn get<'a>(f: &'a [(&str, Value)], key: &str) -> Option<&'a Value> {
    f.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

fn int(f: &[(&str, Value)], key: &str) -> Result<u64, String> {
    match get(f, key) {
        Some(Value::Int(v)) => Ok(*v),
        _ => Err(format!("no integer field {key}")),
    }
}

fn string<'a>(f: &'a [(&str, Value)], key: &str) -> Result<&'a str, String> {
    match get(f, key) {
        Some(Value::Str(v)) => Ok(v),
        _ => Err(format!("no string field {key}")),
    }
}

/// Appends one positioned record with a raw payload, as `armo_proto::records::encode` does.
fn push(
    out: &mut Vec<u8>,
    pid: u32,
    tid: u32,
    info: &EventInfo,
    t_mono_ns: u64,
    payload: &[u8],
    strings: &[&str],
) {
    out.extend_from_slice(bytes_of(&armo_position { pid, tid }));
    let start = out.len();
    out.extend_from_slice(&[0; size_of::<armo_record>()]);
    out.extend_from_slice(payload);
    let mut flags = 0;
    for s in strings {
        let s = s.as_bytes();
        let s = &s[..s.iter().position(|b| *b == 0).unwrap_or(s.len())];
        let keep = s.len().min(ARMO_STRING_MAX - 1);
        if keep < s.len() {
            flags |= ARMO_RECORD_TRUNCATED;
        }
        out.extend_from_slice(&s[..keep]);
        out.push(0);
    }
    out.resize(
        start + (out.len() - start).next_multiple_of(ARMO_RECORD_ALIGN),
        0,
    );
    let header = armo_record {
        event: info.id,
        len: (out.len() - start) as u16,
        flags,
        t_mono_ns,
    };
    out[start..start + size_of::<armo_record>()].copy_from_slice(bytes_of(&header));
}

/// One line of `babeltrace --clock-cycles --no-delta -n payload,context` into `trace`.
pub fn line(text: &str, trace: &mut Trace) -> Result<(), String> {
    let t: u64 = text
        .strip_prefix('[')
        .and_then(|r| r.split_once(']'))
        .and_then(|(t, _)| t.parse().ok())
        .ok_or("no [cycles] timestamp")?;
    let mut rest = text;
    let name = loop {
        let (w, r) = rest.split_once(' ').ok_or("no provider:event")?;
        rest = r;
        if let Some(n) = w.strip_suffix(':')
            && n.contains(':')
        {
            break n;
        }
    };
    let (provider, event) = name.split_once(':').expect("checked");
    trace.events += 1;
    if provider != "armo" && provider != "flux2" {
        trace.skipped += 1;
        return Ok(());
    }
    let f = fields(rest)?;
    let pid = int(&f, "vpid")? as u32;
    let tid = int(&f, "vtid")? as u32;
    if provider == "armo" && event == "missing" {
        trace.missing.push((pid, int(&f, "event")? as u16));
        return Ok(());
    }
    let full = if provider == "flux2" {
        format!("flux2_{event}")
    } else {
        event.to_string()
    };
    let info = catalog::CATALOG
        .iter()
        .find(|i| i.name == full && i.emitted)
        .ok_or_else(|| format!("{name} is not an armo event"))?;
    let mut payload = Vec::with_capacity(info.size);
    let mut strings = Vec::new();
    if provider == "armo" {
        if info.size > 0 {
            let Some(Value::Words(words)) = get(&f, "p") else {
                return Err(format!("{name} has no payload words"));
            };
            if words.len() * 8 != info.size {
                return Err(format!(
                    "{name}: {} words for {} bytes",
                    words.len(),
                    info.size
                ));
            }
            for w in words {
                payload.extend_from_slice(&w.to_le_bytes());
            }
        }
        for i in 0..info.strings {
            strings.push(string(&f, &format!("s{i}"))?);
        }
    } else {
        for (field, width) in flux2_fields(event).ok_or_else(|| format!("{name}: no table"))? {
            let v = if *field == "pad" { 0 } else { int(&f, field)? };
            payload.extend_from_slice(&v.to_le_bytes()[..*width]);
        }
        let names = FLUX2_STRINGS
            .iter()
            .find(|(n, _)| *n == event)
            .map_or(&[][..], |(_, s)| *s);
        for s in names {
            strings.push(string(&f, s)?);
        }
    }
    push(&mut trace.records, pid, tid, info, t, &payload, &strings);
    Ok(())
}

/// Reads the trace (or the traces under it, merged by time) at `path` with babeltrace.
/// Ends babeltrace and returns what it read so far once `stop` is set.
pub fn read_trace(path: &Path, stop: &AtomicBool) -> io::Result<Trace> {
    let mut child = Command::new("babeltrace")
        .args(["--clock-cycles", "--no-delta", "-n", "payload,context"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stderr = child.stderr.take().expect("piped");
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let mut trace = Trace::default();
    let mut bad = None;
    for l in BufReader::new(child.stdout.take().expect("piped")).lines() {
        if stop.load(Ordering::Relaxed) {
            let _ = child.kill();
            break;
        }
        let l = l?;
        if let Err(e) = line(&l, &mut trace)
            && bad.is_none()
        {
            bad = Some(format!("{e}: {l}"));
        }
    }
    let status = child.wait()?;
    let err = errors.join().unwrap_or_default();
    if let Some(e) = bad {
        return Err(io::Error::new(io::ErrorKind::InvalidData, e));
    }
    // babeltrace fails on a directory that holds no trace yet: an empty chunk is not an error.
    if !status.success() && trace.events > 0 {
        return Err(io::Error::other(format!(
            "babeltrace {}: {}",
            path.display(),
            err.lines().find(|l| l.contains("[error]")).unwrap_or("")
        )));
    }
    Ok(trace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flux2_tables_fill_their_payloads() {
        for (event, fields) in FLUX2 {
            let info = catalog::CATALOG
                .iter()
                .find(|i| i.name == format!("flux2_{event}"))
                .unwrap();
            let size: usize = fields.iter().map(|(_, w)| w).sum();
            assert_eq!(size, info.size, "{event}");
            let strings = FLUX2_STRINGS
                .iter()
                .find(|(n, _)| n == event)
                .map_or(0, |(_, s)| s.len());
            assert_eq!(strings, info.strings as usize, "{event}");
        }
        let flux2 = catalog::CATALOG
            .iter()
            .filter(|i| i.name.starts_with("flux2_"))
            .count();
        assert_eq!(flux2, FLUX2.len());
    }

    #[test]
    fn parses_babeltrace_lines() {
        let mut t = Trace::default();
        let lines = [
            "[00000881243177375715] localhost.localdomain armo:rcl_node_init: { cpu_id = 9 }, { vpid = 7, vtid = 8 }, { _p_length = 2, p = [ [0] = 0xAAAB0910A830, [1] = 0x2 ], s0 = \"talker\", s1 = \"/\" }",
            "[5] h flux2:commit: { cpu_id = 1 }, { vpid = 7, vtid = 9 }, { channel = 0x10, ticket = 3, slot = 1, nbytes = 64, woke = -1, stamp = 0 }",
            "[6] h ros2:callback_end: { cpu_id = 1 }, { vpid = 7, vtid = 9 }, { callback = 0x10 }",
            "[7] h armo:rclcpp_executor_get_next_ready: { cpu_id = 1 }, { vpid = 7, vtid = 9 }, { _p_length = 0, p = [ ] }",
            "[8] h armo:missing: { cpu_id = 1 }, { vpid = 7, vtid = 9 }, { event = 34 }",
        ];
        for l in lines {
            line(l, &mut t).unwrap();
        }
        assert_eq!(
            (t.events, t.skipped, t.missing.clone()),
            (5, 1, vec![(7, 34)])
        );
        let records: Vec<_> = armo_proto::records::positioned(&t.records)
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(records.len(), 3);
        assert_eq!((records[0].pid, records[0].tid), (7, 8));
        assert_eq!(records[0].header.t_mono_ns, 881243177375715);
        let node: armo_common::events::armo_ev_rcl_node_init = records[0].payload();
        assert_eq!((node.node, node.rmw_node), (0xAAAB0910A830, 2));
        let names: Vec<&[u8]> = records[0].strings().collect();
        assert_eq!(names, [&b"talker"[..], b"/"]);
        let c: armo_common::events::armo_ev_flux2_commit = records[1].payload();
        assert_eq!(
            (c.channel, c.ticket, c.slot, c.nbytes, c.woke),
            (0x10, 3, 1, 64, -1)
        );
    }
}
