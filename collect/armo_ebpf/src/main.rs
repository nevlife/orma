//! armo's eBPF programs (design 2, 4.2): uprobes, scheduler and futex tracepoints that only
//! count and copy arguments; armod does every division and merge.
#![no_std]
#![no_main]
// The BPF target has no compare-and-swap, so core's atomic types lack fetch_add; the intrinsic
// lowers to BPF's atomic add.
#![feature(core_intrinsics)]
#![allow(internal_features)]

mod cuda;
mod dds;
mod emit;
mod flux2;
mod futex;
mod maps;
mod rcl;
mod rclcpp;
mod ros;
mod sched;
mod span;

#[unsafe(link_section = "license")]
#[unsafe(no_mangle)]
static LICENSE: [u8; 13] = *b"Dual MIT/GPL\0";

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
