//! Map keys and values shared by the eBPF programs and armod. Both sides read these through the
//! same definitions, so a layout change reaches both at once.
#![no_std]

/// log2 buckets over nanoseconds: bucket `i` holds `[2^i, 2^(i+1))`, bucket 0 also holds 0.
/// 2^40 ns is about 18 minutes.
pub const BUCKETS: usize = 40;
pub const NAME_LEN: usize = 128;

pub const KIND_ROS: u32 = 0;
pub const KIND_FLUX: u32 = 1;
/// A `NAMES` key only: `callback` is a flux channel handle.
pub const KIND_FLUX_CHANNEL: u32 = 2;
/// Keyed like `KIND_FLUX`; its `NAMES` entry is the segment the callback first delivered from.
pub const KIND_FLUX2: u32 = 3;

/// One callback in one process. For `KIND_ROS`, `callback` is the address rclcpp passes to
/// `ros_trace_*` and `entry` is 0. For `KIND_FLUX`, `callback` is the executor and `entry` its
/// index there: unlike the channel handle, it survives a publisher restart.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CallbackKey {
    pub tgid: u32,
    pub kind: u32,
    pub callback: u64,
    pub entry: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CallbackStart {
    pub callback: u64,
    pub ns: u64,
}

/// Keyed by executor as well as thread: a callback can dispatch another executor inside it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FluxStartKey {
    pub tid: u64,
    pub executor: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FluxStart {
    pub ns: u64,
    pub entry: u32,
    pub pad: u32,
}

/// Cumulative since the entry was created. Updated with atomic adds only, so concurrent callbacks
/// on other CPUs do not lose counts.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CallbackStat {
    pub count: u64,
    pub sum_ns: u64,
    pub hist: [u64; BUCKETS],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CallbackName {
    pub bytes: [u8; NAME_LEN],
}

/// Indexes into the `LOST` array: events the programs could not record.
pub const LOST_STAT_FULL: u32 = 0;
pub const LOST_NAME_FULL: u32 = 1;
pub const LOST_SLOTS: u32 = 2;

pub const fn bucket(ns: u64) -> usize {
    let mut v = ns;
    let mut b = 0usize;
    if v >= 1 << 32 {
        v >>= 32;
        b += 32;
    }
    if v >= 1 << 16 {
        v >>= 16;
        b += 16;
    }
    if v >= 1 << 8 {
        v >>= 8;
        b += 8;
    }
    if v >= 1 << 4 {
        v >>= 4;
        b += 4;
    }
    if v >= 1 << 2 {
        v >>= 2;
        b += 2;
    }
    if v >= 1 << 1 {
        b += 1;
    }
    if b >= BUCKETS { BUCKETS - 1 } else { b }
}

#[cfg(feature = "user")]
mod pod {
    use super::*;
    unsafe impl aya::Pod for CallbackKey {}
    unsafe impl aya::Pod for CallbackStart {}
    unsafe impl aya::Pod for FluxStart {}
    unsafe impl aya::Pod for FluxStartKey {}
    unsafe impl aya::Pod for CallbackStat {}
    unsafe impl aya::Pod for CallbackName {}
}
