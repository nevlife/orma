//! Callback names from code addresses: the words a collector read at the callback
//! handle are tried in order; the first inside an executable mapping that an ELF symbol covers
//! names the callback.

use std::collections::HashMap;

use object::{Object, ObjectSegment, ObjectSymbol, SymbolKind};

use super::procfs::{self, Mapping};

struct Elf {
    /// `(file offset, size, address)` of each loadable segment.
    segments: Vec<(u64, u64, u64)>,
    /// `(address, size, name)` sorted by address.
    symbols: Vec<(u64, u64, String)>,
}

fn load(data: &[u8]) -> Option<Elf> {
    let file = object::File::parse(data).ok()?;
    let segments = file
        .segments()
        .map(|s| {
            let (off, size) = s.file_range();
            (off, size, s.address())
        })
        .collect();
    let mut symbols: Vec<(u64, u64, String)> = file
        .symbols()
        .chain(file.dynamic_symbols())
        .filter(|s| s.kind() == SymbolKind::Text && s.address() != 0)
        .filter_map(|s| Some((s.address(), s.size(), s.name().ok()?.to_string())))
        .collect();
    symbols.sort_unstable();
    symbols.dedup_by_key(|s| s.0);
    Some(Elf { segments, symbols })
}

impl Elf {
    fn symbol(&self, file_offset: u64) -> Option<&str> {
        let &(off, _, vaddr) = self
            .segments
            .iter()
            .find(|(off, size, _)| (*off..off + size).contains(&file_offset))?;
        let addr = vaddr + (file_offset - off);
        let i = self
            .symbols
            .partition_point(|s| s.0 <= addr)
            .checked_sub(1)?;
        let (start, size, name) = &self.symbols[i];
        (addr < start + (*size).max(1)).then_some(name.as_str())
    }
}

/// Caches each process's maps and each file's symbols.
#[derive(Default)]
pub struct Resolver {
    maps: HashMap<u32, Vec<Mapping>>,
    files: HashMap<(u32, String), Option<Elf>>,
}

impl Resolver {
    pub fn resolve(&mut self, pid: u32, words: &[u64]) -> Option<String> {
        let maps = self.maps.entry(pid).or_insert_with(|| procfs::maps(pid));
        for &w in words {
            let Some(m) = maps
                .iter()
                .find(|m| m.exec && (m.start..m.end).contains(&w))
            else {
                continue;
            };
            let elf = self
                .files
                .entry((pid, m.path.clone()))
                .or_insert_with(|| load(&procfs::read_mapped(pid, &m.path)?));
            if let Some(name) = elf.as_ref().and_then(|e| e.symbol(w - m.start + m.offset)) {
                return Some(shorten(&demangle(name)));
            }
        }
        None
    }

    /// A process's maps change as it loads libraries; read them again on its next callback.
    pub fn forget(&mut self, pid: u32) {
        self.maps.remove(&pid);
    }
}

pub fn demangle(name: &str) -> String {
    cpp_demangle::Symbol::new(name)
        .ok()
        .and_then(|s| s.demangle().ok())
        .unwrap_or_else(|| name.to_string())
}

/// The span from `open` at `at` to its matching close, counting `<>` and `()` together.
fn matching(s: &str, at: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s[at..].char_indices() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// `std::_Function_handler<SIG, INNER>::_M_invoke(...)` becomes INNER, and `{lambda(ARGS)#N}`
/// becomes `{lambda#N}` (roscope `shorten_callback_symbol`).
pub fn shorten(name: &str) -> String {
    let mut out = name.to_string();
    const HANDLER: &str = "std::_Function_handler<";
    if let Some(at) = out.find(HANDLER) {
        let open = at + HANDLER.len() - 1;
        if let Some(close) = matching(&out, open) {
            let args = &out[open + 1..close];
            let mut depth = 0i32;
            let split = args.char_indices().find_map(|(i, c)| {
                match c {
                    '<' | '(' => depth += 1,
                    '>' | ')' => depth -= 1,
                    ',' if depth == 0 => return Some(i),
                    _ => {}
                }
                None
            });
            if let Some(comma) = split {
                out = args[comma + 1..].trim().to_string();
            }
        }
    }
    while let Some(at) = out.find("{lambda(") {
        let open = at + "{lambda".len();
        let Some(close) = matching(&out, open) else {
            break;
        };
        out.replace_range(open..=close, "");
    }
    out
}
