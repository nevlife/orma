//! The model against synthetic event streams: exact counts, pairing, flows,
//! the rcl publish ABI, per-node CPU, and `armo analyze` on a recording built the same way.

use std::path::PathBuf;

use armo_collectd::model::symbols::{Resolver, shorten};
use armo_collectd::model::{Model, Sample, procfs};
use armo_collectd::sources::armod::Sel;
use armo_common::Plain;
use armo_common::events::*;
use armo_common::shm::{ARMO_AGG_CALLBACK, ARMO_AGG_OFFCPU, ARMO_SRC_ROS};
use armo_proto::Ingest;
use armo_proto::msg::*;
use armo_proto::records;

const W: u64 = 1_000_000_000;
const MS: u64 = 1_000_000;
const US: u64 = 1_000;
const T0: u64 = 100 * W;
/// Init events come just before the data.
const INIT: u64 = T0 - MS;
/// `CLOCK_REALTIME - CLOCK_MONOTONIC` of the synthetic host.
const REAL: u64 = 1_790_000_000 * W;

fn stamp_of(real_ns: u64) -> u64 {
    (real_ns / W) | ((real_ns % W) << 32)
}

/// A wall-clock `header.stamp` taken at monotonic time `t`.
fn wall(t: u64) -> u64 {
    stamp_of(t + REAL)
}

struct H {
    m: Model,
    batch: Vec<u8>,
    method: Method,
    windows: Vec<Window>,
}

impl H {
    fn offline() -> Self {
        H {
            m: Model::offline(),
            batch: Vec::new(),
            method: Method::Preload,
            windows: Vec::new(),
        }
    }

    fn ev<T: Plain>(&mut self, pid: u32, tid: u32, t: u64, event: u16, p: T, s: &[&[u8]]) {
        records::encode_positioned(&mut self.batch, pid, tid, event, t, &p, s);
    }

    /// Hands the records so far to the model by `self.method`.
    fn feed(&mut self) {
        let batch = std::mem::take(&mut self.batch);
        for r in records::positioned(&batch) {
            self.m.record(self.method, &r.unwrap());
        }
    }

    fn window(&mut self, end: u64) -> Window {
        self.feed();
        let w = self.m.window(end, end + REAL, W, false);
        self.windows.push(w.clone());
        w
    }

    fn node(&mut self, pid: u32, node: u64, name: &str, ns: &str) {
        let p = armo_ev_rcl_node_init { node, rmw_node: 0 };
        self.ev(
            pid,
            pid,
            INIT + 1,
            ARMO_EV_RCL_NODE_INIT,
            p,
            &[name.as_bytes(), ns.as_bytes()],
        );
    }

    fn publisher(&mut self, pid: u32, handle: u64, node: u64, rmw: u64, topic: &str) {
        let p = armo_ev_rcl_topic_init {
            handle,
            node,
            rmw_handle: rmw,
            queue_depth: 10,
        };
        self.ev(
            pid,
            pid,
            INIT + 2,
            ARMO_EV_RCL_PUBLISHER_INIT,
            p,
            &[topic.as_bytes()],
        );
    }

    /// rcl handle `h`, rmw handle `h + 1`, rclcpp object `h + 2`, callback `h + 3`.
    fn subscription(&mut self, pid: u32, h: u64, node: u64, topic: &str) {
        let p = armo_ev_rcl_topic_init {
            handle: h,
            node,
            rmw_handle: h + 1,
            queue_depth: 10,
        };
        self.ev(
            pid,
            pid,
            INIT + 2,
            ARMO_EV_RCL_SUBSCRIPTION_INIT,
            p,
            &[topic.as_bytes()],
        );
        let p = armo_ev_rclcpp_subscription_init {
            subscription_handle: h,
            subscription: h + 2,
        };
        self.ev(pid, pid, INIT + 3, ARMO_EV_RCLCPP_SUBSCRIPTION_INIT, p, &[]);
        let p = armo_ev_callback_added {
            handle: h + 2,
            callback: h + 3,
        };
        self.ev(
            pid,
            pid,
            INIT + 4,
            ARMO_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED,
            p,
            &[],
        );
    }

    fn timer(&mut self, pid: u32, timer: u64, period: i64, node: u64, callback: u64) {
        self.ev(
            pid,
            pid,
            INIT + 2,
            ARMO_EV_RCL_TIMER_INIT,
            armo_ev_rcl_timer_init { timer, period },
            &[],
        );
        let p = armo_ev_callback_added {
            handle: timer,
            callback,
        };
        self.ev(
            pid,
            pid,
            INIT + 3,
            ARMO_EV_RCLCPP_TIMER_CALLBACK_ADDED,
            p,
            &[],
        );
        let p = armo_ev_rclcpp_timer_link_node { timer, node };
        self.ev(pid, pid, INIT + 4, ARMO_EV_RCLCPP_TIMER_LINK_NODE, p, &[]);
    }

    /// Jazzy: `rcl_publish` then `rmw_publish` on the same thread.
    #[allow(clippy::too_many_arguments)]
    fn publish(&mut self, pid: u32, tid: u32, t: u64, rcl: u64, rmw: u64, ts: i64, stamp: u64) {
        let p = armo_ev_publish {
            publisher: rcl,
            message: 1,
            stamp,
        };
        self.ev(pid, tid, t - 1, ARMO_EV_RCL_PUBLISH, p, &[]);
        let p = armo_ev_rmw_publish {
            rmw_publisher: rmw,
            message: 1,
            timestamp: ts,
            stamp,
        };
        self.ev(pid, tid, t, ARMO_EV_RMW_PUBLISH, p, &[]);
    }

    fn take(&mut self, pid: u32, tid: u32, t: u64, rmw: u64, ts: i64, stamp: u64) {
        let p = armo_ev_rmw_take {
            rmw_subscription: rmw,
            message: 1,
            source_timestamp: ts,
            stamp,
            taken: 1,
            pad: 0,
        };
        self.ev(pid, tid, t, ARMO_EV_RMW_TAKE, p, &[]);
    }

