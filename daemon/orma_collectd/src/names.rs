//! Choosing processes by node and topic names (design 14.3): regular expressions over whole
//! names, matched against what the last window knows.

use std::collections::BTreeSet;

use orma_proto::msg::Window;
use regex::Regex;

/// The names the last window knew, by process.
#[derive(Default)]
pub struct Known {
    nodes: Vec<(String, u32)>,
    topics: Vec<(String, Vec<u32>)>,
    live: Vec<u32>,
}

impl Known {
    pub fn of(w: &Window) -> Known {
        Known {
            nodes: w.nodes.iter().map(|n| (n.fqn.clone(), n.pid)).collect(),
            topics: w
                .topics
                .iter()
                .map(|t| {
                    let ends = t.publishers.iter().chain(&t.subscribers);
                    (t.name.clone(), ends.filter_map(|o| owner_pid(o)).collect())
                })
                .collect(),
            live: w
                .processes
                .iter()
                .filter(|p| !p.exited)
                .map(|p| p.pid)
                .collect(),
        }
    }
}

/// The pid in an owner id, `n:<pid>:...` or `p:<pid>:...`.
fn owner_pid(id: &str) -> Option<u32> {
    id.split(':').nth(1)?.parse().ok()
}

#[derive(Clone, Debug)]
pub struct Names {
    node: Option<Regex>,
    topic: Option<Regex>,
    exclude: Option<Regex>,
}

fn whole(re: Option<&String>) -> Result<Option<Regex>, String> {
    re.map(|r| Regex::new(&format!("^(?:{r})$")).map_err(|e| format!("{r:?}: {e}")))
        .transpose()
}

impl Names {
    /// `None` when no name is given.
    pub fn new(
        node: Option<&String>,
        topic: Option<&String>,
        exclude: Option<&String>,
    ) -> Result<Option<Names>, String> {
        if node.is_none() && topic.is_none() && exclude.is_none() {
            return Ok(None);
        }
        Ok(Some(Names {
            node: whole(node)?,
            topic: whole(topic)?,
            exclude: whole(exclude)?,
        }))
    }

    /// Only `exclude`: what it names is left out of every process.
    pub fn only_excludes(&self) -> bool {
        self.node.is_none() && self.topic.is_none()
    }

    /// The live processes with a matching node, or with a publisher or subscriber of a matching
    /// topic (both ends, for the hops); every live process when only `exclude` is given. A
    /// process whose known nodes all match `exclude` is left out.
    pub fn pids(&self, k: &Known) -> BTreeSet<u32> {
        let mut pids = BTreeSet::new();
        if self.node.is_none() && self.topic.is_none() {
            pids.extend(&k.live);
        }
        if let Some(re) = &self.node {
            pids.extend(k.nodes.iter().filter(|n| re.is_match(&n.0)).map(|n| n.1));
        }
        if let Some(re) = &self.topic {
            for (_, ends) in k.topics.iter().filter(|t| re.is_match(&t.0)) {
                pids.extend(ends);
            }
        }
        if let Some(re) = &self.exclude {
            pids.retain(|pid| {
                let mut nodes = k.nodes.iter().filter(|n| n.1 == *pid).peekable();
                nodes.peek().is_none() || !nodes.all(|n| re.is_match(&n.0))
            });
        }
        pids.retain(|pid| k.live.contains(pid));
        pids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Known {
        Known {
            nodes: vec![
                ("/cam/driver".into(), 1),
                ("/talker".into(), 2),
                ("/talker".into(), 3),
                ("/listener".into(), 3),
            ],
            topics: vec![("/image".into(), vec![1, 4])],
            live: vec![1, 2, 3, 4],
        }
    }

    fn pids(node: Option<&str>, topic: Option<&str>, exclude: Option<&str>) -> Vec<u32> {
        let s = |v: Option<&str>| v.map(String::from);
        let (node, topic, exclude) = (s(node), s(topic), s(exclude));
        let names = Names::new(node.as_ref(), topic.as_ref(), exclude.as_ref());
        names.unwrap().unwrap().pids(&known()).into_iter().collect()
    }

    #[test]
    fn names_match_whole_names() {
        assert_eq!(pids(Some("/cam/.*"), None, None), [1]);
        assert_eq!(pids(Some("/cam"), None, None), Vec::<u32>::new());
        assert_eq!(pids(None, Some("/image"), None), [1, 4]);
        assert_eq!(pids(Some("/talker"), Some("/image"), None), [1, 2, 3, 4]);
    }

    #[test]
    fn exclude_drops_a_process_only_when_all_its_nodes_match() {
        assert_eq!(pids(None, None, Some("/talker")), [1, 3, 4]);
        assert_eq!(pids(None, None, Some("/talker|/listener")), [1, 4]);
    }
}
