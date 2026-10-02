use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fs,
    num::NonZeroU32,
    path::Path,
};

use armo_common::{
    CallbackKey, CallbackName, CallbackStat, KIND_FLUX_CHANNEL, LOST_NAME_FULL, LOST_STAT_FULL,
};
use object::{Object as _, ObjectSymbol as _};

use aya::{
    Ebpf, include_bytes_aligned,
    maps::{Array, HashMap as BpfHashMap, MapData},
    programs::{UProbe, uprobe::UProbeScope},
};

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// The event names and arguments the flux programs read. libflux_tracetools.so exports a symbol
/// of this name for exactly that set (flux/trace.h).
pub const FLUX_CONTRACT: &str = "flux_trace_contract_2";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lib {
    Ros,
    Flux,
}

const PROBES: [(&str, &str, Lib); 6] = [
    ("callback_start", "ros_trace_callback_start", Lib::Ros),
    ("callback_end", "ros_trace_callback_end", Lib::Ros),
    (
        "callback_register",
        "ros_trace_rclcpp_callback_register",
        Lib::Ros,
    ),
    (
        "flux_callback_start",
        "flux_trace_callback_start",
        Lib::Flux,
    ),
    ("flux_callback_end", "flux_trace_callback_end", Lib::Flux),
    ("flux_channel_init", "flux_trace_channel_init", Lib::Flux),
];

pub struct Probes {
    stats: BpfHashMap<MapData, CallbackKey, CallbackStat>,
    names: BpfHashMap<MapData, CallbackKey, CallbackName>,
    channels: BpfHashMap<MapData, CallbackKey, u64>,
    lost: Array<MapData, u64>,
    // Dropping this detaches the probes, so it lives as long as the maps are read.
    _ebpf: Ebpf,
}

#[derive(Default)]
pub struct Snapshot {
    pub stats: HashMap<CallbackKey, CallbackStat>,
    pub names: HashMap<CallbackKey, String>,
    /// The channel each flux callback last delivered from.
    pub channels: HashMap<CallbackKey, u64>,
    pub lost_stat: u64,
    pub lost_name: u64,
}

impl Probes {
    /// Attach to `tracetools`, the libtracetools.so every rclcpp process loads, and to
    /// `flux_tracetools`, flux's libflux_tracetools.so, when given. `pid` limits the probes to
    /// one process; without it they fire in every process that maps the library.
    pub fn attach(
        tracetools: &Path,
        flux_tracetools: Option<&Path>,
        pid: Option<NonZeroU32>,
    ) -> Result<Self> {
        if let Some(f) = flux_tracetools {
            check_flux_contract(f)?;
        }
        let mut ebpf = Ebpf::load(include_bytes_aligned!(concat!(env!("OUT_DIR"), "/armo")))?;
        let scope = match pid {
            Some(p) => UProbeScope::OneProcess(p),
            None => UProbeScope::AllProcesses,
        };
        for (program, symbol, lib) in PROBES {
            let target = match lib {
                Lib::Ros => tracetools,
                Lib::Flux => match flux_tracetools {
                    Some(f) => f,
                    None => continue,
                },
            };
            let p: &mut UProbe = ebpf
                .program_mut(program)
                .ok_or_else(|| format!("program {program} missing from the object"))?
                .try_into()?;
            p.load()?;
            p.attach(symbol, target, scope)
                .map_err(|e| format!("attach {symbol} in {}: {e}", target.display()))?;
        }
        let stats = take(&mut ebpf, "STATS")?.try_into()?;
        let names = take(&mut ebpf, "NAMES")?.try_into()?;
        let channels = take(&mut ebpf, "FLUX_CHANNELS")?.try_into()?;
        let lost = take(&mut ebpf, "LOST")?.try_into()?;
        Ok(Self {
            stats,
            names,
            channels,
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
        for entry in self.channels.iter() {
            let (k, v) = entry?;
            s.channels.insert(k, v);
        }
        s.lost_stat = self.lost.get(&LOST_STAT_FULL, 0)?;
        s.lost_name = self.lost.get(&LOST_NAME_FULL, 0)?;
        Ok(s)
    }

    /// Drop the entries of processes that have exited, so their slots are free for new ones, and
    /// the names of flux channels no callback delivers from: every channel a process opens or
    /// re-attaches adds one, publishers included. The owner file still names a dropped one.
    pub fn prune(&mut self, alive: impl Fn(u32) -> bool) -> Result<()> {
        let delivering: HashSet<(u32, u64)> = self
            .channels
            .iter()
            .filter_map(|e| e.ok())
            .map(|(k, channel)| (k.tgid, channel))
            .collect();
        let dead: Vec<CallbackKey> = self
            .stats
            .keys()
            .chain(self.names.keys())
            .chain(self.channels.keys())
            .filter_map(|k| k.ok())
            .filter(|k| {
                !alive(k.tgid)
                    || (k.kind == KIND_FLUX_CHANNEL && !delivering.contains(&(k.tgid, k.callback)))
            })
            .collect();
        for k in &dead {
            let _ = self.stats.remove(k);
            let _ = self.names.remove(k);
            let _ = self.channels.remove(k);
        }
        Ok(())
    }
}

/// A flux library of another version attaches by name all the same and hands the programs
/// arguments they misread, so it is refused up front.
fn check_flux_contract(lib: &Path) -> Result<()> {
    let bytes = fs::read(lib).map_err(|e| format!("read {}: {e}", lib.display()))?;
    let elf = object::File::parse(&*bytes).map_err(|e| format!("parse {}: {e}", lib.display()))?;
    if elf.dynamic_symbols().any(|s| s.name() == Ok(FLUX_CONTRACT)) {
        return Ok(());
    }
    Err(format!(
        "{} does not export {FLUX_CONTRACT}: its flux trace events are not the ones armod reads",
        lib.display()
    )
    .into())
}

fn take(ebpf: &mut Ebpf, name: &str) -> Result<aya::maps::Map> {
    Ok(ebpf
        .take_map(name)
        .ok_or_else(|| format!("map {name} missing from the object"))?)
}
