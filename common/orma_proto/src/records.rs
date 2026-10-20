//! Records (design 13.4): checked reading of positioned records, as an event batch and an
//! `.ormaev` file carry them, and writing one.

use std::fmt;

use orma_common::catalog::{self, EventInfo};
use orma_common::events::{
    ORMA_RECORD_ALIGN, ORMA_RECORD_TRUNCATED, ORMA_STRING_MAX, orma_position, orma_record,
};
use orma_common::{Plain, bytes_of, read};

const POSITION: usize = size_of::<orma_position>();
const HEADER: usize = size_of::<orma_record>();

#[derive(Clone, Copy, Debug)]
pub struct Record<'a> {
    pub pid: u32,
    pub tid: u32,
    pub header: orma_record,
    pub info: &'static EventInfo,
    /// The bytes after the header, up to `len`: payload, strings, padding.
    pub body: &'a [u8],
}

impl<'a> Record<'a> {
    /// The payload as `T`, which must be the event's payload type.
    pub fn payload<T: Plain>(&self) -> T {
        assert_eq!(
            size_of::<T>(),
            self.info.size,
            "payload type of {}",
            self.info.name
        );
        read(self.body).expect("length checked when read")
    }

    /// The event's strings, without their NULs.
    pub fn strings(&self) -> impl Iterator<Item = &'a [u8]> {
        self.body[self.info.size..]
            .split(|b| *b == 0)
            .take(self.info.strings as usize)
    }

    pub fn truncated(&self) -> bool {
        self.header.flags & ORMA_RECORD_TRUNCATED != 0
    }
}

/// Where and why a run of records stopped making sense (design 13.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordError {
    pub offset: usize,
    pub reason: String,
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "record at byte {}: {}", self.offset, self.reason)
    }
}

impl std::error::Error for RecordError {}

/// One record without its position, checked against the catalog.
pub fn check(
    bytes: &[u8],
    offset: usize,
) -> Result<(orma_record, &'static EventInfo), RecordError> {
    let fail = |reason: String| Err(RecordError { offset, reason });
    let Some(header) = read::<orma_record>(bytes) else {
        return fail(format!(
            "{} bytes left, a header takes {HEADER}",
            bytes.len()
        ));
    };
    let len = header.len as usize;
    let Some(info) = catalog::info(header.event) else {
        return fail(format!("unknown event {}", header.event));
    };
    let (min, max) = info.len_range();
    if !len.is_multiple_of(ORMA_RECORD_ALIGN) || len < min || len > max || len > bytes.len() {
        return fail(format!(
            "{} len {len} outside {min}..={max} or past the end",
            info.name
        ));
    }
    let tail = &bytes[HEADER + info.size..len];
    let mut nuls = tail
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == 0)
        .map(|(i, _)| i);
    let mut start = 0;
    for _ in 0..info.strings {
        match nuls.next() {
            Some(end) if end - start < ORMA_STRING_MAX => start = end + 1,
            _ => {
                return fail(format!(
                    "{} string {start} unterminated or too long",
                    info.name
                ));
            }
        }
    }
    if tail.len() - start >= ORMA_RECORD_ALIGN {
        return fail(format!(
            "{} has {} bytes past its strings",
            info.name,
            tail.len() - start
        ));
    }
    Ok((header, info))
}

/// Positioned records one after another, as in an event batch; stops at the first bad one.
pub fn positioned(batch: &[u8]) -> impl Iterator<Item = Result<Record<'_>, RecordError>> {
    let mut at = 0;
    let mut failed = false;
    std::iter::from_fn(move || {
        if failed || at == batch.len() {
            return None;
        }
        let checked = read::<orma_position>(&batch[at..])
            .ok_or(RecordError {
                offset: at,
                reason: "truncated position".into(),
            })
            .and_then(|pos| {
                let (header, info) = check(&batch[at + POSITION..], at + POSITION)?;
                let start = at + POSITION + HEADER;
                let body = &batch[start..at + POSITION + header.len as usize];
                Ok((
                    Record {
                        pid: pos.pid,
                        tid: pos.tid,
                        header,
                        info,
                        body,
                    },
                    header.len,
                ))
            });
        match checked {
            Ok((record, len)) => {
                at += POSITION + len as usize;
                Some(Ok(record))
            }
            Err(e) => {
                failed = true;
                Some(Err(e))
            }
        }
    })
}

/// Appends one record (without position) and returns its length. Strings longer than
/// `ORMA_STRING_MAX` with their NUL are cut and flagged.
pub fn encode<T: Plain>(
    out: &mut Vec<u8>,
    event: u16,
    t_mono_ns: u64,
    payload: &T,
    strings: &[&[u8]],
) -> usize {
    let info = catalog::info(event).expect("event in the catalog");
    assert_eq!(size_of::<T>(), info.size, "payload type of {}", info.name);
    assert_eq!(
        strings.len(),
        info.strings as usize,
        "strings of {}",
        info.name
    );
    let start = out.len();
    out.extend_from_slice(&[0; HEADER]);
    out.extend_from_slice(bytes_of(payload));
    let mut flags = 0;
    for s in strings {
        let s = &s[..s.iter().position(|b| *b == 0).unwrap_or(s.len())];
        let keep = s.len().min(ORMA_STRING_MAX - 1);
        if keep < s.len() {
            flags |= ORMA_RECORD_TRUNCATED;
        }
        out.extend_from_slice(&s[..keep]);
        out.push(0);
    }
    out.resize(
        start + (out.len() - start).next_multiple_of(ORMA_RECORD_ALIGN),
        0,
    );
    let len = out.len() - start;
    let header = orma_record {
        event,
        len: len as u16,
        flags,
        t_mono_ns,
    };
    out[start..start + HEADER].copy_from_slice(bytes_of(&header));
    len
}

pub fn encode_positioned<T: Plain>(
    out: &mut Vec<u8>,
    pid: u32,
    tid: u32,
    event: u16,
    t_mono_ns: u64,
    payload: &T,
    strings: &[&[u8]],
) {
    out.extend_from_slice(bytes_of(&orma_position { pid, tid }));
    encode(out, event, t_mono_ns, payload, strings);
}
