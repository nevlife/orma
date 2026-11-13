//! flux callback names. The probe records the segment a callback delivered from,
//! `<signpost>.e<epoch>`; the signpost holds the exact key once an endpoint has announced it.

use orma_common::NAME_LEN;

use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

// flux layouts v7 to v9 (flux_core/src/layout.rs): a `SignpostPage` opens with a `Signpost`, whose
// `init_state`, `magic` and `version` are u32 at bytes 0, 4 and 8, and holds its `KeyRecord` at
// byte 64: `state: u32`, `len: u32` and up to 192 key bytes. The page's own version decides, so a
// signpost of another layout is named by its signpost rather than misread.
const NAME_PREFIX: &str = "/flux.v";
const INIT_READY: u32 = 2;
const SIGNPOST_MAGIC: u32 = 0x464C_5350;
/// The flux layouts whose signpost page is this one: v8 and v9 changed only the segment.
const LAYOUTS: std::ops::RangeInclusive<u32> = 7..=9;
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
    let whole = (segment.len() >= NAME_LEN - 1)
        .then(|| complete("/dev/shm", segment))
        .flatten();
    let s = whole.as_deref().unwrap_or_else(|| signpost(segment));
    read_key(s, tgid).unwrap_or_else(|| s.to_owned())
}

/// A segment name the probe's `NAME_LEN` buffer cut short, as the one signpost in `dir` it is the
/// start of. A flux key may be 185 bytes, so its names can be longer than the buffer.
fn complete(dir: &str, cut: &str) -> Option<String> {
    let start = cut.strip_prefix('/')?;
    let mut found = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with(start) && signpost(n) == n);
    let one = found.next()?;
    found.next().is_none().then(|| format!("/{one}"))
}

/// Anyone can create files in /dev/shm, so only a regular file owned by the traced process's
/// user is read, opened without following links or blocking.
fn read_key(signpost: &str, tgid: u32) -> Option<String> {
    if !signpost.starts_with(NAME_PREFIX) || signpost[1..].contains('/') {
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
    if word(0)? != INIT_READY
        || word(4)? != SIGNPOST_MAGIC
        || !LAYOUTS.contains(&word(8)?)
        || word(KEY_RECORD)? != KEY_READY
    {
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
            signpost("/flux.v6..cam.left.00000000000000ab.e12"),
            "/flux.v6..cam.left.00000000000000ab"
        );
        assert_eq!(signpost("/flux.v6..raw.00ab.e"), "/flux.v6..raw.00ab.e");
        assert_eq!(signpost(""), "");
    }

    fn page(state: u32, key: &[u8]) -> Vec<u8> {
        page_of(9, state, key)
    }

    fn page_of(version: u32, state: u32, key: &[u8]) -> Vec<u8> {
        let mut p = vec![0u8; KEY_RECORD + 8 + KEY_BYTES];
        for (at, word) in [(0, INIT_READY), (4, SIGNPOST_MAGIC), (8, version)] {
            p[at..at + 4].copy_from_slice(&word.to_ne_bytes());
        }
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
    fn a_cut_segment_name_is_completed_from_its_one_signpost() {
        let dir = std::env::temp_dir().join(format!("orma_complete_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let sp = format!("flux.v6..{}.00000000000000ab", "k".repeat(150));
        for n in [
            sp.clone(),
            format!("{sp}.e1"),
            format!("flux.v6..k.{}", "0".repeat(16)),
        ] {
            fs::write(dir.join(n), b"").unwrap();
        }
        let dir = dir.to_str().unwrap();
        let cut = &format!("/{sp}.e1")[..NAME_LEN - 1];
        assert_eq!(complete(dir, cut), Some(format!("/{sp}")));
        // Two signposts start with this one: no guess.
        assert_eq!(complete(dir, "/flux.v6..k"), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_segment_of_this_layout_is_named_by_its_key() {
        let sp = format!(
            "/flux.v6..orma_unit_{}.00000000000000ab",
            std::process::id()
        );
        let path = format!("/dev/shm{sp}");
        fs::write(&path, page(KEY_READY, b"/orma_unit/cam")).unwrap();
        let named = topic(&format!("{sp}.e1"), std::process::id());
        fs::remove_file(&path).unwrap();
        assert_eq!(named, "/orma_unit/cam");
    }

    /// flux layouts 7 to 9 changed the segment, not the signpost page.
    #[test]
    fn each_layout_with_this_signpost_page_is_read_and_no_other() {
        for v in [7, 8, 9] {
            let key = parse_key(&page_of(v, KEY_READY, b"/cam/left"));
            assert_eq!(key.as_deref(), Some("/cam/left"), "layout {v}");
        }
        for v in [6, 10] {
            assert_eq!(
                parse_key(&page_of(v, KEY_READY, b"/cam/left")),
                None,
                "layout {v}"
            );
        }
    }

    #[test]
    fn a_key_being_written_or_cut_short_is_not() {
        assert_eq!(parse_key(&page(1, b"/cam")), None);
        assert_eq!(parse_key(&page(KEY_READY, b"/cam")[..KEY_RECORD + 6]), None);
    }
}