    fn cb(&mut self, pid: u32, tid: u32, t0: u64, t1: u64, callback: u64) {
        let p = armo_ev_callback_start {
            callback,
            is_intra_process: 0,
            pad: 0,
        };
        self.ev(pid, tid, t0, ARMO_EV_CALLBACK_START, p, &[]);
        self.ev(
            pid,
            tid,
            t1,
            ARMO_EV_CALLBACK_END,
            armo_ev_handle { handle: callback },
            &[],
        );
    }

    fn start(&mut self, pid: u32, tid: u32, t: u64, callback: u64, intra: bool) {
        let p = armo_ev_callback_start {
            callback,
            is_intra_process: u32::from(intra),
            pad: 0,
        };
        self.ev(pid, tid, t, ARMO_EV_CALLBACK_START, p, &[]);
    }

    fn end(&mut self, pid: u32, tid: u32, t: u64, callback: u64) {
        self.ev(
            pid,
            tid,
            t,
            ARMO_EV_CALLBACK_END,
            armo_ev_handle { handle: callback },
            &[],
        );
    }

    fn wait(&mut self, pid: u32, tid: u32, t0: u64, t1: u64, wait_set: u64) {
        let p = armo_ev_rcl_wait_enter {
            wait_set,
            timeout: -1,
        };
        self.ev(pid, tid, t0, ARMO_EV_RCL_WAIT_ENTER, p, &[]);
        let p = armo_ev_rcl_wait_exit {
            wait_set,
            ret: 0,
            pad: 0,
        };
        self.ev(pid, tid, t1, ARMO_EV_RCL_WAIT_EXIT, p, &[]);
    }

    fn switch(&mut self, prev_pid: u32, prev: u32, t: u64, next: u32) {
        let p = armo_ev_sched_switch {
            prev_state: 0,
            next_tid: next,
            cpu: 3,
        };
        self.ev(prev_pid, prev, t, ARMO_EV_SCHED_SWITCH, p, &[]);
    }
}

fn topic<'a>(w: &'a Window, id: &str) -> &'a Topic {
    w.topics
        .iter()
        .find(|t| t.id == id)
        .unwrap_or_else(|| panic!("no topic {id}"))
}

fn sub<'a>(w: &'a Window, topic: &str, node_pid: u32) -> &'a Subscription {
    w.subscriptions
        .iter()
        .find(|s| s.topic == topic && s.node.starts_with(&format!("n:{node_pid}:")))
        .unwrap_or_else(|| panic!("no subscription of {node_pid} on {topic}"))
}

fn flows(w: &Window, kind: FlowKind) -> Vec<&Flow> {
    w.flows.iter().filter(|f| f.kind == kind).collect()
}

fn cb(w: &Window, pid: u32, handle: u64) -> &Callback {
    let id = format!("c:{pid}:0:ros:{handle:x}");
    w.callbacks
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no {id}"))
}

const CHATTER: &str = "topic:ros:/chatter";

/// One talker, three listeners in three processes: every take pairs with its publish, every
/// interval is exact.
fn fan_out(h: &mut H, n: u64) {
    const LISTENERS: [u32; 3] = [200, 300, 400];
    h.node(100, 0xa0, "talker", "/");
    h.publisher(100, 0xb0, 0xa0, 0xb1, "/chatter");
    for pid in LISTENERS {
        h.node(pid, 0xc0, "listener", "/");
        h.subscription(pid, 0xd0, 0xc0, "/chatter");
    }
    for i in 0..n {
        let t = T0 + i * 10 * MS;
        h.publish(100, 100, t, 0xb0, 0xb1, 1000 + i as i64, wall(t));
        for (k, pid) in LISTENERS.into_iter().enumerate() {
            let hop = 30 * US + k as u64 * 10 * US;
            h.wait(pid, pid, t - MS, t + hop - 5 * US, 0xee);
            h.take(pid, pid, t + hop, 0xd1, 1000 + i as i64, wall(t));
            h.cb(pid, pid, t + hop + 2 * US, t + hop + 102 * US, 0xd3);
        }
    }
}

#[test]
fn fan_out_counts_and_intervals_are_exact() {
    let mut h = H::offline();
    fan_out(&mut h, 50);
    let w = h.window(T0 + W);
    let t = topic(&w, CHATTER);
    assert_eq!(t.pub_hz, Some(50.0));
    assert_eq!(t.hop.as_ref().unwrap().n, 150);
    assert_eq!(t.drops.ambiguous, Some(0));
    assert_eq!(t.age_pub.as_ref().unwrap().max_ns, 0);
    for (k, pid) in [200u32, 300, 400].into_iter().enumerate() {
        let hop = 30 * US + k as u64 * 10 * US;
        let s = sub(&w, CHATTER, pid);
        assert_eq!(s.recv_hz, Some(50.0));
        let lat = s.hop.as_ref().unwrap();
        assert_eq!((lat.n, lat.mean_ns, lat.max_ns), (50, hop, hop));
        let wake = s.wake.as_ref().unwrap();
        assert_eq!((wake.n, wake.max_ns), (50, hop - 5 * US));
        assert_eq!(s.lost, Some(0));
        let c = cb(&w, pid, 0xd3);
        assert_eq!(c.hz, Some(50.0));
        assert_eq!(c.kind, CallbackKind::Sub);
        assert_eq!(c.topic.as_deref(), Some(CHATTER));
        assert_eq!(c.duration.as_ref().unwrap().max_ns, 100 * US);
        assert_eq!(c.dispatch.as_ref().unwrap().max_ns, 7 * US);
        assert_eq!(c.age.as_ref().unwrap().max_ns, hop + 2 * US);
        let e = w
            .executors
            .iter()
            .find(|e| e.id.starts_with(&format!("e:{pid}:")))
            .unwrap();
        assert_eq!(e.wait.as_ref().unwrap().n, 50);
        assert_eq!(e.callbacks, vec![c.id.clone()]);
    }
    let mut samples = h.m.take_samples();
    for _ in 0..3 {
        h.window(h.windows.last().unwrap().t_mono_ns + W);
        samples.extend(h.m.take_samples());
    }
    let deliveries: Vec<_> = samples
        .iter()
        .filter_map(|s| match s {
            Sample::Delivery(d) => Some(d),
            _ => None,
        })
        .collect();
    assert_eq!(deliveries.len(), 150);
    assert!(deliveries.iter().all(|d| d.callback_ns == Some(100 * US)
        && d.dispatch_ns.is_some()
        && d.pub_pid == Some(100)));
}

