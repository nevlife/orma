//! One record at a time, in time order.

use orma_common::events::{ORMA_FLUX_CLAIM_BACKPRESSURE, ORMA_FLUX_LOAN_TOO_LARGE};
use orma_common::shm::*;
use orma_proto::msg::{Aggregate, DeliveryPath, FlowKind, Method, PublishAbi, Transport};

use super::chain::Origins;
use super::decode::{Added, E, Ev};
use super::pair::{PairKey, Pending};
use super::stamp::decode as decode_stamp;
use super::symbols::shorten;
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Via {
    /// rmw (or rcl under the rcl ABI) and flux.
    Main,
    Intra,
}

const NO_SYMBOL: &str = "DISABLED__demangle_symbol";

/// `/flux.v<layout>.<flattened key>.<fingerprint>[.e<epoch>]` as key and fingerprint. The key
/// flattens only '/' to '.', so this restores it.
fn parse_signpost(name: &str) -> Option<(String, String, u64)> {
    let signpost = match name.rsplit_once(".e") {
        Some((s, epoch)) if !epoch.is_empty() && epoch.bytes().all(|b| b.is_ascii_digit()) => s,
        _ => name,
    };
    let rest = signpost.strip_prefix("/flux.v")?;
    let (_, rest) = rest.split_once('.')?;
    let (flat, fp) = rest.rsplit_once('.')?;
    let fp = u64::from_str_radix(fp, 16).ok()?;
    Some((signpost.to_string(), flat.replace('.', "/"), fp))
}

impl Model {
    pub(super) fn thread(&mut self, pid: u32, tid: u32) -> &mut Thread {
        self.threads.entry((pid, tid)).or_default()
    }

    /// Channels of one message type on different keys are different topics.
    fn flux_topic(&mut self, key: &str, fp: u64) -> TopicIx {
        let ix = self.topic(Transport::Flux, &format!("{key}:{fp:016x}"));
        let t = &mut self.topics[ix];
        t.name = key.to_string();
        t.fingerprint = fp;
        ix
    }

