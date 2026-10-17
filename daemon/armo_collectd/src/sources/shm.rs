//! The LD_PRELOAD source: finds the shim's files, holds the reader lock, arms them,
//! drains the rings, takes table differences per window, and removes files of dead writers.
//! libarmo_lttng.so's files (backend 2) only lend their slots' `cur_cb` to armod: their
//! events come through the LTTng source.
#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::offset_of;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use armo_common::events::{ARMO_EV_PAD, armo_position, armo_record};
use armo_common::shm::*;
use armo_common::{BUCKETS, bytes_of, catalog, read};
use armo_proto::msg::{Aggregate, Overflow};
use armo_proto::records;

use crate::model::procfs::{self, host_pid, host_tid};

/// A file whose writer is gone is left this long, in case it is one being created.
const SETTLE: Duration = Duration::from_secs(1);

fn lock_probe(file: &File, kind: i32, byte: u32, cmd: i32) -> io::Result<libc::flock> {
    // SAFETY: flock is plain data; zero is a valid value.
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = kind as libc::c_short;
    fl.l_whence = libc::SEEK_SET as libc::c_short;
    fl.l_start = byte as libc::off_t;
    fl.l_len = 1;
    // SAFETY: fl is a valid flock for the duration of the call.
    if unsafe { libc::fcntl(file.as_raw_fd(), cmd, &mut fl) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fl)
}

/// Whether the writer still holds its liveness lock on byte 0.
fn writer_alive(file: &File) -> io::Result<bool> {
    let fl = lock_probe(file, libc::F_WRLCK, ARMO_LOCK_ALIVE, libc::F_OFD_GETLK)?;
    Ok(fl.l_type != libc::F_UNLCK as libc::c_short)
}

fn open_flags() -> OpenOptions {
    let mut o = OpenOptions::new();
    o.read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    o
}

/// `armo.<pid>.<starttime>` of backend 1, `armo.<pid>.<starttime>.lttng` of backend 2.
fn parse_name(name: &str) -> Option<(u32, u64, u32)> {
    let mut parts = name.strip_prefix("armo.")?.split('.');
    let pid = parts.next()?.parse().ok()?;
    let starttime = parts.next()?.parse().ok()?;
    let backend = match parts.next() {
        None => ARMO_BACKEND_SHM,
        Some("lttng") => ARMO_BACKEND_LTTNG,
        Some(_) => return None,
    };
    parts.next().is_none().then_some((pid, starttime, backend))
}

/// Lowers `armed` and clears `options` in a file without mapping it: the guard's cleanup.
pub fn disarm(path: &Path) -> io::Result<()> {
    let file = match open_flags().open(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        r => r?,
    };
    file.write_all_at(
        &0u32.to_le_bytes(),
        offset_of!(armo_shm_header, armed) as u64,
    )?;
    // The shims read some options without looking at `armed`.
    file.write_all_at(
        &0u32.to_le_bytes(),
        offset_of!(armo_shm_header, options) as u64,
    )
}

#[derive(Clone, Default)]
struct Prev {
    count: u64,
    sum: u64,
    hist: Vec<u64>,
}

pub struct ShmFile {
    pub path: PathBuf,
    /// The host's pid; the file's name and header carry the writer's own, which differs in a
    /// container.
    pub pid: u32,
    pub ns_pid: u32,
    host_tids: BTreeMap<u32, u32>,
    pub starttime: u64,
    pub backend: u32,
    /// Events whose intercept point found no original function (header `missing`).
    pub missing: Vec<u16>,
    pub armed: bool,
    /// Every init record drained so far, positioned, for recordings that start later.
    pub init: Vec<u8>,
    pub init_n: u64,
    options: u32,
    _file: File,
    map: *mut u8,
    len: usize,
    n_threads: usize,
    thread_size: usize,
    table_entries: usize,
    ring_bytes: u64,
    prev: BTreeMap<(usize, usize), Prev>,
    prev_drops: Vec<u64>,
    generations: Vec<u32>,
    /// `table_overflow` and `depth_overflow`: last read per slot, and what slots held before
    /// another thread took them.
    overflow_seen: Vec<[u64; 2]>,
    overflow_retired: [u64; 2],
    pub corrupt: Option<String>,
}

