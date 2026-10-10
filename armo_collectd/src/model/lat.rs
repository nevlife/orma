use armo_common::{BUCKETS, bucket};
use armo_proto::msg::{HIST_LOG2, Hist, Lat};

/// One latency field over a window: exact count, sum and max, and the log2 distribution
///.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatAcc {
    pub n: u64,
    pub sum: u64,
    pub max: u64,
    pub hist: [u64; BUCKETS],
}

impl Default for LatAcc {
    fn default() -> Self {
        LatAcc {
            n: 0,
            sum: 0,
            max: 0,
            hist: [0; BUCKETS],
        }
    }
}

impl LatAcc {
    pub fn add(&mut self, ns: u64) {
        self.n += 1;
        self.sum += ns;
        self.max = self.max.max(ns);
        self.hist[bucket(ns)] += 1;
    }

    pub fn merge(&mut self, count: u64, sum: u64, max: u64, hist: &[u64]) {
        self.n += count;
        self.sum += sum;
        self.max = self.max.max(max);
        for (mine, theirs) in self.hist.iter_mut().zip(hist) {
            *mine += theirs;
        }
    }

    /// Interpolated inside its bucket and never past the exact max.
    pub fn quantile(&self, q: f64) -> u64 {
        let rank = ((q * self.n as f64).ceil() as u64).clamp(1, self.n);
        let mut below = 0;
        for (i, &c) in self.hist.iter().enumerate() {
            if below + c >= rank {
                let lo = if i == 0 { 0 } else { 1u64 << i };
                let hi = 1u64 << (i + 1);
                let at = lo + ((hi - lo) as f64 * (rank - below) as f64 / c as f64) as u64;
                return at.min(self.max);
            }
            below += c;
        }
        self.max
    }

    /// `None` when nothing was measured.
    pub fn lat(&self) -> Option<Lat> {
        (self.n > 0).then(|| Lat {
            n: self.n,
            mean_ns: self.sum / self.n,
            p50_ns: self.quantile(0.5),
            p99_ns: self.quantile(0.99),
            p999_ns: self.quantile(0.999),
            max_ns: self.max,
        })
    }

    pub fn hist(&self) -> Hist {
        Hist {
            scheme: HIST_LOG2.into(),
            counts: self.hist.to_vec(),
        }
    }
}
