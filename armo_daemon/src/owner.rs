//! A flux process's owner file (`/dev/shm/flux.owner.<pid>.<starttime>`), read for what a probe
//! attached after `flux_trace_channel_init` missed: the segment each channel handle is on, and
//! the key each signpost was opened with.

use std::{
    collections::HashMap,
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

const MAX_BYTES: u64 = 1 << 20;

#[derive(Default)]
pub struct Owner {
    segments: HashMap<u64, String>,
    keys: HashMap<String, String>,
}

impl Owner {
    /// The owner file of the live process `tgid`. The name carries the process start time, so a
    /// file an earlier process with the same pid left behind is not read. Anyone can create files
    /// in /dev/shm, so a file owned by another user is not read either, and neither is anything
    /// but a regular file: a FIFO or a link there would block or redirect the read.
    pub fn read(tgid: u32) -> Option<Self> {
        let proc_dir = Path::new("/proc").join(tgid.to_string());
        let stat = fs::read_to_string(proc_dir.join("stat")).ok()?;
        let path = format!("/dev/shm/flux.owner.{tgid}.{}", starttime(&stat)?);
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(path)
            .ok()?;
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.uid() != fs::metadata(&proc_dir).ok()?.uid() {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES).read_to_end(&mut bytes).ok()?;
        Some(Self::parse(&String::from_utf8_lossy(&bytes)))
    }

    pub fn parse(text: &str) -> Self {
        let mut o = Self::default();
        // A line without its newline may still be being written.
        let complete = text.rsplit_once('\n').map_or("", |(head, _)| head);
        for line in complete.split('\n') {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                [
                    "",
                    "ch",
                    handle,
                    _dev,
                    _ino,
                    _publisher,
                    _slots,
                    _size,
                    segment,
                    ..,
                ] => {
                    if let Ok(h) = u64::from_str_radix(handle, 16) {
                        o.segments.insert(h, (*segment).to_owned());
                    }
                }
                [signpost, _role, key, ..] if !signpost.is_empty() => {
                    o.keys.insert((*signpost).to_owned(), (*key).to_owned());
                }
                _ => {}
            }
        }
        o
    }

    pub fn segment(&self, handle: u64) -> Option<&str> {
        self.segments.get(&handle).map(String::as_str)
    }
}

/// The key a segment's channel was opened with, or its signpost when no owner file names it.
/// A segment is named `<signpost>.<pid>.<starttime>`.
pub fn topic(segment: &str, owner: Option<&Owner>) -> String {
    let signpost = segment.rsplitn(3, '.').nth(2).unwrap_or(segment);
    owner
        .and_then(|o| o.keys.get(signpost))
        .cloned()
        .unwrap_or_else(|| signpost.to_owned())
}

/// Field 22 of /proc/<pid>/stat. Field 2 is the command in parentheses and may hold spaces and
/// ')', so fields are counted from the last ')'.
fn starttime(stat: &str) -> Option<&str> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "/flux.v3.s0.img.00ab\tsub\t/img\t/node\n\
                        \tch\t7f10\t5\t9\t0\t8\t4096\t/flux.v3.s0.img.00ab.4321.99\n\
                        \tch\t7f10\t5\t11\t0\t8\t4096\t/flux.v3.s0.img.00ab.4400.120\n\
                        \tch\t7f30\t5\t13\t0\t8\t4096\t/flux.v3.s0.img.00ab.4400.120\n\
                        \tch\t7f30\t0\t0\t0\t4\t64\t\n\
                        \tch\t7f20\t5\t12\t1";

    #[test]
    fn latest_record_of_a_handle_wins() {
        let o = Owner::parse(FILE);
        assert_eq!(o.segment(0x7f10), Some("/flux.v3.s0.img.00ab.4400.120"));
    }

    #[test]
    fn heap_channel_on_a_reused_address_has_no_segment() {
        assert_eq!(Owner::parse(FILE).segment(0x7f30), Some(""));
    }

    #[test]
    fn unterminated_line_is_ignored() {
        assert_eq!(Owner::parse(FILE).segment(0x7f20), None);
    }

    #[test]
    fn topic_is_the_announced_key() {
        let o = Owner::parse(FILE);
        assert_eq!(topic("/flux.v3.s0.img.00ab.4400.120", Some(&o)), "/img");
        assert_eq!(
            topic("/flux.v3.s0.cam.00cd.1.2", Some(&o)),
            "/flux.v3.s0.cam.00cd"
        );
    }

    #[test]
    fn starttime_skips_a_command_with_spaces() {
        let stat = "4321 (a b) c) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 777 19";
        assert_eq!(starttime(stat), Some("777"));
    }
}
