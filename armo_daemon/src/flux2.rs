//! flux2 callback names. The probe records the segment a callback delivered from,
//! `<signpost>.e<epoch>`; the signpost holds the exact key once an endpoint has announced it.

use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

// flux2 layout v1 (flux2_core/src/layout.rs): `SignpostPage::key` is a `KeyRecord` at byte 64,
// holding `state: u32`, `len: u32` and up to 192 key bytes. Only a v1 name is read this way.
const LAYOUT_PREFIX: &str = "/flux2.v1.";
const KEY_RECORD: usize = 64;
const KEY_BYTES: usize = 192;
const KEY_READY: u32 = 2;

/// The signpost a segment belongs to.
pub fn signpost(segment: &str) -> &str {
    match segment.rsplit_once(".e") {
        Some((s, epoch)) if !epoch.is_empty() && epoch.bytes().all(|b| b.is_ascii_digit()) => s,
        _ => segment,
    }
}

/// The exact key when the signpost of `tgid`'s user holds one, else the signpost.
pub fn topic(segment: &str, tgid: u32) -> String {
    let s = signpost(segment);
    read_key(s, tgid).unwrap_or_else(|| s.to_owned())
}

/// Anyone can create files in /dev/shm, so only a regular file owned by the traced process's
/// user is read, opened without following links or blocking.
fn read_key(signpost: &str, tgid: u32) -> Option<String> {
    if !signpost.starts_with(LAYOUT_PREFIX) || signpost[1..].contains('/') {
        return None;
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(format!("/dev/shm{signpost}"))
        .ok()?;
    let meta = file.metadata().ok()?;
    let owner = fs::metadata(Path::new("/proc").join(tgid.to_string())).ok()?;
    if !meta.is_file() || meta.uid() != owner.uid() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take((KEY_RECORD + 8 + KEY_BYTES) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    parse_key(&bytes)
}

pub fn parse_key(page: &[u8]) -> Option<String> {
    let word = |at: usize| Some(u32::from_ne_bytes(page.get(at..at + 4)?.try_into().ok()?));
    if word(KEY_RECORD)? != KEY_READY {
        return None;
    }
    let len = usize::try_from(word(KEY_RECORD + 4)?).ok()?.min(KEY_BYTES);
    let start = KEY_RECORD + 8;
    String::from_utf8(page.get(start..start + len)?.to_vec()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_segment_names_its_signpost() {
        assert_eq!(
            signpost("/flux2.v1.s0.cam.left.00000000000000ab.e12"),
            "/flux2.v1.s0.cam.left.00000000000000ab"
        );
        assert_eq!(
            signpost("/flux2.v1.s0.raw.00ab.e"),
            "/flux2.v1.s0.raw.00ab.e"
        );
        assert_eq!(signpost(""), "");
    }

    fn page(state: u32, key: &[u8]) -> Vec<u8> {
        let mut p = vec![0u8; KEY_RECORD + 8 + KEY_BYTES];
        p[KEY_RECORD..KEY_RECORD + 4].copy_from_slice(&state.to_ne_bytes());
        p[KEY_RECORD + 4..KEY_RECORD + 8].copy_from_slice(&(key.len() as u32).to_ne_bytes());
        p[KEY_RECORD + 8..KEY_RECORD + 8 + key.len()].copy_from_slice(key);
        p
    }

    #[test]
    fn a_ready_key_is_read() {
        assert_eq!(
            parse_key(&page(KEY_READY, b"/cam/left")).as_deref(),
            Some("/cam/left")
        );
    }

    #[test]
    fn a_key_being_written_or_cut_short_is_not() {
        assert_eq!(parse_key(&page(1, b"/cam")), None);
        assert_eq!(parse_key(&page(KEY_READY, b"/cam")[..KEY_RECORD + 6]), None);
    }
}
