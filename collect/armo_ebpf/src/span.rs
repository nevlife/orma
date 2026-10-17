//! `armo_span_begin` and `armo_span_end` of libarmo_span.so (design 20.5): empty functions whose
//! argument, a name of static lifetime, keys the span by its address.

use armo_common::events::*;
use aya_ebpf::{macros::uprobe, programs::ProbeContext};

use crate::{emit::*, maps::*};

// void armo_span_begin(const char * name)
#[uprobe]
pub fn span_begin(ctx: ProbeContext) -> u32 {
    let ns = now();
    if streamed(tgid()) {
        let name = ctx.arg::<u64>(0).unwrap_or(0);
        emit(ARMO_EV_SPAN_BEGIN, ns, &armo_ev_span { key: name }, [name]);
    }
    0
}

// void armo_span_end(const char * name)
#[uprobe]
pub fn span_end(ctx: ProbeContext) -> u32 {
    let ns = now();
    if streamed(tgid()) {
        let key = ctx.arg::<u64>(0).unwrap_or(0);
        emit(ARMO_EV_SPAN_END, ns, &armo_ev_span { key }, []);
    }
    0
}
