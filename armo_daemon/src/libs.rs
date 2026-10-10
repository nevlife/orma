//! The libraries armod attaches to, found in each process's maps and opened through
//! `/proc/<pid>/root` so a container's copy is that container's file (design 2.7.3, 19).

use std::{
    collections::HashMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use object::{Object as _, ObjectSection as _, ObjectSymbol as _};

/// The event names and arguments the flux programs read. libflux_tracetools.so exports a symbol
/// of this name for exactly that set (flux/trace.h).
pub const FLUX_CONTRACT: &str = "flux_trace_contract_2";
/// The same for flux2, whose events live in libflux2_ffi.so (design 11.1).
pub const FLUX2_CONTRACT: &str = "flux2_trace_contract_2";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Lib {
    Tracetools,
    Rcl,
    Rclcpp,
    Flux,
    Flux2,
    Fastdds,
    Cyclone,
}

impl Lib {
    pub fn of(path: &str) -> Option<Lib> {
        let name = Path::new(path).file_name()?.to_str()?;
        Some(match name {
            "libtracetools.so" => Lib::Tracetools,
            "librcl.so" => Lib::Rcl,
            "librclcpp.so" => Lib::Rclcpp,
            "libflux_tracetools.so" => Lib::Flux,
            "librmw_fastrtps_shared_cpp.so" => Lib::Fastdds,
            "librmw_cyclonedds_cpp.so" => Lib::Cyclone,
            // A process maps the file its soname names, libflux2_ffi.so.<abi>.
            n if n.strip_prefix("libflux2_ffi.so").is_some_and(abi) => Lib::Flux2,
            _ => return None,
        })
    }

    /// A library of another version attaches by name all the same and hands the programs
    /// arguments they misread, so one without this symbol is refused.
    pub fn contract(self) -> Option<&'static str> {
        match self {
            Lib::Flux => Some(FLUX_CONTRACT),
            Lib::Flux2 => Some(FLUX2_CONTRACT),
            _ => None,
        }
    }
}

fn abi(suffix: &str) -> bool {
    suffix
        .strip_prefix('.')
        .is_none_or(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
}

pub type FileKey = (u64, u64);

/// One library file, read once.
#[derive(Debug)]
pub struct LibFile {
    pub lib: Lib,
    /// A path that opens this file now: the process's root joined with the mapped path.
    pub path: PathBuf,
    /// The path inside the process that mapped it.
    pub mapped: String,
    /// Defined dynamic function symbols: name to file offset.
    pub symbols: HashMap<String, u64>,
    /// Undefined dynamic symbols (imports).
    pub imports: Vec<String>,
    pub needs_lttng: bool,
    pub build_id: Option<String>,
    /// The major version in `<prefix>/share/rclcpp/package.xml` (design 2.7.2).
    pub rclcpp_major: Option<u32>,
    /// Why this file is not attached to at all.
    pub refused: Option<String>,
    /// For Cyclone: internal symbols from the dbgsym file, or why there is none.
    pub debug_symbols: Result<HashMap<String, u64>, String>,
}

/// The ROS distro a path under `/opt/ros/<distro>/` names.
pub fn distro_of_path(mapped: &str) -> Option<String> {
    let rest = mapped.strip_prefix("/opt/ros/")?;
    let d = rest.split('/').next()?;
    (!d.is_empty()).then(|| d.to_string())
}

fn package_major(root: &Path, prefix: &Path, package: &str) -> Option<u32> {
    let xml = root
        .join(prefix.strip_prefix("/").unwrap_or(prefix))
        .join("share")
        .join(package)
        .join("package.xml");
    let text = fs::read_to_string(xml).ok()?;
    let v = text.split("<version>").nth(1)?.split('<').next()?;
    v.trim().split('.').next()?.parse().ok()
}

/// `<root>` is `/proc/<pid>/root`; `mapped` the path in that process.
pub fn read(lib: Lib, root: &Path, mapped: &str) -> Result<(FileKey, LibFile), String> {
    let path = root.join(mapped.trim_start_matches('/'));
    let meta = fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let elf = object::File::parse(&*bytes).map_err(|e| format!("parse {}: {e}", path.display()))?;
    let mut symbols = HashMap::new();
    let mut imports = Vec::new();
    for s in elf.dynamic_symbols() {
        let Ok(name) = s.name() else {
            continue;
        };
        if s.is_undefined() {
            imports.push(name.to_string());
        } else if s.kind() == object::SymbolKind::Text
            && let Some(off) = file_offset(&elf, s.address())
        {
            symbols.insert(name.to_string(), off);
        }
    }
    let needs_lttng = elf
        .section_by_name(".dynstr")
        .and_then(|s| s.data().ok())
        .is_some_and(|d| d.windows(13).any(|w| w == b"liblttng-ust."));
    let build_id = elf
        .build_id()
        .ok()
        .flatten()
        .map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<String>());
    let prefix = Path::new(mapped)
        .parent()
        .and_then(Path::parent)
        .unwrap_or(Path::new("/"));
    let rclcpp_major = package_major(root, prefix, "rclcpp");
    let refused = lib.contract().and_then(|c| {
        (!symbols.contains_key(c)).then(|| {
            format!("{mapped} does not export {c}: its trace events are not the ones armod reads")
        })
    });
    let debug_symbols = if lib == Lib::Cyclone {
        debug_symbols(root, &elf, build_id.as_deref())
    } else {
        Err(String::new())
    };
    let file = LibFile {
        lib,
        path,
        mapped: mapped.to_string(),
        symbols,
        imports,
        needs_lttng,
        build_id,
        rclcpp_major,
        refused,
        debug_symbols,
    };
    Ok(((meta.dev(), meta.ino()), file))
}

