//! The `LTTng` source in the server: which session orma-collectd wants, and what
//! its chunks bring: records for the model and the recordings, discarded events, missing
//! intercept points.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use orma_common::catalog;
use orma_proto::msg::*;
use orma_proto::records;

use super::Core;
use crate::record::{self, Active};
use crate::sources::lttng::ctf::Trace;
use crate::sources::lttng::{FromWorker, Link, ToWorker, Want};
use crate::sys;

const FLUSH_WAIT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(in crate::server) struct Src {
    link: Option<Link>,
    /// The processes registered with the session daemon.
    pub(super) apps: BTreeSet<u32>,
    sent: Option<Want>,
    session: Option<String>,
    /// Discarded events since the last window.
    discarded: u64,
    missing: BTreeMap<u32, BTreeSet<u16>>,
    /// The time the chunks taken in since the last window cover.
    pub(super) span_ns: u64,
}

impl Src {
    pub(super) fn on(&self) -> bool {
        self.link.is_some()
    }
}

/// Whether `r` takes `LTTng` records of some process.
fn lttng_recording(r: &Active) -> bool {
    r.method() == RecordMethod::Lttng || r.methods().contains(&Method::Lttng)
}

impl Core {
    pub(super) fn lttng_start(&mut self) {
        if self.lttng == LttngState::Available {
            let home = self.config.lttng_home();
            self.lt.link = Some(Link::spawn(home.clone(), home.join("trace")));
        }
    }

    /// Whether monitoring takes `pid`'s `LTTng` records: chosen with method lttng, or chosen
    /// without a method and not writing shared memory, where `LD_PRELOAD` comes first.
    pub(super) fn lttng_monitors(&self, pid: u32) -> bool {
        self.monitoring()
            && self.selected(pid)
            && match self.method_by_pid.get(&pid.to_string()) {
                Some(m) => *m == Method::Lttng,
                None => !self.shm.has_pid(pid),
            }
    }

    /// Whether monitoring or a recording takes `pid`'s `LTTng` records; its backend 2 file then
    /// lends ormad the current callbacks.
    pub(super) fn lttng_takes(&self, pid: u32) -> bool {
        self.lttng_monitors(pid) || self.active.iter().any(|r| r.wants_lttng(pid))
    }

    fn lttng_wanted(&self) -> bool {
        let apps = &self.lt.apps;
        let explicit = self
            .method_by_pid
            .iter()
            .any(|(p, m)| *m == Method::Lttng && p.parse().is_ok_and(|p| self.selected(p)));
        let monitor =
            self.monitoring() && (explicit || apps.iter().any(|p| self.lttng_monitors(*p)));
        let auto = |r: &Active| {
            r.method() == RecordMethod::Auto
                && apps.iter().any(|p| {
                    r.targets_pid(*p)
                        && !self.shm.has_pid(*p)
                        && r.meta
                            .methods_by_pid
                            .get(&p.to_string())
                            .is_none_or(|m| *m == Method::Lttng)
                })
        };
        monitor
            || self
                .active
                .iter()
                .any(|r| r.method() == RecordMethod::Lttng || auto(r))
    }

    /// Brings the session in line with monitoring and the recordings; part of `refresh`. The
    /// session lives as long as orma-collectd and takes the naming events of every process from
    /// its start: a process names its nodes and topics before the session could be made for it.
    pub(super) fn lttng_refresh(&mut self) {
        if !self.lt.on() {
            return;
        }
        if self.lt.session.is_none() {
            self.lt.session = Some(self.own_lttng_session());
        }
        let copy_to: Vec<PathBuf> = self.active.iter().filter_map(Active::ctf_dir).collect();
        let want = Want {
            session: self.lt.session.clone(),
            all: self.lttng_wanted(),
            ros2: !copy_to.is_empty(),
            copy_to,
        };
        if self.lt.sent.as_ref() != Some(&want)
            && let Some(link) = &self.lt.link
        {
            let _ = link.tx.send(ToWorker::Want(want.clone()));
            self.lt.sent = Some(want);
        }
    }

    /// A window ended: the session's chunk so far goes out, to reach the next window.
    pub(super) fn lttng_rotate(&self) {
        let taking = self
            .lt
            .sent
            .as_ref()
            .is_some_and(|w| w.all || !w.copy_to.is_empty());
        if let Some(link) = self.lt.link.as_ref().filter(|_| taking) {
            let _ = link.tx.send(ToWorker::Rotate);
        }
    }

