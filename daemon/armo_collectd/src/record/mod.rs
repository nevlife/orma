//! Recordings armo-collectd owns: checked start with limits that always exist,
//! appending, ends by limit, and finalize. A recording outlives the client that started it.

pub mod export;
pub mod files;
pub mod replay;
pub mod show;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use armo_proto::msg::*;
use serde::Deserialize;

use crate::host;
use crate::names::Names;
use crate::sys::{self, Disk};
use files::{EventFiles, Split};

pub const MAX_ACTIVE: usize = 4;
pub const META: &str = "meta.json";
pub const LOCK: &str = "lock";
pub const WINDOWS: &str = "windows.jsonl";
pub const CTF: &str = "ctf";
pub const PER_ENTITY_MAX: usize = 200;

const GIB: u64 = 1 << 30;
const SEC: u64 = 1_000_000_000;
const META_EVERY_NS: u64 = 10 * SEC;
const DISK_EVERY_NS: u64 = SEC;

/// `~/.config/armo/collectd.toml`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Caps {
    #[serde(default = "week")]
    pub max_duration_s: u64,
    /// Otherwise max(2 GiB, 5% of the file system).
    #[serde(default)]
    pub disk_floor_bytes: Option<u64>,
}

fn week() -> u64 {
    7 * 24 * 3600
}

impl Default for Caps {
    fn default() -> Self {
        Caps {
            max_duration_s: week(),
            disk_floor_bytes: None,
        }
    }
}

impl Caps {
    pub fn load(path: &Path) -> io::Result<Caps> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Caps::default()),
            Err(e) => Err(e),
        }
    }

    pub fn floor(&self, disk: Disk) -> u64 {
        self.disk_floor_bytes
            .unwrap_or((2 * GIB).max(disk.total / 20))
    }
}

/// What the server knows when a recording starts.
pub struct Context<'a> {
    pub id: String,
    pub records_dir: &'a Path,
    pub caps: &'a Caps,
    pub disk: fn(&Path) -> io::Result<Disk>,
    pub owner: Owner,
    pub host: Host,
    pub armod: bool,
    pub preload: &'a dyn Fn(u32) -> bool,
    /// Whether a pid is registered with armo's LTTng session daemon; `None` without LTTng.
    pub lttng: Option<&'a dyn Fn(u32) -> bool>,
    pub mono_ns: u64,
    pub real_ns: u64,
}

pub fn all_targets() -> Targets {
    Targets {
        all: true,
        pids: Vec::new(),
        node: None,
        topic: None,
        exclude: None,
    }
}

pub fn entity(pid: u32, starttime: u64) -> String {
    format!("p:{pid}:{starttime}")
}

pub struct Active {
    pub id: String,
    pub dir: PathBuf,
    pub meta: RecordMeta,
    pub limits: Limits,
    events: Option<EventFiles>,
    windows: File,
    windows_bytes: u64,
    _lock: File,
    meta_at: u64,
    disk_at: u64,
    open_gap: Option<Gap>,
    /// Bytes of the LTTng chunks copied into `ctf/`.
    ctf_bytes: u64,
    /// Set when the recording should end; the server finalizes it on its next tick.
    pub end: Option<(EndReason, Option<String>)>,
    /// The name targets, matched again each window.
    pub names: Option<Names>,
    /// Kernel counters at the start, for `interference`.
    counters: host::Counters,
}