// SAFETY: the mapping is shared memory touched only through atomics and the ring protocol.
unsafe impl Send for ShmFile {}

enum Open {
    File(Box<ShmFile>),
    /// Not ready yet, or another reader holds it: try again at the next scan.
    Later,
    Dead,
    Bad(String),
}

impl ShmFile {
    fn open(path: &Path, pid: u32, starttime: u64, backend: u32, uid: u32) -> io::Result<Open> {
        let file = open_flags().open(path)?;
        let md = file.metadata()?;
        if !md.is_file() || md.uid() != uid {
            return Ok(Open::Bad("not a regular file of this user".into()));
        }
        if !writer_alive(&file)? {
            let age = SystemTime::now()
                .duration_since(md.modified()?)
                .unwrap_or_default();
            return Ok(if age >= SETTLE {
                Open::Dead
            } else {
                Open::Later
            });
        }
        let mut raw = vec![0u8; ARMO_SHM_HEADER_SIZE];
        if md.len() < ARMO_SHM_HEADER_SIZE as u64 {
            return Ok(Open::Later);
        }
        file.read_exact_at(&mut raw, 0)?;
        let h: armo_shm_header = read(&raw).expect("a whole header");
        if h.magic == 0 {
            return Ok(Open::Later);
        }
        let thread_size = armo_thread_size(h.table_entries, h.ring_bytes);
        let len = ARMO_SHM_HEADER_SIZE + h.n_threads as usize * thread_size;
        let layout = match backend {
            ARMO_BACKEND_SHM => {
                h.ring_bytes > 0 && (h.ring_bytes as usize).is_multiple_of(ARMO_CACHE_LINE)
            }
            _ => h.ring_bytes == 0 && h.table_entries == 0,
        };
        let fits = h.magic == ARMO_SHM_MAGIC
            && h.version == ARMO_SHM_VERSION
            && h.header_size as usize == ARMO_SHM_HEADER_SIZE
            && h.thread_size as usize == thread_size
            && h.backend == backend
            && layout
            && md.len() == len as u64;
        if !fits {
            return Ok(Open::Bad(format!(
                "header does not fit: version {}, backend {}, size {} for {} threads",
                h.version,
                h.backend,
                md.len(),
                h.n_threads
            )));
        }
        if h.pid != pid || h.starttime != starttime {
            return Ok(Open::Bad(format!("header says {}.{}", h.pid, h.starttime)));
        }
        if lock_probe(&file, libc::F_WRLCK, ARMO_LOCK_READER, libc::F_OFD_SETLK).is_err() {
            return Ok(Open::Later);
        }
        // SAFETY: a shared mapping of the whole file, which keeps its size; unmapped in Drop.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let missing = (0..ARMO_MISSING_WORDS * 64)
            .filter(|bit| h.missing[bit / 64] & (1 << (bit % 64)) != 0)
            .map(|bit| bit as u16)
            .collect();
        let mut f = ShmFile {
            path: path.to_path_buf(),
            pid: host_pid(pid, starttime).unwrap_or(pid),
            ns_pid: pid,
            host_tids: BTreeMap::new(),
            starttime,
            backend,
            missing,
            armed: false,
            init: Vec::new(),
            init_n: 0,
            options: 0,
            _file: file,
            map: map.cast(),
            len,
            n_threads: h.n_threads as usize,
            thread_size,
            table_entries: h.table_entries as usize,
            ring_bytes: h.ring_bytes as u64,
            prev: BTreeMap::new(),
            prev_drops: vec![0; h.n_threads as usize],
            generations: vec![0; h.n_threads as usize],
            overflow_seen: vec![[0; 2]; h.n_threads as usize],
            overflow_retired: [0; 2],
            corrupt: None,
        };
        f.store_header(offset_of!(armo_shm_header, armed), 0);
        f.store_header(offset_of!(armo_shm_header, options), 0);
        // Counts from before this reader are not this reader's windows.
        f.tables(&mut Vec::new(), &mut |_, _| {});
        Ok(Open::File(Box::new(f)))
    }

    fn u16_at(&self, at: usize) -> &AtomicU16 {
        assert!(at.is_multiple_of(2) && at + 2 <= self.len);
        // SAFETY: aligned, in bounds, and accessed only atomically while mapped.
        unsafe { AtomicU16::from_ptr(self.map.wrapping_add(at).cast()) }
    }

