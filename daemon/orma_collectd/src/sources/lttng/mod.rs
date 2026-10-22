//! The LTTng source. A worker thread owns the user session daemon, started
//! as orma-collectd's child under `LTTNG_HOME`, and at most one session, which orma-collectd names
//! and the guard knows. The session writes to the runtime directory and is rotated at each window:
//! each finished chunk is copied into the `ctf/` directory of every LTTng recording, then read
//! through babeltrace (`ctf`) into records for the model on a reader thread, and deleted. The
//! reader is apart so that rotations and copies keep their pace when a read is slow (babeltrace
//! prints about 400k events/s); a chunk the reader cannot take is deleted unread and reported.

pub mod ctf;

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use orma_common::catalog;

use crate::sys;

const APPS_EVERY: Duration = Duration::from_secs(2);
const ALL: &str = "orma:*,flux:*";

/// The LTTng names of the events that name and link entities (`catalog::init`).
fn init_events() -> String {
    catalog::CATALOG
        .iter()
        .filter(|e| catalog::init(e.id))
        .map(|e| match e.name.strip_prefix("flux_") {
            Some(flux) => format!("flux:{flux}"),
            None => format!("orma:{}", e.name),
        })
        .collect::<Vec<_>>()
        .join(",")
}
const SESSIOND_UP: Duration = Duration::from_secs(5);
/// Chunks waiting for the reader.
const READ_QUEUE: usize = 2;

/// What orma-collectd wants of LTTng now.
#[derive(Clone, Debug, PartialEq)]
pub struct Want {
    /// The session to have; `None` for none.
    pub session: Option<String>,
    /// Every event; without it only the ones that name and link entities, which a process emits
    /// once at its start, before orma-collectd could notice it and want the rest.
    pub all: bool,
    /// Also the ros2 events, for ros2_tracing's tools reading a recording's CTF.
    pub ros2: bool,
    /// The `ctf/` directories of the LTTng recordings.
    pub copy_to: Vec<PathBuf>,
}

pub enum ToWorker {
    Want(Want),
    /// A window ended: one chunk per window, so that no window gets two.
    Rotate,
    /// Rotate now and answer once the chunk is copied.
    Flush(Sender<()>),
}

pub enum FromWorker {
    /// A chunk went to these recording directories, with its bytes; the session's channels
    /// discarded `discarded` events since the last rotation.
    Copied {
        copied: Vec<(PathBuf, u64)>,
        discarded: u64,
    },
    /// The chunk's events and the nanoseconds it covers, from one rotation to the next.
    Chunk(ctf::Trace, u64),
    /// The processes registered with the session daemon.
    Apps(BTreeSet<u32>),
    /// The session is gone; the guard need not destroy it.
    Destroyed(String),
    Error(String),
}

pub struct Link {
    pub tx: Sender<ToWorker>,
    pub rx: Receiver<FromWorker>,
    thread: JoinHandle<()>,
}

pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

pub fn available() -> bool {
    ["lttng", "lttng-sessiond", "babeltrace", "setpriv"]
        .iter()
        .all(|p| on_path(p))
}

impl Link {
    /// Starts the worker, which starts the session daemon; `out` holds the session's output.
    pub fn spawn(home: PathBuf, out: PathBuf) -> Link {
        let (tx, worker_rx) = mpsc::channel();
        let (worker_tx, rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let closing = Arc::new(AtomicBool::new(false));
            let (worker, reader) = Worker::new(home, out, worker_tx, Arc::clone(&closing));
            worker.run(worker_rx);
            // Nothing takes a chunk's records any more: the reader only deletes what is queued.
            closing.store(true, Ordering::Relaxed);
            let _ = reader.join();
        });
        Link { tx, rx, thread }
    }

    /// Destroys the session, ends the session daemon and the worker; returns what the worker
    /// sent last.
    pub fn close(self) -> Vec<FromWorker> {
        drop(self.tx);
        let _ = self.thread.join();
        self.rx.try_iter().collect()
    }
}

