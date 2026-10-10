//! One record at a time, in time order.

use armo_common::events::{ARMO_FLUX2_CLAIM_BACKPRESSURE, ARMO_FLUX2_LOAN_TOO_LARGE};
use armo_common::shm::*;
use armo_proto::msg::{Aggregate, FlowKind, Method, PublishAbi, Transport};

use super::decode::{Added, E, Ev};
use super::pair::{PairKey, Pending};
use super::stamp::decode as decode_stamp;
use super::symbols::shorten;
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Via {
    /// rmw (or rcl under the rcl ABI) and flux2.
    Main,
    Intra,
}

const NO_SYMBOL: &str = "DISABLED__demangle_symbol";

/// `/flux2.v<layout>.<flattened key>.<fingerprint>[.e<epoch>]` as key and fingerprint. The key
/// flattens only '/' to '.', so this restores it.
fn parse_signpost(name: &str) -> Option<(String, String, u64)> {
    let signpost = match name.rsplit_once(".e") {
        Some((s, epoch)) if !epoch.is_empty() && epoch.bytes().all(|b| b.is_ascii_digit()) => s,
        _ => name,
    };
    let rest = signpost.strip_prefix("/flux2.v")?;
    let (_, rest) = rest.split_once('.')?;
    let (flat, fp) = rest.rsplit_once('.')?;
    let fp = u64::from_str_radix(fp, 16).ok()?;
    Some((signpost.to_string(), flat.replace('.', "/"), fp))
}

impl Model {
    fn thread(&mut self, pid: u32, tid: u32) -> &mut Thread {
        self.threads.entry((pid, tid)).or_default()
    }

    /// Channels of one message type on different keys are different topics.
    fn flux2_topic(&mut self, key: &str, fp: u64) -> TopicIx {
        let ix = self.topic(Transport::Flux2, &format!("{key}:{fp:016x}"));
        let t = &mut self.topics[ix];
        t.name = key.to_string();
        t.fingerprint = fp;
        ix
    }

    fn flux2_win(&mut self, topic: TopicIx) -> &mut TopicWin {
        let w = self.win.topics.entry(topic).or_default();
        w.drop_publish.get_or_insert(0);
        w.drop_record.get_or_insert(0);
        w
    }