#[test]
fn same_timestamp_on_two_topics_pairs_each_with_its_own() {
    let mut h = H::offline();
    h.node(100, 0xa0, "a", "/");
    h.publisher(100, 0x10, 0xa0, 0x11, "/a");
    h.node(101, 0xa0, "b", "/");
    h.publisher(101, 0x20, 0xa0, 0x21, "/b");
    h.node(200, 0xc0, "sa", "/");
    h.subscription(200, 0x30, 0xc0, "/a");
    h.node(201, 0xc0, "sb", "/");
    h.subscription(201, 0x40, 0xc0, "/b");
    h.publish(100, 100, T0 + MS, 0x10, 0x11, 777, 0);
    h.publish(101, 101, T0 + 2 * MS, 0x20, 0x21, 777, 0);
    h.take(200, 200, T0 + 3 * MS, 0x31, 777, 0);
    h.take(201, 201, T0 + 4 * MS, 0x41, 777, 0);
    let w = h.window(T0 + W);
    let a = topic(&w, "topic:ros:/a");
    let b = topic(&w, "topic:ros:/b");
    assert_eq!(a.hop.as_ref().unwrap().max_ns, 2 * MS);
    assert_eq!(b.hop.as_ref().unwrap().max_ns, 2 * MS);
    assert_eq!((a.drops.ambiguous, b.drops.ambiguous), (Some(0), Some(0)));
}

#[test]
fn same_timestamp_from_two_publishers_of_one_topic_is_ambiguous() {
    let mut h = H::offline();
    for pid in [100u32, 102] {
        h.node(pid, 0xa0, "p", "/");
        h.publisher(pid, 0x10, 0xa0, 0x11, "/c");
    }
    h.node(200, 0xc0, "s", "/");
    h.subscription(200, 0x30, 0xc0, "/c");
    h.publish(100, 100, T0 + MS, 0x10, 0x11, 999, 0);
    h.publish(102, 102, T0 + 2 * MS, 0x10, 0x11, 999, 0);
    h.take(200, 200, T0 + 3 * MS, 0x31, 999, 0);
    h.publish(100, 100, T0 + 4 * MS, 0x10, 0x11, 1000, 0);
    h.take(200, 200, T0 + 5 * MS, 0x31, 1000, 0);
    let w = h.window(T0 + W);
    let c = topic(&w, "topic:ros:/c");
    assert_eq!(c.drops.ambiguous, Some(1));
    assert_eq!(c.hop.as_ref().unwrap().n, 1);
    assert_eq!(sub(&w, "topic:ros:/c", 200).recv_hz, Some(2.0));
}

#[test]
fn a_take_that_arrives_before_its_publish_still_pairs() {
    let mut h = H::offline();
    h.node(100, 0xa0, "p", "/");
    h.publisher(100, 0x10, 0xa0, 0x11, "/late");
    h.node(200, 0xc0, "s", "/");
    h.subscription(200, 0x30, 0xc0, "/late");
    h.take(200, 200, T0 + W - MS, 0x31, 5, 0);
    let w = h.window(T0 + W);
    assert!(topic(&w, "topic:ros:/late").hop.is_none());
    h.publish(100, 100, T0 + W - 3 * MS, 0x10, 0x11, 5, 0);
    let w = h.window(T0 + 2 * W);
    assert_eq!(
        topic(&w, "topic:ros:/late").hop.as_ref().unwrap().max_ns,
        2 * MS
    );
}

#[test]
fn loss_is_counted_only_on_connected_subscriptions() {
    let mut h = H::offline();
    h.node(100, 0xa0, "p", "/");
    h.publisher(100, 0x10, 0xa0, 0x11, "/l");
    h.node(200, 0xc0, "s", "/");
    h.subscription(200, 0x30, 0xc0, "/l");
    h.node(300, 0xc0, "never", "/");
    h.subscription(300, 0x30, 0xc0, "/l");
    for i in 0..10u64 {
        let t = T0 + i * 10 * MS;
        h.publish(100, 100, t, 0x10, 0x11, 1 + i as i64, 0);
        if i != 3 && i != 7 {
            h.take(200, 200, t + 50 * US, 0x31, 1 + i as i64, 0);
        }
    }
    let mut lost = 0;
    for k in 1..=7 {
        let w = h.window(T0 + k * W);
        lost += sub(&w, "topic:ros:/l", 200).lost.unwrap();
        assert_eq!(sub(&w, "topic:ros:/l", 300).lost, None);
    }
    assert_eq!(lost, 2);
}

/// Node /fuse (0xe1) takes /in1 and /in2 and publishes /out from a timer; node /other (0xe2) in
/// the same process publishes /out2.
fn fuse(h: &mut H) {
    h.node(500, 0xe1, "fuse", "/");
    h.node(500, 0xe2, "other", "/");
    h.subscription(500, 0x10, 0xe1, "/in1");
    h.subscription(500, 0x20, 0xe1, "/in2");
    h.publisher(500, 0x30, 0xe1, 0x31, "/out");
    h.publisher(500, 0x40, 0xe2, 0x41, "/out2");
    h.timer(500, 0x50, 100 * MS as i64, 0xe1, 0x51);
}

