use armo_common::{BUCKETS, CallbackStat};

pub fn delta(now: &CallbackStat, before: Option<&CallbackStat>) -> CallbackStat {
    let Some(b) = before else {
        return *now;
    };
    let mut hist = [0; BUCKETS];
    for (i, h) in hist.iter_mut().enumerate() {
        *h = now.hist[i].wrapping_sub(b.hist[i]);
    }
    CallbackStat {
        count: now.count.wrapping_sub(b.count),
        sum_ns: now.sum_ns.wrapping_sub(b.sum_ns),
        hist,
    }
}

/// Upper bound of the bucket holding the `q` quantile: the true value is at most this, and more
/// than half of it.
pub fn quantile_upper_ns(hist: &[u64; BUCKETS], q: f64) -> Option<u64> {
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return None;
    }
    let rank = ((q * total as f64).ceil() as u64).max(1);
    let mut seen = 0;
    for (i, &n) in hist.iter().enumerate() {
        seen += n;
        if seen >= rank {
            return Some(1u64 << (i + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use armo_common::bucket;

    fn stat(samples: &[u64]) -> CallbackStat {
        let mut s = CallbackStat {
            count: 0,
            sum_ns: 0,
            hist: [0; BUCKETS],
        };
        for &ns in samples {
            s.count += 1;
            s.sum_ns += ns;
            s.hist[bucket(ns)] += 1;
        }
        s
    }

    #[test]
    fn bucket_edges() {
        assert_eq!(bucket(0), 0);
        assert_eq!(bucket(1), 0);
        assert_eq!(bucket(2), 1);
        assert_eq!(bucket(3), 1);
        assert_eq!(bucket(1023), 9);
        assert_eq!(bucket(1024), 10);
        assert_eq!(bucket(u64::MAX), BUCKETS - 1);
        for i in 1..BUCKETS as u32 {
            assert_eq!(bucket(1 << i), i as usize);
            assert_eq!(bucket((1 << i) - 1), i as usize - 1);
        }
    }

    #[test]
    fn delta_subtracts_the_previous_window() {
        let before = stat(&[100, 200]);
        let now = stat(&[100, 200, 5000]);
        let d = delta(&now, Some(&before));
        assert_eq!(d.count, 1);
        assert_eq!(d.sum_ns, 5000);
        assert_eq!(d.hist[bucket(5000)], 1);
        assert_eq!(d.hist.iter().sum::<u64>(), 1);
    }

    #[test]
    fn quantile_bounds_the_sample() {
        let s = stat(
            &[1000; 99]
                .iter()
                .copied()
                .chain([1_000_000])
                .collect::<Vec<_>>(),
        );
        let p50 = quantile_upper_ns(&s.hist, 0.5).unwrap();
        assert!(p50 >= 1000 && p50 / 2 < 1000);
        let p100 = quantile_upper_ns(&s.hist, 1.0).unwrap();
        assert!(p100 >= 1_000_000 && p100 / 2 < 1_000_000);
        assert_eq!(quantile_upper_ns(&[0; BUCKETS], 0.5), None);
    }
}
