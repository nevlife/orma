//! armo-collectd's server: one instance per user, a unix socket for clients, the
//! shared state, the sources, the recordings, and the tick that drives them.

mod client;
mod core;

use std::collections::BTreeMap;

use crate::sources::armod::Sel;
use std::fs::{File, Permissions};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use armo_proto::Ingest;

pub use self::core::Core;
use crate::guard::Guard;
use crate::record::Caps;
use crate::sources::armod;
use crate::sys::{self, Disk};

const TICK: Duration = Duration::from_millis(20);

pub struct Config {
    /// `$XDG_RUNTIME_DIR/armo`: socket, instance lock, `LTTNG_HOME`.
    pub runtime_dir: PathBuf,
    /// `~/.local/share/armo`: default recordings and the list of recordings in progress.
    pub data_dir: PathBuf,
    pub shm_dir: PathBuf,
    pub armod_socket: PathBuf,
    pub caps: Caps,
    /// Without clients and recordings for this long, armo-collectd ends: 5 minutes.
    pub idle: Duration,
    pub disk: fn(&Path) -> io::Result<Disk>,
    /// The executable to start as the guard; tests without one leave it out.
    pub guard: Option<PathBuf>,
    /// Whether to run the LTTng source: its session daemon starts with
    /// armo-collectd.
    pub lttng: bool,
}

fn env_dir(name: &str) -> io::Result<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{name} is not set")))
}

impl Config {
    pub fn from_env() -> io::Result<Config> {
        let home = env_dir("HOME")?;
        Ok(Config {
            runtime_dir: env_dir("XDG_RUNTIME_DIR")?.join("armo"),
            data_dir: home.join(".local/share/armo"),
            // Tests keep their fake writers out of the real /dev/shm.
            shm_dir: env_dir("ARMO_TEST_SHM_DIR").unwrap_or_else(|_| "/dev/shm".into()),
            armod_socket: "/run/armo/armod.sock".into(),
            caps: Caps::load(&home.join(".config/armo/collectd.toml"))?,
            idle: Duration::from_secs(300),
            disk: sys::disk,
            guard: Some(std::env::current_exe()?),
            lttng: crate::sources::lttng::available(),
        })
    }

    pub fn socket(&self) -> PathBuf {
        self.runtime_dir.join("collectd.sock")
    }

    pub fn lttng_home(&self) -> PathBuf {
        self.runtime_dir.join("lttng")
    }

    pub fn records_dir(&self) -> PathBuf {
        self.data_dir.join("records")
    }

    /// One directory per line: the recordings in progress, for marking truncated after a crash.
    pub fn active_list(&self) -> PathBuf {
        self.data_dir.join("active")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// Another armo-collectd holds the instance lock.
    Running,
    Idle,
    Signal,
}

/// The model the server feeds (WP3b's `model::Model` in the binary): the records and table
/// differences in, a window out, and the settings it needs from the server.
pub trait Model: Ingest + Send {
    /// Whether armod delivers scheduler events.
    fn set_sched(&mut self, pids: Sel);
    /// `configure.expected_hz`, by entity id.
    fn set_expected_hz(&mut self, by_id: BTreeMap<String, f64>);
    /// The time the LTTng chunks of the coming window cover.
    fn set_lttng_span(&mut self, ns: u64);
}

impl Model for crate::model::Model {
    fn set_sched(&mut self, pids: Sel) {
        crate::model::Model::set_sched(self, pids);
    }
    fn set_expected_hz(&mut self, by_id: BTreeMap<String, f64>) {
        crate::model::Model::set_expected_hz(self, by_id);
    }
    fn set_lttng_span(&mut self, ns: u64) {
        crate::model::Model::set_lttng_span(self, ns);
    }
}

/// Serves until `stop` is set (SIGTERM, SIGINT) or the idle time passes.
pub fn run(config: Config, model: Box<dyn Model>, stop: &AtomicBool) -> io::Result<Exit> {
    std::fs::create_dir_all(&config.runtime_dir)?;
    std::fs::set_permissions(&config.runtime_dir, Permissions::from_mode(0o700))?;
    let instance = File::create(config.runtime_dir.join("collectd.lock"))?;
    if let Err(std::fs::TryLockError::WouldBlock) = instance.try_lock() {
        return Ok(Exit::Running);
    }
    let guard = config.guard.as_deref().map(Guard::spawn).transpose()?;
    let socket = config.socket();
    match std::fs::remove_file(&socket) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;

    let armod_socket = config.armod_socket.clone();
    let core = Arc::new(Mutex::new(Core::new(config, model, guard)));
    core.lock().unwrap().startup();
    let armod_stop = Arc::new(AtomicBool::new(false));
    armod::spawn(Arc::clone(&core), armod_socket, Arc::clone(&armod_stop));

    let exit = loop {
        while let Ok((stream, _)) = listener.accept() {
            client::start(&core, stream);
        }
        if let Some(exit) = core.lock().unwrap().tick(stop.load(Ordering::Relaxed)) {
            break exit;
        }
        thread::sleep(TICK);
    };
    armod_stop.store(true, Ordering::Relaxed);
    drop(listener);
    let _ = std::fs::remove_file(&socket);
    core.lock().unwrap().shutdown();
    Ok(exit)
}
