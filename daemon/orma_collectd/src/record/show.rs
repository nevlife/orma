//! A recording's directory read back: `meta.json`, marking one orma-collectd never finalized as
//! truncated, and the verification table.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use orma_common::events::{ORMA_EVFILE_MAGIC, orma_evfile_header, orma_position, orma_record};
use orma_common::read;
use orma_proto::msg::*;
use orma_proto::records;

use super::{LOCK, META, WINDOWS};

const SEC: f64 = 1e9;

pub fn read_meta(dir: &Path) -> io::Result<RecordMeta> {
    let text = std::fs::read(dir.join(META))?;
    serde_json::from_slice(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Through a synced `meta.json.tmp` and a rename, so neither a reader nor a crash leaves half a
/// file.
pub fn write_meta(dir: &Path, meta: &RecordMeta) -> io::Result<()> {
    let tmp = dir.join("meta.json.tmp");
    let mut text = serde_json::to_vec_pretty(meta).expect("meta serializes");
    text.push(b'\n');
    let mut file = File::create(&tmp)?;
    file.write_all(&text)?;
    file.sync_all()?;
    std::fs::rename(tmp, dir.join(META))
}

fn locked(dir: &Path) -> io::Result<bool> {
    match File::open(dir.join(LOCK))?.try_lock() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// The meta of the recording in `dir`, first marked truncated if it says `recording` while no
/// one holds its lock. Counts after the last meta write come from the event files.
pub fn recover(dir: &Path) -> io::Result<RecordMeta> {
    let mut meta = read_meta(dir)?;
    if meta.state != RecordState::Recording || locked(dir)? {
        return Ok(meta);
    }
    let (files, bytes) = list_files(dir)?;
    let (per_pid, last_ns) = recount(dir, &files)?;
    let mut by_pid: BTreeMap<u32, String> = BTreeMap::new();
    for key in meta.counts.keys() {
        if let Some(pid) = key.split(':').nth(1).and_then(|p| p.parse().ok()) {
            by_pid.insert(pid, key.clone());
        }
    }
    for (pid, events) in per_pid {
        let key = by_pid.remove(&pid).unwrap_or_else(|| super::entity(pid, 0));
        meta.counts
            .entry(key)
            .or_insert(Counts { events: 0, lost: 0 })
            .events = events;
    }
    meta.ended = last_ns.map(|mono_ns| Instant {
        mono_ns,
        real_ns: meta.started.real_ns + (mono_ns - meta.started.mono_ns),
    });
    meta.state = RecordState::Truncated;
    meta.end_reason = Some(EndReason::CollectorExit);
    meta.end_detail = Some("orma-collectd ended without finalizing; event counts recounted".into());
    meta.files = files.iter().map(|f| f.display().to_string()).collect();
    meta.bytes = bytes;
    write_meta(dir, &meta)?;
    Ok(meta)
}

/// The recording's data files relative to `dir`, and their size.
fn list_files(dir: &Path) -> io::Result<(Vec<PathBuf>, u64)> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir.join("events")) {
        for e in entries {
            files.push(Path::new("events").join(e?.file_name()));
        }
    }
    files.sort();
    if dir.join(WINDOWS).exists() {
        files.push(PathBuf::from(WINDOWS));
    }
    let mut bytes = 0;
    for f in &files {
        bytes += std::fs::metadata(dir.join(f))?.len();
    }
    Ok((files, bytes))
}

/// Records per pid in the event files and the last record's time. A file ends at its first
/// record that is cut off or does not check, as the last one of a killed writer is.
fn recount(dir: &Path, files: &[PathBuf]) -> io::Result<(BTreeMap<u32, u64>, Option<u64>)> {
    let mut per_pid = BTreeMap::new();
    let mut last = None;
    for f in files.iter().filter(|f| f.starts_with("events")) {
        let file = BufReader::new(File::open(dir.join(f))?);
        let mut input: Box<dyn Read> = if f.extension().is_some_and(|e| e == "zst") {
            Box::new(zstd::Decoder::with_buffer(file)?)
        } else {
            Box::new(file)
        };
        let mut head = [0u8; size_of::<orma_evfile_header>()];
        if input.read_exact(&mut head).is_err()
            || read::<orma_evfile_header>(&head).map(|h| h.magic) != Some(ORMA_EVFILE_MAGIC)
        {
            continue;
        }
        let fixed = size_of::<orma_position>() + size_of::<orma_record>();
        let mut record = vec![0u8; fixed];
        loop {
            record.resize(fixed, 0);
            if input.read_exact(&mut record).is_err() {
                break;
            }
            let header: orma_record = read(&record[size_of::<orma_position>()..]).unwrap();
            let len = header.len as usize;
            if len < size_of::<orma_record>() {
                break;
            }
            record.resize(size_of::<orma_position>() + len, 0);
            if input.read_exact(&mut record[fixed..]).is_err()
                || records::check(&record[size_of::<orma_position>()..], 0).is_err()
            {
                break;
            }
            let pos: orma_position = read(&record).unwrap();
            *per_pid.entry(pos.pid).or_insert(0) += 1;
            last = last.max(Some(header.t_mono_ns));
        }
    }
    Ok((per_pid, last))
}

/// The verification table, from the meta. `now_ns` measures a recording still going.
pub fn table(meta: &RecordMeta, now_ns: u64) -> VerifyTable {
    let end = meta.ended.as_ref().map_or(now_ns, |e| e.mono_ns);
    let duration_s = end.saturating_sub(meta.started.mono_ns) as f64 / SEC;
    let entities = meta
        .counts
        .iter()
        .map(|(id, c)| VerifyRow {
            id: id.clone(),
            events: c.events,
            lost: c.lost,
            hz: (duration_s > 0.0).then(|| c.events as f64 / duration_s),
        })
        .collect();
    let mut warnings = Vec::new();
    let targets = meta.options.targets.as_ref();
    for pid in targets.map(|t| t.pids.as_slice()).unwrap_or_default() {
        let prefix = format!("p:{pid}:");
        let events: u64 = meta
            .counts
            .iter()
            .filter(|(id, _)| id.starts_with(&prefix))
            .map(|(_, c)| c.events)
            .sum();
        if events == 0 {
            warnings.push(Warning {
                kind: "empty_target".into(),
                text: format!("target pid {pid} has no events"),
                ids: Vec::new(),
            });
        }
    }
    for m in &meta.probes_missing {
        warnings.push(Warning {
            kind: "probe_missing".into(),
            text: format!("{} ({:?}): {}", m.point, m.method, m.reason),
            ids: meta
                .counts
                .keys()
                .filter(|id| m.pid.is_some_and(|p| id.starts_with(&format!("p:{p}:"))))
                .cloned()
                .collect(),
        });
    }
    for g in &meta.gaps {
        warnings.push(Warning {
            kind: "gap".into(),
            text: format!(
                "{} missing for {:.3} s from mono {} ns: {}",
                g.source,
                (g.to_ns - g.from_ns) as f64 / SEC,
                g.from_ns,
                g.reason
            ),
            ids: Vec::new(),
        });
    }
    VerifyTable {
        entities,
        warnings,
        totals: VerifyTotals {
            events: meta.counts.values().map(|c| c.events).sum(),
            lost: meta.counts.values().map(|c| c.lost).sum(),
            duration_s,
            bytes: meta.bytes,
        },
        end_reason: meta.end_reason,
        end_detail: meta.end_detail.clone(),
    }
}
