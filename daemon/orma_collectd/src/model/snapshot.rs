//! The window snapshot and per-node CPU.

use std::collections::{BTreeMap, BTreeSet};

use orma_common::shm::ORMA_SRC_FLUX;
use orma_proto::msg::{
    self, Agent, Callback, CpuSource, CudaUse, Drops, Executor, ExecutorKind, Flow, FlowKind, Hist,
    Node, ProcCuda, ProcUse, Process, ShmUse, Span, Subscription, Topic, Window,
};

use super::*;

/// Windows of callback history that decide whether a thread belongs to one node.
const HISTORY: usize = 10;
const PSS_EVERY_NS: u64 = 5_000_000_000;

fn pct(ns: u64, window_ns: u64) -> f64 {
    ns as f64 * 100.0 / window_ns as f64
}

fn hz(n: u64, window_ns: u64) -> f64 {
    n as f64 * 1e9 / window_ns as f64
}

impl Model {
    pub(super) fn proc_id(&self, pid: u32) -> String {
        format!("p:{pid}:{}", self.starttime(pid))
    }

    pub(super) fn thread_id(&self, pid: u32, tid: u32) -> String {
        format!("t:{pid}:{}:{tid}", self.starttime(pid))
    }

    pub(super) fn node_id(&self, ix: NodeIx) -> String {
        let n = &self.nodes[ix];
        let st = self.starttime(n.pid);
        match n.handle {
            Some(h) => format!("n:{}:{st}:{h:x}", n.pid),
            None => format!("n:{}:{st}:{}", n.pid, n.fqn),
        }
    }

    fn owner_id(&self, o: Owner) -> String {
        match o {
            Owner::Node(n) => self.node_id(n),
            Owner::Proc(p) => self.proc_id(p),
        }
    }

    pub(super) fn exec_id(&self, e: &ExecKey) -> String {
        let kind = if e.src == ORMA_SRC_FLUX {
            "flux"
        } else if self.exec_rclcpp.contains(e) {
            "rclcpp"
        } else {
            "rclpy"
        };
        format!(
            "e:{}:{}:{kind}:{:x}",
            e.pid,
            self.starttime(e.pid),
            e.handle
        )
    }

    pub(super) fn cb_id(&self, k: &CbKey) -> String {
        let st = self.starttime(k.pid);
        if k.src == ORMA_SRC_FLUX {
            format!("c:{}:{st}:flux:{:x}:{}", k.pid, k.key, k.entry)
        } else {
            format!("c:{}:{st}:ros:{:x}", k.pid, k.key)
        }
    }

    pub(super) fn topic_id(&self, ix: TopicIx) -> String {
        self.topics[ix].id()
    }

    pub(super) fn sub_id(&self, s: &SubKey) -> String {
        format!("s:{}#{}", self.topic_id(s.topic), self.owner_id(s.owner))
    }

    pub(super) fn flow_id(&self, f: &FlowKey) -> String {
        let kind = match f.kind {
            FlowKind::Sync => "sync",
            FlowKind::Stamp => "stamp",
            FlowKind::Blind => "blind",
            FlowKind::Latest => "latest",
        };
        let input = f.input.map(|i| self.topic_id(i)).unwrap_or_default();
        format!(
            "f:{kind}:{}#{input}#{}",
            self.owner_id(f.node),
            self.topic_id(f.output)
        )
    }

    /// Thread CPU, memory and liveness of every known process.
    pub(super) fn sample_proc(&mut self, now: u64) {
        let pids: Vec<u32> = self.procs.keys().copied().collect();
        for pid in pids {
            let st = self.procs[&pid].starttime;
            let stat = self.kept.read(pid, 0, "stat").and_then(procfs::parse_stat);
            if stat.is_none_or(|s| s.starttime() != st) {
                self.procs.get_mut(&pid).expect("listed").exited = true;
                self.kept.retain(pid, None);
                continue;
            }
            let tids = procfs::tids(pid);
            self.kept.retain(pid, Some(&tids));
            self.threads
                .retain(|&(p, t), _| p != pid || tids.binary_search(&t).is_ok());
            for tid in tids {
                let ns = self
                    .kept
                    .read(pid, tid, "schedstat")
                    .and_then(procfs::schedstat_ns);
                if let Some(ns) = ns {
                    self.thread_cpu(pid, tid, ns);
                }
            }
            let rss = self.kept.read(pid, 0, "statm").and_then(procfs::statm_rss);
            let pss_due = self.pss && {
                let p = &self.procs[&pid];
                p.pss.is_none() || now >= p.pss_at + PSS_EVERY_NS
            };
            let pss = pss_due.then(|| {
                self.kept
                    .read(pid, 0, "smaps_rollup")
                    .and_then(procfs::rollup_pss)
            });
            let p = self.procs.get_mut(&pid).expect("listed");
            p.rss = rss;
            match pss {
                Some(v) => (p.pss, p.pss_at) = (v, now),
                None if !self.pss => p.pss = None,
                None => {}
            }
        }
    }