#[test]
fn stamp_flows_link_exact_stamps_in_one_node_and_abstain_otherwise() {
    let mut h = H::offline();
    fuse(&mut h);
    let s1 = wall(T0);
    let s2 = wall(T0 + MS);
    h.take(500, 501, T0 + 10 * MS, 0x11, 0, s1);
    h.cb(500, 501, T0 + 10 * MS + US, T0 + 11 * MS, 0x13);
    h.start(500, 502, T0 + 30 * MS, 0x51, false);
    h.publish(500, 502, T0 + 30 * MS + US, 0x30, 0x31, 0, s1);
    h.end(500, 502, T0 + 31 * MS, 0x51);
    h.take(500, 501, T0 + 40 * MS, 0x11, 0, s2);
    h.take(500, 501, T0 + 41 * MS, 0x21, 0, s2);
    h.publish(500, 502, T0 + 50 * MS, 0x30, 0x31, 0, s2);
    h.publish(500, 502, T0 + 60 * MS, 0x30, 0x31, 0, wall(T0 + 59 * MS));
    h.publish(500, 503, T0 + 70 * MS, 0x40, 0x41, 0, s1);
    let w = h.window(T0 + W);
    let stamp = flows(&w, FlowKind::Stamp);
    let linked: Vec<_> = stamp.iter().filter(|f| f.count > 0).collect();
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0].input, "topic:ros:/in1");
    assert_eq!(linked[0].output, "topic:ros:/out");
    assert_eq!(linked[0].count, 1);
    assert_eq!(linked[0].dwell.as_ref().unwrap().max_ns, 20 * MS + US);
    assert_eq!(linked[0].ambiguous, 1);
    let in2 = stamp.iter().find(|f| f.input == "topic:ros:/in2").unwrap();
    assert_eq!((in2.count, in2.ambiguous), (0, 1));
    let blind = flows(&w, FlowKind::Blind);
    assert_eq!(blind.len(), 1);
    assert_eq!(
        (blind[0].output.as_str(), blind[0].count),
        ("topic:ros:/out", 1)
    );
    assert!(w.flows.iter().all(|f| f.output != "topic:ros:/out2"));
    assert_eq!(topic(&w, "topic:ros:/out").expected_hz, Some(10.0));
}

/// roscope `stamp-false-edge`: the first 8 bytes of a message without a header can look like a
/// time; such values fail the clock check and never link two topics.
#[test]
fn bytes_that_only_look_like_a_stamp_do_not_link() {
    let mut h = H::offline();
    fuse(&mut h);
    let fake = stamp_of(42 * W + 7);
    for i in 0..20u64 {
        h.take(500, 501, T0 + i * MS, 0x11, 0, fake + (i << 32));
        h.publish(
            500,
            502,
            T0 + i * MS + 500 * US,
            0x30,
            0x31,
            0,
            fake + (i << 32),
        );
    }
    let w = h.window(T0 + W);
    assert!(w.flows.is_empty(), "{:?}", w.flows);
    assert!(topic(&w, "topic:ros:/in1").age_take.is_none());
}

#[test]
fn sim_time_stamps_link_once_clock_is_published() {
    let mut h = H::offline();
    fuse(&mut h);
    h.node(600, 0xa0, "sim", "/");
    h.publisher(600, 0x60, 0xa0, 0x61, "/clock");
    let sim = stamp_of(42 * W);
    h.publish(600, 600, T0, 0x60, 0x61, 0, stamp_of(42 * W + 5 * MS));
    h.take(500, 501, T0 + MS, 0x11, 0, sim);
    h.publish(500, 502, T0 + 2 * MS, 0x30, 0x31, 0, sim);
    let w = h.window(T0 + W);
    let f = flows(&w, FlowKind::Stamp);
    assert_eq!((f.len(), f[0].count), (1, 1));
    assert_eq!(
        topic(&w, "topic:ros:/in1")
            .age_take
            .as_ref()
            .unwrap()
            .max_ns,
        5 * MS
    );
}

#[test]
fn sync_flow_counts_a_dual_publish_once() {
    let mut h = H::offline();
    h.node(700, 0xf0, "filter", "/");
    h.subscription(700, 0x10, 0xf0, "/scan");
    h.publisher(700, 0x20, 0xf0, 0x21, "/scan_f");
    for i in 0..10u64 {
        let t = T0 + i * 10 * MS;
        h.take(700, 701, t, 0x11, 0, wall(t));
        h.start(700, 701, t + US, 0x13, false);
        let p = armo_ev_publish {
            publisher: 0x20,
            message: 2,
            stamp: wall(t),
        };
        h.ev(700, 701, t + 99 * US, ARMO_EV_RCLCPP_INTRA_PUBLISH, p, &[]);
        h.publish(700, 701, t + 100 * US, 0x20, 0x21, 1 + i as i64, wall(t));
        h.end(700, 701, t + 200 * US, 0x13);
    }
    let w = h.window(T0 + W);
    let sync = flows(&w, FlowKind::Sync);
    assert_eq!(sync.len(), 1);
    assert_eq!(
        (sync[0].count, sync[0].input.as_str()),
        (10, "topic:ros:/scan")
    );
    assert_eq!(sync[0].dwell.as_ref().unwrap().max_ns, 100 * US);
    assert_eq!(flows(&w, FlowKind::Stamp)[0].count, 10);
    assert_eq!(topic(&w, "topic:ros:/scan_f").pub_hz, Some(10.0));
}

/// Humble: `rcl_publish` carries the handle and stamp, `rmw_publish(message)` has neither, so
/// the hop is the take's wall time minus the DDS source timestamp, and loss is by count.
#[test]
fn rcl_publish_abi_hops_and_counts() {
    let mut h = H::offline();
    h.node(800, 0xa0, "talker", "/");
    h.publisher(800, 0x81, 0xa0, 0x82, "/h");
    h.node(900, 0xc0, "listener", "/");
    h.subscription(900, 0x90, 0xc0, "/h");
    for i in 0..10u64 {
        let t = T0 + i * 10 * MS;
        let p = armo_ev_publish {
            publisher: 0x81,
            message: 1,
            stamp: wall(t),
        };
        h.ev(800, 800, t, ARMO_EV_RCL_PUBLISH, p, &[]);
        let p = armo_ev_rmw_publish_rcl {
            message: 1,
            stamp: wall(t),
        };
        h.ev(800, 800, t + 1, ARMO_EV_RMW_PUBLISH_RCL, p, &[]);
        if i != 4 {
            h.take(900, 900, t + 40 * US, 0x91, (t + REAL) as i64, wall(t));
        }
    }
    let w = h.window(T0 + W);
    let t = topic(&w, "topic:ros:/h");
    assert_eq!(t.pub_hz, Some(10.0));
    let hop = t.hop.as_ref().unwrap();
    assert_eq!((hop.n, hop.max_ns, hop.mean_ns), (9, 40 * US, 40 * US));
    let s = sub(&w, "topic:ros:/h", 900);
    assert_eq!((s.lost, s.recv_hz), (Some(1), Some(9.0)));
    let p = w.processes.iter().find(|p| p.pid == 800).unwrap();
    assert!(p.ros.is_none());
}

