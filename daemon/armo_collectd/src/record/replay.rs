//! Replay of a recording's `windows.jsonl` to the client that opened it: one
//! `window` per frame with `replay: true`, at the recorded pace times `rate`.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use armo_proto::msg::{ServerMessage, WindowLine};

use super::WINDOWS;

/// Delivers a message to the client; false once it is gone.
pub type Send = Box<dyn Fn(&ServerMessage) -> bool + std::marker::Send>;

struct Line {
    offset: u64,
    len: usize,
    t_mono_ns: u64,
}

struct Play {
    frame: usize,
    rate: f64,
    playing: bool,
    show: bool,
    closed: bool,
    /// Bumped by every request, so a paced wait knows it was interrupted.
    generation: u64,
}

pub struct Replay {
    play: Arc<(Mutex<Play>, Condvar)>,
    frames: usize,
    thread: Option<JoinHandle<()>>,
}

/// `t_mono_ns` leads every line armo-collectd writes; reading only it keeps the index cheap.
fn leading_t_mono(line: &[u8]) -> Option<u64> {
    let rest = line.strip_prefix(b"{\"t_mono_ns\":")?;
    let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    std::str::from_utf8(&rest[..digits]).ok()?.parse().ok()
}

fn index(path: &Path) -> io::Result<Vec<Line>> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut lines = Vec::new();
    let mut offset = 0;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 || buf.last() != Some(&b'\n') {
            break;
        }
        let t_mono_ns = leading_t_mono(&buf).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{}: line {} has no leading t_mono_ns",
                    path.display(),
                    lines.len() + 1
                ),
            )
        })?;
        lines.push(Line {
            offset,
            len: n - 1,
            t_mono_ns,
        });
        offset += n as u64;
    }
    Ok(lines)
}

fn frame(file: &mut File, line: &Line) -> io::Result<ServerMessage> {
    let mut buf = vec![0u8; line.len];
    file.seek(SeekFrom::Start(line.offset))?;
    file.read_exact(&mut buf)?;
    let mut line: WindowLine =
        serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    line.snap.replay = true;
    line.snap.hists = None;
    Ok(ServerMessage::Window(Box::new(line.snap)))
}

impl Replay {
    /// Stopped at frame 0 until `play` or `seek`.
    pub fn open(dir: &Path, send: Send) -> io::Result<Replay> {
        let path = dir.join(WINDOWS);
        let lines = index(&path)?;
        if lines.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: no windows", path.display()),
            ));
        }
        let mut file = File::open(&path)?;
        let frames = lines.len();
        let play = Arc::new((
            Mutex::new(Play {
                frame: 0,
                rate: 1.0,
                playing: false,
                show: false,
                closed: false,
                generation: 0,
            }),
            Condvar::new(),
        ));
        let shared = Arc::clone(&play);
        let thread = thread::spawn(move || {
            let (lock, wake) = &*shared;
            let mut p = lock.lock().unwrap();
            loop {
                while !p.closed && !p.show && !p.playing {
                    p = wake.wait(p).unwrap();
                }
                if p.closed {
                    return;
                }
                p.show = false;
                let at = p.frame;
                let picked = p.generation;
                drop(p);
                let read = frame(&mut file, &lines[at]);
                p = lock.lock().unwrap();
                // A request answered meanwhile overrides this frame; sending under the lock
                // keeps every frame ahead of the reply to a later request.
                if p.generation != picked {
                    continue;
                }
                let sent = read.map(|m| send(&m));
                if !matches!(sent, Ok(true)) {
                    p.closed = true;
                    return;
                }
                if !p.playing || p.show {
                    continue;
                }
                if at + 1 == frames {
                    p.playing = false;
                    continue;
                }
                let gap = lines[at + 1].t_mono_ns.saturating_sub(lines[at].t_mono_ns);
                // A rate small enough to overflow the delay waits for the next request.
                let delay =
                    Duration::try_from_secs_f64(gap as f64 / 1e9 / p.rate).unwrap_or(Duration::MAX);
                let generation = p.generation;
                p = wake
                    .wait_timeout_while(p, delay, |p| p.generation == generation)
                    .unwrap()
                    .0;
                if p.generation == generation {
                    p.frame = at + 1;
                    p.show = true;
                }
            }
        });
        Ok(Replay {
            play,
            frames,
            thread: Some(thread),
        })
    }

    fn change(&self, f: impl FnOnce(&mut Play)) {
        let (lock, wake) = &*self.play;
        let mut p = lock.lock().unwrap();
        f(&mut p);
        p.generation += 1;
        wake.notify_all();
    }

    pub fn seek(&self, frame: u64) -> Result<(), String> {
        let frame = usize::try_from(frame)
            .ok()
            .filter(|f| *f < self.frames)
            .ok_or_else(|| format!("frame {frame} outside 0..{}", self.frames))?;
        self.change(|p| {
            p.frame = frame;
            p.show = true;
        });
        Ok(())
    }

    pub fn play(&self, rate: f64) -> Result<(), String> {
        if !(rate.is_finite() && rate > 0.0) {
            return Err(format!("rate {rate} is not a positive number"));
        }
        self.change(|p| {
            p.rate = rate;
            p.playing = true;
        });
        Ok(())
    }

    pub fn pause(&self) {
        self.change(|p| p.playing = false);
    }
}

impl Drop for Replay {
    fn drop(&mut self) {
        self.change(|p| p.closed = true);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