    /// Off-CPU of an aggregate row has no thread; share it by each thread's wall time.
    fn split_offcpu(&mut self) {
        for (k, cw) in &self.win.cbs {
            let total: u64 = cw.wall_by_tid.values().sum();
            if cw.off == 0 || total == 0 {
                continue;
            }
            for (&tid, &wall) in &cw.wall_by_tid {
                let share = (u128::from(cw.off) * u128::from(wall) / u128::from(total)) as u64;
                self.win.threads.entry((k.pid, tid)).or_default().in_cb_off += share;
            }
        }
    }

    /// What turns a process's counts and callback times into the window's: its LTTng chunks
    /// cover their rotations rather than the window, whose length comes from ormad.
    fn scale(&self, pid: u32, window_ns: u64) -> f64 {
        let lttng = self
            .procs
            .get(&pid)
            .is_some_and(|p| p.methods.contains(&Method::Lttng));
        match lttng && self.lttng_span > 0 {
            true => window_ns as f64 / self.lttng_span as f64,
            false => 1.0,
        }
    }

    fn owner_pid(&self, owner: Owner) -> u32 {
        match owner {
            Owner::Node(n) => self.nodes[n].pid,
            Owner::Proc(pid) => pid,
        }
    }

    fn cb_oncpu(&self, pid: u32, cw: &CbWin, scale: f64) -> u64 {
        let wall = (cw.dur.sum as f64 * scale) as u64;
        if self.sched.contains(pid) {
            wall.saturating_sub(cw.off)
        } else {
            wall
        }
    }

    fn bucket(&self, pid: u32, tid: u32) -> (String, Option<NodeIx>) {
        let mut owners: BTreeSet<Option<NodeIx>> = self
            .threads
            .get(&(pid, tid))
            .map(|t| t.history.iter().flatten().copied().collect())
            .unwrap_or_default();
        if let Some(tw) = self.win.threads.get(&(pid, tid)) {
            owners.extend(tw.owners.iter().copied());
        }
        let mut it = owners.iter();
        match (it.next(), it.next()) {
            (None, _) => ("shared".into(), None),
            (Some(Some(n)), None) => (format!("node:{}", self.node_id(*n)), Some(*n)),
            _ => match self.threads.get(&(pid, tid)).and_then(|t| t.exec) {
                Some(e) => (format!("executor:{}", self.exec_id(&e)), None),
                None => ("shared".into(), None),
            },
        }
    }

