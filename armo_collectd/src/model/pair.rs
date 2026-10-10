//! Pairing a message's publish with its takes and the stamp index of
//! asynchronous flows inside a node.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use super::stamp::AGE_MAX_NS;
use super::{NodeIx, PubKey, SubKey, TopicIx};

/// How long a publish waits for its takes; a connected subscription that has not taken it by
/// then lost it.
pub const HORIZON_NS: u64 = 5_000_000_000;
/// Stamps indexed per collector, oldest dropped first (roscope's map size).
pub const STAMP_CAP: usize = 131_072;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PairKey {
    /// `(topic, source_timestamp)`: the timestamp alone would join different topics.
    Ros(TopicIx, i64),
    /// `(SegmentId(dev, ino), ticket)`.
    Flux2(u64, u64, u64),
}

pub struct Sent {
    pub t: u64,
    pub publisher: PubKey,
    pub topic: TopicIx,
    /// Another publisher of the topic sent the same key: no take of it is paired.
    pub ambiguous: bool,
    pub takers: Vec<SubKey>,
}

pub struct Pending {
    pub key: PairKey,
    pub sub: SubKey,
    pub t: u64,
    pub wake: Option<u64>,
    pub source_ts: Option<i64>,
    pub delivery: Option<u64>,
    pub windows: u8,
}

#[derive(Default)]
pub struct Pairing {
    pub sent: HashMap<PairKey, Sent>,
    order: VecDeque<(u64, PairKey)>,
    /// Per publisher, its subscriptions and the publish time of the first message each took.
    pub connected: HashMap<PubKey, BTreeMap<SubKey, u64>>,
    pub pending: Vec<Pending>,
}

impl Pairing {
    pub fn publish(&mut self, key: PairKey, t: u64, publisher: PubKey, topic: TopicIx) {
        if let Some(s) = self.sent.get_mut(&key) {
            if s.publisher != publisher {
                s.ambiguous = true;
            }
            return;
        }
        self.sent.insert(
            key,
            Sent {
                t,
                publisher,
                topic,
                ambiguous: false,
                takers: Vec::new(),
            },
        );
        self.order.push_back((t, key));
    }

    /// Publishes older than `before`, removed.
    pub fn evict(&mut self, before: u64) -> Vec<Sent> {
        let mut out = Vec::new();
        while let Some(&(t, key)) = self.order.front() {
            if t >= before {
                break;
            }
            self.order.pop_front();
            if self.sent.get(&key).is_some_and(|s| s.t == t) {
                out.extend(self.sent.remove(&key));
            }
        }
        out
    }
}

pub struct StampTake {
    /// The input topics that took this stamp in the node; more than one is ambiguous.
    pub inputs: Vec<TopicIx>,
    pub t: u64,
}

#[derive(Default)]
pub struct StampIndex {
    map: HashMap<(NodeIx, u64), StampTake>,
    order: VecDeque<(u64, NodeIx, u64)>,
    /// Nodes that took a message with a stamp that passed the clock check.
    pub stamped: HashSet<NodeIx>,
}

impl StampIndex {
    pub fn take(&mut self, node: NodeIx, stamp_ns: u64, topic: TopicIx, t: u64) {
        self.stamped.insert(node);
        let e = self
            .map
            .entry((node, stamp_ns))
            .or_insert_with(|| StampTake {
                inputs: Vec::new(),
                t,
            });
        if !e.inputs.contains(&topic) {
            e.inputs.push(topic);
        }
        e.t = t;
        self.order.push_back((t, node, stamp_ns));
        while self.order.len() > STAMP_CAP {
            self.drop_front();
        }
    }

    pub fn find(&self, node: NodeIx, stamp_ns: u64) -> Option<&StampTake> {
        self.map.get(&(node, stamp_ns))
    }

    fn drop_front(&mut self) {
        if let Some((t, node, stamp)) = self.order.pop_front()
            && self.map.get(&(node, stamp)).is_some_and(|e| e.t == t)
        {
            self.map.remove(&(node, stamp));
        }
    }

    pub fn evict(&mut self, now: u64) {
        let before = now.saturating_sub(AGE_MAX_NS);
        while self.order.front().is_some_and(|&(t, _, _)| t < before) {
            self.drop_front();
        }
    }
}