/// `YYYYMMDD-HHMMSS` in UTC.
pub fn utc_stamp(real_ns: u64) -> String {
    let secs = real_ns / SEC;
    let (days, rest) = ((secs / 86400) as i64, secs % 86400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn nonzero(name: &str, value: Option<u64>) -> Result<Option<u64>, String> {
    match value {
        Some(0) => Err(format!("{name} 0: a recording always has limits")),
        v => Ok(v),
    }
}

fn make_dir(parent: &Path, name: Option<&str>, real_ns: u64) -> Result<PathBuf, String> {
    let fail = |dir: &Path, e: io::Error| format!("{}: {e}", dir.display());
    if let Some(name) = name {
        let dir = parent.join(name);
        std::fs::create_dir(&dir).map_err(|e| fail(&dir, e))?;
        return Ok(dir);
    }
    let base = format!("armo-{}", utc_stamp(real_ns));
    for n in 1.. {
        let dir = match n {
            1 => parent.join(&base),
            n => parent.join(format!("{base}-{n}")),
        };
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(fail(&dir, e)),
        }
    }
    unreachable!()
}

/// The recording `options` asks for, or why not (an error's `message`).
/// Whether `t` takes every process: `all`, or neither pids nor names.
pub fn every_process(t: &Targets) -> bool {
    t.all || (t.pids.is_empty() && t.node.is_none() && t.topic.is_none() && t.exclude.is_none())
}

/// The method a recording of `method` takes `pid` by, when it can say so now.
fn settle(
    method: RecordMethod,
    pid: u32,
    armod: bool,
    preload: &dyn Fn(u32) -> bool,
    lttng: Option<&dyn Fn(u32) -> bool>,
) -> Option<Method> {
    match method {
        RecordMethod::Uprobe => Some(Method::Uprobe),
        RecordMethod::Preload => Some(Method::Preload),
        RecordMethod::Lttng => Some(Method::Lttng),
        RecordMethod::Auto if preload(pid) => Some(Method::Preload),
        RecordMethod::Auto if lttng.is_some_and(|l| l(pid)) => Some(Method::Lttng),
        RecordMethod::Auto if armod => Some(Method::Uprobe),
        RecordMethod::Auto => None,
    }
}

pub fn start(options: RecordOptions, cx: Context<'_>) -> Result<Active, String> {
    let kind = options.kind.unwrap_or(RecordKind::Events);
    let method = options.method.unwrap_or(RecordMethod::Auto);
    if method == RecordMethod::Lttng && cx.lttng.is_none() {
        return Err(
            "method lttng needs lttng, lttng-sessiond, babeltrace and setpriv on PATH".into(),
        );
    }
    let targets = options.targets.clone().unwrap_or_else(all_targets);
    let cap_s = cx.caps.max_duration_s;
    let max_duration_s = match nonzero("max_duration_s", options.max_duration_s)? {
        Some(s) if s > cap_s => {
            return Err(format!(
                "max_duration_s {s} above the cap {cap_s} (collectd.toml)"
            ));
        }
        Some(s) => s,
        None => cap_s.min(match kind {
            RecordKind::Events => 1800,
            RecordKind::Windows => 86400,
        }),
    };
    let parent = options
        .dir
        .as_ref()
        .map_or_else(|| cx.records_dir.to_path_buf(), PathBuf::from);
    std::fs::create_dir_all(&parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let disk = (cx.disk)(&parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let floor = cx.caps.floor(disk);
    if disk.avail < floor + GIB {
        return Err(format!(
            "{} bytes free on {}, under the disk floor {floor} plus 1 GiB",
            disk.avail,
            parent.display()
        ));
    }
    let cap = disk.avail - floor;
    let max_bytes = match nonzero("max_bytes", options.max_bytes)? {
        Some(b) if b > cap => {
            return Err(format!(
                "max_bytes {b} above the free space over the disk floor, {cap}"
            ));
        }
        Some(b) => b,
        None => cap.min(match kind {
            RecordKind::Events => 8 * GIB,
            RecordKind::Windows => GIB,
        }),
    };
    let split_bytes = nonzero("split_bytes", options.split_bytes)?.unwrap_or(GIB);
    let split_duration_s = nonzero("split_duration_s", options.split_duration_s)?;
    let split_ns = match split_duration_s {
        Some(s) => Some(
            s.checked_mul(SEC)
                .ok_or(format!("split_duration_s {s} is too large"))?,
        ),
        None => None,
    };
    let cache_bytes = nonzero("cache_bytes", options.cache_bytes)?.unwrap_or(16 << 20);
    let memory = sys::physical_memory();
    if cache_bytes > memory {
        return Err(format!(
            "cache_bytes {cache_bytes} above the physical memory, {memory}"
        ));
    }
    let compression = options.compression.unwrap_or(Compression::None);

    let dir = make_dir(&parent, options.name.as_deref(), cx.real_ns)?;
    let io_err = |e: io::Error| format!("{}: {e}", dir.display());
    let lock = File::create(dir.join(LOCK)).map_err(io_err)?;
    lock.try_lock()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let windows = OpenOptions::new()
        .append(true)
        .create_new(true)
        .open(dir.join(WINDOWS))
        .map_err(io_err)?;
    // An LTTng recording keeps the session's CTF, not event files.
    if method == RecordMethod::Lttng && kind == RecordKind::Events {
        std::fs::create_dir(dir.join(CTF)).map_err(io_err)?;
    }
    let events = match kind {
        RecordKind::Events if method == RecordMethod::Lttng => None,
        RecordKind::Events => Some(
            EventFiles::create(
                &dir,
                compression,
                cache_bytes as usize,
                Split {
                    bytes: split_bytes,
                    ns: split_ns,
                },
                cx.mono_ns,
            )
            .map_err(io_err)?,
        ),
        RecordKind::Windows => None,
    };

    let mut methods_by_pid = BTreeMap::new();
    if !targets.all {
        for pid in &targets.pids {
            if let Some(m) = settle(method, *pid, cx.armod, cx.preload, cx.lttng) {
                methods_by_pid.insert(pid.to_string(), m);
            }
        }
    }
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
    let counters = host::Counters::take(&targets.pids);
    let system_start = host::system(&targets.pids);
    let resolved = RecordOptions {
        method: Some(method),
        targets: Some(targets),
        kind: Some(kind),
        dir: Some(parent.display().to_string()),
        name,
        max_duration_s: Some(max_duration_s),
        max_bytes: Some(max_bytes),
        split_bytes: Some(split_bytes),
        split_duration_s,
        cache_bytes: Some(cache_bytes),
        compression: Some(compression),
        bandwidth: Some(options.bandwidth.unwrap_or(false)),
        sched: Some(options.sched.unwrap_or(cx.armod)),
        malloc: Some(options.malloc.unwrap_or(false)),
    };
    let mut files = events.as_ref().map(|e| e.files.clone()).unwrap_or_default();
    files.push(WINDOWS.into());
    let meta = RecordMeta {
        schema: META_SCHEMA,
        armo_version: env!("CARGO_PKG_VERSION").into(),
        id: cx.id.clone(),
        host: cx.host,
        ros: Vec::new(),
        flux2_contract: None,
        options: resolved,
        methods_by_pid,
        owner: cx.owner,
        started: Instant {
            mono_ns: cx.mono_ns,
            real_ns: cx.real_ns,
        },
        ended: None,
        state: RecordState::Recording,
        end_reason: None,
        end_detail: None,
        probes_missing: Vec::new(),
        gaps: Vec::new(),
        counts: BTreeMap::new(),
        drops: RecordDrops {
            shm_ring: 0,
            armod_ring: 0,
            lttng: 0,
        },
        files,
        bytes: 0,
        system_start,
        system_end: BTreeMap::new(),
        interference: None,
        realtime_steps: Vec::new(),
        probe_changes: Vec::new(),
    };
    let mut active = Active {
        id: cx.id,
        dir,
        meta,
        limits: Limits {
            max_duration_s,
            max_bytes,
            disk_floor_bytes: floor,
        },
        events,
        windows,
        windows_bytes: 0,
        _lock: lock,
        meta_at: cx.mono_ns,
        disk_at: cx.mono_ns,
        open_gap: None,
        ctf_bytes: 0,
        end: None,
        names: None,
        counters,
    };
    active
        .write_meta()
        .map_err(|e| format!("{}: {e}", active.dir.display()))?;
    Ok(active)
}

impl Active {
    pub fn options(&self) -> &RecordOptions {
        &self.meta.options
    }

    pub fn kind(&self) -> RecordKind {
        self.meta.options.kind.expect("resolved at start")
    }

    pub fn method(&self) -> RecordMethod {
        self.meta.options.method.expect("resolved at start")
    }

    pub fn targets(&self) -> &Targets {
        self.meta
            .options
            .targets
            .as_ref()
            .expect("resolved at start")
    }

    pub fn targets_pid(&self, pid: u32) -> bool {
        let t = self.targets();
        every_process(t) || t.pids.contains(&pid)
    }

    /// Takes in the processes the name targets matched since the start; true if one is new.
    pub fn add_pids(
        &mut self,
        pids: &BTreeSet<u32>,
        armod: bool,
        preload: &dyn Fn(u32) -> bool,
        lttng: Option<&dyn Fn(u32) -> bool>,
    ) -> bool {
        let method = self.method();
        let mut new = false;
        for &pid in pids {
            let t = self
                .meta
                .options
                .targets
                .as_mut()
                .expect("resolved at start");
            if t.pids.contains(&pid) {
                continue;
            }
            t.pids.push(pid);
            self.counters.add_threads(pid);
            new = true;
            if let Some(m) = settle(method, pid, armod, preload, lttng) {
                self.meta.methods_by_pid.entry(pid.to_string()).or_insert(m);
            }
        }
        new
    }

    fn method_of(&self, pid: u32) -> Option<Method> {
        self.meta.methods_by_pid.get(&pid.to_string()).copied()
    }

    /// Whether `pid`'s shared memory file should be armed for this recording.
    pub fn wants_preload(&self, pid: u32) -> bool {
        self.end.is_none()
            && self.targets_pid(pid)
            && match self.method() {
                RecordMethod::Preload => true,
                RecordMethod::Auto => {
                    !matches!(self.method_of(pid), Some(Method::Uprobe | Method::Lttng))
                }
                _ => false,
            }
    }

    /// Whether this recording takes `pid`'s LTTng events, by choice or as settled for it.
    pub fn wants_lttng(&self, pid: u32) -> bool {
        self.end.is_none()
            && self.targets_pid(pid)
            && match self.method() {
                RecordMethod::Lttng => true,
                RecordMethod::Auto => self.method_of(pid) == Some(Method::Lttng),
                _ => false,
            }
    }

    /// The pids whose uprobe events this recording takes; `None` for every process.
    pub fn uprobe_pids(&self) -> Option<Vec<u32>> {
        if self.method() == RecordMethod::Uprobe && every_process(self.targets()) {
            return None;
        }
        Some(
            self.meta
                .methods_by_pid
                .iter()
                .filter(|(_, m)| **m == Method::Uprobe)
                .filter_map(|(pid, _)| pid.parse().ok())
                .collect(),
        )
    }

    /// Whether records of `pid` that came by `from` belong here; `auto` settles on the first
    /// method that delivers.
    pub fn takes(&mut self, pid: u32, from: Method) -> bool {
        if self.end.is_some() || !self.targets_pid(pid) {
            return false;
        }
        if let Some(m) = self.method_of(pid) {
            return m == from;
        }
        let ok = match self.method() {
            RecordMethod::Auto => from != Method::Uprobe,
            RecordMethod::Uprobe => from == Method::Uprobe,
            RecordMethod::Preload => from == Method::Preload,
            RecordMethod::Lttng => from == Method::Lttng,
        };
        if ok {
            self.meta.methods_by_pid.insert(pid.to_string(), from);
        }
        ok
    }

    pub fn methods(&self) -> Vec<Method> {
        let mut methods: Vec<Method> = self.meta.methods_by_pid.values().copied().collect();
        if self.method() == RecordMethod::Uprobe {
            methods.push(Method::Uprobe);
        }
        methods.sort();
        methods.dedup();
        methods
    }

    fn fail(&mut self, e: io::Error) {
        if self.end.is_none() {
            self.end = Some((EndReason::Error, Some(e.to_string())));
        }
    }

    /// `positioned` holds `n` records of the process `entity`.
    pub fn append_events(&mut self, entity: &str, positioned: &[u8], n: u64, now_ns: u64) {
        let Some(events) = self.events.as_mut() else {
            return;
        };
        if let Err(e) = events.append(positioned, now_ns) {
            return self.fail(e);
        }
        let new = !self.meta.counts.contains_key(entity);
        self.meta
            .counts
            .entry(entity.to_string())
            .or_insert(Counts { events: 0, lost: 0 })
            .events += n;
        // The meta names every process the event files hold, for a recount after a crash.
        if new && let Err(e) = self.write_meta() {
            self.fail(e);
        }
    }

    /// The `ctf/` directory LTTng chunks go to: an events recording of method lttng.
    pub fn ctf_dir(&self) -> Option<PathBuf> {
        (self.method() == RecordMethod::Lttng && self.kind() == RecordKind::Events)
            .then(|| self.dir.join(CTF))
    }

    /// An LTTng chunk of `bytes` went into `ctf/`.
    pub fn add_ctf(&mut self, bytes: u64) {
        self.ctf_bytes += bytes;
    }

    /// `n` records of `entity` that are in the recording's CTF rather than its event files.
    pub fn count_events(&mut self, entity: &str, n: u64) {
        if self.end.is_some() {
            return;
        }
        let new = !self.meta.counts.contains_key(entity);
        self.meta
            .counts
            .entry(entity.to_string())
            .or_insert(Counts { events: 0, lost: 0 })
            .events += n;
        if new && let Err(e) = self.write_meta() {
            self.fail(e);
        }
    }

    /// `n` records of `entity` the source could not deliver. armod's ring is one for every
    /// process, so its losses are the entity `armod`.
    pub fn add_lost(&mut self, entity: &str, n: u64, method: Method) {
        self.meta
            .counts
            .entry(entity.to_string())
            .or_insert(Counts { events: 0, lost: 0 })
            .lost += n;
        match method {
            Method::Preload => self.meta.drops.shm_ring += n,
            Method::Uprobe => self.meta.drops.armod_ring += n,
            Method::Lttng => self.meta.drops.lttng += n,
        }
    }

    pub fn add_probes_missing(&mut self, missing: &[ProbeMissing]) {
        for m in missing {
            if !self.meta.probes_missing.contains(m) {
                self.meta.probes_missing.push(m.clone());
            }
        }
    }

    pub fn gap_open(&mut self, source: &str, reason: &str, now_ns: u64) {
        if self.open_gap.is_none() && self.end.is_none() {
            self.open_gap = Some(Gap {
                from_ns: now_ns,
                to_ns: now_ns,
                source: source.into(),
                reason: reason.into(),
            });
        }
    }

    pub fn gap_close(&mut self, now_ns: u64) {
        if let Some(mut gap) = self.open_gap.take() {
            gap.to_ns = now_ns;
            self.meta.gaps.push(gap);
        }
    }

    pub fn append_window(&mut self, line: &[u8]) {
        if self.end.is_some() {
            return;
        }
        let r = self
            .windows
            .write_all(line)
            .and_then(|()| self.windows.write_all(b"\n"));
        match r {
            Ok(()) => self.windows_bytes += line.len() as u64 + 1,
            Err(e) => self.fail(e),
        }
    }

    pub fn bytes(&self) -> u64 {
        self.windows_bytes + self.ctf_bytes + self.events.as_ref().map_or(0, EventFiles::bytes)
    }

    /// Ends the recording when a limit is reached and rewrites the meta every 10 s.
    pub fn check(&mut self, now_ns: u64, disk: fn(&Path) -> io::Result<Disk>) {
        if self.end.is_some() {
            return;
        }
        let elapsed = now_ns - self.meta.started.mono_ns;
        if elapsed >= self.limits.max_duration_s * SEC {
            self.end = Some((EndReason::MaxDuration, None));
        } else if self.bytes() >= self.limits.max_bytes {
            self.end = Some((EndReason::MaxSize, None));
        } else if now_ns - self.disk_at >= DISK_EVERY_NS {
            self.disk_at = now_ns;
            match disk(&self.dir) {
                Ok(d) if d.avail < self.limits.disk_floor_bytes => {
                    let detail = format!(
                        "{} bytes free, under the floor {}",
                        d.avail, self.limits.disk_floor_bytes
                    );
                    self.end = Some((EndReason::DiskFloor, Some(detail)));
                }
                Ok(_) => {}
                Err(e) => self.fail(e),
            }
        }
        if self.end.is_none()
            && now_ns.saturating_sub(self.meta_at) >= META_EVERY_NS
            && let Err(e) = self.write_meta()
        {
            self.fail(e);
        }
    }

    pub fn write_meta(&mut self) -> io::Result<()> {
        self.meta.bytes = self.bytes();
        self.meta_at = sys::mono_ns();
        show::write_meta(&self.dir, &self.meta)
    }

    /// The last steps of finalizing; the server turned the sources off and drained them first.
    pub fn finalize(mut self, mono_ns: u64, real_ns: u64) -> Recording {
        let (mut reason, mut detail) = self.end.take().expect("finalize is for ended recordings");
        self.gap_close(mono_ns);
        let closed = match self.events.take() {
            Some(events) => {
                let files = events.files.clone();
                events.close().map(|bytes| (files, bytes))
            }
            None => Ok((Vec::new(), 0)),
        }
        .and_then(|r| self.windows.sync_all().map(|()| r));
        match closed {
            Ok((mut files, bytes)) => {
                files.push(WINDOWS.into());
                if self.ctf_dir().is_some() {
                    files.push(CTF.into());
                }
                self.meta.files = files;
                self.meta.bytes = bytes + self.windows_bytes + self.ctf_bytes;
            }
            Err(e) => {
                reason = EndReason::Error;
                detail = Some(e.to_string());
            }
        }
        let pids = self.targets().pids.clone();
        self.meta.interference = Some(self.counters.until(&host::Counters::take(&pids)));
        self.meta.system_end = host::system(&pids);
        self.meta.state = RecordState::Finalized;
        self.meta.end_reason = Some(reason);
        self.meta.end_detail = detail;
        self.meta.ended = Some(Instant { mono_ns, real_ns });
        let summary = self.summary(mono_ns);
        if let Err(e) =
            show::write_meta(&self.dir, &self.meta).and_then(|()| File::open(&self.dir)?.sync_all())
        {
            eprintln!("armo-collectd: {}: {e}", self.dir.display());
        }
        summary
    }

    pub fn summary(&self, now_ns: u64) -> Recording {
        summary(
            &self.meta,
            &self.dir,
            self.limits.clone(),
            self.bytes(),
            now_ns,
        )
    }
}

/// A recording as `state` and `record_list` show it.
pub fn summary(
    meta: &RecordMeta,
    dir: &Path,
    limits: Limits,
    bytes: u64,
    now_ns: u64,
) -> Recording {
    let end_ns = meta.ended.as_ref().map_or(now_ns, |e| e.mono_ns);
    let mut per_entity: Vec<EntityCount> = meta
        .counts
        .iter()
        .map(|(id, c)| EntityCount {
            id: id.clone(),
            events: c.events,
            lost: c.lost,
        })
        .collect();
    per_entity.sort_by_key(|e| e.lost == 0);
    let per_entity_truncated = per_entity.len() > PER_ENTITY_MAX;
    per_entity.truncate(PER_ENTITY_MAX);
    let mut methods: Vec<Method> = meta.methods_by_pid.values().copied().collect();
    if meta.options.method == Some(RecordMethod::Uprobe) {
        methods.push(Method::Uprobe);
    }
    methods.sort();
    methods.dedup();
    Recording {
        id: meta.id.clone(),
        state: meta.state,
        dir: dir.display().to_string(),
        kind: meta
            .options
            .kind
            .expect("start resolves the kind into the meta"),
        methods,
        targets: meta
            .options
            .targets
            .clone()
            .expect("start resolves the targets into the meta"),
        started_real_ns: meta.started.real_ns,
        elapsed_s: end_ns.saturating_sub(meta.started.mono_ns) as f64 / SEC as f64,
        bytes,
        limits,
        owner: meta.owner.clone(),
        totals: Counts {
            events: meta.counts.values().map(|c| c.events).sum(),
            lost: meta.counts.values().map(|c| c.lost).sum(),
        },
        per_entity,
        per_entity_truncated,
        end_reason: meta.end_reason,
    }
}