    pub(super) fn snapshot(
        &mut self,
        t_mono: u64,
        t_real: u64,
        window_ns: u64,
        hists: bool,
    ) -> Window {
        self.split_offcpu();
        let mut hist_map: BTreeMap<String, BTreeMap<String, Hist>> = BTreeMap::new();
        let mut keep = |id: &str, field: &str, acc: &LatAcc| {
            if hists && acc.n > 0 {
                hist_map
                    .entry(id.to_string())
                    .or_default()
                    .insert(field.to_string(), acc.hist());
            }
        };

        let mut node_cpu: BTreeMap<NodeIx, u64> = BTreeMap::new();
        let mut proc_cpu: BTreeMap<u32, u64> = BTreeMap::new();
        let mut threads = Vec::new();
        let mut thread_keys: Vec<(u32, u32)> = self.threads.keys().copied().collect();
        thread_keys.sort_unstable();
        for (pid, tid) in thread_keys {
            let tw = self.win.threads.get(&(pid, tid));
            let cpu = tw.and_then(|w| w.cpu_ns.or(w.oncpu));
            let in_cb = tw.map(|w| {
                let off = if self.sched.contains(pid) {
                    w.in_cb_off
                } else {
                    0
                };
                ((w.in_cb_wall as f64 * self.scale(pid, window_ns)) as u64).saturating_sub(off)
            });
            let (bucket, dedicated) = self.bucket(pid, tid);
            if let Some(c) = cpu {
                *proc_cpu.entry(pid).or_default() += c;
                if let Some(n) = dedicated {
                    *node_cpu.entry(n).or_default() += c.saturating_sub(in_cb.unwrap_or(0));
                }
            }
            let id = self.thread_id(pid, tid);
            let cpu_wait = tw.and_then(|w| w.cpu_wait.lat());
            if let Some(w) = tw {
                keep(&id, "cpu_wait", &w.cpu_wait);
                keep(&id, "futex", &w.futex);
            }
            let stat = if self.live {
                self.kept
                    .read(pid, tid, "stat")
                    .and_then(procfs::parse_stat)
            } else {
                None
            };
            let mut cpus: Vec<u32> = tw
                .map(|w| w.cpus.iter().copied().collect())
                .unwrap_or_default();
            if let Some(s) = stat.as_ref().filter(|_| cpus.is_empty()) {
                cpus.push(s.processor());
            }
            threads.push(msg::Thread {
                id,
                tid,
                name: stat.as_ref().map(|s| s.comm.clone()).unwrap_or_default(),
                policy: stat.as_ref().map_or("unknown", |s| s.policy()).to_string(),
                prio: stat.as_ref().map_or(0, |s| s.prio()),
                cpu_pct: cpu.map(|c| pct(c, window_ns)),
                in_callback_cpu_pct: in_cb.map(|c| pct(c, window_ns)),
                cpu_wait,
                futex: tw.and_then(|w| w.futex.lat()),
                bucket,
                cpus,
            });
        }

        let mut cb_keys: BTreeSet<CbKey> = self.cbs.keys().copied().collect();
        cb_keys.extend(self.win.cbs.keys().copied());
        let cb_nodes: BTreeMap<CbKey, Option<NodeIx>> =
            cb_keys.iter().map(|k| (*k, self.cb_node(k))).collect();
        let empty = CbWin::default();
        let mut callbacks = Vec::new();
        let mut node_cbs: BTreeMap<NodeIx, (Vec<String>, f64, f64)> = BTreeMap::new();
        for k in &cb_keys {
            let cw = self.win.cbs.get(k).unwrap_or(&empty);
            let id = self.cb_id(k);
            let node = cb_nodes[k];
            let scale = self.scale(k.pid, window_ns);
            let oncpu = self.cb_oncpu(k.pid, cw, scale);
            if let Some(n) = node {
                *node_cpu.entry(n).or_default() += oncpu;
                let e = node_cbs.entry(n).or_default();
                e.0.push(id.clone());
                e.1 += cw.n as f64 * scale;
                e.2 += cw.dur.sum as f64 * scale;
            }
            let cb = self.cbs.get(k);
            let period = match cb.and_then(|c| c.owner) {
                Some(CbOwner::Timer(t)) => self.timers.get(&(k.pid, t)).and_then(|t| t.period),
                _ => None,
            };
            let group = match cb.and_then(|c| c.owner) {
                Some(CbOwner::Sub(obj)) => self.sub_groups.get(&(k.pid, obj)).copied(),
                Some(CbOwner::Timer(t)) => self.timers.get(&(k.pid, t)).and_then(|t| t.group),
                _ => None,
            };
            keep(&id, "duration", &cw.dur);
            keep(&id, "dispatch", &cw.dispatch);
            keep(&id, "age", &cw.age);
            let hot = self.hot.contains(k.pid);
            let runs = (hot && cw.n > 0).then_some(cw.n);
            let cuda_proc = self.gpu && self.procs.get(&k.pid).is_some_and(|p| p.cuda);
            callbacks.push(Callback {
                node: node.map(|n| self.node_id(n)),
                kind: self.cb_kind(k),
                topic: self.cb_topic(k).map(|t| self.topic_id(t)),
                service: match cb.and_then(|c| c.owner) {
                    Some(CbOwner::Service(s)) => self
                        .services
                        .get(&(k.pid, s))
                        .map(|s| s.1.clone())
                        .filter(|n| !n.is_empty()),
                    _ => None,
                },
                symbol: cb.and_then(|c| c.symbol.clone()),
                group: group.map(|g| format!("g:{}:{}:{g:x}", k.pid, self.starttime(k.pid))),
                executor: cb.and_then(|c| c.executor).map(|e| self.exec_id(&e)),
                hz: hot.then(|| hz(cw.n, window_ns) * scale),
                expected_hz: self
                    .expected
                    .get(&id)
                    .copied()
                    .or(period.filter(|p| *p > 0).map(|p| 1e9 / p as f64)),
                duration: cw.dur.lat().filter(|_| hot),
                oncpu_mean_ns: (hot && self.sched.contains(k.pid) && cw.n > 0)
                    .then(|| (oncpu as f64 / (cw.n as f64 * scale)) as u64),
                dispatch: cw.dispatch.lat().filter(|_| hot),
                age: cw.age.lat().filter(|_| hot),
                intra_pct: (cw.starts > 0).then(|| cw.intra as f64 * 100.0 / cw.starts as f64),
                nested: cw.nested,
                cuda: runs.filter(|_| cuda_proc).map(|n| CudaUse {
                    calls: (cw.cuda.calls as f64 * scale) as u64,
                    launches: (cw.cuda.launches as f64 * scale) as u64,
                    memcpy_bytes: (cw.cuda.memcpy_bytes as f64 * scale) as u64,
                    api_mean_ns: cw.cuda.ns / n,
                    sync_mean_ns: cw.cuda.sync_ns / n,
                }),
                futex_mean_ns: runs
                    .filter(|_| self.futex.contains(k.pid))
                    .map(|n| cw.futex / n),
                id,
            });
        }

        let mut nodes = Vec::new();
        for (ix, n) in self.nodes.iter().enumerate() {
            if !self.procs.contains_key(&n.pid) {
                continue;
            }
            let (cbs, count, busy) = node_cbs.remove(&ix).unwrap_or_default();
            let id = self.node_id(ix);
            let measured = self.hot.contains(n.pid);
            nodes.push(Node {
                fqn: n.fqn.clone(),
                pid: n.pid,
                cpu_pct: measured.then(|| pct(node_cpu.get(&ix).copied().unwrap_or(0), window_ns)),
                cpu_source: match self.sched.contains(n.pid) {
                    true => CpuSource::Sched,
                    false => CpuSource::WallBound,
                },
                busy_pct: measured.then(|| busy * 100.0 / window_ns as f64),
                cb_hz: measured.then(|| count * 1e9 / window_ns as f64),
                callbacks: cbs,
                malloc: self.win.malloc.get(&Owner::Node(ix)).map(|m| msg::Malloc {
                    alloc_bps: hz(m.alloc, window_ns),
                    free_bps: hz(m.free, window_ns),
                    alloc_hz: hz(m.alloc_n, window_ns),
                }),
                id,
            });
        }

        let mut processes = Vec::new();
        for (&pid, p) in &self.procs {
            let cpu = proc_cpu.get(&pid).copied();
            let nodes_sum: u64 = p
                .nodes
                .iter()
                .map(|n| node_cpu.get(n).copied().unwrap_or(0))
                .sum();
            let mut ros = p.ros.clone();
            if let (Some(r), Some(abi)) = (ros.as_mut(), p.abi) {
                r.publish_abi = abi;
            }
            processes.push(Process {
                id: self.proc_id(pid),
                pid,
                ns_pid: p.ns_pid,
                comm: p.comm.clone(),
                ros,
                methods_available: p.methods.iter().copied().collect(),
                method: p.method.filter(|_| self.hot.contains(pid)),
                cpu_pct: cpu.map(|c| pct(c, window_ns)),
                rss_bytes: p.rss,
                pss_bytes: p.pss,
                flux_gpu_wedged: p.gpu_wedged,
                cuda: (self.gpu && p.cuda && self.hot.contains(pid)).then(|| {
                    let empty = CudaWin::default();
                    let c = self.win.cuda.get(&pid).unwrap_or(&empty);
                    let scale = self.scale(pid, window_ns);
                    ProcCuda {
                        calls_hz: hz(c.calls, window_ns) * scale,
                        launch_hz: hz(c.launches, window_ns) * scale,
                        memcpy_bps: hz(c.memcpy_bytes, window_ns) * scale,
                        alloc_hz: hz(c.allocs, window_ns) * scale,
                        errors: c.errors,
                        sync: c.sync.lat(),
                    }
                }),
                gpu_mem_bytes: None,
                shared_cpu_pct: cpu
                    .filter(|_| self.hot.contains(pid))
                    .map(|c| pct(c.saturating_sub(nodes_sum), window_ns)),
                threads: threads
                    .iter()
                    .filter(|t| t.id.starts_with(&format!("t:{pid}:")))
                    .map(|t| t.id.clone())
                    .collect(),
                nodes: p.nodes.iter().map(|&n| self.node_id(n)).collect(),
                exited: p.exited,
                cold: p.cold,
                overflow: None,
            });
        }

        let mut exec_keys: BTreeSet<ExecKey> = self.exec_cbs.keys().copied().collect();
        exec_keys.extend(self.win.execs.keys().copied());
        let empty_exec = ExecWin::default();
        let mut executors = Vec::new();
        for e in &exec_keys {
            let ew = self.win.execs.get(e).unwrap_or(&empty_exec);
            let cbs: Vec<CbKey> = self
                .exec_cbs
                .get(e)
                .map(|s| s.iter().copied().collect())
                .unwrap_or_default();
            let mut enodes = BTreeSet::new();
            let mut groups = BTreeSet::new();
            for k in &cbs {
                if let Some(n) = cb_nodes.get(k).copied().flatten() {
                    enodes.insert(self.node_id(n));
                }
                if let Some(CbOwner::Sub(obj)) = self.cbs.get(k).and_then(|c| c.owner)
                    && let Some(g) = self.sub_groups.get(&(k.pid, obj))
                {
                    groups.insert(format!("g:{}:{}:{g:x}", k.pid, self.starttime(k.pid)));
                }
            }
            let id = self.exec_id(e);
            keep(&id, "wait", &ew.wait);
            keep(&id, "dispatch", &ew.dispatch);
            executors.push(Executor {
                kind: if e.src == ORMA_SRC_FLUX {
                    ExecutorKind::Flux
                } else if self.exec_rclcpp.contains(e) {
                    ExecutorKind::Rclcpp
                } else {
                    ExecutorKind::Rclpy
                },
                nodes: enodes.into_iter().collect(),
                groups: groups.into_iter().collect(),
                callbacks: cbs.iter().map(|k| self.cb_id(k)).collect(),
                busy_pct: Some(pct(ew.busy, window_ns) * self.scale(e.pid, window_ns)),
                oncpu_pct: self.sched.contains(e.pid).then(|| {
                    let busy = (ew.busy as f64 * self.scale(e.pid, window_ns)) as u64;
                    pct(busy.saturating_sub(ew.off), window_ns)
                }),
                wait: ew.wait.lat(),
                dispatch: ew.dispatch.lat(),
                id,
            });
        }

        let empty_topic = TopicWin::default();
        let mut topics = Vec::new();
        for (ix, t) in self.topics.iter().enumerate() {
            let tw = self.win.topics.get(&ix).unwrap_or(&empty_topic);
            let id = self.topic_id(ix);
            let pub_hz: f64 = tw
                .pubs
                .iter()
                .map(|(k, c)| hz(c[0].max(c[1]), window_ns) * self.scale(k.pid, window_ns))
                .sum();
            let timer = (t.timers.len() == 1)
                .then(|| *t.timers.first().expect("one"))
                .filter(|p| *p > 0);
            keep(&id, "hop", &tw.hop);
            keep(&id, "age_pub", &tw.age_pub);
            keep(&id, "age_take", &tw.age_take);
            keep(&id, "pub_to_dds", &tw.pub_to_dds);
            keep(&id, "dds_write", &tw.dds_write);
            topics.push(Topic {
                transport: t.transport,
                name: t.name.clone(),
                publishers: t.publishers.iter().map(|&o| self.owner_id(o)).collect(),
                subscribers: t.subscribers.iter().map(|&o| self.owner_id(o)).collect(),
                pub_hz: t
                    .publishers
                    .iter()
                    .all(|o| self.hot.contains(self.owner_pid(*o)))
                    .then_some(pub_hz),
                expected_hz: self
                    .expected
                    .get(&id)
                    .copied()
                    .or(timer.map(|p| 1e9 / p as f64)),
                bw_bps: tw.bytes.as_ref().map(|b| {
                    b.iter()
                        .map(|(p, n)| hz(*n, window_ns) * self.scale(*p, window_ns))
                        .sum()
                }),
                hop: tw.hop.lat(),
                age_pub: tw.age_pub.lat(),
                age_take: tw.age_take.lat(),
                drops: Drops {
                    publish: tw.drop_publish,
                    record: tw.drop_record,
                    ambiguous: (t.transport == Transport::Ros).then_some(tw.ambiguous),
                },
                pub_to_dds: tw.pub_to_dds.lat(),
                dds_write: tw.dds_write.lat(),
                id,
            });
        }

        let mut sub_keys = self.known_subs.clone();
        sub_keys.extend(self.win.subs.keys().copied());
        let empty_sub = SubWin::default();
        let mut subscriptions = Vec::new();
        for s in &sub_keys {
            let sw = self.win.subs.get(s).unwrap_or(&empty_sub);
            let topic = &self.topics[s.topic];
            let measured = self.hot.contains(self.owner_pid(s.owner));
            let connected = self.pair.connected.values().any(|c| c.contains_key(s));
            let lost = if topic.transport == Transport::Flux {
                sw.flux_lost
            } else if topic.rcl_abi {
                let pubs: u64 = self
                    .win
                    .topics
                    .get(&s.topic)
                    .map_or(0, |tw| tw.pubs.values().map(|c| c[0]).sum());
                (sw.takes > 0).then(|| pubs.saturating_sub(sw.takes))
            } else {
                connected.then_some(sw.lost)
            }
            .filter(|_| measured);
            let id = self.sub_id(s);
            keep(&id, "hop", &sw.hop);
            keep(&id, "wake", &sw.wake);
            subscriptions.push(Subscription {
                topic: self.topic_id(s.topic),
                node: self.owner_id(s.owner),
                recv_hz: measured.then(|| {
                    hz(sw.takes, window_ns) * self.scale(self.owner_pid(s.owner), window_ns)
                }),
                loss_pct: lost
                    .filter(|l| sw.takes + l > 0)
                    .map(|l| l as f64 * 100.0 / (sw.takes + l) as f64),
                lost,
                intra_pct: (sw.callbacks > 0)
                    .then(|| sw.intra as f64 * 100.0 / sw.callbacks as f64),
                hop: sw.hop.lat().filter(|_| measured),
                wake: sw.wake.lat().filter(|_| measured),
                id,
            });
        }

        let mut flow_keys: Vec<&FlowKey> = self.win.flows.keys().collect();
        flow_keys.sort();
        let mut flows = Vec::new();
        for fk in flow_keys {
            let fw = &self.win.flows[fk];
            let id = self.flow_id(fk);
            keep(&id, "dwell", &fw.dwell);
            flows.push(Flow {
                kind: fk.kind,
                node: self.owner_id(fk.node),
                input: fk.input.map(|i| self.topic_id(i)).unwrap_or_default(),
                output: self.topic_id(fk.output),
                count: fw.count,
                dwell: fw.dwell.lat(),
                ambiguous: fw.ambiguous,
                id,
            });
        }

        let mut spans = Vec::new();
        for (id, sw) in &self.win.spans {
            keep(id, "duration", &sw.dur);
            spans.push(Span {
                id: id.clone(),
                name: sw.name.clone(),
                pid: sw.pid,
                node: sw
                    .cb
                    .and_then(|k| cb_nodes.get(&k).copied().flatten())
                    .map(|n| self.node_id(n)),
                callback: sw.cb.map(|k| self.cb_id(&k)),
                parent: sw.parent.clone(),
                hz: hz(sw.dur.n, window_ns) * self.scale(sw.pid, window_ns),
                duration: sw.dur.lat().expect("a span ended to make the row"),
            });
        }

        self.seq += 1;
        Window {
            seq: self.seq,
            replay: false,
            t_mono_ns: t_mono,
            t_real_ns: t_real,
            window_ns,
            behind: false,
            processes,
            threads,
            nodes,
            executors,
            callbacks,
            topics,
            subscriptions,
            flows,
            spans,
            chains: self.chains.rows(),
            host: None,
            agent: Agent {
                ormad: None,
                collectd: ProcUse {
                    cpu_pct: None,
                    rss_bytes: if self.live {
                        std::fs::read_to_string("/proc/self/statm")
                            .ok()
                            .and_then(|s| procfs::statm_rss(&s))
                    } else {
                        None
                    },
                },
                bpf: Vec::new(),
                maps: Vec::new(),
                shm: ShmUse {
                    files: 0,
                    bytes: 0,
                    ring_drops: 0,
                },
                lttng_discarded: None,
                collect_ns: 0,
                probes_missing: Vec::new(),
            },
            hists: hists.then_some(hist_map),
        }
    }

