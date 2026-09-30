use std::{
    collections::HashMap,
    env,
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::ExitCode,
    thread,
    time::{Duration, Instant},
};

use armo_common::{CallbackKey, CallbackStat};
use armod::{
    probes::{Probes, Result},
    stats::{delta, quantile_upper_ns},
};

const USAGE: &str = "usage: armod [--lib PATH] [--pid PID] [--interval SECONDS]";

struct Args {
    lib: PathBuf,
    pid: Option<NonZeroU32>,
    interval: Duration,
}

fn parse() -> Result<Args> {
    let mut lib = None;
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
            "--pid" => pid = Some(value()?.parse::<NonZeroU32>()?),
            "--interval" => interval = Duration::from_secs_f64(value()?.parse::<f64>()?),
            _ => return Err(format!("unknown argument {a}\n{USAGE}").into()),
        }
    }
    let lib = match lib {
        Some(l) => l,
        None => find_tracetools()
            .ok_or("libtracetools.so is not under AMENT_PREFIX_PATH (sudo drops it): pass --lib")?,
    };
    Ok(Args { lib, pid, interval })
}

fn find_tracetools() -> Option<PathBuf> {
    env::var("AMENT_PREFIX_PATH")
        .ok()?
        .split(':')
        .map(|p| Path::new(p).join("lib/libtracetools.so"))
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
    secs: f64,
) {
    let mut rows: Vec<_> = now
        .iter()
        .map(|(k, v)| (k, delta(v, before.get(k))))
        .filter(|(_, d)| d.count > 0)
        .collect();
    rows.sort_by_key(|(k, _)| (k.tgid, k.callback));
    println!(
        "{:>8} {:>18} {:>8} {:>9} {:>10} {:>10} {:>10}  name",
        "pid", "callback", "count", "rate/s", "mean_us", "p50<=us", "p99<=us"
    );
    for (k, d) in rows {
        let q = |p| quantile_upper_ns(&d.hist, p).map_or(0.0, us);
        println!(
            "{:>8} {:>#18x} {:>8} {:>9.1} {:>10.1} {:>10.1} {:>10.1}  {}",
            k.tgid,
            k.callback,
            d.count,
            d.count as f64 / secs,
            us(d.sum_ns) / d.count as f64,
            q(0.5),
            q(0.99),
            names.get(k).map_or("", String::as_str)
        );
    }
}

fn run() -> Result<()> {
    let args = parse()?;
    let mut probes = Probes::attach(&args.lib, args.pid)?;
    eprintln!("armod: attached to {}", args.lib.display());
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
        print_window(&snap.stats, &before, &snap.names, secs);
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
