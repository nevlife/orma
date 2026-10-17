use armo_common::BUCKETS;

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

    fn hist(samples: &[u64]) -> [u64; BUCKETS] {
        let mut h = [0; BUCKETS];
        for &ns in samples {
            h[bucket(ns)] += 1;
        }
        h
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
    fn quantile_bounds_the_sample() {
        let h = hist(
            &[1000; 99]
                .iter()
                .copied()
                .chain([1_000_000])
                .collect::<Vec<_>>(),
        );
        let p50 = quantile_upper_ns(&h, 0.5).unwrap();
        assert!(p50 >= 1000 && p50 / 2 < 1000);
        let p100 = quantile_upper_ns(&h, 1.0).unwrap();
        assert!(p100 >= 1_000_000 && p100 / 2 < 1_000_000);
        assert_eq!(quantile_upper_ns(&[0; BUCKETS], 0.5), None);
    }
}