    fn u32_at(&self, at: usize) -> &AtomicU32 {
        assert!(at.is_multiple_of(4) && at + 4 <= self.len);
        // SAFETY: as in u16_at.
        unsafe { AtomicU32::from_ptr(self.map.wrapping_add(at).cast()) }
    }

    fn u64_at(&self, at: usize) -> &AtomicU64 {
        assert!(at.is_multiple_of(8) && at + 8 <= self.len);
        // SAFETY: as in u16_at.
        unsafe { AtomicU64::from_ptr(self.map.wrapping_add(at).cast()) }
    }

    fn copy(&self, at: usize, out: &mut [u8]) {
        assert!(at + out.len() <= self.len);
        // SAFETY: in bounds; the ring protocol keeps the writer off the bytes between tail and
        // head, which are the only ones copied.
        unsafe {
            std::ptr::copy_nonoverlapping(self.map.wrapping_add(at), out.as_mut_ptr(), out.len())
        }
    }

    fn store_header(&self, field: usize, value: u32) {
        self.u32_at(field).store(value, Ordering::Release);
    }

    fn slot(&self, slot: usize) -> usize {
        ARMO_SHM_HEADER_SIZE + slot * self.thread_size
    }

    fn slot_word(&self, slot: usize, field: usize) -> &AtomicU64 {
        self.u64_at(self.slot(slot) + field)
    }

    fn state(&self, slot: usize) -> u32 {
        self.u32_at(self.slot(slot) + offset_of!(armo_thread_slot, state))
            .load(Ordering::Acquire)
    }

    fn ns_tid(&self, slot: usize) -> u32 {
        self.u32_at(self.slot(slot) + offset_of!(armo_thread_slot, tid))
            .load(Ordering::Relaxed)
    }

    /// The slot's thread id as the host sees it.
    fn tid(&mut self, slot: usize) -> u32 {
        let ns_tid = self.ns_tid(slot);
        if self.pid == self.ns_pid {
            return ns_tid;
        }
        let pid = self.pid;
        *self
            .host_tids
            .entry(ns_tid)
            .or_insert_with(|| host_tid(pid, ns_tid).unwrap_or(ns_tid))
    }

    pub fn entity(&self) -> String {
        crate::record::entity(self.pid, self.starttime)
    }

    /// Whether the header's pid and starttime name a host process, and of the live slots' thread
    /// ids, how many name a thread of it through NSpid: the container check of `selfcheck`.
    pub fn host_ids(&self) -> (bool, usize, usize) {
        let found = procfs::started_at(self.pid, self.starttime);
        let live: Vec<u32> = (0..self.n_threads)
            .filter(|s| self.state(*s) == ARMO_THREAD_LIVE)
            .map(|s| self.ns_tid(s))
            .collect();
        let mapped = live
            .iter()
            .filter(|t| host_tid(self.pid, **t).is_some())
            .count();
        (found, mapped, live.len())
    }

    /// libarmo_preload.so's file, whose rings and tables are a source.
    pub fn preload(&self) -> bool {
        self.backend == ARMO_BACKEND_SHM
    }

    fn set_armed(&mut self, armed: bool, options: u32) {
        if (armed, options) == (self.armed, self.options) {
            return;
        }
        if armed && !self.armed {
            self.u32_at(offset_of!(armo_shm_header, reader_gen))
                .fetch_add(1, Ordering::Relaxed);
        }
        self.store_header(offset_of!(armo_shm_header, options), options);
        self.store_header(offset_of!(armo_shm_header, armed), u32::from(armed));
        self.armed = armed;
        self.options = options;
    }