    pub(super) fn handle(&mut self, ev: Ev) {
        let Ev {
            pid,
            tid,
            t,
            method,
            e,
        } = ev;
        let sched = matches!(e, E::SchedSwitch { .. } | E::SchedWakeup { .. });
        if !sched {
            self.tid_pid.insert(tid, pid);
            let p = self.proc_mut(pid);
            p.methods.insert(method);
            p.method = Some(method);
        }
        if !matches!(
            e,
            E::RmwPublish { .. } | E::RmwPublishRcl | E::DdsSize { .. }
        ) && let Some((h, stamp, t0)) = self.thread(pid, tid).pending_rcl.take()
        {
            self.publish_rcl(pid, tid, t0, h, stamp);
        }
        let iv = self.records_intervals(method);
        match e {
            E::NodeInit { node, name, ns } => self.node_init(pid, node, name, ns),
            E::PublisherInit {
                handle,
                node,
                rmw,
                topic,
            } => {
                let topic = self.topic(Transport::Ros, &topic);
                self.pubs.insert((pid, handle), Ep { topic, node });
                self.rmw_pubs.insert((pid, rmw), handle);
                let owner = self.owner(pid, Some(node));
                self.topics[topic].publishers.insert(owner);
            }
            E::SubscriptionInit {
                handle,
                node,
                rmw,
                topic,
            } => {
                let topic = self.topic(Transport::Ros, &topic);
                self.subs.insert((pid, handle), Ep { topic, node });
                self.rmw_subs.insert((pid, rmw), handle);
                let owner = self.owner(pid, Some(node));
                self.topics[topic].subscribers.insert(owner);
                self.known_subs.insert(SubKey { topic, owner });
            }
            E::RmwTopic { rmw, topic } => {
                let topic = self.topic(Transport::Ros, &topic);
                self.rmw_topics.insert((pid, rmw), topic);
                self.proc_mut(pid).cold = true;
            }
            E::RclcppSubscriptionInit { rcl, rclcpp } => {
                self.rclcpp_subs.insert((pid, rclcpp), rcl);
            }
            E::ServiceInit { handle, node } => {
                self.services.insert((pid, handle), node);
            }
            E::TimerInit { timer, period } => {
                self.timers.entry((pid, timer)).or_default().period = Some(period);
            }
            E::TimerLinkNode { timer, node } => {
                let group = self.thread(pid, tid).pending_group_timer.take();
                let tm = self.timers.entry((pid, timer)).or_default();
                tm.node = Some(node);
                tm.group = group.or(tm.group);
            }
            E::TimerFini { timer } => {
                self.timers.remove(&(pid, timer));
                self.cbs
                    .retain(|k, c| k.pid != pid || c.owner != Some(CbOwner::Timer(timer)));
            }
            E::CallbackAdded {
                what,
                handle,
                callback,
            } => {
                self.cbs.entry(CbKey::ros(pid, callback)).or_default().owner = Some(match what {
                    Added::Sub => CbOwner::Sub(handle),
                    Added::Service => CbOwner::Service(handle),
                    Added::Timer => CbOwner::Timer(handle),
                });
            }
            E::CallbackRegister { callback, symbol } => {
                if !symbol.is_empty() && symbol != NO_SYMBOL {
                    self.cbs
                        .entry(CbKey::ros(pid, callback))
                        .or_default()
                        .symbol = Some(shorten(&symbol));
                }
            }
            E::CallbackCode { words } => self.callback_code(pid, words),
            E::GroupAddSubscription { group, rclcpp } => {
                self.sub_groups.insert((pid, rclcpp), group);
            }
            E::GroupAddTimer { group } => {
                self.thread(pid, tid).pending_group_timer = Some(group);
            }
            E::CallbackStart { callback, intra } => {
                self.callback_start(pid, tid, t, CbKey::ros(pid, callback), intra, iv);
            }
            E::CallbackEnd { callback } => {
                self.callback_end(pid, tid, t, CbKey::ros(pid, callback), true, iv);
            }
            E::RclcppExecutor => {
                let th = self.thread(pid, tid);
                th.rclcpp = true;
                if let Some(e) = th.exec.filter(|e| e.src == ARMO_SRC_ROS) {
                    self.exec_rclcpp.insert(e);
                }
            }
            E::RclPublish { publisher, stamp } => {
                self.thread(pid, tid).pending_rcl = Some((publisher, stamp, t));
            }
            E::RmwPublishRcl => {
                self.proc_mut(pid).abi = Some(PublishAbi::Rcl);
                if let Some((h, stamp, t0)) = self.thread(pid, tid).pending_rcl.take() {
                    self.publish_rcl(pid, tid, t0, h, stamp);
                }
            }
            E::RmwPublish {
                rmw,
                timestamp,
                stamp,
            } => self.publish_rmw(pid, tid, t, rmw, timestamp, stamp),
            E::IntraPublish { publisher, stamp } => {
                let Some(ep) = self.pubs.get(&(pid, publisher)).copied() else {
                    self.proc_mut(pid).cold = true;
                    return;
                };
                let owner = self.owner(pid, Some(ep.node));
                let key = PubKey {
                    pid,
                    handle: publisher,
                };
                self.publish(pid, tid, t, ep.topic, owner, key, stamp, Via::Intra);
            }
            E::RmwTake {
                rmw,
                source_timestamp,
                stamp,
            } => {
                let found = self
                    .rmw_subs
                    .get(&(pid, rmw))
                    .and_then(|rcl| self.subs.get(&(pid, *rcl)))
                    .map(|ep| (ep.topic, Some(ep.node)))
                    .or_else(|| self.rmw_topics.get(&(pid, rmw)).map(|&tp| (tp, None)));
                let Some((topic, node)) = found else {
                    self.proc_mut(pid).cold = true;
                    return;
                };
                let owner = self.owner(pid, node);
                let sub = SubKey { topic, owner };
                self.topics[topic].subscribers.insert(owner);
                let key = (source_timestamp != 0).then_some(PairKey::Ros(topic, source_timestamp));
                self.take(pid, tid, t, sub, stamp, key, Some(source_timestamp));
            }
            E::DdsSize { size } => {
                if let Some(topic) = self.thread(pid, tid).last_pub_topic {
                    *self
                        .win
                        .topics
                        .entry(topic)
                        .or_default()
                        .bytes
                        .get_or_insert_default()
                        .entry(pid)
                        .or_default() += size;
                }
            }
            E::WaitEnter { wait_set } => {
                let exec = ExecKey {
                    pid,
                    src: ARMO_SRC_ROS,
                    handle: wait_set,
                };
                let th = self.thread(pid, tid);
                th.wait_enter = Some(t);
                th.exec = Some(exec);
                if th.rclcpp {
                    self.exec_rclcpp.insert(exec);
                }
            }
            E::WaitExit { wait_set } => self.wake(pid, tid, t, ARMO_SRC_ROS, wait_set, iv),
            E::Flux2Wait { executor } => {
                let th = self.thread(pid, tid);
                th.wait_enter = Some(t);
                th.exec = Some(ExecKey {
                    pid,
                    src: ARMO_SRC_FLUX2,
                    handle: executor,
                });
            }
            E::Flux2Wake { executor } => self.wake(pid, tid, t, ARMO_SRC_FLUX2, executor, iv),
            E::ChannelInit {
                channel,
                dev,
                ino,
                publisher,
                segment,
            } => {
                let (signpost, key, fp) =
                    parse_signpost(&segment).unwrap_or((segment.clone(), segment.clone(), 0));
                let topic = self.flux2_topic(&key, fp);
                let endpoint = self
                    .endpoint_signposts
                    .get(&(pid, signpost.clone(), publisher))
                    .and_then(|v| (v.len() == 1).then(|| v[0]));
                self.channels.insert(
                    (pid, channel),
                    Channel {
                        dev,
                        ino,
                        signpost,
                        publisher,
                        topic,
                        endpoint,
                        lost: 0,
                    },
                );
            }
            E::EndpointInit {
                endpoint,
                fingerprint,
                publisher,
                node,
                key,
                signpost,
            } => self.endpoint_init(pid, endpoint, fingerprint, publisher, node, key, signpost),
            E::Claim { channel, result } => {
                if let Some(topic) = self.channels.get(&(pid, channel)).map(|c| c.topic) {
                    let w = self.flux2_win(topic);
                    if result >= ARMO_FLUX2_CLAIM_BACKPRESSURE {
                        *w.drop_publish.get_or_insert(0) += 1;
                    }
                }
            }
            E::LoanEnd { channel, result } => {
                if let Some(topic) = self.channels.get(&(pid, channel)).map(|c| c.topic) {
                    let w = self.flux2_win(topic);
                    if result >= ARMO_FLUX2_LOAN_TOO_LARGE {
                        *w.drop_publish.get_or_insert(0) += 1;
                    }
                }
            }
            E::RecordDrop { channel, count } => {
                if let Some(topic) = self.channels.get(&(pid, channel)).map(|c| c.topic) {
                    *self.flux2_win(topic).drop_record.get_or_insert(0) += u64::from(count);
                }
            }
            E::Commit {
                channel,
                ticket,
                nbytes,
                stamp,
            } => self.commit(pid, tid, t, channel, ticket, nbytes, stamp),
            E::Take {
                channel,
                ticket,
                lost,
                stamp,
            } => self.flux2_take(pid, tid, t, channel, ticket, lost, stamp),
            E::Flux2Start {
                executor,
                entry,
                endpoint,
            } => {
                let key = CbKey::flux2(pid, executor, entry);
                let cb = self.cbs.entry(key).or_default();
                if endpoint != 0 {
                    cb.owner = Some(CbOwner::Endpoint(endpoint));
                }
                cb.executor = Some(ExecKey {
                    pid,
                    src: ARMO_SRC_FLUX2,
                    handle: executor,
                });
                self.callback_start(pid, tid, t, key, false, iv);
            }
            E::Flux2End {
                executor,
                entry,
                delivered,
                channel,
                segment,
            } => {
                let key = CbKey::flux2(pid, executor, entry);
                let owner = self.cbs.get(&key).and_then(|c| c.owner);
                match (owner, self.channels.get_mut(&(pid, channel))) {
                    (Some(CbOwner::Endpoint(ep)), Some(ch)) => ch.endpoint = Some(ep),
                    (None, None) => {
                        if let Some((_, k, fp)) = parse_signpost(&segment) {
                            let topic = self.flux2_topic(&k, fp);
                            self.cbs.entry(key).or_default().late_topic = Some(topic);
                            self.proc_mut(pid).cold = true;
                        }
                    }
                    _ => {}
                }
                self.callback_end(pid, tid, t, key, delivered, iv);
            }
            E::SchedSwitch { next_tid, cpu } => {
                if !self.live {
                    self.sched_switch(pid, tid, t, next_tid, cpu);
                }
            }
            E::SchedWakeup { tid: woken } => {
                if !self.live
                    && let Some(&wpid) = self.tid_pid.get(&woken)
                {
                    self.thread(wpid, woken).woken_at = Some(t);
                }
            }
        }
    }