/// Four composable nodes on one executor thread; node i's timer runs i x 0.5 ms of CPU every
/// 10 ms. Node 2 also sleeps 2 ms inside its callback. CPU splits 1:2:3:4 and the sleep counts
/// only as wall time; nodes plus the shared bucket equal the process.
#[test]
fn per_node_cpu_in_a_single_threaded_container() {
    let mut h = H::offline();
    let (pid, tid) = (1000u32, 1000u32);
    for i in 1..=4u64 {
        h.node(pid, 0x100 + i, &format!("n{i}"), "/");
        h.timer(pid, 0x300 + i, 10 * MS as i64, 0x100 + i, 0x200 + i);
    }
    h.m.thread_cpu(pid, tid, 0);
    for k in 0..100u64 {
        let base = T0 + k * 10 * MS;
        h.switch(0, 0, base, tid);
        let p = armo_ev_rcl_wait_exit {
            wait_set: 0x400,
            ret: 0,
            pad: 0,
        };
        h.ev(pid, tid, base, ARMO_EV_RCL_WAIT_EXIT, p, &[]);
        let mut t = base + 100 * US;
        for i in 1..=4u64 {
            let busy = i * 500 * US;
            h.start(pid, tid, t, 0x200 + i, false);
            if i == 2 {
                h.switch(pid, tid, t + busy / 2, 0);
                h.switch(0, 0, t + busy / 2 + 2 * MS, tid);
                t += 2 * MS;
            }
            t += busy;
            h.end(pid, tid, t, 0x200 + i);
        }
        let p = armo_ev_rcl_wait_enter {
            wait_set: 0x400,
            timeout: -1,
        };
        h.ev(pid, tid, t + 100 * US, ARMO_EV_RCL_WAIT_ENTER, p, &[]);
        h.switch(pid, tid, t + 100 * US, 0);
    }
    h.m.thread_cpu(pid, tid, 520 * MS);
    let w = h.window(T0 + W);
    let cpu = |i: u64| {
        let id = format!("n:{pid}:0:{:x}", 0x100 + i);
        w.nodes.iter().find(|n| n.id == id).unwrap()
    };
    for i in 1..=4u64 {
        let n = cpu(i);
        assert_eq!(n.cpu_source, CpuSource::Sched);
        assert!((n.cpu_pct.unwrap() - 5.0 * i as f64).abs() < 1e-9, "{n:?}");
    }
    assert!((cpu(2).busy_pct.unwrap() - 30.0).abs() < 1e-9);
    let p = w.processes.iter().find(|p| p.pid == pid).unwrap();
    assert!((p.cpu_pct.unwrap() - 52.0).abs() < 1e-9);
    let nodes: f64 = (1..=4).map(|i| cpu(i).cpu_pct.unwrap()).sum();
    assert!((nodes + p.shared_cpu_pct.unwrap() - p.cpu_pct.unwrap()).abs() < 1e-9);
    assert!((p.shared_cpu_pct.unwrap() - 2.0).abs() < 1e-9);
    let th = w.threads.iter().find(|t| t.tid == tid).unwrap();
    assert!(th.bucket.starts_with("executor:e:1000:0:"), "{}", th.bucket);
    assert!((th.cpu_pct.unwrap() - 52.0).abs() < 1e-9);
    assert!((th.in_callback_cpu_pct.unwrap() - 50.0).abs() < 1e-9);
    let c2 = cb(&w, pid, 0x202);
    assert_eq!(c2.oncpu_mean_ns, Some(MS));
    assert_eq!(c2.duration.as_ref().unwrap().max_ns, 3 * MS);
}

/// `component_container_isolated`: a thread per node, so each thread's CPU outside callbacks is
/// its node's too.
#[test]
fn per_node_cpu_with_a_thread_per_node() {
    let mut h = H::offline();
    let pid = 1100u32;
    for i in 1..=2u64 {
        let tid = pid + i as u32;
        h.node(pid, 0x100 + i, &format!("n{i}"), "/");
        h.timer(pid, 0x300 + i, 10 * MS as i64, 0x100 + i, 0x200 + i);
        h.m.thread_cpu(pid, tid, 0);
        for k in 0..10u64 {
            let t = T0 + k * 10 * MS;
            h.cb(pid, tid, t, t + i * MS, 0x200 + i);
        }
        h.m.thread_cpu(pid, tid, 10 * i * MS + 3 * MS);
    }
    let w = h.window(T0 + W);
    for i in 1..=2u64 {
        let id = format!("n:{pid}:0:{:x}", 0x100 + i);
        let n = w.nodes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(n.cpu_source, CpuSource::WallBound);
        assert!(
            (n.cpu_pct.unwrap() - (i as f64 + 0.3)).abs() < 1e-9,
            "{n:?}"
        );
        let th = w.threads.iter().find(|t| t.tid == pid + i as u32).unwrap();
        assert_eq!(th.bucket, format!("node:{id}"));
    }
    let p = w.processes.iter().find(|p| p.pid == pid).unwrap();
    assert_eq!(p.shared_cpu_pct, Some(0.0));
}

