//! CSV export of a recording's windows. One row per window, entity and
//! field: `t_mono_ns,t_real_ns,metric,id,field,value`, with the field names of the window message
//! (`duration.p99_ns` for a latency bundle) and an empty value for `null`. The meta goes next
//! to the CSV as the sidecar.

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
            let entities: Vec<&Value> = match &v["snap"][*metric] {
                Value::Array(list) => list.iter().collect(),
                Value::Null => Vec::new(),
                one => vec![one],
            };
            for entity in entities {
                let id = entity["id"].as_str().unwrap_or(metric);
                let mut fields = Vec::new();
                leaves("", entity, &mut fields);
                for (field, value) in fields.iter().filter(|(f, _)| f != "id") {
                    write(format!(
                        "{t_mono},{t_real},{metric},{},{},{}\n",
                        cell(id),
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
