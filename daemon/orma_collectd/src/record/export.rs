//! CSV export of a recording's windows. One row per window, entity and
//! field: `t_mono_ns,t_real_ns,metric,id,field,value`, with the field names of the window message
//! (`duration.p99_ns` for a latency bundle) and an empty value for `null`. The meta goes next
//! to the CSV as the sidecar. A chain's id is its name and its steps are `steps.<k>.<field>`;
//! `host` has a row per CPU, zone, counter, interface and GPU (`cpu:<n>`, `thermal:<name>`,
//! `throttle:<name>`, `net:<name>`, `udp`, `gpu:<index>`).

use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use serde_json::Value;

use super::{META, WINDOWS};

pub const METRICS: &[&str] = &[
    "processes",
    "threads",
    "nodes",
    "executors",
    "callbacks",
    "topics",
    "subscriptions",
    "flows",
    "spans",
    "chains",
    "host",
    "agent",
];

fn cell(text: &str) -> String {
    if text.contains([',', '"', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// Scalar leaves of `value` under `prefix`; lists of ids and other arrays are left out.
fn leaves(prefix: &str, value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let name = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                leaves(&name, v, out);
            }
        }
        Value::Array(_) => {}
        Value::Null => out.push((prefix.into(), String::new())),
        Value::String(s) => out.push((prefix.into(), s.clone())),
        v => out.push((prefix.into(), v.to_string())),
    }
}

/// The host's rows: id, then its scalar leaves.
fn host_rows(host: &Value) -> Vec<(String, Vec<(String, String)>)> {
    // The field the id already carries is left out.
    let row = |id: String, v: &Value, in_id: &str| {
        let mut fields = Vec::new();
        leaves("", v, &mut fields);
        fields.retain(|(f, _)| f != in_id);
        (id, fields)
    };
    let list = |key: &str| host[key].as_array().cloned().unwrap_or_default();
    let mut rows = Vec::new();
    for (n, mhz) in list("cpu_mhz").iter().enumerate() {
        rows.push(row(
            format!("cpu:{n}"),
            &serde_json::json!({ "mhz": mhz }),
            "",
        ));
    }
    for t in &list("thermal") {
        rows.push(row(
            format!("thermal:{}", t["name"].as_str().unwrap_or("")),
            t,
            "name",
        ));
    }
    for (name, count) in host["throttle"].as_object().into_iter().flatten() {
        let counter = serde_json::json!({ "count": count });
        rows.push(row(format!("throttle:{name}"), &counter, ""));
    }
    for i in &list("net") {
        rows.push(row(
            format!("net:{}", i["name"].as_str().unwrap_or("")),
            i,
            "name",
        ));
    }
    if host["udp"].is_object() {
        rows.push(row("udp".into(), &host["udp"], ""));
    }
    for g in &list("gpus") {
        let (id, mut fields) = row(format!("gpu:{}", g["index"]), g, "index");
        let throttle: Vec<&str> = g["throttle"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        fields.push(("throttle".into(), throttle.join("|")));
        rows.push((id, fields));
    }
    rows
}

/// A window entity's id and scalar leaves; a chain's steps are by index.
fn entity_row(metric: &str, entity: &Value) -> (String, Vec<(String, String)>) {
    let id = entity["id"]
        .as_str()
        .or_else(|| entity["name"].as_str().filter(|_| metric == "chains"))
        .unwrap_or(metric)
        .to_string();
    let mut fields = Vec::new();
    leaves("", entity, &mut fields);
    if metric == "chains" {
        for (k, step) in entity["steps"].as_array().into_iter().flatten().enumerate() {
            leaves(&format!("steps.{k}"), step, &mut fields);
        }
    }
    fields.retain(|(f, _)| f != "id");
    (id, fields)
}

/// Writes the CSV of `metrics` (all when empty) and returns the rows written.
pub fn export(dir: &Path, out: &Path, metrics: &[String]) -> Result<u64, String> {
    if let Some(bad) = metrics.iter().find(|m| !METRICS.contains(&m.as_str())) {
        return Err(format!("unknown metric {bad:?}, not one of {METRICS:?}"));
    }
    let chosen: Vec<&str> = if metrics.is_empty() {
        METRICS.to_vec()
    } else {
        metrics.iter().map(String::as_str).collect()
    };
    let io_err = |p: &Path, e: io::Error| format!("{}: {e}", p.display());
    let windows = dir.join(WINDOWS);
    let input = BufReader::new(File::open(&windows).map_err(|e| io_err(&windows, e))?);
    let mut csv = BufWriter::new(File::create(out).map_err(|e| io_err(out, e))?);
    let mut rows = 0;
    let mut write = |line: String| csv.write_all(line.as_bytes()).map_err(|e| io_err(out, e));
    write("t_mono_ns,t_real_ns,metric,id,field,value\n".into())?;
    for line in input.lines() {
        let line = line.map_err(|e| io_err(&windows, e))?;
        let v: Value =
            serde_json::from_str(&line).map_err(|e| format!("{}: {e}", windows.display()))?;
        let (t_mono, t_real) = (&v["t_mono_ns"], &v["t_real_ns"]);
        for metric in &chosen {
            let snap = &v["snap"][*metric];
            let entities = match (snap, *metric) {
                (Value::Null, _) => Vec::new(),
                (host, "host") => host_rows(host),
                (Value::Array(list), _) => list.iter().map(|e| entity_row(metric, e)).collect(),
                (one, _) => vec![entity_row(metric, one)],
            };
            for (id, fields) in entities {
                for (field, value) in &fields {
                    write(format!(
                        "{t_mono},{t_real},{metric},{},{},{}\n",
                        cell(&id),
                        cell(field),
                        cell(value)
                    ))?;
                    rows += 1;
                }
            }
        }
    }
    csv.flush().map_err(|e| io_err(out, e))?;
    let sidecar = out.with_extension("meta.json");
    std::fs::copy(dir.join(META), &sidecar).map_err(|e| io_err(&sidecar, e))?;
    Ok(rows)
}
