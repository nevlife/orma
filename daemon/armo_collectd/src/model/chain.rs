//! Chains measured end to end over the topics the user names (design 20.6), and the checks of
//! the chain and latest-node settings that the server and `armo analyze` share.

use std::collections::{HashMap, VecDeque};

use armo_proto::msg::{Chain, ChainMode, ChainSpec};
use regex::Regex;

use super::lat::LatAcc;
use super::pair::STAMP_CAP;
use super::stamp::AGE_MAX_NS;

/// `(chain, time of the source's publish)` for each lineage chain a message carries.
pub type Origins = Vec<(usize, u64)>;

pub fn check(chains: &[ChainSpec]) -> Result<(), String> {
    match chains.iter().find(|c| c.topics.len() < 2) {
        Some(c) => Err(format!("chain {:?} needs at least two topics", c.name)),
        None => Ok(()),
    }
}

/// `latest_nodes` as regular expressions over whole node names.
pub fn latest_nodes(res: &[String]) -> Result<Vec<Regex>, String> {
    res.iter()
        .map(|r| Regex::new(&format!("^(?:{r})$")).map_err(|e| format!("latest_nodes {r:?}: {e}")))
        .collect()
}

/// A `first` chain's source publish whose steps are still being found.
struct Run {
    t0: u64,
    next: usize,
    last: u64,
}

#[derive(Default)]
struct ChainWin {
    e2e: LatAcc,
    steps: Vec<LatAcc>,
    broken: u64,
}

#[derive(Default)]
pub struct Chains {
    pub specs: Vec<ChainSpec>,
    /// Per topic, the chains and positions it is at, once looked up.
    at: HashMap<usize, Vec<(usize, usize)>>,
    runs: Vec<Option<Run>>,
    win: Vec<ChainWin>,
    /// The origins of each take, by take number, oldest dropped first.
    taken: HashMap<u64, Origins>,
    order: VecDeque<(u64, u64)>,
    /// The origins of the last intra-process publish per topic, which the callback it starts
    /// takes.
    pub intra: HashMap<usize, Origins>,
}

impl Chains {
    pub fn new(specs: Vec<ChainSpec>) -> Chains {
        let mut c = Chains {
            runs: specs.iter().map(|_| None).collect(),
            specs,
            ..Chains::default()
        };
        c.end_window();
        c
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// A topic is named by its name or its entity id.
    pub fn positions(
        &mut self,
        topic: usize,
        name: &str,
        id: impl FnOnce() -> String,
    ) -> Vec<(usize, usize)> {
        let specs = &self.specs;
        self.at
            .entry(topic)
            .or_insert_with(|| {
                let id = id();
                let mut at = Vec::new();
                for (c, spec) in specs.iter().enumerate() {
                    for (k, t) in spec.topics.iter().enumerate() {
                        if t == name || *t == id {
                            at.push((c, k));
                        }
                    }
                }
                at
            })
            .clone()
    }

    pub fn lineage(&self, chain: usize) -> bool {
        self.specs[chain].mode == ChainMode::Lineage
    }

    pub fn took(&mut self, take: u64, t: u64, origins: Origins) {
        self.taken.insert(take, origins);
        self.order.push_back((t, take));
        while self.order.len() > STAMP_CAP {
            self.drop_front();
        }
    }

    pub fn origin(&self, take: u64, chain: usize) -> Option<u64> {
        self.taken
            .get(&take)?
            .iter()
            .find(|(c, _)| *c == chain)
            .map(|(_, t)| *t)
    }

    fn drop_front(&mut self) {
        if let Some((_, take)) = self.order.pop_front() {
            self.taken.remove(&take);
        }
    }

    pub fn evict(&mut self, now: u64) {
        let before = now.saturating_sub(AGE_MAX_NS);
        while self.order.front().is_some_and(|&(t, _)| t < before) {
            self.drop_front();
        }
    }

    /// A lineage publish on `topics[k]`, `k > 0`, with the origin it reached or none.
    pub fn step(&mut self, chain: usize, k: usize, t: u64, origin: Option<u64>) {
        let last = self.specs[chain].topics.len() - 1;
        let w = &mut self.win[chain];
        match origin {
            Some(o) => {
                w.steps[k - 1].add(t.saturating_sub(o));
                if k == last {
                    w.e2e.add(t.saturating_sub(o));
                }
            }
            None => w.broken += 1,
        }
    }

    /// A publish on `topics[k]` of a `first` chain; the source's time when it is a step.
    pub fn first(&mut self, chain: usize, k: usize, t: u64) -> Option<u64> {
        let last = self.specs[chain].topics.len() - 1;
        let w = &mut self.win[chain];
        let run = &mut self.runs[chain];
        if k == 0 {
            if run.is_some() {
                w.broken += 1;
            }
            *run = Some(Run {
                t0: t,
                next: 1,
                last: t,
            });
            return None;
        }
        let r = run.as_mut().filter(|r| r.next == k && t > r.last)?;
        let t0 = r.t0;
        w.steps[k - 1].add(t - t0);
        r.next += 1;
        r.last = t;
        if k == last {
            w.e2e.add(t - t0);
            *run = None;
        }
        Some(t0)
    }

    pub fn rows(&self) -> Vec<Chain> {
        self.specs
            .iter()
            .zip(&self.win)
            .map(|(s, w)| Chain {
                name: s.name.clone(),
                mode: s.mode,
                topics: s.topics.clone(),
                count: w.e2e.n,
                e2e: w.e2e.lat(),
                steps: w.steps.iter().map(LatAcc::lat).collect(),
                broken: w.broken,
            })
            .collect()
    }

    pub fn end_window(&mut self) {
        self.win = self
            .specs
            .iter()
            .map(|s| ChainWin {
                steps: vec![LatAcc::default(); s.topics.len() - 1],
                ..ChainWin::default()
            })
            .collect();
    }
}
