//! `header.stamp` read without the message type, used only when it ages
//! plausibly against the clock it claims (roscope `age_of_stamp_auto`).

const NS: u64 = 1_000_000_000;
const WALL_SEC: std::ops::Range<u64> = NS..4 * NS;
const FUTURE_NS: u64 = NS;
pub const AGE_MAX_NS: u64 = 600 * NS;

/// Nanoseconds of a stamp word (`sec` low, `nanosec` high); `None` when it is not a time.
pub fn decode(stamp: u64) -> Option<u64> {
    let sec = stamp & 0xffff_ffff;
    let nanosec = stamp >> 32;
    (sec != 0 && sec < 4 * NS && nanosec < NS).then_some(sec * NS + nanosec)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Clock {
    /// `CLOCK_REALTIME - CLOCK_MONOTONIC`, measured every window.
    pub rt_offset: Option<i64>,
    /// The last `/clock` value.
    pub sim_ns: Option<u64>,
}

impl Clock {
    pub fn real(&self, t_mono: u64) -> Option<u64> {
        self.rt_offset
            .and_then(|off| t_mono.checked_add_signed(off))
    }

    /// The age at `t_mono` of a decoded stamp, against wall time when its seconds are an epoch
    /// and against `/clock` otherwise.
    pub fn age(&self, stamp_ns: u64, t_mono: u64) -> Option<u64> {
        let now = if WALL_SEC.contains(&(stamp_ns / NS)) {
            self.real(t_mono)?
        } else {
            self.sim_ns?
        };
        if stamp_ns > now + FUTURE_NS {
            return None;
        }
        let age = now.saturating_sub(stamp_ns);
        (age <= AGE_MAX_NS).then_some(age)
    }

    pub fn gate(&self, stamp: u64, t_mono: u64) -> Option<(u64, u64)> {
        let ns = decode(stamp)?;
        Some((ns, self.age(ns, t_mono)?))
    }
}