    /// Takes in what the worker sent.
    pub(super) fn lttng_poll(&mut self) {
        let Some(link) = &self.lt.link else {
            return;
        };
        let messages: Vec<FromWorker> = link.rx.try_iter().collect();
        for m in messages {
            self.lttng_message(m);
        }
    }

    fn lttng_message(&mut self, m: FromWorker) {
        match m {
            FromWorker::Apps(apps) => {
                if apps != self.lt.apps {
                    self.lt.apps = apps;
                    self.refresh();
                }
            }
            FromWorker::Copied { copied, discarded } => self.lttng_copied(copied, discarded),
            FromWorker::Chunk(trace, span_ns) => {
                self.lt.span_ns += span_ns;
                self.lttng_chunk(trace);
            }
            FromWorker::Destroyed(name) => self.release_lttng_session(&name),
            FromWorker::Error(e) => self.event_line(Level::Warn, "lttng_error", e, Vec::new()),
        }
    }

    fn lttng_copied(&mut self, copied: Vec<(PathBuf, u64)>, discarded: u64) {
        for (dir, bytes) in &copied {
            for r in &mut self.active {
                if r.ctf_dir().as_ref() == Some(dir) {
                    r.add_ctf(*bytes);
                }
            }
        }
        if discarded > 0 {
            self.lt.discarded += discarded;
            for r in self.active.iter_mut().filter(|r| lttng_recording(r)) {
                r.add_lost("lttng", discarded, Method::Lttng);
            }
            let text = format!("LTTng discarded {discarded} events: its buffers were full");
            self.event_line(Level::Warn, "loss", text, Vec::new());
        }
    }

    fn lttng_chunk(&mut self, trace: Trace) {
        let now = sys::mono_ns();
        let mut by_pid: BTreeMap<u32, (bool, Vec<usize>, String)> = BTreeMap::new();
        let mut runs: Vec<(u32, usize, usize, u64)> = Vec::new();
        let mut at = 0;
        for r in records::positioned(&trace.records) {
            let Ok(r) = r else {
                self.event_line(
                    Level::Error,
                    "lttng_error",
                    "a converted LTTng chunk does not check".into(),
                    Vec::new(),
                );
                break;
            };
            let len = size_of::<orma_common::events::orma_position>() + r.header.len as usize;
            if let std::collections::btree_map::Entry::Vacant(v) = by_pid.entry(r.pid) {
                let monitored = self.lttng_monitors(r.pid);
                let takers = (0..self.active.len())
                    .filter(|i| self.active[*i].takes(r.pid, Method::Lttng))
                    .collect();
                let entity = record::entity(r.pid, sys::starttime(r.pid).unwrap_or(0));
                v.insert((monitored, takers, entity));
            }
            let (monitored, takers, _) = &by_pid[&r.pid];
            let taken = *monitored || !takers.is_empty();
            // As from a preload file: the session rotates for other processes while this one
            // names its entities before it is selected.
            if taken || catalog::init(r.header.event) {
                self.ingest.record(Method::Lttng, &r);
            }
            if taken {
                match runs.last_mut() {
                    Some((pid, _, end, n)) if *pid == r.pid && *end == at => {
                        *end += len;
                        *n += 1;
                    }
                    _ => runs.push((r.pid, at, at + len, 1)),
                }
            }
            at += len;
        }
        for (pid, start, end, n) in runs {
            let (_, takers, entity) = &by_pid[&pid];
            for i in takers {
                let r = &mut self.active[*i];
                if r.ctf_dir().is_some() {
                    r.count_events(entity, n);
                } else {
                    r.append_events(entity, &trace.records[start..end], n, now);
                }
            }
        }

        for (pid, event) in trace.missing {
            if self.lt.missing.entry(pid).or_default().insert(event) {
                let name = catalog::info(event).map_or("unknown", |i| i.name);
                let text = format!("{name}: liborma_lttng.so found no original function");
                let ids = vec![record::entity(pid, sys::starttime(pid).unwrap_or(0))];
                self.event_line(Level::Warn, "probe_missing", text, ids);
            }
        }
    }