    /// Appends the slot's unread records, positioned, to `out`; their count, or why the ring
    /// stopped making sense.
    fn drain_slot(&self, slot: usize, tid: u32, out: &mut Vec<u8>) -> Result<u64, String> {
        let ring = self.slot(slot) + armo_ring_offset(self.table_entries as u32);
        let head = self
            .slot_word(slot, offset_of!(armo_thread_slot, ring_head))
            .load(Ordering::Acquire);
        let tail_word = self.slot_word(slot, offset_of!(armo_thread_slot, ring_tail));
        let mut tail = tail_word.load(Ordering::Relaxed);
        if head < tail || head - tail > self.ring_bytes {
            return Err(format!("thread slot {slot}: head {head}, tail {tail}"));
        }
        let position = armo_position { pid: self.pid, tid };
        let mut n = 0;
        while tail < head {
            let pos = tail % self.ring_bytes;
            let to_end = self.ring_bytes - pos;
            let mut word = [0u8; 4];
            self.copy(ring + pos as usize, &mut word);
            let event = u16::from_le_bytes([word[0], word[1]]);
            let len = u16::from_le_bytes([word[2], word[3]]) as u64;
            if event == ARMO_EV_PAD {
                if len != to_end {
                    return Err(format!("thread slot {slot}: PAD of {len} at {pos}"));
                }
                tail += to_end;
                continue;
            }
            if len < size_of::<armo_record>() as u64 || len > to_end || tail + len > head {
                return Err(format!("thread slot {slot}: record of {len} at {pos}"));
            }
            let start = out.len();
            out.extend_from_slice(bytes_of(&position));
            out.resize(start + size_of::<armo_position>() + len as usize, 0);
            self.copy(
                ring + pos as usize,
                &mut out[start + size_of::<armo_position>()..],
            );
            if let Err(e) = records::check(&out[start + size_of::<armo_position>()..], 0) {
                out.truncate(start);
                return Err(format!("thread slot {slot}: {e}"));
            }
            n += 1;
            tail += len;
        }
        tail_word.store(tail, Ordering::Release);
        Ok(n)
    }

    /// Unread records of every thread, positioned.
    pub fn drain(&mut self, out: &mut Vec<u8>) -> u64 {
        let mut n = 0;
        if !self.preload() {
            return n;
        }
        let start = out.len();
        for slot in 0..self.n_threads {
            if self.corrupt.is_some() || self.state(slot) == ARMO_THREAD_FREE {
                continue;
            }
            let tid = self.tid(slot);
            match self.drain_slot(slot, tid, out) {
                Ok(k) => n += k,
                Err(e) => self.corrupt = Some(e),
            }
        }
        let mut at = start;
        for r in records::positioned(&out[start..]).flatten() {
            let len = size_of::<armo_position>() + r.header.len as usize;
            if catalog::init(r.header.event) {
                self.init.extend_from_slice(&out[at..at + len]);
                self.init_n += 1;
            }
            at += len;
        }
        n
    }

