//! orma's wire contracts and their test doubles (design 12, 13.4, 14.4).

pub mod fake;
pub mod frame;
pub mod ingest;
pub mod msg;
pub mod records;

pub use ingest::Ingest;