/// `lttng` against orma's session daemon only: without `--no-sessiond` the client starts a
/// daemon of its own when orma's is gone, which nothing would end.
pub fn lttng(home: &Path, args: &[&str]) -> io::Result<String> {
    let out = Command::new("lttng")
        .arg("--no-sessiond")
        .args(args)
        .env("LTTNG_HOME", home)
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "lttng {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The pids of `lttng list -u`.
pub fn apps(listing: &str) -> BTreeSet<u32> {
    listing
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("PID: ")?
                .split(' ')
                .next()?
                .parse()
                .ok()
        })
        .collect()
}

/// The sum of the channels' `Discarded events` in `lttng list SESSION`.
/// The session's discarded events; `None` when lttng-tools 2.13 printed one of the counts with
/// bit 63 set (2^63 plus a plausible count, seen after bursts), which is no count at all.
fn discarded(listing: &str) -> Option<u64> {
    let counts: Vec<u64> = listing
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("Discarded events: ")?
                .parse::<u64>()
                .ok()
        })
        .collect();
    counts
        .iter()
        .all(|&n| n < 1 << 63)
        .then(|| counts.iter().sum())
}

/// The archive path `lttng rotate` prints.
fn archive(rotate: &str) -> Option<PathBuf> {
    rotate.lines().find_map(|l| {
        l.split_once("is now readable at ")
            .map(|(_, p)| p.trim().into())
    })
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut bytes = 0;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        bytes += if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?
        } else {
            std::fs::copy(entry.path(), &target)?
        };
    }
    Ok(bytes)
}

struct Session {
    name: String,
    dir: PathBuf,
    all: bool,
    ros2: bool,
    discarded: u64,
    /// When the chunk being written began: the session's start or its last rotation.
    rotated_ns: u64,
}

struct Worker {
    home: PathBuf,
    out: PathBuf,
    tx: Sender<FromWorker>,
    sessiond: Option<Child>,
    session: Option<Session>,
    want: Want,
    listed: Instant,
    /// Unbounded so that a removal never waits behind reads; `reads` bounds the chunks.
    reader: Sender<Job>,
    reads: Arc<AtomicUsize>,
}

enum Job {
    /// Read the chunk, then delete it; with the nanoseconds it covers.
    Read(PathBuf, u64),
    /// Delete a destroyed session's output, after the reads queued before.
    Remove(PathBuf),
}