    /// Each table row's difference since the last call, and ring drops by thread id. Frees the
    /// slots of exited threads once their rings are empty. `max` is swapped to 0,
    /// so it is the maximum within the window.
    pub fn tables(&mut self, rows: &mut Vec<Aggregate>, drops: &mut dyn FnMut(u32, u64)) {
        let row_size = size_of::<armo_table_entry>();
        for slot in 0..self.n_threads {
            let state = self.state(slot);
            if state == ARMO_THREAD_FREE {
                continue;
            }
            let generation = self
                .u32_at(self.slot(slot) + offset_of!(armo_thread_slot, generation))
                .load(Ordering::Acquire);
            let fresh = generation != self.generations[slot];
            if fresh {
                // A new thread took the slot and zeroed its table and counters.
                self.generations[slot] = generation;
                self.prev.retain(|&(s, _), _| s != slot);
                self.prev_drops[slot] = 0;
                self.host_tids.remove(&self.ns_tid(slot));
            }
            for (i, field) in [
                offset_of!(armo_thread_slot, table_overflow),
                offset_of!(armo_thread_slot, depth_overflow),
            ]
            .into_iter()
            .enumerate()
            {
                let now = self.slot_word(slot, field).load(Ordering::Relaxed);
                let seen = &mut self.overflow_seen[slot][i];
                // A thread taking the slot zeroes the counters before it moves `generation`.
                if fresh || now < *seen {
                    self.overflow_retired[i] += *seen;
                }
                *seen = now;
            }
            let tid = self.tid(slot);
            let table = self.slot(slot) + size_of::<armo_thread_slot>();
            let entries = if self.preload() {
                self.table_entries
            } else {
                0
            };
            for i in 0..entries {
                let row = table + i * row_size;
                let kind = self
                    .u16_at(row + offset_of!(armo_table_entry, kind))
                    .load(Ordering::Acquire);
                if kind == 0 {
                    continue;
                }
                let word = |f: usize| self.u64_at(row + f);
                let count = word(offset_of!(armo_table_entry, count)).load(Ordering::Relaxed);
                let sum = word(offset_of!(armo_table_entry, sum)).load(Ordering::Relaxed);
                let max = word(offset_of!(armo_table_entry, max)).swap(0, Ordering::Relaxed);
                let key = word(offset_of!(armo_table_entry, key)).load(Ordering::Relaxed);
                let hist: Vec<u64> = (0..BUCKETS)
                    .map(|b| {
                        word(offset_of!(armo_table_entry, hist) + b * 8).load(Ordering::Relaxed)
                    })
                    .collect();
                let src = self
                    .u16_at(row + offset_of!(armo_table_entry, src))
                    .load(Ordering::Relaxed);
                let entry = self
                    .u32_at(row + offset_of!(armo_table_entry, entry))
                    .load(Ordering::Relaxed);
                let prev = self.prev.entry((slot, i)).or_default();
                prev.hist.resize(BUCKETS, 0);
                if count == prev.count {
                    continue;
                }
                rows.push(Aggregate {
                    pid: self.pid,
                    tid,
                    kind,
                    src,
                    entry,
                    key,
                    count: count - prev.count,
                    sum: sum.wrapping_sub(prev.sum),
                    max,
                    hist: hist
                        .iter()
                        .zip(&prev.hist)
                        .map(|(h, p)| h.saturating_sub(*p))
                        .collect(),
                });
                *prev = Prev { count, sum, hist };
            }
            let total = self
                .slot_word(slot, offset_of!(armo_thread_slot, ring_drops))
                .load(Ordering::Relaxed);
            let new = total.saturating_sub(self.prev_drops[slot]);
            self.prev_drops[slot] = total;
            if new > 0 {
                drops(tid, new);
            }
            let ring_empty = self
                .slot_word(slot, offset_of!(armo_thread_slot, ring_head))
                .load(Ordering::Acquire)
                == self
                    .slot_word(slot, offset_of!(armo_thread_slot, ring_tail))
                    .load(Ordering::Relaxed);
            if state == ARMO_THREAD_EXITED && ring_empty {
                self.u32_at(self.slot(slot) + offset_of!(armo_thread_slot, state))
                    .store(ARMO_THREAD_FREE, Ordering::Release);
            }
        }
    }

    /// What the shim could not take since it made the file, as of the last `tables`.
    pub fn overflow(&self) -> Overflow {
        let sum = |i: usize| {
            self.overflow_retired[i] + self.overflow_seen.iter().map(|s| s[i]).sum::<u64>()
        };
        Overflow {
            threads: self
                .u64_at(offset_of!(armo_shm_header, threads_overflow))
                .load(Ordering::Relaxed),
            table: sum(0),
            depth: sum(1),
        }
    }

    /// Thread id to the address of its slot's `cur_cb` in the writer, for armod's off-CPU
    /// attribution, from where `/proc/<pid>/maps` shows the file mapped.
    pub fn cur_cb_words(&mut self) -> BTreeMap<String, u64> {
        let mut words = BTreeMap::new();
        let maps = std::fs::read_to_string(format!("/proc/{}/maps", self.pid)).unwrap_or_default();
        let name = self.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let base = maps
            .lines()
            .find(|l| l.ends_with(&format!("/{name}")))
            .and_then(|l| u64::from_str_radix(l.split('-').next()?, 16).ok());
        let Some(base) = base else { return words };
        for slot in 0..self.n_threads {
            if self.state(slot) == ARMO_THREAD_LIVE {
                let at = self.slot(slot) + offset_of!(armo_thread_slot, cur_cb);
                words.insert(self.tid(slot).to_string(), base + at as u64);
            }
        }
        words
    }
}

impl Drop for ShmFile {
    fn drop(&mut self) {
        // SAFETY: the mapping made in open, not used after this.
        unsafe { libc::munmap(self.map.cast(), self.len) };
    }
}

