use crate::msg::{Aggregate, Method, Window};
use crate::records::Record;

/// orma-collectd's sources (`WP3a`) feed its model (`WP3b`); the window timer takes snapshots out.
pub trait Ingest {
    fn record(&mut self, method: Method, record: &Record<'_>);
    fn aggregates(&mut self, method: Method, rows: &[Aggregate]);
    fn window(&mut self, t_mono_ns: u64, t_real_ns: u64, window_ns: u64, hists: bool) -> Window;
}
