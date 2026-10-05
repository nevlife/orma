use std::{
    collections::HashMap,
    env,
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::ExitCode,
    thread,
    time::{Duration, Instant},
};

use armo_common::{CallbackKey, CallbackStat, KIND_FLUX, KIND_FLUX_CHANNEL, KIND_FLUX2, NAME_LEN};
use armod::{
    flux2,
    owner::{Owner, topic},
    probes::{Probes, Result},
    stats::{delta, quantile_upper_ns},
};

const USAGE: &str = "usage: armod [--lib PATH]... [--pid PID] [--interval SECONDS]";

/// How often armod looks for newly loaded libraries across every process. Reading every
/// /proc/<pid>/maps costs tens of ms on a busy host; one process's is read every window.
const RESCAN_ALL: Duration = Duration::from_secs(5);

struct Args {
    libs: Vec<PathBuf>,
    pid: Option<NonZeroU32>,
    interval: Duration,
}

fn parse() -> Result<Args> {
    let mut libs = Vec::new();
    let mut pid = None;
    let mut interval = Duration::from_secs(1);
    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| format!("{a} needs a value\n{USAGE}"))
        };
        match a.as_str() {
            "--lib" => libs.push(PathBuf::from(value()?)),
            "--pid" => pid = Some(value()?.parse::<NonZeroU32>()?),
            "--interval" => interval = Duration::from_secs_f64(value()?.parse::<f64>()?),
            _ => return Err(format!("unknown argument {a}\n{USAGE}").into()),
        }
    }
    Ok(Args {
        libs,
        pid,
        interval,
    })
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
        // A heap channel has no segment, and so no topic.
        if k.kind == KIND_FLUX2 {
            return names
                .get(k)
                .filter(|s| !s.is_empty())
                .map_or_else(String::new, |s| flux2::topic(s, k.tgid));
        }
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
        if k.kind == KIND_FLUX || k.kind == KIND_FLUX2 {
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
            match k.kind {
                KIND_FLUX => "flux",
                KIND_FLUX2 => "flux2",
                _ => "ros",
            },
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
    let mut probes = Probes::load(args.pid)?;
    for lib in &args.libs {
        if probes.attach_path(lib)? {
            eprintln!("armod: attached to {}", lib.display());
        }
    }
    let scan = |probes: &mut Probes| {
        for line in probes.attach_mapped() {
            eprintln!("armod: {line}");
        }
    };
    scan(&mut probes);
    let mut scanned = Instant::now();
    let mut before = probes.read()?.stats;
    let mut last = Instant::now();
    loop {
        thread::sleep(args.interval);
        if args.pid.is_some() || scanned.elapsed() >= RESCAN_ALL {
            scan(&mut probes);
            scanned = Instant::now();
        }
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