pub enum Change {
    Opened {
        pid: u32,
        starttime: u64,
    },
    Closed {
        pid: u32,
        starttime: u64,
    },
    Corrupt {
        pid: u32,
        starttime: u64,
        reason: String,
    },
    Rejected {
        path: PathBuf,
        reason: String,
    },
}

/// Every shim file of this user in one directory.
pub struct Shm {
    dir: PathBuf,
    uid: u32,
    pub files: BTreeMap<PathBuf, ShmFile>,
    rejected: BTreeMap<PathBuf, String>,
}

impl Shm {
    pub fn new(dir: PathBuf, uid: u32) -> Shm {
        Shm {
            dir,
            uid,
            files: BTreeMap::new(),
            rejected: BTreeMap::new(),
        }
    }

    /// Opens new files and removes the files of dead writers. A file this reader holds gets a
    /// last drain into `sink` first.
    pub fn scan(&mut self, sink: &mut dyn FnMut(&ShmFile, &[u8], u64)) -> Vec<Change> {
        let mut changes = Vec::new();
        let gone: Vec<PathBuf> = self
            .files
            .iter()
            .filter(|(_, f)| !writer_alive(&f._file).unwrap_or(false))
            .map(|(p, _)| p.clone())
            .collect();
        for path in gone {
            let mut f = self.files.remove(&path).expect("listed");
            let mut out = Vec::new();
            let n = f.drain(&mut out);
            if n > 0 {
                sink(&f, &out, n);
            }
            let _ = std::fs::remove_file(&path);
            changes.push(Change::Closed {
                pid: f.pid,
                starttime: f.starttime,
            });
        }
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return changes;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some((pid, starttime, backend)) = entry.file_name().to_str().and_then(parse_name)
            else {
                continue;
            };
            if self.files.contains_key(&path) || self.rejected.contains_key(&path) {
                continue;
            }
            match ShmFile::open(&path, pid, starttime, backend, self.uid) {
                Ok(Open::File(f)) => {
                    let pid = f.pid;
                    self.files.insert(path, *f);
                    changes.push(Change::Opened { pid, starttime });
                }
                Ok(Open::Later) => {}
                Ok(Open::Dead) => {
                    let _ = std::fs::remove_file(&path);
                }
                Ok(Open::Bad(reason)) => {
                    self.rejected.insert(path.clone(), reason.clone());
                    changes.push(Change::Rejected { path, reason });
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    self.rejected.insert(path.clone(), e.to_string());
                    changes.push(Change::Rejected {
                        path,
                        reason: e.to_string(),
                    });
                }
            }
        }
        self.rejected.retain(|p, _| p.exists());
        changes
    }

    /// Unread records of every file, through `sink` per file; the corruptions found.
    pub fn drain(&mut self, sink: &mut dyn FnMut(&ShmFile, &[u8], u64)) -> Vec<Change> {
        let mut changes = Vec::new();
        let mut out = Vec::new();
        for f in self.files.values_mut() {
            let was = f.corrupt.is_some();
            out.clear();
            let n = f.drain(&mut out);
            if n > 0 {
                sink(f, &out, n);
            }
            if let (false, Some(reason)) = (was, &f.corrupt) {
                changes.push(Change::Corrupt {
                    pid: f.pid,
                    starttime: f.starttime,
                    reason: reason.clone(),
                });
            }
        }
        changes
    }

    /// Sets `armed` and `options` per file from `want`, which takes the pid and whether the file
    /// is libarmo_preload.so's (`None` lowers it).
    pub fn arm(&mut self, want: &dyn Fn(u32, bool) -> Option<u32>) {
        for f in self.files.values_mut() {
            let w = want(f.pid, f.preload()).filter(|_| f.corrupt.is_none());
            f.set_armed(w.is_some(), w.unwrap_or(0));
        }
    }

    pub fn armed_paths(&self) -> Vec<PathBuf> {
        self.files
            .values()
            .filter(|f| f.armed)
            .map(|f| f.path.clone())
            .collect()
    }

    /// Whether `pid` writes libarmo_preload.so's file.
    pub fn has_pid(&self, pid: u32) -> bool {
        self.files.values().any(|f| f.pid == pid && f.preload())
    }

    pub fn bytes(&self) -> u64 {
        self.files.values().map(|f| f.len as u64).sum()
    }
}