#[test]
fn flux2_channels_of_one_type_on_two_keys_are_two_topics() {
    let mut h = H::offline();
    h.node(1200, 0x1, "cams", "/");
    for (channel, key, commits) in [(0xa1u64, "cam0", 10u64), (0xa2, "cam1", 5)] {
        let segment = format!("/flux2.v6..{key}.image.0123456789abcdef.e0");
        let init = armo_ev_flux2_channel_init {
            channel,
            dev: 7,
            ino: channel,
            publisher: 1,
            slot_count: 4,
            slot_size: 4096,
            stamp_offset: 0,
        };
        h.ev(
            1200,
            1200,
            INIT + 2,
            ARMO_EV_FLUX2_CHANNEL_INIT,
            init,
            &[segment.as_bytes()],
        );
        for i in 1..=commits {
            let t = T0 + i * 10 * MS;
            let p = armo_ev_flux2_commit {
                channel,
                ticket: i,
                nbytes: 1000,
                stamp: wall(t),
                slot: 0,
                woke: 0,
            };
            h.ev(1200, 1200, t, ARMO_EV_FLUX2_COMMIT, p, &[]);
        }
    }
    let w = h.window(T0 + W);
    assert_eq!(
        topic(&w, "topic:flux2:/cam0/image:0123456789abcdef").pub_hz,
        Some(10.0)
    );
    assert_eq!(
        topic(&w, "topic:flux2:/cam1/image:0123456789abcdef").pub_hz,
        Some(5.0)
    );
}

#[test]
fn flux2_pairs_by_segment_and_ticket_and_counts_its_own_losses() {
    let mut h = H::offline();
    let signpost = "/flux2.v6..image.0123456789abcdef";
    let segment = format!("{signpost}.e0");
    h.node(1200, 0x1, "cam", "/");
    let ch = |channel, publisher| armo_ev_flux2_channel_init {
        channel,
        dev: 7,
        ino: 9,
        publisher,
        slot_count: 4,
        slot_size: 4096,
        stamp_offset: 0,
    };
    h.ev(
        1200,
        1200,
        INIT + 2,
        ARMO_EV_FLUX2_CHANNEL_INIT,
        ch(0xa1, 1),
        &[segment.as_bytes()],
    );
    let ep = |endpoint, role| armo_ev_flux2_endpoint_init {
        endpoint,
        fingerprint: 0x0123_4567_89ab_cdef,
        role,
        lang: 0,
    };
    let strs = |node: &'static str| [node.as_bytes(), b"/image".as_slice(), signpost.as_bytes()];
    h.ev(
        1200,
        1200,
        INIT + 3,
        ARMO_EV_FLUX2_ENDPOINT_INIT,
        ep(0xe1, 0),
        &strs("/cam"),
    );
    for pid in [1300u32, 1400] {
        h.node(pid, 0x1, "viewer", "/");
        h.ev(
            pid,
            pid,
            INIT + 2,
            ARMO_EV_FLUX2_CHANNEL_INIT,
            ch(0xb1, 0),
            &[segment.as_bytes()],
        );
        h.ev(
            pid,
            pid,
            INIT + 3,
            ARMO_EV_FLUX2_ENDPOINT_INIT,
            ep(0xe2, 1),
            &strs("/viewer"),
        );
    }
    for i in 1..=20u64 {
        let t = T0 + i * 10 * MS;
        let p = armo_ev_flux2_commit {
            channel: 0xa1,
            ticket: i,
            nbytes: 1000,
            stamp: wall(t),
            slot: 0,
            woke: 1,
        };
        h.ev(1200, 1200, t, ARMO_EV_FLUX2_COMMIT, p, &[]);
        for (k, pid) in [1300u32, 1400].into_iter().enumerate() {
            if pid == 1400 && (i == 5 || i == 6) {
                continue;
            }
            let at = t + (20 + 10 * k as u64) * US;
            let p = armo_ev_flux2_wake {
                executor: 0xee,
                result: 1,
                entry: 1,
            };
            h.ev(pid, pid, at - 5 * US, ARMO_EV_FLUX2_WAKE, p, &[]);
            let p = armo_ev_flux2_callback_start {
                executor: 0xee,
                endpoint: 0xe2,
                entry: 1,
                pad: 0,
            };
            h.ev(pid, pid, at - 2 * US, ARMO_EV_FLUX2_CALLBACK_START, p, &[]);
            let p = armo_ev_flux2_take {
                channel: 0xb1,
                ticket: i,
                lost: u64::from(pid == 1400 && i >= 7) * 2,
                stamp: wall(t),
                slot: 0,
                pad: 0,
            };
            h.ev(pid, pid, at, ARMO_EV_FLUX2_TAKE, p, &[]);
            let p = armo_ev_flux2_callback_end {
                executor: 0xee,
                channel: 0xb1,
                entry: 1,
                delivered: 1,
            };
            h.ev(
                pid,
                pid,
                at + 50 * US,
                ARMO_EV_FLUX2_CALLBACK_END,
                p,
                &[segment.as_bytes()],
            );
        }
    }
    let claim = armo_ev_flux2_claim {
        channel: 0xa1,
        slot: ARMO_FLUX2_NO_SLOT,
        result: ARMO_FLUX2_CLAIM_BACKPRESSURE,
        loan: 0,
        pad: 0,
    };
    h.ev(1200, 1200, T0 + 500 * MS, ARMO_EV_FLUX2_CLAIM, claim, &[]);
    let drop = armo_ev_flux2_record_drop {
        channel: 0xa1,
        ticket: 0,
        count: 3,
        reason: ARMO_FLUX2_RECORD_CACHE_FULL,
    };
    h.ev(
        1200,
        1200,
        T0 + 600 * MS,
        ARMO_EV_FLUX2_RECORD_DROP,
        drop,
        &[],
    );
    let w = h.window(T0 + W);
    let id = "topic:flux2:/image:0123456789abcdef";
    let t = topic(&w, id);
    assert_eq!(t.pub_hz, Some(20.0));
    assert_eq!(t.bw_bps, Some(20_000.0));
    assert_eq!(t.hop.as_ref().unwrap().n, 38);
    assert_eq!(
        (t.drops.publish, t.drops.record, t.drops.ambiguous),
        (Some(1), Some(3), None)
    );
    assert_eq!(t.publishers, vec!["n:1200:0:1".to_string()]);
    let a = sub(&w, id, 1300);
    assert_eq!((a.lost, a.hop.as_ref().unwrap().max_ns), (Some(0), 20 * US));
    assert_eq!(a.wake.as_ref().unwrap().max_ns, 15 * US);
    let b = sub(&w, id, 1400);
    assert_eq!((b.lost, b.recv_hz), (Some(2), Some(18.0)));
    let c = w
        .callbacks
        .iter()
        .find(|c| c.id == "c:1300:0:flux2:ee:1")
        .unwrap();
    assert_eq!(
        (c.kind, c.node.as_deref()),
        (CallbackKind::Flux2, Some("n:1300:0:1"))
    );
    assert_eq!(c.topic.as_deref(), Some(id));
    assert_eq!(c.dispatch.as_ref().unwrap().max_ns, 3 * US);
}

