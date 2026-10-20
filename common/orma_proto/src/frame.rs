//! Frames on both sockets (design 12.2): `u32 length` (little-endian, kind byte and payload),
//! `u8 kind`, payload.

use std::io::{self, Read, Write};

use serde::Serialize;

/// Above this `length` the receiver drops the connection.
pub const MAX_LENGTH: u32 = 16 << 20;
pub const HEADER_LEN: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// One UTF-8 JSON object.
    Json = 1,
    /// Positioned records (13.4), ormad to orma-collectd only.
    Events = 2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: Kind,
    pub payload: Vec<u8>,
}

fn invalid(what: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

fn kind_of(byte: u8) -> io::Result<Kind> {
    match byte {
        1 => Ok(Kind::Json),
        2 => Ok(Kind::Events),
        k => Err(invalid(format!("unknown frame kind {k}"))),
    }
}

fn check_length(length: u32) -> io::Result<()> {
    if length == 0 || length > MAX_LENGTH {
        return Err(invalid(format!(
            "frame length {length} outside 1..={MAX_LENGTH}"
        )));
    }
    Ok(())
}

pub fn encode(kind: Kind, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&(payload.len() as u32 + 1).to_le_bytes());
    out.push(kind as u8);
    out.extend_from_slice(payload);
    out
}

pub fn encode_json<T: Serialize>(message: &T) -> Vec<u8> {
    encode(
        Kind::Json,
        &serde_json::to_vec(message).expect("message types serialize"),
    )
}

/// The first whole frame in `buf` and the bytes it took, or `None` until more arrive.
pub fn decode(buf: &[u8]) -> io::Result<Option<(Frame, usize)>> {
    if buf.len() < HEADER_LEN {
        return Ok(None);
    }
    let length = u32::from_le_bytes(buf[..4].try_into().unwrap());
    check_length(length)?;
    let kind = kind_of(buf[4])?;
    let end = 4 + length as usize;
    if buf.len() < end {
        return Ok(None);
    }
    let payload = buf[HEADER_LEN..end].to_vec();
    Ok(Some((Frame { kind, payload }, end)))
}

pub fn write<W: Write>(w: &mut W, kind: Kind, payload: &[u8]) -> io::Result<()> {
    w.write_all(&encode(kind, payload))
}

pub fn write_json<W: Write, T: Serialize>(w: &mut W, message: &T) -> io::Result<()> {
    w.write_all(&encode_json(message))
}

/// `Ok(None)` at a clean end of stream, before any byte of a frame.
pub fn read<R: Read>(r: &mut R) -> io::Result<Option<Frame>> {
    let mut header = [0u8; HEADER_LEN];
    let mut got = 0;
    while got < HEADER_LEN {
        match r.read(&mut header[got..])? {
            0 if got == 0 => return Ok(None),
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => got += n,
        }
    }
    let length = u32::from_le_bytes(header[..4].try_into().unwrap());
    check_length(length)?;
    let kind = kind_of(header[4])?;
    let mut payload = vec![0u8; length as usize - 1];
    r.read_exact(&mut payload)?;
    Ok(Some(Frame { kind, payload }))
}