    fn node_init(&mut self, pid: u32, node: u64, name: String, ns: String) {
        let fqn = if ns.ends_with('/') {
            format!("{ns}{name}")
        } else {
            format!("{ns}/{name}")
        };
        let named = self.node_fqn.entry((pid, fqn.clone())).or_default();
        if let Some(&ix) = named.iter().find(|&&ix| self.nodes[ix].handle.is_none()) {
            self.nodes[ix].handle = Some(node);
            self.node_ix.insert((pid, node), ix);
            return;
        }
        let ix = self.node(pid, node);
        self.nodes[ix].fqn = fqn.clone();
        self.node_fqn.entry((pid, fqn)).or_default().push(ix);
    }

    #[allow(clippy::too_many_arguments)]
    fn endpoint_init(
        &mut self,
        pid: u32,
        endpoint: u64,
        fingerprint: u64,
        publisher: bool,
        node: String,
        key: String,
        signpost: String,
    ) {
        let topic = self.flux2_topic(&key, fingerprint);
        let same = self
            .endpoint_signposts
            .entry((pid, signpost.clone(), publisher))
            .or_default();
        same.push(endpoint);
        let unique = same.len() == 1;
        for ch in self.channels.iter_mut().filter_map(|((p, _), c)| {
            (*p == pid && c.signpost == signpost && c.publisher == publisher).then_some(c)
        }) {
            ch.endpoint = if unique { Some(endpoint) } else { None };
        }
        let owner = if node.starts_with('/') {
            self.node_by_fqn(pid, &node)
                .map_or(Owner::Proc(pid), Owner::Node)
        } else {
            Owner::Proc(pid)
        };
        let t = &mut self.topics[topic];
        if publisher {
            t.publishers.insert(owner);
        } else {
            t.subscribers.insert(owner);
            self.known_subs.insert(SubKey { topic, owner });
        }
        self.endpoints
            .insert((pid, endpoint), Endpoint { node, topic });
    }