    fn flux_win(&mut self, topic: TopicIx) -> &mut TopicWin {
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
        // Kernel records come from ormad whatever method takes the process.
        let kernel = matches!(
            e,
            E::SchedSwitch { .. } | E::SchedWakeup { .. } | E::FutexWait { .. }
        );
        if !kernel {
            self.tid_pid.insert(tid, pid);
            let p = self.proc_mut(pid);
            p.methods.insert(method);
            p.method = Some(method);
        }
        if !matches!(
            e,
            E::RmwPublish { .. }
                | E::RmwPublishRcl
                | E::DdsSize { .. }
                | E::DdsWrite { .. }
                | E::ShmPayload
                | E::IpcPublish { .. }
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
            E::ServiceInit { handle, node, name } => {
                self.services.insert((pid, handle), (node, name));
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
            E::CallbackOwner {
                what,
                callback,
                object,
                handle,
                node,
                fqn,
            } => self.callback_owner(pid, tid, what, callback, [object, handle, node], fqn),
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
                if let Some(e) = th.exec.filter(|e| e.src == ORMA_SRC_ROS) {
                    self.exec_rclcpp.insert(e);
                }
            }
            E::RclcppPublish => self.thread(pid, tid).rclcpp_pub = Some(t),
            E::RclPublish { publisher, stamp } => {
                let th = self.thread(pid, tid);
                th.pending_rcl = Some((publisher, stamp, t));
                th.pub_from = Some(th.rclcpp_pub.take().unwrap_or(t));
                th.dds_write = None;
                th.dds_topic = None;
                th.shm_payload = false;
                th.loan_publish = std::mem::take(&mut th.loan_next);
                let py = th.py_publish.take();
                if let (Some(t0), Some(ep)) = (py, self.pubs.get(&(pid, publisher)).copied()) {
                    let tw = self.win.topics.entry(ep.topic).or_default();
                    tw.py_convert.add(t.saturating_sub(t0));
                }
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
                let origins = self.publish(pid, tid, t, ep.topic, owner, key, stamp, Via::Intra);
                if !self.chains.is_empty() {
                    self.chains.intra.insert(ep.topic, origins);
                }
            }
            E::RmwTake {
                rmw,
                message,
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
                let th = self.thread(pid, tid);
                th.last_take_rmw = Some(rmw);
                if std::mem::take(&mut th.loan_take) {
                    self.win.subs.entry(sub).or_default().loaned += 1;
                    self.holds.insert((pid, message), (t, sub));
                }
                let key = (source_timestamp != 0).then_some(PairKey::Ros(topic, source_timestamp));
                self.take(pid, tid, t, sub, stamp, key, Some(source_timestamp), None);
            }
            E::IpcPublish {
                publisher,
                source_timestamp,
            } => {
                // After an rmw_publish of the same message its key is already sent.
                if let Some((h, stamp, t0)) = self.thread(pid, tid).pending_rcl.take()
                    && let Some((topic, origins)) = self.publish_ipc(pid, tid, t0, h, stamp)
                    && source_timestamp != 0
                {
                    let key = PubKey {
                        pid,
                        handle: publisher,
                    };
                    let k = PairKey::Ros(topic, source_timestamp);
                    self.pair
                        .publish(k, t0, key, topic, origins, DeliveryPath::Broker);
                }
            }
            E::IpcTake {
                subscription,
                message,
                source_timestamp,
                stamp,
                local,
            } => {
                let Some(ep) = self.subs.get(&(pid, subscription)).copied() else {
                    self.proc_mut(pid).cold = true;
                    return;
                };
                let owner = self.owner(pid, Some(ep.node));
                let sub = SubKey {
                    topic: ep.topic,
                    owner,
                };
                self.topics[ep.topic].subscribers.insert(owner);
                if message != 0 {
                    self.holds.insert((pid, message), (t, sub));
                }
                let key =
                    (source_timestamp != 0).then_some(PairKey::Ros(ep.topic, source_timestamp));
                let path = local.then_some(DeliveryPath::Broker);
                self.take(pid, tid, t, sub, stamp, key, Some(source_timestamp), path);
            }
            E::IpcRelease { message } => self.release(pid, message, t),
            E::LoanBorrow { publisher, message } => {
                if let Some(ep) = self.pubs.get(&(pid, publisher)).copied() {
                    self.pub_loans.insert((pid, message), (t, ep.topic));
                }
            }
            E::LoanPublish { message } => {
                let th = self.thread(pid, tid);
                th.loan_next = true;
                th.py_publish = None;
                self.loan_end(pid, message, t);
            }
            E::LoanTake => self.thread(pid, tid).loan_take = true,
            E::LoanReturn {
                message,
                subscription,
            } => {
                if subscription {
                    self.release(pid, message, t);
                } else {
                    self.loan_end(pid, message, t);
                }
            }
            E::ShmPayload => self.thread(pid, tid).shm_payload = true,
            E::PyPublish => self.thread(pid, tid).py_publish = Some(t),
            E::PyConverted => {
                let th = self.threads.entry((pid, tid)).or_default();
                if let Some(tk) = th.last_take.filter(|tk| tk.t <= t) {
                    let ns = t - tk.t;
                    let d = th.delivery;
                    self.win.subs.entry(tk.sub).or_default().py_convert.add(ns);
                    if let Some(o) = d.and_then(|d| self.deliveries.get_mut(&d)) {
                        o.row.py_convert_ns = Some(ns);
                    }
                }
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
                    src: ORMA_SRC_ROS,
                    handle: wait_set,
                };
                let th = self.thread(pid, tid);
                th.wait_enter = Some(t);
                th.exec = Some(exec);
                if th.rclcpp {
                    self.exec_rclcpp.insert(exec);
                }
            }
            E::WaitExit { wait_set } => self.wake(pid, tid, t, ORMA_SRC_ROS, wait_set, iv),
            E::FluxWait { executor } => {
                let th = self.thread(pid, tid);
                th.wait_enter = Some(t);
                th.exec = Some(ExecKey {
                    pid,
                    src: ORMA_SRC_FLUX,
                    handle: executor,
                });
            }
            E::FluxWake { executor } => self.wake(pid, tid, t, ORMA_SRC_FLUX, executor, iv),
            E::ChannelInit {
                channel,
                dev,
                ino,
                publisher,
                segment,
            } => {
                let (signpost, key, fp) =
                    parse_signpost(&segment).unwrap_or((segment.clone(), segment.clone(), 0));
                let topic = self.flux_topic(&key, fp);
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
                    let w = self.flux_win(topic);
                    if result >= ORMA_FLUX_CLAIM_BACKPRESSURE {
                        *w.drop_publish.get_or_insert(0) += 1;
                    }
                }
            }
            E::LoanEnd { channel, result } => {
                if let Some(topic) = self.channels.get(&(pid, channel)).map(|c| c.topic) {
                    let w = self.flux_win(topic);
                    if result >= ORMA_FLUX_LOAN_TOO_LARGE {
                        *w.drop_publish.get_or_insert(0) += 1;
                    }
                }
            }
            E::GpuWedged { result } => self.proc_mut(pid).gpu_wedged = Some(result),
            E::RecordDrop { channel, count } => {
                if let Some(topic) = self.channels.get(&(pid, channel)).map(|c| c.topic) {
                    *self.flux_win(topic).drop_record.get_or_insert(0) += u64::from(count);
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
            } => self.flux_take(pid, tid, t, channel, ticket, lost, stamp),
            E::FluxStart {
                executor,
                entry,
                endpoint,
            } => {
                let key = CbKey::flux(pid, executor, entry);
                let cb = self.cbs.entry(key).or_default();
                if endpoint != 0 {
                    cb.owner = Some(CbOwner::Endpoint(endpoint));
                }
                cb.executor = Some(ExecKey {
                    pid,
                    src: ORMA_SRC_FLUX,
                    handle: executor,
                });
                self.callback_start(pid, tid, t, key, false, iv);
            }
            E::FluxEnd {
                executor,
                entry,
                delivered,
                channel,
                segment,
            } => {
                let key = CbKey::flux(pid, executor, entry);
                let owner = self.cbs.get(&key).and_then(|c| c.owner);
                match (owner, self.channels.get_mut(&(pid, channel))) {
                    (Some(CbOwner::Endpoint(ep)), Some(ch)) => ch.endpoint = Some(ep),
                    (None, None) => {
                        if let Some((_, k, fp)) = parse_signpost(&segment) {
                            let topic = self.flux_topic(&k, fp);
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
            E::DdsWrite { duration } => self.dds_write(pid, tid, t, duration),
            E::CudaCall {
                duration,
                bytes,
                api,
                error,
            } => self.cuda_call(pid, tid, duration, bytes, api, error),
            E::SpanBegin { key, name } => self.span_begin(pid, tid, t, key, name),
            E::SpanEnd { key } => self.span_end(pid, tid, t, key),
            E::FutexWait { duration } => self.futex_wait(pid, tid, t, duration),
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

    /// What the executor read for a callback of a process ormad attached to late: it stands for
    /// the init events ormad missed. A subscription's topic is that of the take before it.
    fn callback_owner(
        &mut self,
        pid: u32,
        tid: u32,
        what: Added,
        callback: u64,
        [object, handle, node]: [u64; 3],
        fqn: String,
    ) {
        let key = CbKey::ros(pid, callback);
        if self.cbs.get(&key).is_some_and(|c| c.owner.is_some()) {
            return;
        }
        if fqn.starts_with('/') && !self.node_ix.contains_key(&(pid, node)) {
            let (ns, name) = fqn.rsplit_once('/').unwrap_or_default();
            self.node_init(pid, node, name.into(), ns.into());
        }
        let owner = match what {
            Added::Sub => {
                self.rclcpp_subs.entry((pid, object)).or_insert(handle);
                let th = self.thread(pid, tid);
                let take = th
                    .last_take
                    .filter(|tk| tk.t >= th.last_cb_end)
                    .map(|tk| (tk.topic, th.last_take_rmw));
                if let Some((topic, rmw)) = take
                    && !self.subs.contains_key(&(pid, handle))
                {
                    self.subs.insert((pid, handle), Ep { topic, node });
                    if let Some(rmw) = rmw {
                        self.rmw_subs.insert((pid, rmw), handle);
                    }
                    let owner = self.owner(pid, Some(node));
                    if owner != Owner::Proc(pid) {
                        let unnamed = SubKey {
                            topic,
                            owner: Owner::Proc(pid),
                        };
                        let named = SubKey { topic, owner };
                        self.known_subs.remove(&unnamed);
                        self.known_subs.insert(named);
                        self.topics[topic].subscribers.remove(&unnamed.owner);
                        self.topics[topic].subscribers.insert(owner);
                        if let Some(sw) = self.win.subs.remove(&unnamed) {
                            self.win.subs.insert(named, sw);
                        }
                        if let Some(tk) = self.thread(pid, tid).last_take.as_mut() {
                            tk.sub = named;
                        }
                    }
                }
                CbOwner::Sub(object)
            }
            Added::Service => {
                self.services
                    .entry((pid, handle))
                    .or_insert((node, String::new()));
                CbOwner::Service(handle)
            }
            Added::Timer => {
                let tm = self.timers.entry((pid, handle)).or_default();
                if node != 0 {
                    tm.node.get_or_insert(node);
                }
                CbOwner::Timer(handle)
            }
        };
        self.cbs.entry(key).or_default().owner = Some(owner);
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
        let topic = self.flux_topic(&key, fingerprint);
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
        if input.is_some() && key.src == ORMA_SRC_FLUX {
            input = None;
        }
        let taken = input;
        let dispatch = th.last_wake.filter(|_| iv).map(|w| t.saturating_sub(w));
        let delivery = if key.src == ORMA_SRC_ROS && input.is_some() {
            th.delivery.take()
        } else {
            None
        };
        let exec = if key.src == ORMA_SRC_ROS {
            th.exec
        } else {
            self.cbs.get(&key).and_then(|c| c.executor)
        };
        let cb = self.cbs.entry(key).or_default();
        if cb.first == 0 {
            cb.first = t;
        }
        cb.executor = exec.or(cb.executor);
        if cb.owner.is_none()
            && let Some(tk) = input
        {
            cb.late_topic = Some(tk.topic);
        }
        if input.is_none()
            && key.src == ORMA_SRC_ROS
            && let Some(topic) = topic.filter(|_| matches!(cb.owner, Some(CbOwner::Sub(_))))
        {
            let owner = node.map_or(Owner::Proc(pid), Owner::Node);
            let take = self.next_take;
            self.next_take += 1;
            if let Some(o) = self.chains.intra.get(&topic) {
                self.chains.took(take, t, o.clone());
            }
            input = Some(Input {
                topic,
                sub: SubKey { topic, owner },
                t,
                stamp: None,
                take,
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
            o.row.deliver_ns = taken.map(|tk| t.saturating_sub(tk.t));
        }
        if let Some(tk) = taken {
            let sw = self.win.subs.entry(tk.sub).or_default();
            sw.deliver.add(t.saturating_sub(tk.t));
        }
        self.thread(pid, tid).stack.push(Frame {
            cb: key,
            t,
            input,
            outputs: Vec::new(),
            dispatch,
            off: 0,
            delivery,
            cuda_ns: 0,
            cuda_sync_ns: 0,
            futex_ns: 0,
        });
    }

    fn callback_end(&mut self, pid: u32, tid: u32, t: u64, key: CbKey, counted: bool, iv: bool) {
        let th = self.thread(pid, tid);
        let Some(pos) = th.stack.iter().rposition(|f| f.cb == key) else {
            return;
        };
        th.stack.truncate(pos + 1);
        let frame = th.stack.pop().expect("found above");
        // A span begun in the callback and not ended there never will be.
        th.spans.retain(|s| s.cb != Some(key));
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
            let cuda = self.gpu && self.procs.get(&pid).is_some_and(|p| p.cuda);
            let futex = self.futex.contains(pid);
            self.sample(Sample::Callback {
                id,
                node: node_id,
                tid,
                t_ns: frame.t,
                duration_ns: dur,
                oncpu_ns: sched.then(|| dur.saturating_sub(frame.off)),
                dispatch_ns: frame.dispatch,
                cuda_ns: cuda.then_some(frame.cuda_ns),
                cuda_sync_ns: cuda.then_some(frame.cuda_sync_ns),
                futex_ns: futex.then_some(frame.futex_ns),
            });
        }
    }

    fn publish_rmw(&mut self, pid: u32, tid: u32, t: u64, rmw: u64, timestamp: i64, stamp: u64) {
        let th = self.thread(pid, tid);
        th.pending_rcl = None;
        let path = if std::mem::take(&mut th.loan_publish) {
            DeliveryPath::Loan
        } else if std::mem::take(&mut th.shm_payload) {
            DeliveryPath::Shm
        } else {
            DeliveryPath::Net
        };
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
        self.dds_publish(pid, tid, topic);
        let origins = self.publish(pid, tid, t, topic, owner, key, stamp, Via::Main);
        if timestamp != 0 {
            self.pair
                .publish(PairKey::Ros(topic, timestamp), t, key, topic, origins, path);
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
        self.dds_publish(pid, tid, ep.topic);
        self.publish(pid, tid, t, ep.topic, owner, key, stamp, Via::Main);
    }

    /// An `rcl_publish` that only the broker delivered: no rmw publish, so not the rcl ABI.
    fn publish_ipc(
        &mut self,
        pid: u32,
        tid: u32,
        t: u64,
        handle: u64,
        stamp: u64,
    ) -> Option<(TopicIx, Origins)> {
        let Some(ep) = self.pubs.get(&(pid, handle)).copied() else {
            self.proc_mut(pid).cold = true;
            return None;
        };
        let owner = self.owner(pid, Some(ep.node));
        let key = PubKey { pid, handle };
        let origins = self.publish(pid, tid, t, ep.topic, owner, key, stamp, Via::Main);
        Some((ep.topic, origins))
    }

    /// A taken broker message released, or a subscription's loan returned.
    fn release(&mut self, pid: u32, message: u64, t: u64) {
        let Some((t0, sub)) = self.holds.remove(&(pid, message)) else {
            return;
        };
        let ns = t.saturating_sub(t0);
        self.win.subs.entry(sub).or_default().hold.add(ns);
        if self.samples.is_some() {
            let s = Sample::Hold {
                topic: self.topic_id(sub.topic),
                subscription: Some(self.sub_id(&sub)),
                ns,
            };
            self.sample(s);
        }
    }

    /// A publisher's loan published or returned.
    fn loan_end(&mut self, pid: u32, message: u64, t: u64) {
        let Some((t0, topic)) = self.pub_loans.remove(&(pid, message)) else {
            return;
        };
        let ns = t.saturating_sub(t0);
        self.win.topics.entry(topic).or_default().loan_hold.add(ns);
        if self.samples.is_some() {
            let s = Sample::Hold {
                topic: self.topic_id(topic),
                subscription: None,
                ns,
            };
            self.sample(s);
        }
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
            .flux_win(topic)
            .bytes
            .get_or_insert_default()
            .entry(pid)
            .or_default() += nbytes;
        let key = PubKey {
            pid,
            handle: channel,
        };
        let origins = self.publish(pid, tid, t, topic, owner, key, stamp, Via::Main);
        self.pair.publish(
            PairKey::Flux(dev, ino, ticket),
            t,
            key,
            topic,
            origins,
            DeliveryPath::Unknown,
        );
    }

    /// The origins the message carries for the lineage chains of its topic.
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
    ) -> Origins {
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
        let sync = th.stack.last().and_then(|f| f.input);
        // A dual (intra and rmw) publish is one message: only its first half counts.
        let mut fresh = true;
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
            let o = &mut f.outputs[pos];
            match via {
                Via::Main => o.main.push(dwell),
                Via::Intra => o.intra.push(dwell),
            }
            fresh = match via {
                Via::Main => o.main.len() > o.intra.len(),
                Via::Intra => o.intra.len() > o.main.len(),
            };
        }
        let mut unique = None;
        if let (Owner::Node(node), Some((ns, _))) = (owner, gate) {
            let dual = self.last_stamp.insert(key, ns) == Some(ns);
            fresh &= !dual;
            unique = self.stamp_link(owner, node, ns, topic, t, sync.map(|i| i.topic), dual);
        }
        let mut links: Vec<(TopicIx, u64)> = unique.into_iter().collect();
        links.extend(sync.map(|i| (i.topic, i.take)));
        if let (None, Owner::Node(node)) = (unique, owner)
            && self.latest_node(node)
        {
            let latest: Vec<(TopicIx, (u64, u64))> = self
                .latest
                .get(&node)
                .map(|m| m.iter().map(|(k, v)| (*k, *v)).collect())
                .unwrap_or_default();
            for (input, (t_take, take)) in latest {
                if sync.is_some_and(|i| i.topic == input) {
                    continue;
                }
                links.push((input, take));
                if !fresh {
                    continue;
                }
                let fk = FlowKey {
                    kind: FlowKind::Latest,
                    node: owner,
                    input: Some(input),
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
        }
        self.chain_publish(topic, t, fresh, &links)
    }

    /// The stamp flows of a publish in `node`; the input and take of a unique link.
    #[allow(clippy::too_many_arguments)]
    fn stamp_link(
        &mut self,
        owner: Owner,
        node: NodeIx,
        ns: u64,
        topic: TopicIx,
        t: u64,
        sync_input: Option<TopicIx>,
        dual: bool,
    ) -> Option<(TopicIx, u64)> {
        match self
            .stamps
            .find(node, ns)
            .map(|s| (s.inputs.clone(), s.t, s.take))
        {
            Some((inputs, t_take, take)) if inputs.len() == 1 => {
                if dual {
                    return Some((inputs[0], take));
                }
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
                Some((inputs[0], take))
            }
            Some(_) if dual => None,
            Some((inputs, _, _)) => {
                for input in inputs {
                    let fk = FlowKey {
                        kind: FlowKind::Stamp,
                        node: owner,
                        input: Some(input),
                        output: topic,
                    };
                    self.win.flows.entry(fk).or_default().ambiguous += 1;
                }
                None
            }
            None if !dual && self.stamps.stamped.contains(&node) => {
                let fk = FlowKey {
                    kind: FlowKind::Blind,
                    node: owner,
                    input: sync_input,
                    output: topic,
                };
                self.win.flows.entry(fk).or_default().count += 1;
                None
            }
            None => None,
        }
    }

    fn latest_node(&mut self, node: NodeIx) -> bool {
        if self.latest_nodes.is_empty() {
            return false;
        }
        let (res, nodes) = (&self.latest_nodes, &self.nodes);
        *self
            .latest_match
            .entry(node)
            .or_insert_with(|| res.iter().any(|r| r.is_match(&nodes[node].fqn)))
    }

    fn chain_at(&mut self, topic: TopicIx) -> Vec<(usize, usize)> {
        let t = &self.topics[topic];
        self.chains.positions(topic, &t.name, || t.id())
    }

    /// A publish on a chain's topic: the steps it is of, and the origins it carries on. `links`
    /// are the takes it came from inside its node, in the order to trust them.
    fn chain_publish(
        &mut self,
        topic: TopicIx,
        t: u64,
        fresh: bool,
        links: &[(TopicIx, u64)],
    ) -> Origins {
        let mut origins = Vec::new();
        if self.chains.is_empty() {
            return origins;
        }
        for (c, k) in self.chain_at(topic) {
            let t0 = if !self.chains.lineage(c) {
                match fresh {
                    true => self.chains.first(c, k, t),
                    false => None,
                }
            } else {
                let origin = match k {
                    0 => Some(t),
                    _ => links.iter().find_map(|&(input, take)| {
                        self.chain_at(input)
                            .contains(&(c, k - 1))
                            .then(|| self.chains.origin(take, c))
                            .flatten()
                    }),
                };
                if let Some(o) = origin {
                    origins.push((c, o));
                }
                if k > 0 && fresh {
                    self.chains.step(c, k, t, origin);
                }
                origin.filter(|_| k > 0 && fresh)
            };
            if let Some(t0) = t0.filter(|_| self.samples.is_some()) {
                let s = &self.chains.specs[c];
                let (name, mode) = (s.name.clone(), s.mode);
                let topic = self.topic_id(topic);
                self.sample(Sample::Chain {
                    name,
                    mode,
                    step: k,
                    topic,
                    t_source_ns: t0,
                    latency_ns: t - t0,
                });
            }
        }
        origins
    }

    #[allow(clippy::too_many_arguments)]
    fn flux_take(
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
        self.flux_win(topic);
        *self
            .win
            .subs
            .entry(sub)
            .or_default()
            .flux_lost
            .get_or_insert(0) += new_lost;
        self.take(
            pid,
            tid,
            t,
            sub,
            stamp,
            Some(PairKey::Flux(dev, ino, ticket)),
            None,
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
        path: Option<DeliveryPath>,
    ) {
        self.known_subs.insert(sub);
        self.win.subs.entry(sub).or_default().takes += 1;
        let take = self.next_take;
        self.next_take += 1;
        if let Owner::Node(n) = sub.owner
            && self.latest_node(n)
        {
            self.latest
                .entry(n)
                .or_default()
                .insert(sub.topic, (t, take));
        }
        let gate = self.clock.gate(stamp, t);
        if let Some((ns, age)) = gate {
            self.win
                .topics
                .entry(sub.topic)
                .or_default()
                .age_take
                .add(age);
            if let Owner::Node(n) = sub.owner {
                self.stamps.take(n, ns, sub.topic, t, take);
            }
        }
        let input = Input {
            topic: sub.topic,
            sub,
            t,
            stamp: gate.map(|g| g.0),
            take,
        };
        let clock = self.clock;
        let th = self.threads.entry((pid, tid)).or_default();
        th.last_take = Some(input);
        let wake = th.last_wake;
        let mut in_flux = false;
        if let Some(f) = th
            .stack
            .last_mut()
            .filter(|f| f.cb.src == ORMA_SRC_FLUX && f.input.is_none())
        {
            f.input = Some(input);
            in_flux = true;
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
            if in_flux {
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
            take,
            windows: 0,
            path,
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
            let path = p.path.unwrap_or(DeliveryPath::Unknown);
            self.path_take(p, path, hop);
            if let Some(o) = p.delivery.and_then(|d| self.deliveries.get_mut(&d)) {
                o.paired = true;
                o.row.hop_ns = hop;
                o.row.path = Some(path);
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
        let (t_pub, publisher, sent_path) = (s.t, s.publisher, s.path);
        if !s.origins.is_empty() {
            let origins = s.origins.clone();
            self.chains.took(p.take, p.t, origins);
        }
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
        let path = p.path.unwrap_or(sent_path);
        self.path_take(p, path, Some(hop));
        if let Some(o) = p.delivery.and_then(|d| self.deliveries.get_mut(&d)) {
            o.paired = true;
            o.row.pub_pid = Some(publisher.pid);
            o.row.t_pub_ns = Some(t_pub);
            o.row.hop_ns = Some(hop);
            o.row.wake_ns = wake;
            o.row.path = Some(path);
        }
        true
    }

    /// A ROS take counted under its delivery path (docs/instrumentation.md 2.13).
    fn path_take(&mut self, p: &Pending, path: DeliveryPath, hop: Option<u64>) {
        if self.topics[p.sub.topic].transport != Transport::Ros {
            return;
        }
        let tw = self.win.topics.entry(p.sub.topic).or_default();
        let sw = self.win.subs.entry(p.sub).or_default();
        for w in [
            tw.paths.entry(path).or_default(),
            sw.paths.entry(path).or_default(),
        ] {
            w.takes += 1;
            if let Some(h) = hop {
                w.hop.add(h);
            }
        }
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
    /// one lost it (ROS; flux counts its own losses).
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
        self.chains.evict(now);
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
            ORMA_AGG_CALLBACK => {
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
            ORMA_AGG_DISPATCH => {
                let cw = self.win.cbs.entry(key).or_default();
                cw.dispatch.merge(row.count, row.sum, row.max, &row.hist);
                if let Some(e) = self.cbs.get(&key).and_then(|c| c.executor) {
                    let ew = self.win.execs.entry(e).or_default();
                    ew.dispatch.merge(row.count, row.sum, row.max, &row.hist);
                }
            }
            ORMA_AGG_WAIT => {
                let exec = ExecKey {
                    pid,
                    src: row.src,
                    handle: row.key,
                };
                let ew = self.win.execs.entry(exec).or_default();
                ew.wait.merge(row.count, row.sum, row.max, &row.hist);
            }
            ORMA_AGG_OFFCPU => {
                self.win.cbs.entry(key).or_default().off += row.sum;
                if let Some(e) = self.cbs.get(&key).and_then(|c| c.executor) {
                    self.win.execs.entry(e).or_default().off += row.sum;
                }
            }
            ORMA_AGG_CPU_WAIT => {
                let tid = row.key as u32;
                let tw = self.win.threads.entry((pid, tid)).or_default();
                tw.cpu_wait.merge(row.count, row.sum, row.max, &row.hist);
            }
            ORMA_AGG_ALLOC | ORMA_AGG_FREE => {
                let owner = if row.key == 0 {
                    Owner::Proc(pid)
                } else {
                    self.cb_node(&key).map_or(Owner::Proc(pid), Owner::Node)
                };
                let m = self.win.malloc.entry(owner).or_default();
                if row.kind == ORMA_AGG_ALLOC {
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