#[test]
fn a_late_attached_take_names_the_next_callback() {
    let mut h = H::offline();
    let p = armo_ev_rmw_topic { rmw_handle: 0x77 };
    h.ev(
        1500,
        1500,
        T0,
        ARMO_EV_RMW_SUBSCRIPTION_TOPIC,
        p,
        &[b"/late"],
    );
    h.take(1500, 1500, T0 + MS, 0x77, 0, 0);
    h.cb(1500, 1500, T0 + MS + US, T0 + 2 * MS, 0x99);
    let w = h.window(T0 + W);
    let c = cb(&w, 1500, 0x99);
    assert_eq!(
        (c.kind, c.topic.as_deref(), c.node.as_ref()),
        (CallbackKind::Sub, Some("topic:ros:/late"), None)
    );
    assert!(w.processes.iter().find(|p| p.pid == 1500).unwrap().cold);
}

/// The live model takes the uprobe and LD_PRELOAD methods' callback times from their aggregate
/// tables and leaves the records to pairing; it reads `/proc` of a real process (this one).
#[test]
fn live_model_takes_callback_times_from_aggregates() {
    let pid = std::process::id();
    let mut m = Model::live();
    m.set_sched(Sel::All);
    let mut hist = vec![0; armo_common::BUCKETS];
    hist[armo_common::bucket(3 * MS)] = 4;
    let row = |kind, sum| Aggregate {
        pid,
        tid: pid,
        kind,
        src: ARMO_SRC_ROS,
        entry: 0,
        key: 0x55,
        count: 4,
        sum,
        max: 3 * MS,
        hist: hist.clone(),
    };
    m.aggregates(
        Method::Preload,
        &[
            row(ARMO_AGG_CALLBACK, 12 * MS),
            row(ARMO_AGG_OFFCPU, 4 * MS),
        ],
    );
    let mut batch = Vec::new();
    let p = armo_ev_callback_start {
        callback: 0x55,
        is_intra_process: 0,
        pad: 0,
    };
    records::encode_positioned(&mut batch, pid, pid, ARMO_EV_CALLBACK_START, T0, &p, &[]);
    let p = armo_ev_handle { handle: 0x55 };
    records::encode_positioned(&mut batch, pid, pid, ARMO_EV_CALLBACK_END, T0 + MS, &p, &[]);
    for r in records::positioned(&batch) {
        m.record(Method::Preload, &r.unwrap());
    }
    let w = m.window(T0 + W, T0 + W + REAL, W, true);
    let st = procfs::stat(pid).unwrap().starttime();
    let c = w
        .callbacks
        .iter()
        .find(|c| c.id == format!("c:{pid}:{st}:ros:55"))
        .unwrap();
    assert_eq!((c.hz, c.oncpu_mean_ns), (Some(4.0), Some(2 * MS)));
    assert_eq!(c.duration.as_ref().unwrap().n, 4);
    let p = w.processes.iter().find(|p| p.pid == pid).unwrap();
    assert!(!p.exited && p.rss_bytes.is_some() && !p.threads.is_empty());
    assert!(w.hists.unwrap().contains_key(&c.id));
}

#[inline(never)]
fn a_function_to_name() -> u64 {
    std::hint::black_box(7)
}

#[test]
fn code_addresses_resolve_through_maps_and_elf_symbols() {
    assert_eq!(a_function_to_name(), 7);
    let addr = a_function_to_name as fn() -> u64 as usize as u64;
    let name = Resolver::default()
        .resolve(std::process::id(), &[0x10, addr])
        .expect("a symbol");
    assert!(name.contains("a_function_to_name"), "{name}");
}

#[test]
fn std_function_names_shorten_to_their_callable() {
    let long = "std::_Function_handler<void (std::shared_ptr<sensor_msgs::msg::Imu_<std::allocator<void> > >), ImuSub::ImuSub()::{lambda(std::shared_ptr<sensor_msgs::msg::Imu_<std::allocator<void> > >)#1}>::_M_invoke(std::_Any_data const&, std::shared_ptr<sensor_msgs::msg::Imu_<std::allocator<void> > >&&)";
    assert_eq!(shorten(long), "ImuSub::ImuSub()::{lambda#1}");
    assert_eq!(shorten("Driver::on_scan(int)"), "Driver::on_scan(int)");
}