    fn endpoint_owner(&mut self, pid: u32, endpoint: Option<u64>) -> Owner {
        let fqn = endpoint
            .and_then(|e| self.endpoints.get(&(pid, e)))
            .map(|e| e.node.clone())
            .filter(|n| n.starts_with('/'));
        fqn.and_then(|n| self.node_by_fqn(pid, &n))
            .map_or(Owner::Proc(pid), Owner::Node)
    }

    fn callback_code(&mut self, pid: u32, words: [u64; 6]) {
        let key = CbKey::ros(pid, words[0]);
        let cb = self.cbs.entry(key).or_default();
        if !self.live || cb.symbol.is_some() || cb.code_tried {
            return;
        }
        cb.code_tried = true;
        let mut name = self.resolver.resolve(pid, &words);
        if name.is_none() {
            self.resolver.forget(pid);
            name = self.resolver.resolve(pid, &words);
        }
        self.cbs.entry(key).or_default().symbol = name;
    }

    fn wake(&mut self, pid: u32, tid: u32, t: u64, src: u16, handle: u64, iv: bool) {
        let exec = ExecKey { pid, src, handle };
        let th = self.thread(pid, tid);
        let enter = th.wait_enter.take();
        th.last_wake = Some(t);
        th.exec = Some(exec);
        if let Some(t0) = enter.filter(|_| iv) {
            self.win.execs.entry(exec).or_default().wait.add(t - t0);
            let executor = self.exec_id(&exec);
            self.sample(Sample::Wait {
                executor,
                ns: t - t0,
            });
        }
    }

    fn callback_start(&mut self, pid: u32, tid: u32, t: u64, key: CbKey, intra: bool, iv: bool) {
        if !self.cbs.contains_key(&key) {
            self.proc_mut(pid).cold = true;
        }
        let node = self.cb_node(&key);
        let topic = self.cb_topic(&key);
        let clock = self.clock;
        let th = self.threads.entry((pid, tid)).or_default();
        let nested = !th.stack.is_empty();
        let mut input = th.last_take.filter(|tk| tk.t >= th.last_cb_end && !nested);
        if input.is_some() && key.src == ARMO_SRC_FLUX2 {
            input = None;
        }
        let dispatch = th.last_wake.filter(|_| iv).map(|w| t.saturating_sub(w));
        let delivery = if key.src == ARMO_SRC_ROS && input.is_some() {
            th.delivery.take()
        } else {
            None
        };
        let exec = if key.src == ARMO_SRC_ROS {
            th.exec
        } else {
            self.cbs.get(&key).and_then(|c| c.executor)
        };
        let cb = self.cbs.entry(key).or_default();
        cb.executor = exec.or(cb.executor);
        if cb.owner.is_none()
            && let Some(tk) = input
        {
            cb.late_topic = Some(tk.topic);
        }
        if input.is_none()
            && key.src == ARMO_SRC_ROS
            && let Some(topic) = topic.filter(|_| matches!(cb.owner, Some(CbOwner::Sub(_))))
        {
            let owner = node.map_or(Owner::Proc(pid), Owner::Node);
            input = Some(Input {
                topic,
                sub: SubKey { topic, owner },
                t,
                stamp: None,
            });
        }
        if let Some(e) = exec {
            self.exec_cbs.entry(e).or_default().insert(key);
        }
        let cw = self.win.cbs.entry(key).or_default();
        cw.starts += 1;
        cw.nested |= nested;
        if intra {
            cw.intra += 1;
        }
        if let Some(stamp) = input.and_then(|i| i.stamp)
            && let Some(age) = clock.age(stamp, t)
        {
            cw.age.add(age);
        }
        if let Some(inp) = input {
            let sw = self.win.subs.entry(inp.sub).or_default();
            sw.callbacks += 1;
            sw.intra += u64::from(intra);
        }
        self.win
            .threads
            .entry((pid, tid))
            .or_default()
            .owners
            .insert(node);
        if let Some(o) = delivery.and_then(|d| self.deliveries.get_mut(&d)) {
            o.row.dispatch_ns = o.t_wake.map(|w| t.saturating_sub(w));
        }
        self.thread(pid, tid).stack.push(Frame {
            cb: key,
            t,
            input,
            outputs: Vec::new(),
            dispatch,
            off: 0,
            delivery,
        });
    }

