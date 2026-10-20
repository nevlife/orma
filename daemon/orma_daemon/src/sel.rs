//! Which processes a probe set or the event stream covers.

use std::collections::BTreeSet;

use orma_proto::msg::Selection;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Sel {
    #[default]
    None,
    All,
    Pids(BTreeSet<u32>),
}

impl Sel {
    pub fn of(s: &Selection) -> Sel {
        if s.all {
            Sel::All
        } else {
            Sel::pids(s.pids.iter().copied())
        }
    }

    pub fn pids(pids: impl IntoIterator<Item = u32>) -> Sel {
        let set: BTreeSet<u32> = pids.into_iter().collect();
        if set.is_empty() {
            Sel::None
        } else {
            Sel::Pids(set)
        }
    }

    pub fn add(&mut self, other: &Sel) {
        *self = match (std::mem::take(self), other) {
            (Sel::All, _) | (_, Sel::All) => Sel::All,
            (Sel::None, o) => o.clone(),
            (s, Sel::None) => s,
            (Sel::Pids(mut a), Sel::Pids(b)) => {
                a.extend(b);
                Sel::Pids(a)
            }
        };
    }

    /// An empty list detaches every process of the set (WP3a's reading of 12.8).
    pub fn detach(&mut self, pids: &[u32]) {
        if pids.is_empty() {
            *self = Sel::None;
            return;
        }
        if let Sel::Pids(p) = self {
            for pid in pids {
                p.remove(pid);
            }
            if p.is_empty() {
                *self = Sel::None;
            }
        }
    }

    pub fn contains(&self, pid: u32) -> bool {
        match self {
            Sel::None => false,
            Sel::All => true,
            Sel::Pids(p) => p.contains(&pid),
        }
    }

    pub fn list(&self) -> Option<&BTreeSet<u32>> {
        match self {
            Sel::Pids(p) => Some(p),
            _ => None,
        }
    }

    /// Drops processes that no longer exist.
    pub fn retain(&mut self, alive: impl Fn(u32) -> bool) {
        if let Sel::Pids(p) = self {
            p.retain(|pid| alive(*pid));
            if p.is_empty() {
                *self = Sel::None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_detach_clears_the_set() {
        let mut s = Sel::pids([1, 2]);
        s.detach(&[1]);
        assert_eq!(s, Sel::pids([2]));
        s.detach(&[]);
        assert_eq!(s, Sel::None);
        let mut all = Sel::All;
        all.detach(&[]);
        assert_eq!(all, Sel::None);
    }

    #[test]
    fn all_absorbs_a_list() {
        let mut s = Sel::pids([1]);
        s.add(&Sel::All);
        assert_eq!(s, Sel::All);
        s.add(&Sel::pids([3]));
        assert_eq!(s, Sel::All);
        let mut n = Sel::None;
        n.add(&Sel::pids([4]));
        assert!(n.contains(4) && !n.contains(5));
    }
}
