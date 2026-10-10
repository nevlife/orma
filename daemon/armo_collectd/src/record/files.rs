//! The event files of a recording: `events/NNN.armoev`, or `.armoev.zst`, each the
//! file header and positioned records, split by size and age.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use armo_common::bytes_of;
use armo_common::events::{ARMO_EVFILE_MAGIC, armo_evfile_header};
use armo_common::shm::ARMO_SHM_VERSION;
use armo_proto::msg::Compression;

/// Counts what reaches the file.
struct Counting {
    file: File,
    bytes: u64,
}

impl Write for Counting {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.file.write(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

enum Inner {
    Plain(Counting),
    Zstd(zstd::Encoder<'static, Counting>),
}

impl Write for Inner {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Inner::Plain(w) => w.write(buf),
            Inner::Zstd(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Inner::Plain(w) => w.flush(),
            Inner::Zstd(w) => w.flush(),
        }
    }
}

pub struct Split {
    pub bytes: u64,
    pub ns: Option<u64>,
}

pub struct EventFiles {
    dir: PathBuf,
    compression: Compression,
    cache: usize,
    split: Split,
    out: BufWriter<Inner>,
    opened_ns: u64,
    closed_bytes: u64,
    /// Relative to the recording directory.
    pub files: Vec<String>,
}

fn file_name(n: usize, compression: Compression) -> String {
    match compression {
        Compression::None => format!("{n:03}.armoev"),
        Compression::Zstd => format!("{n:03}.armoev.zst"),
    }
}

impl EventFiles {
    pub fn create(
        record_dir: &Path,
        compression: Compression,
        cache: usize,
        split: Split,
        now_ns: u64,
    ) -> io::Result<EventFiles> {
        let dir = record_dir.join("events");
        std::fs::create_dir(&dir)?;
        let out = open(&dir, 0, compression, cache)?;
        Ok(EventFiles {
            dir,
            compression,
            cache,
            split,
            out,
            opened_ns: now_ns,
            closed_bytes: 0,
            files: vec![format!("events/{}", file_name(0, compression))],
        })
    }

    fn current_bytes(&self) -> u64 {
        match self.out.get_ref() {
            Inner::Plain(c) => c.bytes + self.out.buffer().len() as u64,
            Inner::Zstd(e) => e.get_ref().bytes,
        }
    }

    pub fn bytes(&self) -> u64 {
        self.closed_bytes + self.current_bytes()
    }

    pub fn append(&mut self, positioned: &[u8], now_ns: u64) -> io::Result<()> {
        let aged = self
            .split
            .ns
            .is_some_and(|ns| now_ns - self.opened_ns >= ns);
        if self.current_bytes() >= self.split.bytes || aged {
            let n = self.files.len();
            let next = open(&self.dir, n, self.compression, self.cache)?;
            let done = std::mem::replace(&mut self.out, next);
            self.closed_bytes += finish(done)?;
            self.files
                .push(format!("events/{}", file_name(n, self.compression)));
            self.opened_ns = now_ns;
        }
        self.out.write_all(positioned)
    }

    /// Flushes, ends the compressed stream and syncs; the byte total after.
    pub fn close(self) -> io::Result<u64> {
        Ok(self.closed_bytes + finish(self.out)?)
    }
}

fn open(
    dir: &Path,
    n: usize,
    compression: Compression,
    cache: usize,
) -> io::Result<BufWriter<Inner>> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(file_name(n, compression)))?;
    let counting = Counting { file, bytes: 0 };
    let inner = match compression {
        Compression::None => Inner::Plain(counting),
        Compression::Zstd => Inner::Zstd(zstd::Encoder::new(counting, 0)?),
    };
    let mut out = BufWriter::with_capacity(cache, inner);
    let header = armo_evfile_header {
        magic: ARMO_EVFILE_MAGIC,
        version: ARMO_SHM_VERSION,
        pad: 0,
    };
    out.write_all(bytes_of(&header))?;
    Ok(out)
}

fn finish(out: BufWriter<Inner>) -> io::Result<u64> {
    let counting = match out.into_inner().map_err(|e| e.into_error())? {
        Inner::Plain(c) => c,
        Inner::Zstd(e) => e.finish()?,
    };
    counting.file.sync_all()?;
    Ok(counting.bytes)
}