fn reader(
    jobs: Receiver<Job>,
    tx: Sender<FromWorker>,
    reads: Arc<AtomicUsize>,
    closing: Arc<AtomicBool>,
) {
    for job in jobs {
        match job {
            Job::Read(chunk, _) if closing.load(Ordering::Relaxed) => {
                let _ = std::fs::remove_dir_all(&chunk);
            }
            Job::Read(chunk, span_ns) => {
                let m = match ctf::read_trace(&chunk, &closing) {
                    Ok(trace) => FromWorker::Chunk(trace, span_ns),
                    Err(e) => FromWorker::Error(format!("{}: {e}", chunk.display())),
                };
                let _ = tx.send(m);
                let _ = std::fs::remove_dir_all(&chunk);
                reads.fetch_sub(1, Ordering::Relaxed);
            }
            Job::Remove(dir) => {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
}

impl Worker {
    fn new(
        home: PathBuf,
        out: PathBuf,
        tx: Sender<FromWorker>,
        closing: Arc<AtomicBool>,
    ) -> (Worker, JoinHandle<()>) {
        let now = Instant::now();
        let (reader_tx, chunks) = mpsc::channel();
        let to_model = tx.clone();
        let reads = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&reads);
        let read = thread::spawn(move || reader(chunks, to_model, counter, closing));
        let worker = Worker {
            home,
            out,
            tx,
            sessiond: None,
            session: None,
            want: Want {
                session: None,
                all: false,
                ros2: false,
                copy_to: Vec::new(),
            },
            listed: now - APPS_EVERY,
            reader: reader_tx,
            reads,
        };
        (worker, read)
    }

    fn send(&self, m: FromWorker) {
        let _ = self.tx.send(m);
    }

    fn error(&self, e: impl std::fmt::Display) {
        self.send(FromWorker::Error(e.to_string()));
    }

    fn lttng(&self, args: &[&str]) -> io::Result<String> {
        lttng(&self.home, args)
    }

    /// Starts the session daemon as this thread's child, ended by SIGTERM when orma-collectd
    /// ends however it ends (`setpriv --pdeathsig`, which ties to the thread that forks).
    /// A daemon started right after the last one was killed exits at once while that one still
    /// holds its lock, so one that ends before it answers is started again.
    fn start_sessiond(&mut self) {
        let _ = std::fs::create_dir_all(&self.home);
        let start = Instant::now();
        loop {
            if self
                .sessiond
                .as_mut()
                .is_none_or(|c| c.try_wait().is_ok_and(|e| e.is_some()))
            {
                let child = Command::new("setpriv")
                    .args(["--pdeathsig", "TERM", "lttng-sessiond", "--no-kernel"])
                    .env("LTTNG_HOME", &self.home)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                match child {
                    Ok(c) => self.sessiond = Some(c),
                    Err(e) => return self.error(format!("lttng-sessiond: {e}")),
                }
            }
            if self.lttng(&["list"]).is_ok() {
                return;
            }
            if start.elapsed() > SESSIOND_UP {
                return self.error("lttng-sessiond did not come up in 5 s");
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn create(&mut self, name: &str) -> io::Result<()> {
        let dir = self.out.join(name);
        let d = dir.to_string_lossy().into_owned();
        self.lttng(&["create", name, "--output", &d])?;
        self.session = Some(Session {
            name: name.into(),
            dir,
            all: false,
            ros2: false,
            discarded: 0,
            rotated_ns: sys::mono_ns(),
        });
        self.lttng(&["enable-event", "-u", &init_events(), "-s", name])?;
        self.lttng(&["add-context", "-u", "-t", "vpid", "-t", "vtid", "-s", name])?;
        self.lttng(&["start", name])?;
        Ok(())
    }

    fn destroy(&mut self) {
        if let Some(s) = self.session.take() {
            if let Err(e) = self.lttng(&["destroy", &s.name]) {
                self.error(e);
            }
            let _ = self.reader.send(Job::Remove(s.dir));
            self.send(FromWorker::Destroyed(s.name));
        }
    }

    fn reconcile(&mut self) {
        let have = self.session.as_ref().map(|s| s.name.clone());
        if have != self.want.session {
            self.destroy();
            if let Some(name) = self.want.session.clone()
                && let Err(e) = self.create(&name)
            {
                self.error(e);
                self.destroy();
            }
        }
        if let Some(s) = self.session.as_ref()
            && self.want.all != s.all
        {
            let name = s.name.clone();
            let verb = if self.want.all {
                "enable-event"
            } else {
                "disable-event"
            };
            match self.lttng(&[verb, "-u", ALL, "-s", &name]) {
                Ok(_) => self.session.as_mut().expect("checked").all = self.want.all,
                Err(e) => self.error(e),
            }
        }
        if let Some(s) = self.session.as_ref()
            && self.want.ros2
            && !s.ros2
        {
            let name = s.name.clone();
            match self.lttng(&["enable-event", "-u", "ros2:*", "-s", &name]) {
                Ok(_) => self.session.as_mut().expect("checked").ros2 = true,
                Err(e) => self.error(e),
            }
        }
    }

    /// Closes the chunk written since the last rotation and copies it; `copied` is called then,
    /// before the chunk's records are read and handed on.
    fn rotate(&mut self, copied: impl FnOnce()) {
        let Some(name) = self.session.as_ref().map(|s| s.name.clone()) else {
            return copied();
        };
        let now = sys::mono_ns();
        let chunk = match self.lttng(&["rotate", &name]).map(|o| archive(&o)) {
            Ok(Some(path)) => path,
            Ok(None) => {
                self.error(format!("lttng rotate {name}: no archive path"));
                return copied();
            }
            Err(e) => {
                self.error(e);
                return copied();
            }
        };
        let total = match self.lttng(&["list", &name]) {
            Ok(listing) => discarded(&listing),
            Err(e) => {
                self.error(e);
                None
            }
        };
        if total.is_none() {
            self.error(format!(
                "lttng list {name}: no usable discarded count; the losses since the last one are \
                 counted at the next rotation"
            ));
        }
        let s = self.session.as_mut().expect("rotated it");
        let span_ns = now - s.rotated_ns;
        s.rotated_ns = now;
        let total = total.unwrap_or(s.discarded);
        let lost = total.saturating_sub(s.discarded);
        s.discarded = s.discarded.max(total);
        let mut done = Vec::new();
        for dir in &self.want.copy_to {
            let target = dir.join(chunk.file_name().unwrap_or_default());
            match copy_dir(&chunk, &target) {
                Ok(bytes) => done.push((dir.clone(), bytes)),
                Err(e) => self.error(format!("{}: {e}", target.display())),
            }
        }
        self.send(FromWorker::Copied {
            copied: done,
            discarded: lost,
        });
        copied();
        if self.reads.load(Ordering::Relaxed) < READ_QUEUE {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let _ = self.reader.send(Job::Read(chunk, span_ns));
        } else {
            self.error(format!(
                "{} not read: the reader is {READ_QUEUE} chunks behind; its events reach the \
                 recordings' CTF but not the model",
                chunk.display()
            ));
            let _ = std::fs::remove_dir_all(&chunk);
        }
    }

    fn list_apps(&mut self) {
        self.listed = Instant::now();
        match self.lttng(&["list", "-u"]) {
            Ok(listing) => self.send(FromWorker::Apps(apps(&listing))),
            Err(e) => self.error(e),
        }
    }

    fn run(mut self, rx: Receiver<ToWorker>) {
        self.start_sessiond();
        loop {
            match rx.recv_timeout(APPS_EVERY.saturating_sub(self.listed.elapsed())) {
                Ok(ToWorker::Want(w)) => {
                    self.want = w;
                    self.reconcile();
                }
                Ok(ToWorker::Rotate) => self.rotate(|| {}),
                Ok(ToWorker::Flush(done)) => self.rotate(|| {
                    let _ = done.send(());
                }),
                Err(RecvTimeoutError::Timeout) => self.list_apps(),
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        self.destroy();
        if let Some(mut c) = self.sessiond.take() {
            sys::terminate(c.id());
            let _ = c.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lttng_output() {
        let listing = "UST events:\n-------------\n\nPID: 3880525 - Name: sleep\n      \
                       flux:py_gil (loglevel: TRACE_DEBUG_LINE (13)) (type: tracepoint)\n\n\
                       PID: 12 - Name: /usr/bin/x\n";
        assert_eq!(apps(listing), BTreeSet::from([12, 3880525]));
        let list = "    Statistics:\n      Discarded events: 245476\n\n    Statistics:\n      \
                    Discarded events: 4\n";
        assert_eq!(discarded(list), Some(245480));
        let bogus = "      Discarded events: 9223372036856271975\n      Discarded events: 4\n";
        assert_eq!(discarded(bogus), None);
        let rotate = "Waiting for rotation to complete...\nTrace chunk archive for session t1 \
                      is now readable at /run/a/archives/20261009T001323+0900-20261009T001324+0900-0\n";
        assert_eq!(
            archive(rotate),
            Some(PathBuf::from(
                "/run/a/archives/20261009T001323+0900-20261009T001324+0900-0"
            ))
        );
    }
}