#[test]
fn nspid_maps_a_namespace_pid_back_to_the_host() {
    let pid = std::process::id();
    let st = procfs::stat(pid).unwrap().starttime();
    assert_eq!(procfs::host_pid(pid, st), Some(pid));
    assert_eq!(procfs::ns_pid(pid), None);
    let tid = procfs::tids(pid)[0];
    assert_eq!(procfs::host_tid(pid, tid), Some(tid));
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("armo_wp3b_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("events")).unwrap();
    dir
}

#[test]
fn analyze_reads_a_recording_and_reports_exact_tails() {
    let dir = scratch("analyze");
    let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../common/armo_proto/golden/meta/meta.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(golden).unwrap()).unwrap();
    meta["ros"] = serde_json::json!([]);
    meta["started"] = serde_json::json!({"mono_ns": T0, "real_ns": T0 + REAL});
    meta["methods_by_pid"] = [100, 200, 300, 400]
        .into_iter()
        .map(|p| (p.to_string(), serde_json::json!("preload")))
        .collect();
    std::fs::write(dir.join("meta.json"), meta.to_string()).unwrap();
    let line = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../common/armo_proto/golden/window_line/line.json");
    let line: serde_json::Value = serde_json::from_slice(&std::fs::read(line).unwrap()).unwrap();
    std::fs::write(dir.join("windows.jsonl"), format!("{line}\n")).unwrap();

    let mut h = H::offline();
    fan_out(&mut h, 1000);
    let header = armo_evfile_header {
        magic: ARMO_EVFILE_MAGIC,
        version: armo_common::shm::ARMO_SHM_VERSION,
        pad: 0,
    };
    let mut file = armo_common::bytes_of(&header).to_vec();
    file.extend_from_slice(&h.batch);
    std::fs::write(dir.join("events/000.armoev"), file).unwrap();

    let report = armo_collectd::analyze::run(&dir).unwrap();
    assert!(report.text.contains("transport hop"), "{}", report.text);
    let deliveries = std::fs::read_to_string(dir.join("analysis/deliveries.csv")).unwrap();
    assert_eq!(deliveries.lines().count(), 1 + 3000);
    let summary = std::fs::read_to_string(dir.join("analysis/summary.csv")).unwrap();
    let hop = summary
        .lines()
        .find(|l| l.starts_with("transport hop") && l.contains(CHATTER))
        .unwrap();
    let f: Vec<&str> = hop.split(',').collect();
    assert_eq!(f[2..], ["3000", "50000", "50000", "50000", "50000"]);
    let wake = summary
        .lines()
        .find(|l| l.starts_with("wake latency") && l.contains("#n:300:"))
        .unwrap();
    assert!(wake.ends_with(",1000,35000,35000,35000,35000"), "{wake}");
    let callbacks = std::fs::read_to_string(dir.join("analysis/callbacks.csv")).unwrap();
    assert_eq!(callbacks.lines().count(), 1 + 3000);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn lttng_counts_are_over_the_chunk_span() {
    let mut h = H::offline();
    h.method = Method::Lttng;
    for i in 0..53 {
        let t = T0 + i * 19 * MS;
        h.cb(500, 500, t, t + 2 * MS, 0xa1);
    }
    h.m.set_lttng_span(1_050 * MS);
    let c = h
        .window(T0 + W)
        .callbacks
        .iter()
        .find(|c| c.id.ends_with(":a1"))
        .cloned()
        .unwrap();
    assert!((c.hz.unwrap() - 53.0 / 1.05).abs() < 1e-9, "{:?}", c.hz);
    h.method = Method::Preload;
    h.cb(600, 600, T0 + W + MS, T0 + W + 3 * MS, 0xb1);
    h.m.set_lttng_span(1_050 * MS);
    let c = h
        .window(T0 + 2 * W)
        .callbacks
        .iter()
        .find(|c| c.id.ends_with(":b1"))
        .cloned()
        .unwrap();
    assert_eq!(c.hz, Some(1.0), "a preload process keeps the window");
}

#[test]
fn a_process_outside_the_hot_set_has_no_rates() {
    let mut h = H::offline();
    h.m.set_hot(Sel::pids([100]));
    h.node(100, 0xa0, "talker", "/");
    h.publisher(100, 0xb0, 0xa0, 0xb1, "/chatter");
    h.node(200, 0xc0, "listener", "/");
    h.subscription(200, 0xd0, 0xc0, "/chatter");
    h.publish(100, 100, T0 + MS, 0xb0, 0xb1, 1000, wall(T0 + MS));
    h.cb(100, 100, T0 + 2 * MS, T0 + 3 * MS, 0xa1);
    h.cb(200, 200, T0 + 2 * MS, T0 + 3 * MS, 0xc1);
    let w = h.window(T0 + W);
    assert_eq!(cb(&w, 100, 0xa1).hz, Some(1.0));
    assert_eq!(cb(&w, 200, 0xc1).hz, None);
    assert_eq!(cb(&w, 200, 0xc1).duration, None);
    let method = |pid| w.processes.iter().find(|p| p.pid == pid).unwrap().method;
    assert_eq!(method(100), Some(Method::Preload));
    assert_eq!(method(200), None);
    let node = |pid| w.nodes.iter().find(|n| n.pid == pid).unwrap();
    assert!(node(100).cb_hz.is_some());
    assert_eq!((node(200).cpu_pct, node(200).cb_hz), (None, None));
    assert_eq!(topic(&w, CHATTER).pub_hz, Some(1.0));
    let s = sub(&w, CHATTER, 200);
    assert_eq!((s.recv_hz, s.lost), (None, None));
}

#[test]
fn a_topic_scales_only_its_lttng_part() {
    let mut h = H::offline();
    h.method = Method::Lttng;
    h.node(100, 0xa0, "talker", "/");
    h.publisher(100, 0xb0, 0xa0, 0xb1, "/chatter");
    h.node(200, 0xc0, "listener", "/");
    h.subscription(200, 0xd0, 0xc0, "/chatter");
    for i in 0..53 {
        let t = T0 + i * 19 * MS;
        h.publish(100, 100, t, 0xb0, 0xb1, 1000 + i as i64, wall(t));
        h.take(200, 200, t + 30 * US, 0xd1, 1000 + i as i64, wall(t));
    }
    h.feed();
    h.method = Method::Preload;
    h.node(150, 0xa0, "talker2", "/");
    h.publisher(150, 0xb0, 0xa0, 0xb1, "/chatter");
    for i in 0..50 {
        let t = T0 + i * 20 * MS + 5 * MS;
        h.publish(150, 150, t, 0xb0, 0xb1, 5000 + i as i64, wall(t));
    }
    h.m.set_lttng_span(1_050 * MS);
    let w = h.window(T0 + W);
    let t = topic(&w, CHATTER);
    assert!(
        (t.pub_hz.unwrap() - (53.0 / 1.05 + 50.0)).abs() < 1e-9,
        "{:?}",
        t.pub_hz
    );
    let s = sub(&w, CHATTER, 200);
    assert!(
        (s.recv_hz.unwrap() - 53.0 / 1.05).abs() < 1e-9,
        "{:?}",
        s.recv_hz
    );
}