    fn callback_end(&mut self, pid: u32, tid: u32, t: u64, key: CbKey, counted: bool, iv: bool) {
        let th = self.thread(pid, tid);
        let Some(pos) = th.stack.iter().rposition(|f| f.cb == key) else {
            return;
        };
        th.stack.truncate(pos + 1);
        let frame = th.stack.pop().expect("found above");
        let top = th.stack.is_empty();
        th.last_cb_end = t;
        let exec = th.exec;
        if !counted {
            return;
        }
        let dur = t.saturating_sub(frame.t);
        let node = self.cb_node(&key);
        let owner = node.map_or(Owner::Proc(pid), Owner::Node);
        if iv {
            let cw = self.win.cbs.entry(key).or_default();
            cw.n += 1;
            cw.dur.add(dur);
            if let Some(d) = frame.dispatch {
                cw.dispatch.add(d);
            }
            if top {
                self.win.threads.entry((pid, tid)).or_default().in_cb_wall += dur;
            }
            if let Some(e) = self.cbs.get(&key).and_then(|c| c.executor).or(exec) {
                let ew = self.win.execs.entry(e).or_default();
                if let Some(d) = frame.dispatch {
                    ew.dispatch.add(d);
                }
                if top {
                    ew.busy += dur;
                }
            }
        }
        let timer = match self.cbs.get(&key).and_then(|c| c.owner) {
            Some(CbOwner::Timer(tm)) => self.timers.get(&(pid, tm)).and_then(|t| t.period),
            _ => None,
        };
        for out in &frame.outputs {
            if let Some(period) = timer {
                self.topics[out.topic].timers.insert(period);
            }
            let Some(input) = frame.input else {
                continue;
            };
            let dwells = if out.intra.len() > out.main.len() {
                &out.intra
            } else {
                &out.main
            };
            let fk = FlowKey {
                kind: FlowKind::Sync,
                node: owner,
                input: Some(input.topic),
                output: out.topic,
            };
            let fw = self.win.flows.entry(fk).or_default();
            fw.count += dwells.len() as u64;
            for &d in dwells {
                fw.dwell.add(d);
            }
            if self.samples.is_some() {
                let id = self.flow_id(&fk);
                for &d in dwells {
                    self.sample(Sample::Flow {
                        id: id.clone(),
                        dwell_ns: d,
                    });
                }
            }
        }
        if self.samples.is_some() {
            let id = self.cb_id(&key);
            let node_id = node.map(|n| self.node_id(n));
            if let Some(o) = frame.delivery.and_then(|d| self.deliveries.get_mut(&d)) {
                o.row.callback = Some(id.clone());
                o.row.callback_ns = Some(dur);
                o.done = true;
            }
            let sched = self.sched.contains(pid);
            self.sample(Sample::Callback {
                id,
                node: node_id,
                tid,
                t_ns: frame.t,
                duration_ns: dur,
                oncpu_ns: sched.then(|| dur.saturating_sub(frame.off)),
                dispatch_ns: frame.dispatch,
            });
        }
    }

    fn publish_rmw(&mut self, pid: u32, tid: u32, t: u64, rmw: u64, timestamp: i64, stamp: u64) {
        self.thread(pid, tid).pending_rcl = None;
        self.proc_mut(pid).abi = Some(PublishAbi::Rmw);
        let rcl = self.rmw_pubs.get(&(pid, rmw)).copied();
        let found = rcl
            .and_then(|h| self.pubs.get(&(pid, h)))
            .map(|ep| (ep.topic, Some(ep.node)))
            .or_else(|| self.rmw_topics.get(&(pid, rmw)).map(|&tp| (tp, None)));
        let Some((topic, node)) = found else {
            self.proc_mut(pid).cold = true;
            return;
        };
        let owner = self.owner(pid, node);
        let key = PubKey {
            pid,
            handle: rcl.unwrap_or(rmw),
        };
        self.publish(pid, tid, t, topic, owner, key, stamp, Via::Main);
        if timestamp != 0 {
            self.pair
                .publish(PairKey::Ros(topic, timestamp), t, key, topic);
        }
    }

    fn publish_rcl(&mut self, pid: u32, tid: u32, t: u64, handle: u64, stamp: u64) {
        let Some(ep) = self.pubs.get(&(pid, handle)).copied() else {
            self.proc_mut(pid).cold = true;
            return;
        };
        self.topics[ep.topic].rcl_abi = true;
        let owner = self.owner(pid, Some(ep.node));
        let key = PubKey { pid, handle };
        self.publish(pid, tid, t, ep.topic, owner, key, stamp, Via::Main);
    }

    #[allow(clippy::too_many_arguments)]
    fn commit(
        &mut self,
        pid: u32,
        tid: u32,
        t: u64,
        channel: u64,
        ticket: u64,
        nbytes: u64,
        stamp: u64,
    ) {
        let Some(ch) = self.channels.get(&(pid, channel)) else {
            self.proc_mut(pid).cold = true;
            return;
        };
        let (topic, dev, ino, endpoint) = (ch.topic, ch.dev, ch.ino, ch.endpoint);
        let owner = self.endpoint_owner(pid, endpoint);
        *self
            .flux2_win(topic)
            .bytes
            .get_or_insert_default()
            .entry(pid)
            .or_default() += nbytes;
        let key = PubKey {
            pid,
            handle: channel,
        };
        self.publish(pid, tid, t, topic, owner, key, stamp, Via::Main);
        self.pair
            .publish(PairKey::Flux2(dev, ino, ticket), t, key, topic);
    }