/// The file offset of a virtual address, through the section holding it.
fn file_offset(elf: &object::File, address: u64) -> Option<u64> {
    elf.sections().find_map(|s| {
        let (start, size) = (s.address(), s.size());
        let (off, _) = s.file_range()?;
        (address >= start && address < start + size).then(|| address - start + off)
    })
}

/// Internal symbols of a stripped library from its dbgsym file under the same root, at the
/// production file's offsets (the debug file keeps the addresses).
fn debug_symbols(
    root: &Path,
    elf: &object::File,
    build_id: Option<&str>,
) -> Result<HashMap<String, u64>, String> {
    let id = build_id.ok_or("no build id")?;
    let debug = root.join(format!(
        "usr/lib/debug/.build-id/{}/{}.debug",
        &id[..2],
        &id[2..]
    ));
    let bytes = fs::read(&debug).map_err(|_| {
        format!(
            "no dbgsym file /usr/lib/debug/.build-id/{}/{}.debug",
            &id[..2],
            &id[2..]
        )
    })?;
    let dbg = object::File::parse(&*bytes).map_err(|e| format!("parse dbgsym: {e}"))?;
    Ok(dbg
        .symbols()
        .filter(|s| s.kind() == object::SymbolKind::Text && !s.is_undefined())
        .filter_map(|s| {
            let name = s.name().ok()?;
            (!name.contains(".cold")).then_some((name.to_string(), file_offset(elf, s.address())?))
        })
        .collect())
}

/// The one function whose mangled name starts with `prefix`, as offsets (constructor C1 and C2
/// aliases count as one function). Two functions are an error unless `overloads`: attaching the
/// wrong overload is worse than not attaching (design 2.1.2).
pub fn by_prefix(
    symbols: &HashMap<String, u64>,
    prefix: &str,
    overloads: bool,
) -> Result<Vec<u64>, String> {
    let mut hits: Vec<(&String, u64)> = symbols
        .iter()
        .filter(|(n, _)| n.starts_with(prefix))
        .map(|(n, o)| (n, *o))
        .collect();
    hits.sort();
    let functions: std::collections::BTreeSet<String> = hits
        .iter()
        .map(|(n, _)| constructor_alias(prefix, n))
        .collect();
    if hits.is_empty() {
        return Err(format!("no exported symbol starts with {prefix}"));
    }
    if functions.len() > 1 && !overloads {
        return Err(format!(
            "{prefix} matches {} functions; not attached",
            functions.len()
        ));
    }
    let mut offsets: Vec<u64> = hits.iter().map(|(_, o)| *o).collect();
    offsets.sort_unstable();
    offsets.dedup();
    Ok(offsets)
}

fn constructor_alias(prefix: &str, name: &str) -> String {
    if !prefix.ends_with('C') {
        return name.to_string();
    }
    let rest = &name[prefix.len()..];
    match rest.strip_prefix('1').or_else(|| rest.strip_prefix('2')) {
        Some(tail) => format!("{prefix}{tail}"),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syms(names: &[(&str, u64)]) -> HashMap<String, u64> {
        names.iter().map(|(n, o)| (n.to_string(), *o)).collect()
    }

    #[test]
    fn libraries_are_known_by_file_name() {
        assert_eq!(
            Lib::of("/opt/ros/jazzy/lib/libtracetools.so"),
            Some(Lib::Tracetools)
        );
        assert_eq!(Lib::of("/x/libflux2_ffi.so.3"), Some(Lib::Flux2));
        assert_eq!(Lib::of("/x/libflux2_ffi.so.3.1"), None);
        assert_eq!(Lib::of("/x/libflux2_ffi.so"), Some(Lib::Flux2));
        assert_eq!(Lib::of("/x/librclcpp_action.so"), None);
    }

    #[test]
    fn constructor_aliases_are_one_function() {
        let s = syms(&[
            ("_ZN6rclcpp13PublisherBaseC1EPNS_15node_interfaces", 0x10),
            ("_ZN6rclcpp13PublisherBaseC2EPNS_15node_interfaces", 0x10),
        ]);
        assert_eq!(
            by_prefix(&s, "_ZN6rclcpp13PublisherBaseC", false),
            Ok(vec![0x10])
        );
    }

    #[test]
    fn an_ambiguous_prefix_is_refused_unless_overloads_are_wanted() {
        let s = syms(&[
            ("_ZN6rclcpp8ExecutorC1ERKNS_15ExecutorOptionsE", 0x10),
            ("_ZN6rclcpp8ExecutorC1ERKSt10shared_ptrINS_7ContextEE", 0x20),
        ]);
        assert!(by_prefix(&s, "_ZN6rclcpp8ExecutorC", false).is_err());
        assert_eq!(
            by_prefix(&s, "_ZN6rclcpp8ExecutorC", true),
            Ok(vec![0x10, 0x20])
        );
        assert!(by_prefix(&s, "_ZN6rclcpp9executors", false).is_err());
    }

    #[test]
    fn a_path_under_opt_ros_names_its_distro() {
        assert_eq!(
            distro_of_path("/opt/ros/humble/lib/libtracetools.so").as_deref(),
            Some("humble")
        );
        assert_eq!(
            distro_of_path("/home/u/ws/install/lib/libtracetools.so"),
            None
        );
    }
}
