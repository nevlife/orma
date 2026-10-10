//! The guard: a child process, `armo-collectd --guard`, that cleans up after an
//! armo-collectd that died without doing so. armo-collectd writes what to clean, one JSON line per
//! change, to the guard's stdin. When armo-collectd ends, however it ends, the kernel closes that
//! pipe; at end of input the guard cleans the last list. A normal exit sends an empty list first.

use std::io::{self, BufRead, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::record::show;
use crate::sources::lttng::lttng;
use crate::sources::shm;
use crate::sys;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cleanup {
    pub lttng_home: PathBuf,
    /// LTTng sessions to destroy.
    pub sessions: Vec<String>,
    /// Shared memory files whose `armed` to lower.
    pub armed: Vec<PathBuf>,
    /// Recording directories to mark truncated.
    pub recordings: Vec<PathBuf>,
}

pub struct Guard {
    child: Child,
    stdin: ChildStdin,
    sent: Cleanup,
}

impl Guard {
    /// Starts `exe --guard` in its own process group, so a terminal's SIGINT to armo-collectd
    /// does not reach it.
    pub fn spawn(exe: &Path) -> io::Result<Guard> {
        let mut child = Command::new(exe)
            .arg("--guard")
            .stdin(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let stdin = child.stdin.take().expect("piped");
        Ok(Guard {
            child,
            stdin,
            sent: Cleanup::default(),
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn update(&mut self, list: &Cleanup) -> io::Result<()> {
        if *list == self.sent {
            return Ok(());
        }
        let mut line = serde_json::to_vec(list).expect("cleanup serializes");
        line.push(b'\n');
        self.stdin.write_all(&line)?;
        self.sent = list.clone();
        Ok(())
    }

    /// armo-collectd cleaned up itself: the guard ends with nothing to do.
    pub fn close(mut self) -> io::Result<()> {
        let lttng_home = self.sent.lttng_home.clone();
        self.update(&Cleanup {
            lttng_home,
            ..Cleanup::default()
        })?;
        drop(self.stdin);
        self.child.wait()?;
        Ok(())
    }
}

/// `armo-<pid>-<starttime>-<n>`, the name of armo-collectd's `n`th LTTng session.
pub fn session_name(n: u64) -> String {
    let pid = sys::pid();
    let starttime = sys::starttime(pid).expect("this process exists");
    format!("armo-{pid}-{starttime}-{n}")
}

pub fn destroy_session(home: &Path, name: &str) -> io::Result<()> {
    lttng(home, &["destroy", name]).map(|_| ())
}

/// Destroys the `armo-` sessions whose armo-collectd is not alive, for an armo-collectd that
/// died with its guard. Returns their names.
pub fn sweep_sessions(home: &Path) -> Vec<String> {
    let Ok(text) = lttng(home, &["list"]) else {
        return Vec::new();
    };
    let mut stale = Vec::new();
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        let Some(rest) = word.strip_prefix("armo-") else {
            continue;
        };
        let parts: Vec<u64> = rest.split('-').filter_map(|p| p.parse().ok()).collect();
        let [pid, starttime, _] = parts[..] else {
            continue;
        };
        let alive = sys::starttime(pid as u32) == Some(starttime);
        if !alive && !stale.iter().any(|s| s == word) {
            stale.push(word.to_string());
        }
    }
    stale.retain(|name| destroy_session(home, name).is_ok());
    stale
}

pub fn clean(list: &Cleanup) {
    for name in &list.sessions {
        if let Err(e) = destroy_session(&list.lttng_home, name) {
            eprintln!("armo-collectd --guard: {e}");
        }
    }
    for path in &list.armed {
        if let Err(e) = shm::disarm(path) {
            eprintln!("armo-collectd --guard: {}: {e}", path.display());
        }
    }
    for dir in &list.recordings {
        if let Err(e) = show::recover(dir) {
            eprintln!("armo-collectd --guard: {}: {e}", dir.display());
        }
    }
}

/// The guard's main: keeps the last list from `input` and cleans it at end of input.
pub fn run(input: impl BufRead) -> io::Result<()> {
    let mut last = Cleanup::default();
    for line in input.lines() {
        last = serde_json::from_str(&line?).map_err(io::Error::other)?;
    }
    clean(&last);
    Ok(())
}
