use std::{collections::HashMap, error::Error, num::NonZeroU32, path::Path};

use armo_common::{CallbackKey, CallbackName, CallbackStat, LOST_NAME_FULL, LOST_STAT_FULL};
use aya::{
    Ebpf, include_bytes_aligned,
    maps::{Array, HashMap as BpfHashMap, MapData},
    programs::{UProbe, uprobe::UProbeScope},
};

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

const PROBES: [(&str, &str); 3] = [
    ("callback_start", "ros_trace_callback_start"),
    ("callback_end", "ros_trace_callback_end"),
    ("callback_register", "ros_trace_rclcpp_callback_register"),
];

pub struct Probes {
    stats: BpfHashMap<MapData, CallbackKey, CallbackStat>,
    names: BpfHashMap<MapData, CallbackKey, CallbackName>,
    lost: Array<MapData, u64>,
    // Dropping this detaches the probes, so it lives as long as the maps are read.
    _ebpf: Ebpf,
}

#[derive(Default)]
pub struct Snapshot {
    pub stats: HashMap<CallbackKey, CallbackStat>,
    pub names: HashMap<CallbackKey, String>,
    pub lost_stat: u64,
    pub lost_name: u64,
}

impl Probes {
    /// Attach to `tracetools`, the libtracetools.so every rclcpp process loads. `pid` limits the
    /// probes to one process; without it they fire in every process that maps the library.
    pub fn attach(tracetools: &Path, pid: Option<NonZeroU32>) -> Result<Self> {
        let mut ebpf = Ebpf::load(include_bytes_aligned!(concat!(env!("OUT_DIR"), "/armo")))?;
        let scope = match pid {
            Some(p) => UProbeScope::OneProcess(p),
            None => UProbeScope::AllProcesses,
        };
        for (program, symbol) in PROBES {
            let p: &mut UProbe = ebpf
                .program_mut(program)
                .ok_or_else(|| format!("program {program} missing from the object"))?
                .try_into()?;
            p.load()?;
            p.attach(symbol, tracetools, scope)
                .map_err(|e| format!("attach {symbol} in {}: {e}", tracetools.display()))?;
        }
        let stats = take(&mut ebpf, "STATS")?.try_into()?;
        let names = take(&mut ebpf, "NAMES")?.try_into()?;
        let lost = take(&mut ebpf, "LOST")?.try_into()?;
        Ok(Self {
            stats,
            names,
            lost,
            _ebpf: ebpf,
        })
    }

    pub fn read(&self) -> Result<Snapshot> {
        let mut s = Snapshot::default();
        for entry in self.stats.iter() {
            let (k, v) = entry?;
            s.stats.insert(k, v);
        }
        for entry in self.names.iter() {
            let (k, v) = entry?;
            let len = v
                .bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(v.bytes.len());
            s.names
                .insert(k, String::from_utf8_lossy(&v.bytes[..len]).into_owned());
        }
        s.lost_stat = self.lost.get(&LOST_STAT_FULL, 0)?;
        s.lost_name = self.lost.get(&LOST_NAME_FULL, 0)?;
        Ok(s)
    }

    /// Drop the entries of processes that have exited, so their slots are free for new ones.
    pub fn prune(&mut self, alive: impl Fn(u32) -> bool) -> Result<()> {
        let dead: Vec<CallbackKey> = self
            .stats
            .keys()
            .chain(self.names.keys())
            .filter_map(|k| k.ok())
            .filter(|k| !alive(k.tgid))
            .collect();
        for k in &dead {
            let _ = self.stats.remove(k);
            let _ = self.names.remove(k);
        }
        Ok(())
    }
}

fn take(ebpf: &mut Ebpf, name: &str) -> Result<aya::maps::Map> {
    Ok(ebpf
        .take_map(name)
        .ok_or_else(|| format!("map {name} missing from the object"))?)
}
