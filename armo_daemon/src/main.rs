use std::{
    collections::HashMap,
    env,
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::ExitCode,
    thread,
    time::{Duration, Instant},
};

use armo_common::{CallbackKey, CallbackStat, KIND_FLUX, KIND_FLUX_CHANNEL, NAME_LEN};
use armod::{
    owner::{Owner, topic},
    probes::{Probes, Result},
    stats::{delta, quantile_upper_ns},
};

const USAGE: &str = "usage: armod [--lib PATH] [--flux-lib PATH] [--pid PID] [--interval SECONDS]";

struct Args {
    lib: PathBuf,
    flux_lib: Option<PathBuf>,
    pid: Option<NonZeroU32>,
    interval: Duration,
}

fn parse() -> Result<Args> {
    let mut lib = None;
    let mut flux_lib = None;
    let mut pid = None;
    let mut interval = Duration::from_secs(1);
    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| format!("{a} needs a value\n{USAGE}"))
        };
        match a.as_str() {
            "--lib" => lib = Some(PathBuf::from(value()?)),
            "--flux-lib" => flux_lib = Some(PathBuf::from(value()?)),
            "--pid" => pid = Some(value()?.parse::<NonZeroU32>()?),
            "--interval" => interval = Duration::from_secs_f64(value()?.parse::<f64>()?),
            _ => return Err(format!("unknown argument {a}\n{USAGE}").into()),
        }
    }
    let lib = match lib {
        Some(l) => l,
        None => find_lib("libtracetools.so")
            .ok_or("libtracetools.so is not under AMENT_PREFIX_PATH (sudo drops it): pass --lib")?,
    };
    // Optional: a host without flux is still a ROS host to watch.
    let flux_lib = flux_lib.or_else(|| find_lib("libflux_tracetools.so"));
    Ok(Args {
        lib,
        flux_lib,
        pid,
        interval,
    })
}

fn find_lib(name: &str) -> Option<PathBuf> {
    env::var("AMENT_PREFIX_PATH")
        .ok()?
        .split(':')
        .map(|p| Path::new(p).join("lib").join(name))
        .find(|p| p.is_file())
}

fn alive(tgid: u32) -> bool {
    Path::new("/proc").join(tgid.to_string()).exists()
}

fn us(ns: u64) -> f64 {
    ns as f64 / 1000.0
}

fn print_window(
    now: &HashMap<CallbackKey, CallbackStat>,
    before: &HashMap<CallbackKey, CallbackStat>,
    names: &HashMap<CallbackKey, String>,
    channels: &HashMap<CallbackKey, u64>,
    secs: f64,
) {
    let mut rows: Vec<_> = now
        .iter()
        .map(|(k, v)| (k, delta(v, before.get(k))))
        .filter(|(_, d)| d.count > 0)
        .collect();
    rows.sort_by_key(|(k, _)| (k.tgid, k.kind, k.callback, k.entry));
    // Read once a window: a channel opened or re-attached since the last one adds a record.
    let mut owners: HashMap<u32, Option<Owner>> = HashMap::new();
    for (k, _) in &rows {
        if k.kind == KIND_FLUX {
            owners.entry(k.tgid).or_insert_with(|| Owner::read(k.tgid));
        }
    }
    let name = |k: &CallbackKey| -> String {
        if k.kind != KIND_FLUX {
            return names.get(k).cloned().unwrap_or_default();
        }
        let Some(&channel) = channels.get(k) else {
            return String::new();
        };
        let owner = owners.get(&k.tgid).and_then(Option::as_ref);
        let ck = CallbackKey {
            tgid: k.tgid,
            kind: KIND_FLUX_CHANNEL,
            callback: channel,
            entry: 0,
            pad: 0,
        };
        // The probe saw this handle's channel_init after armod attached, so it is newer than any
        // record the owner file holds for a reused address, unless it was cut at NAME_LEN.
        let segment = match names.get(&ck) {
            Some(p) if p.len() < NAME_LEN - 1 => Some(p.as_str()),
            p => owner
                .and_then(|o| o.segment(channel))
                .or(p.map(String::as_str)),
        };
        // A heap channel has no segment, and so no topic.
        segment
            .filter(|s| !s.is_empty())
            .map_or_else(String::new, |s| topic(s, owner))
    };
    let id = |k: &CallbackKey| {
        if k.kind == KIND_FLUX {
            format!("{:#x}:{}", k.callback, k.entry)
        } else {
            format!("{:#x}", k.callback)
        }
    };
    println!(
        "{:>8} {:>4} {:>18} {:>8} {:>9} {:>10} {:>10} {:>10}  name",
        "pid", "src", "callback", "count", "rate/s", "mean_us", "p50<=us", "p99<=us"
    );
    for (k, d) in rows {
        let q = |p| quantile_upper_ns(&d.hist, p).map_or(0.0, us);
        println!(
            "{:>8} {:>4} {:>18} {:>8} {:>9.1} {:>10.1} {:>10.1} {:>10.1}  {}",
            k.tgid,
            if k.kind == KIND_FLUX { "flux" } else { "ros" },
            id(k),
            d.count,
            d.count as f64 / secs,
            us(d.sum_ns) / d.count as f64,
            q(0.5),
            q(0.99),
            name(k)
        );
    }
}

fn run() -> Result<()> {
    let args = parse()?;
    let mut probes = Probes::attach(&args.lib, args.flux_lib.as_deref(), args.pid)?;
    eprintln!("armod: attached to {}", args.lib.display());
    match &args.flux_lib {
        Some(f) => eprintln!("armod: attached to {}", f.display()),
        None => eprintln!("armod: libflux_tracetools.so not found, flux callbacks not timed"),
    }
    let mut before = probes.read()?.stats;
    let mut last = Instant::now();
    loop {
        thread::sleep(args.interval);
        let snap = probes.read()?;
        let now = Instant::now();
        let secs = now.duration_since(last).as_secs_f64();
        println!(
            "-- window {secs:.2} s, lost: stats {} names {}",
            snap.lost_stat, snap.lost_name
        );
        print_window(&snap.stats, &before, &snap.names, &snap.channels, secs);
        probes.prune(alive)?;
        before = snap.stats;
        last = now;
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("armod: {e}");
            ExitCode::FAILURE
        }
    }
}
