use std::{
    env, fs,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::ExitCode,
};

use armod::{
    daemon::Daemon,
    probes::{Kernel, Result},
    text,
};

// --socket and --group let tests serve beside an installed armod, on hosts without the group.
const USAGE: &str = "usage:
  armod [--socket PATH] [--group NAME]       serve armo-collectd
  armod --pid PID|--all [--interval S] [--lib PATH]...
                                             print a callback table per window";

const SOCKET: &str = "/run/armo/armod.sock";

enum Mode {
    Serve {
        socket: PathBuf,
        group: String,
    },
    Text {
        pid: Option<u32>,
        interval: f64,
        libs: Vec<PathBuf>,
    },
}

fn parse() -> Result<Mode> {
    let mut socket = PathBuf::from(SOCKET);
    let mut group = "armo".to_string();
    let mut pids = Vec::new();
    let mut all = false;
    let mut interval = 1.0;
    let mut libs = Vec::new();
    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| format!("{a} needs a value\n{USAGE}"))
        };
        match a.as_str() {
            "--socket" => socket = PathBuf::from(value()?),
            "--group" => group = value()?,
            "--pid" => pids.push(value()?.parse::<u32>()?),
            "--all" => all = true,
            "--interval" => interval = value()?.parse()?,
            "--lib" => libs.push(PathBuf::from(value()?)),
            _ => return Err(format!("unknown argument {a}\n{USAGE}").into()),
        }
    }
    Ok(if all || !pids.is_empty() || !libs.is_empty() {
        if pids.len() > 1 {
            return Err(format!("the text mode takes one --pid\n{USAGE}").into());
        }
        Mode::Text {
            pid: pids.first().copied(),
            interval,
            libs,
        }
    } else {
        Mode::Serve { socket, group }
    })
}

/// The gid of `name` in /etc/group.
fn group_id(name: &str) -> Option<u32> {
    fs::read_to_string("/etc/group")
        .ok()?
        .lines()
        .find_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.first() == Some(&name)).then(|| f.get(2)?.parse().ok())?
        })
}

/// The socket for armo-collectd: only root and the group's members connect (design 5.1).
fn listen(socket: &Path, group: &str) -> Result<UnixListener> {
    if let Some(dir) = socket.parent() {
        fs::create_dir_all(dir)?;
    }
    if socket.exists() {
        if UnixStream::connect(socket).is_ok() {
            return Err(format!("{}: another armod serves it", socket.display()).into());
        }
        fs::remove_file(socket)?;
    }
    let l = UnixListener::bind(socket).map_err(|e| format!("{}: {e}", socket.display()))?;
    l.set_nonblocking(true)?;
    let gid = group_id(group).ok_or_else(|| format!("no group {group} in /etc/group"))?;
    std::os::unix::fs::chown(socket, None, Some(gid))?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o660))?;
    Ok(l)
}

fn run() -> Result<bool> {
    let mode = parse()?;
    let k = Kernel::load()?;
    if let Err(why) = &k.sched_tp.layout {
        eprintln!("armod: the scheduler probes stay off: {why}");
    }
    if let Err(why) = &k.futex_tp.layout {
        eprintln!("armod: the futex probes stay off: {why}");
    }
    match mode {
        Mode::Serve { socket, group } => {
            let l = listen(&socket, &group)?;
            eprintln!("armod: serving {}", socket.display());
            Daemon::new(k, Some(l)).run()
        }
        Mode::Text {
            pid,
            interval,
            libs,
        } => text::run(Daemon::new(k, None), pid, interval, &libs),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("armod: {e}");
            ExitCode::FAILURE
        }
    }
}
