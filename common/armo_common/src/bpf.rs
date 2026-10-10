//! The eBPF maps of armo_ebpf and armod (design 4.3, 13.2, 15.2); not in the C headers.

use crate::BUCKETS;

/// A row of the kernel's aggregate table, as `armo_table_entry` (13.2); one writer, `tid`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AggKey {
    pub tgid: u32,
    pub tid: u32,
    pub kind: u16,
    pub src: u16,
    pub entry: u32,
    pub key: u64,
}

/// `count`, `sum` and `hist` are cumulative. `max` is the largest sample of window `max_gen`
/// (`CONFIG_GEN` when it was written): armod bumps the generation and reads the closed one.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AggVal {
    pub count: u64,
    pub sum: u64,
    pub max: u64,
    pub max_gen: u64,
    pub hist: [u64; BUCKETS],
}

/// A running callback on a thread's stack: `src` and `key` as in `AggKey`, `ns` its start.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub key: u64,
    pub ns: u64,
    pub src: u32,
    pub entry: u32,
}

pub const STACK_DEPTH: usize = 16;

/// Per thread, written by its own uprobes: the callback stack and the last wait and wake (2.5).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ThreadState {
    pub depth: u32,
    pub wake_depth: u32,
    pub wait_src: u32,
    pub wake_src: u32,
    pub wait_ns: u64,
    pub wait_key: u64,
    pub wake_ns: u64,
    pub wake_key: u64,
    /// What `Executor::execute_any_executable` is about to run at depth `owner_depth`, for the
    /// next callback start (design 2.1.4); `owner_kind` 0 when nothing.
    pub owner_kind: u32,
    pub owner_depth: u32,
    pub owner_object: u64,
    pub owner_handle: u64,
    pub owner_node: u64,
    pub frames: [Frame; STACK_DEPTH],
}

/// A thread that left the CPU inside a callback (design 15.2).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OffStart {
    pub ns: u64,
    pub key: u64,
    pub src: u32,
    pub entry: u32,
    pub tgid: u32,
    pub pad: u32,
}

/// What a process has shown once: a handle whose code words or topic were read, or an rclcpp
/// object a tracepoint named, so a C++ symbol probe can check its argument against it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SeenKey {
    pub tgid: u32,
    pub what: u32,
    pub handle: u64,
}

pub const SEEN_CODE: u32 = 1;
pub const SEEN_SUB_TOPIC: u32 = 2;
pub const SEEN_PUB_TOPIC: u32 = 3;
pub const KNOWN_RCLCPP_SUB: u32 = 4;
pub const KNOWN_RCLCPP_PUB: u32 = 5;
/// A subscription a group was linked to: the numerator of the coverage selfcheck (6.7).
pub const LINKED_RCLCPP_SUB: u32 = 6;
pub const KNOWN_EXECUTOR: u32 = 7;
pub const KNOWN_COLLECTOR: u32 = 8;
pub const SEEN_CALLBACK_OWNER: u32 = 9;

/// `CONFIG` slots, written by armod.
pub const CONFIG_STREAM_ALL: u32 = 0;
pub const CONFIG_GEN: u32 = 1;
/// Byte offset of `serdata_rmw::m_size` in Cyclone (design 2.1.2).
pub const CONFIG_SERDATA_SIZE_OFFSET: u32 = 2;
pub const CONFIG_SLOTS: u32 = 4;

/// `LOST` slots: what the programs could not record. 0 and 1 are the flux v1 slots of lib.rs.
pub const LOST_RING: u32 = 2;
pub const LOST_AGG_FULL: u32 = 3;
pub const LOST_DEPTH: u32 = 4;
pub const LOST_USER_WORD: u32 = 5;
pub const LOST_THREAD_FULL: u32 = 6;
/// A C++ symbol probe whose argument no tracepoint had named; not a lost measurement.
pub const LOST_UNVALIDATED: u32 = 7;
pub const LOST_UNPAIRED: u32 = 8;
pub const LOST_SLOTS_ALL: u32 = 9;

/// `SAMPLE` slots: diagnostics the selfcheck reads (design 6.7).
pub const SAMPLE_SERDATA_72: u32 = 0;
pub const SAMPLE_SERDATA_56: u32 = 1;
pub const SAMPLE_SERDATA_N: u32 = 2;
pub const SAMPLE_SLOTS: u32 = 4;
/// Above this a serialized size is a pointer or a timestamp read at the wrong offset.
pub const SERDATA_PLAUSIBLE_MAX: u64 = 1 << 30;

/// A ring buffer entry: the record's position and header, the payload, then `N` string slots of
/// `STRING_SLOT` bytes. armod packs the strings and sets `len` before it passes the record on.
pub const STRING_SLOT: usize = 256;

#[cfg(feature = "user")]
#[allow(unsafe_code)]
mod pod {
    use super::*;
    // SAFETY: repr(C) integers shared with the eBPF programs; any bit pattern is a value.
    unsafe impl aya::Pod for AggKey {}
    // SAFETY: repr(C) integers shared with the eBPF programs; any bit pattern is a value.
    unsafe impl aya::Pod for AggVal {}
    // SAFETY: repr(C) integers shared with the eBPF programs; any bit pattern is a value.
    unsafe impl aya::Pod for ThreadState {}
    // SAFETY: repr(C) integers shared with the eBPF programs; any bit pattern is a value.
    unsafe impl aya::Pod for OffStart {}
    // SAFETY: repr(C) integers shared with the eBPF programs; any bit pattern is a value.
    unsafe impl aya::Pod for SeenKey {}
}