    /// The window's `LTTng` part of `agent`, and the missing points of the recordings.
    pub(super) fn lttng_window(&mut self, w: &mut Window) {
        if !self.lt.on() {
            return;
        }
        w.agent.lttng_discarded = Some(std::mem::take(&mut self.lt.discarded));
        self.lt
            .missing
            .retain(|pid, _| sys::starttime(*pid).is_some());
        let missing: Vec<ProbeMissing> = self
            .lt
            .missing
            .iter()
            .flat_map(|(pid, events)| {
                events.iter().map(|e| ProbeMissing {
                    pid: Some(*pid),
                    method: Method::Lttng,
                    point: catalog::info(*e).map_or("unknown", |i| i.name).into(),
                    reason: "liborma_lttng.so found no original function".into(),
                })
            })
            .collect();
        for r in self.active.iter_mut().filter(|r| lttng_recording(r)) {
            r.add_probes_missing(&missing);
        }
        w.agent.probes_missing.extend(missing);
    }

    /// Before ending recordings: the events up to now reach them.
    pub(super) fn lttng_flush(&mut self) {
        let ending = self
            .active
            .iter()
            .any(|r| r.end.is_some() && lttng_recording(r));
        let Some(link) = &self.lt.link else {
            return;
        };
        if !ending || self.lt.session.is_none() {
            return;
        }
        let (done, flushed) = mpsc::channel();
        if link.tx.send(ToWorker::Flush(done)).is_ok() && flushed.recv_timeout(FLUSH_WAIT).is_err()
        {
            let text = "the last LTTng chunk did not come in 5 s".into();
            self.event_line(Level::Warn, "lttng_error", text, Vec::new());
        }
        self.lttng_poll();
    }

    /// Ends the session and the session daemon (shutdown).
    pub(super) fn lttng_close(&mut self) {
        if let Some(link) = self.lt.link.take() {
            for m in link.close() {
                if let FromWorker::Destroyed(name) = m {
                    self.release_lttng_session(&name);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use orma_common::events::*;
    use orma_proto::Ingest;
    use orma_proto::records::{Record, encode_positioned};
    use regex::Regex;

    use super::*;
    use crate::server::{Config, Model};
    use crate::sources::ormad::Sel;

    #[derive(Clone, Default)]
    struct Seen(Arc<Mutex<Vec<(u32, u16)>>>);

    impl Ingest for Seen {
        fn record(&mut self, _: Method, r: &Record<'_>) {
            self.0.lock().unwrap().push((r.pid, r.header.event));
        }
        fn aggregates(&mut self, _: Method, _: &[Aggregate]) {}
        fn window(&mut self, _: u64, _: u64, _: u64, _: bool) -> Window {
            unreachable!()
        }
    }

    impl Model for Seen {
        fn set_sched(&mut self, _: Sel) {}
        fn set_hot(&mut self, _: Sel) {}
        fn set_pss(&mut self, _: bool) {}
        fn set_expected_hz(&mut self, _: BTreeMap<String, f64>) {}
        fn set_lttng_span(&mut self, _: u64) {}
        fn unowned(&self) -> BTreeSet<u32> {
            BTreeSet::new()
        }
        fn set_probes(&mut self, _: bool, _: Sel) {}
        fn set_flows(&mut self, _: Vec<ChainSpec>, _: Vec<Regex>) {}
    }

    #[test]
    fn a_process_not_yet_monitored_keeps_its_names() {
        let dir = std::env::temp_dir().join(format!("orma_lttng_names_{}", std::process::id()));
        let config = Config {
            runtime_dir: dir.join("run"),
            data_dir: dir.join("data"),
            shm_dir: dir.join("shm"),
            ormad_socket: dir.join("ormad.sock"),
            caps: Default::default(),
            idle: Duration::from_secs(300),
            disk: sys::disk,
            guard: None,
            lttng: false,
        };
        let seen = Seen::default();
        let mut core = Core::new(config, Box::new(seen.clone()), None);
        let mut records = Vec::new();
        let node = orma_ev_rcl_node_init {
            node: 1,
            rmw_node: 2,
        };
        encode_positioned(
            &mut records,
            7,
            7,
            ORMA_EV_RCL_NODE_INIT,
            1,
            &node,
            &[b"n", b"/"],
        );
        let publish = orma_ev_rmw_publish::default();
        encode_positioned(&mut records, 7, 7, ORMA_EV_RMW_PUBLISH, 2, &publish, &[]);
        core.lttng_chunk(Trace {
            records,
            events: 2,
            skipped: 0,
            missing: Vec::new(),
        });
        assert_eq!(*seen.0.lock().unwrap(), [(7, ORMA_EV_RCL_NODE_INIT)]);
    }
}
