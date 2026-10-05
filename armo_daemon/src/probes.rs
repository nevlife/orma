use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fs,
    num::NonZeroU32,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
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
/// The same for flux2, whose events live in libflux2_ffi.so (flux2_core/src/trace.rs).
pub const FLUX2_CONTRACT: &str = "flux2_trace_contract_1";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lib {
    Ros,
    Flux,
    Flux2,
}

impl Lib {
    fn of(path: &Path) -> Option<Self> {
        match path.file_name()?.to_str()? {
            "libtracetools.so" => Some(Self::Ros),
            "libflux_tracetools.so" => Some(Self::Flux),
            "libflux2_ffi.so" => Some(Self::Flux2),
            _ => None,
        }
    }

    /// A library of another version attaches by name all the same and hands the programs
    /// arguments they misread, so one without this symbol is refused.
    fn contract(self) -> Option<&'static str> {
        match self {
            Self::Ros => None,
            Self::Flux => Some(FLUX_CONTRACT),
            Self::Flux2 => Some(FLUX2_CONTRACT),
        }
    }
}

const PROBES: [(&str, &str, Lib); 9] = [
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
    ("rmw_take", "ros_trace_rmw_take", Lib::Ros),
    (
        "flux2_callback_start",
        "flux2_trace_callback_start",
        Lib::Flux2,
    ),
    ("flux2_callback_end", "flux2_trace_callback_end", Lib::Flux2),
];

pub struct Probes {
    stats: BpfHashMap<MapData, CallbackKey, CallbackStat>,
    names: BpfHashMap<MapData, CallbackKey, CallbackName>,
    channels: BpfHashMap<MapData, CallbackKey, u64>,
    lost: Array<MapData, u64>,
    scope: UProbeScope,
    pid: Option<NonZeroU32>,
    /// Library files tried, by (dev, inode): uprobes are per file, and a refused one stays refused.
    tried: HashSet<(u64, u64)>,
    // Dropping this detaches the probes, so it lives as long as the maps are read.
    ebpf: Ebpf,
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
    /// Load the programs. Nothing fires until a library is attached. `pid` limits the probes to
    /// one process; without it they fire in every process that maps an attached library.
    pub fn load(pid: Option<NonZeroU32>) -> Result<Self> {
        let mut ebpf = Ebpf::load(include_bytes_aligned!(concat!(env!("OUT_DIR"), "/armo")))?;
        for (program, _, _) in PROBES {
            let p: &mut UProbe = ebpf
                .program_mut(program)
                .ok_or_else(|| format!("program {program} missing from the object"))?
                .try_into()?;
            p.load()?;
        }
        let stats = take(&mut ebpf, "STATS")?.try_into()?;
        let names = take(&mut ebpf, "NAMES")?.try_into()?;
        let channels = take(&mut ebpf, "FLUX_CHANNELS")?.try_into()?;
        let lost = take(&mut ebpf, "LOST")?.try_into()?;
        let scope = match pid {
            Some(p) => UProbeScope::OneProcess(p),
            None => UProbeScope::AllProcesses,
        };
        Ok(Self {
            stats,
            names,
            channels,
            lost,
            scope,
            pid,
            tried: HashSet::new(),
            ebpf,
        })
    }

    /// Attach to the library at `path`, named on the command line. Returns false if it was
    /// already attached.
    pub fn attach_path(&mut self, path: &Path) -> Result<bool> {
        let lib = Lib::of(path).ok_or_else(|| {
            format!(
                "{}: not libtracetools.so, libflux_tracetools.so or libflux2_ffi.so",
                path.display()
            )
        })?;
        let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !self.tried.insert((meta.dev(), meta.ino())) {
            return Ok(false);
        }
        self.attach(lib, path)?;
        Ok(true)
    }

    /// Attach to each tracetools library the watched processes have mapped and armod has not
    /// tried yet. A process that loads its own copy (a build tree, another install, a
    /// container) is traced through that copy. Returns one line per library attached or refused.
    pub fn attach_mapped(&mut self) -> Vec<String> {
        let pids: Vec<u32> = match self.pid {
            Some(p) => vec![p.get()],
            None => fs::read_dir("/proc")
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
                .collect(),
        };
        let mut out = Vec::new();
        for pid in pids {
            let Ok(maps) = fs::read_to_string(format!("/proc/{pid}/maps")) else {
                continue;
            };
            for line in maps.lines() {
                let f: Vec<&str> = line.split_whitespace().collect();
                let (Some(&mapped), Some(inode)) =
                    (f.get(5), f.get(4).and_then(|i| i.parse().ok()))
                else {
                    continue;
                };
                let Some(lib) = Lib::of(Path::new(mapped)) else {
                    continue;
                };
                // Rebuilt while the process runs: the path now names a file it does not execute.
                if f.get(6) == Some(&"(deleted)") {
                    if self.tried.insert((u64::MAX, inode)) {
                        out.push(format!(
                            "not attached to {mapped} (pid {pid}): replaced on disk since it was \
                             loaded; restart the process to trace it"
                        ));
                    }
                    continue;
                }
                // Through the process's root, so a container's copy resolves to its own file.
                let path = PathBuf::from(format!("/proc/{pid}/root{mapped}"));
                let Ok(meta) = fs::metadata(&path) else {
                    continue;
                };
                if !self.tried.insert((meta.dev(), meta.ino())) {
                    continue;
                }
                out.push(match self.attach(lib, &path) {
                    Ok(()) => format!("attached to {mapped} (pid {pid})"),
                    Err(e) => format!("not attached to {mapped} (pid {pid}): {e}"),
                });
            }
        }
        out
    }

    fn attach(&mut self, lib: Lib, path: &Path) -> Result<()> {
        if let Some(contract) = lib.contract() {
            check_contract(path, contract)?;
        }
        for (program, symbol, l) in PROBES {
            if l != lib {
                continue;
            }
            let p: &mut UProbe = self
                .ebpf
                .program_mut(program)
                .ok_or_else(|| format!("program {program} missing from the object"))?
                .try_into()?;
            p.attach(symbol, path, self.scope)
                .map_err(|e| format!("attach {symbol}: {e}"))?;
        }
        Ok(())
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

fn check_contract(lib: &Path, contract: &str) -> Result<()> {
    let bytes = fs::read(lib).map_err(|e| format!("read {}: {e}", lib.display()))?;
    let elf = object::File::parse(&*bytes).map_err(|e| format!("parse {}: {e}", lib.display()))?;
    if elf.dynamic_symbols().any(|s| s.name() == Ok(contract)) {
        return Ok(());
    }
    Err(format!(
        "{} does not export {contract}: its trace events are not the ones armod reads",
        lib.display()
    )
    .into())
}

fn take(ebpf: &mut Ebpf, name: &str) -> Result<aya::maps::Map> {
    Ok(ebpf
        .take_map(name)
        .ok_or_else(|| format!("map {name} missing from the object"))?)
}