    #[allow(clippy::too_many_arguments)]
    fn publish(
        &mut self,
        pid: u32,
        tid: u32,
        t: u64,
        topic: TopicIx,
        owner: Owner,
        key: PubKey,
        stamp: u64,
        via: Via,
    ) {
        self.topics[topic].publishers.insert(owner);
        if via == Via::Main
            && self.topics[topic].transport == Transport::Ros
            && self.topics[topic].name == "/clock"
        {
            self.clock.sim_ns = decode_stamp(stamp).or(self.clock.sim_ns);
        }
        let gate = self.clock.gate(stamp, t);
        let tw = self.win.topics.entry(topic).or_default();
        tw.pubs.entry(key).or_default()[usize::from(via == Via::Intra)] += 1;
        if via == Via::Main
            && let Some((_, age)) = gate
        {
            tw.age_pub.add(age);
        }
        let th = self.threads.entry((pid, tid)).or_default();
        th.last_pub_topic = Some(topic);
        let sync_input = th.stack.last().and_then(|f| f.input).map(|i| i.topic);
        if let Some(f) = th.stack.last_mut() {
            let dwell = f.input.map_or(0, |i| t.saturating_sub(i.t));
            let pos = match f.outputs.iter().position(|o| o.topic == topic) {
                Some(p) => p,
                None => {
                    f.outputs.push(Out {
                        topic,
                        main: Vec::new(),
                        intra: Vec::new(),
                    });
                    f.outputs.len() - 1
                }
            };
            match via {
                Via::Main => f.outputs[pos].main.push(dwell),
                Via::Intra => f.outputs[pos].intra.push(dwell),
            }
        }
        let (Owner::Node(node), Some((ns, _))) = (owner, gate) else {
            return;
        };
        if self.last_stamp.insert(key, ns) == Some(ns) {
            return;
        }
        match self.stamps.find(node, ns).map(|s| (s.inputs.clone(), s.t)) {
            Some((inputs, t_take)) if inputs.len() == 1 => {
                let fk = FlowKey {
                    kind: FlowKind::Stamp,
                    node: owner,
                    input: Some(inputs[0]),
                    output: topic,
                };
                let fw = self.win.flows.entry(fk).or_default();
                fw.count += 1;
                fw.dwell.add(t.saturating_sub(t_take));
                if self.samples.is_some() {
                    let id = self.flow_id(&fk);
                    self.sample(Sample::Flow {
                        id,
                        dwell_ns: t.saturating_sub(t_take),
                    });
                }
            }
            Some((inputs, _)) => {
                for input in inputs {
                    let fk = FlowKey {
                        kind: FlowKind::Stamp,
                        node: owner,
                        input: Some(input),
                        output: topic,
                    };
                    self.win.flows.entry(fk).or_default().ambiguous += 1;
                }
            }
            None if self.stamps.stamped.contains(&node) => {
                let fk = FlowKey {
                    kind: FlowKind::Blind,
                    node: owner,
                    input: sync_input,
                    output: topic,
                };
                self.win.flows.entry(fk).or_default().count += 1;
            }
            None => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn flux2_take(
        &mut self,
        pid: u32,
        tid: u32,
        t: u64,
        channel: u64,
        ticket: u64,
        lost: u64,
        stamp: u64,
    ) {
        let Some(ch) = self.channels.get_mut(&(pid, channel)) else {
            self.proc_mut(pid).cold = true;
            return;
        };
        let new_lost = lost - ch.lost;
        ch.lost = lost;
        let (topic, dev, ino) = (ch.topic, ch.dev, ch.ino);
        let frame_ep = self
            .threads
            .get(&(pid, tid))
            .and_then(|th| th.stack.last())
            .and_then(|f| self.cbs.get(&f.cb))
            .and_then(|c| match c.owner {
                Some(CbOwner::Endpoint(e)) => Some(e),
                _ => None,
            });
        let endpoint = frame_ep.or(ch.endpoint);
        let owner = self.endpoint_owner(pid, endpoint);
        let sub = SubKey { topic, owner };
        self.topics[topic].subscribers.insert(owner);
        self.flux2_win(topic);
        *self
            .win
            .subs
            .entry(sub)
            .or_default()
            .flux2_lost
            .get_or_insert(0) += new_lost;
        self.take(
            pid,
            tid,
            t,
            sub,
            stamp,
            Some(PairKey::Flux2(dev, ino, ticket)),
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn take(
        &mut self,
        pid: u32,
        tid: u32,
        t: u64,
        sub: SubKey,
        stamp: u64,
        key: Option<PairKey>,
        source_ts: Option<i64>,
    ) {
        self.known_subs.insert(sub);
        self.win.subs.entry(sub).or_default().takes += 1;
        let gate = self.clock.gate(stamp, t);
        if let Some((ns, age)) = gate {
            self.win
                .topics
                .entry(sub.topic)
                .or_default()
                .age_take
                .add(age);
            if let Owner::Node(n) = sub.owner {
                self.stamps.take(n, ns, sub.topic, t);
            }
        }
        let input = Input {
            topic: sub.topic,
            sub,
            t,
            stamp: gate.map(|g| g.0),
        };
        let clock = self.clock;
        let th = self.threads.entry((pid, tid)).or_default();
        th.last_take = Some(input);
        let wake = th.last_wake;
        let mut in_flux2 = false;
        if let Some(f) = th
            .stack
            .last_mut()
            .filter(|f| f.cb.src == ARMO_SRC_FLUX2 && f.input.is_none())
        {
            f.input = Some(input);
            in_flux2 = true;
            if let Some(age) = input.stamp.and_then(|s| clock.age(s, f.t)) {
                let cb = f.cb;
                self.win.cbs.entry(cb).or_default().age.add(age);
            }
        }
        let delivery = self.samples.is_some().then(|| {
            let id = self.next_delivery;
            self.next_delivery += 1;
            let row = Delivery {
                topic: self.topic_id(sub.topic),
                subscription: self.sub_id(&sub),
                sub_pid: pid,
                sub_tid: tid,
                t_take_ns: t,
                ..Delivery::default()
            };
            self.deliveries.insert(
                id,
                Open {
                    row,
                    paired: false,
                    done: false,
                    windows: 0,
                    t_wake: wake,
                },
            );
            id
        });
        let th = self.threads.entry((pid, tid)).or_default();
        if let Some(d) = delivery {
            if in_flux2 {
                if let Some(f) = th.stack.last_mut() {
                    f.delivery = Some(d);
                }
            } else {
                th.delivery = Some(d);
            }
        }
        let pending = Pending {
            key: key.unwrap_or(PairKey::Ros(sub.topic, 0)),
            sub,
            t,
            wake,
            source_ts,
            delivery,
            windows: 0,
        };
        if key.is_none() || !self.pair_take(&pending, false) {
            if key.is_some() {
                self.pair.pending.push(pending);
            } else if let Some(o) = delivery.and_then(|d| self.deliveries.get_mut(&d)) {
                o.paired = true;
            }
        }
    }

    /// Pairs a take with its publish; `last` is its final try, after which a topic with an rcl
    /// ABI publisher falls back to the wall-clock hop.
    pub(super) fn pair_take(&mut self, p: &Pending, last: bool) -> bool {
        let rcl_topic = matches!(p.key, PairKey::Ros(tp, _) if self.topics[tp].rcl_abi);
        let Some(s) = self.pair.sent.get_mut(&p.key).filter(|s| s.t <= p.t) else {
            if !(rcl_topic || last) {
                return false;
            }
            let hop = p
                .source_ts
                .filter(|_| rcl_topic)
                .and_then(|ts| self.clock.real(p.t)?.checked_sub(u64::try_from(ts).ok()?));
            if let Some(h) = hop {
                self.win.topics.entry(p.sub.topic).or_default().hop.add(h);
                self.win.subs.entry(p.sub).or_default().hop.add(h);
            }
            if let Some(o) = p.delivery.and_then(|d| self.deliveries.get_mut(&d)) {
                o.paired = true;
                o.row.hop_ns = hop;
            }
            return true;
        };
        if s.ambiguous {
            self.win.topics.entry(p.sub.topic).or_default().ambiguous += 1;
            if let Some(o) = p.delivery.and_then(|d| self.deliveries.get_mut(&d)) {
                o.paired = true;
            }
            return true;
        }
        s.takers.push(p.sub);
        let (t_pub, publisher) = (s.t, s.publisher);
        self.pair
            .connected
            .entry(publisher)
            .or_default()
            .entry(p.sub)
            .or_insert(t_pub);
        let hop = p.t - t_pub;
        let wake = p.wake.filter(|&w| w >= t_pub).map(|w| w - t_pub);
        self.win.topics.entry(p.sub.topic).or_default().hop.add(hop);
        let sw = self.win.subs.entry(p.sub).or_default();
        sw.hop.add(hop);
        if let Some(w) = wake {
            sw.wake.add(w);
        }
        if let Some(o) = p.delivery.and_then(|d| self.deliveries.get_mut(&d)) {
            o.paired = true;
            o.row.pub_pid = Some(publisher.pid);
            o.row.t_pub_ns = Some(t_pub);
            o.row.hop_ns = Some(hop);
            o.row.wake_ns = wake;
        }
        true
    }

    pub(super) fn retry_pending(&mut self) {
        for mut p in std::mem::take(&mut self.pair.pending) {
            p.windows += 1;
            if !self.pair_take(&p, p.windows >= 2) {
                self.pair.pending.push(p);
            }
        }
    }

    /// Publishes past the horizon: a subscription connected to their publisher that did not take
    /// one lost it (ROS; flux2 counts its own losses).
    pub(super) fn evict(&mut self, now: u64) {
        for s in self.pair.evict(now.saturating_sub(pair::HORIZON_NS)) {
            if s.ambiguous || self.topics[s.topic].transport != Transport::Ros {
                continue;
            }
            let Some(subs) = self.pair.connected.get(&s.publisher) else {
                continue;
            };
            for (sub, _) in subs
                .iter()
                .filter(|(sub, since)| **since <= s.t && !s.takers.contains(sub))
            {
                self.win.subs.entry(*sub).or_default().lost += 1;
            }
        }
        self.stamps.evict(now);
    }

    fn sched_switch(&mut self, pid: u32, tid: u32, t: u64, next_tid: u32, cpu: u32) {
        self.sched = Sel::All;
        let th = self.thread(pid, tid);
        let on = th.on_since.take();
        th.off_since = th.stack.last().map(|f| (t, f.cb));
        let tw = self.win.threads.entry((pid, tid)).or_default();
        tw.cpus.insert(cpu);
        if let Some(t0) = on {
            *tw.oncpu.get_or_insert(0) += t - t0;
        }
        let Some(&npid) = self.tid_pid.get(&next_tid).filter(|_| next_tid != 0) else {
            return;
        };
        let nth = self.thread(npid, next_tid);
        nth.on_since = Some(t);
        let woken = nth.woken_at.take();
        let off = nth.off_since.take();
        let exec = nth.exec;
        if let Some((t0, cb)) = off {
            let d = t - t0;
            if let Some(f) = nth.stack.last_mut().filter(|f| f.cb == cb) {
                f.off += d;
            }
            self.win.cbs.entry(cb).or_default().off += d;
            if let Some(e) = exec {
                self.win.execs.entry(e).or_default().off += d;
            }
            self.win
                .threads
                .entry((npid, next_tid))
                .or_default()
                .in_cb_off += d;
        }
        let ntw = self.win.threads.entry((npid, next_tid)).or_default();
        ntw.cpus.insert(cpu);
        if let Some(w) = woken {
            ntw.cpu_wait.add(t - w);
            let thread = self.thread_id(npid, next_tid);
            self.sample(Sample::CpuWait { thread, ns: t - w });
        }
    }

    /// One row of an aggregate table's window difference.
    pub(super) fn aggregate(&mut self, method: Method, row: &Aggregate) {
        let pid = row.pid;
        self.proc_mut(pid).methods.insert(method);
        let key = CbKey {
            pid,
            src: row.src,
            key: row.key,
            entry: row.entry,
        };
        match row.kind {
            ARMO_AGG_CALLBACK => {
                if !self.cbs.contains_key(&key) {
                    self.proc_mut(pid).cold = true;
                    self.cbs.insert(key, Cb::default());
                }
                let node = self.cb_node(&key);
                let cw = self.win.cbs.entry(key).or_default();
                cw.n += row.count;
                cw.dur.merge(row.count, row.sum, row.max, &row.hist);
                if row.tid != 0 {
                    *cw.wall_by_tid.entry(row.tid).or_default() += row.sum;
                    let tw = self.win.threads.entry((pid, row.tid)).or_default();
                    tw.in_cb_wall += row.sum;
                    tw.owners.insert(node);
                    self.tid_pid.insert(row.tid, pid);
                    self.threads.entry((pid, row.tid)).or_default();
                }
                if let Some(e) = self.cbs.get(&key).and_then(|c| c.executor) {
                    self.win.execs.entry(e).or_default().busy += row.sum;
                }
            }
            ARMO_AGG_DISPATCH => {
                let cw = self.win.cbs.entry(key).or_default();
                cw.dispatch.merge(row.count, row.sum, row.max, &row.hist);
                if let Some(e) = self.cbs.get(&key).and_then(|c| c.executor) {
                    let ew = self.win.execs.entry(e).or_default();
                    ew.dispatch.merge(row.count, row.sum, row.max, &row.hist);
                }
            }
            ARMO_AGG_WAIT => {
                let exec = ExecKey {
                    pid,
                    src: row.src,
                    handle: row.key,
                };
                let ew = self.win.execs.entry(exec).or_default();
                ew.wait.merge(row.count, row.sum, row.max, &row.hist);
            }
            ARMO_AGG_OFFCPU => {
                self.win.cbs.entry(key).or_default().off += row.sum;
                if let Some(e) = self.cbs.get(&key).and_then(|c| c.executor) {
                    self.win.execs.entry(e).or_default().off += row.sum;
                }
            }
            ARMO_AGG_CPU_WAIT => {
                let tid = row.key as u32;
                let tw = self.win.threads.entry((pid, tid)).or_default();
                tw.cpu_wait.merge(row.count, row.sum, row.max, &row.hist);
            }
            ARMO_AGG_ALLOC | ARMO_AGG_FREE => {
                let owner = if row.key == 0 {
                    Owner::Proc(pid)
                } else {
                    self.cb_node(&key).map_or(Owner::Proc(pid), Owner::Node)
                };
                let m = self.win.malloc.entry(owner).or_default();
                if row.kind == ARMO_AGG_ALLOC {
                    m.alloc += row.sum;
                    m.alloc_n += row.count;
                } else {
                    m.free += row.sum;
                }
            }
            _ => {}
        }
    }
}