    /// Deliveries whose pairing and callback are known go out as samples; a callback that never
    /// came is given up after two windows.
    pub(super) fn finish_deliveries(&mut self) {
        let mut out = Vec::new();
        self.deliveries.retain(|_, o| {
            o.windows += 1;
            if o.paired && (o.done || o.windows > 2) {
                out.push(Sample::Delivery(std::mem::take(&mut o.row)));
                return false;
            }
            true
        });
        for s in out {
            self.sample(s);
        }
    }

    pub(super) fn end_window(&mut self) {
        self.chains.end_window();
        let mut win = std::mem::take(&mut self.win);
        for (key, th) in &mut self.threads {
            let owners = win
                .threads
                .remove(key)
                .map(|w| w.owners)
                .unwrap_or_default();
            th.history.push_back(owners);
            while th.history.len() > HISTORY {
                th.history.pop_front();
            }
        }
        let gone: Vec<u32> = self
            .procs
            .iter()
            .filter(|(_, p)| p.exited)
            .map(|(&pid, _)| pid)
            .collect();
        for pid in gone {
            self.purge(pid);
        }
    }

    /// Everything about a process that exited, after the window that reported it.
    fn purge(&mut self, pid: u32) {
        self.procs.remove(&pid);
        let keep2 = |k: &(u32, u64)| k.0 != pid;
        self.pubs.retain(|k, _| keep2(k));
        self.rmw_pubs.retain(|k, _| keep2(k));
        self.subs.retain(|k, _| keep2(k));
        self.rmw_subs.retain(|k, _| keep2(k));
        self.rmw_topics.retain(|k, _| keep2(k));
        self.rclcpp_subs.retain(|k, _| keep2(k));
        self.sub_groups.retain(|k, _| keep2(k));
        self.timers.retain(|k, _| keep2(k));
        self.services.retain(|k, _| keep2(k));
        self.channels.retain(|k, _| keep2(k));
        self.endpoints.retain(|k, _| keep2(k));
        self.endpoint_signposts.retain(|k, _| k.0 != pid);
        self.node_ix.retain(|k, _| keep2(k));
        self.node_fqn.retain(|k, _| k.0 != pid);
        self.cbs.retain(|k, _| k.pid != pid);
        self.exec_cbs.retain(|k, _| k.pid != pid);
        self.exec_rclcpp.retain(|k| k.pid != pid);
        self.threads.retain(|k, _| k.0 != pid);
        self.tid_pid.retain(|_, p| *p != pid);
        self.last_stamp.retain(|k, _| k.pid != pid);
        let nodes = &self.nodes;
        let alive = |o: &Owner| match *o {
            Owner::Node(n) => nodes[n].pid != pid,
            Owner::Proc(p) => p != pid,
        };
        self.known_subs.retain(|s| alive(&s.owner));
        self.latest.retain(|n, _| nodes[*n].pid != pid);
        self.latest_match.retain(|n, _| nodes[*n].pid != pid);
        for c in self.pair.connected.values_mut() {
            c.retain(|s, _| alive(&s.owner));
        }
        self.pair.connected.retain(|k, _| k.pid != pid);
        for t in &mut self.topics {
            t.publishers.retain(|o| alive(o));
            t.subscribers.retain(|o| alive(o));
        }
    }
}
