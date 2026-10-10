//! The text mode: one process (or all), a callback table per window on stdout. A debugging aid
//! next to the server; flux v1 is visible only here (design D12).

use std::collections::{BTreeSet, HashMap};

use armo_common::{
    CallbackKey, KIND_FLUX, KIND_FLUX_CHANNEL, NAME_LEN,
    events::*,
    shm::{ARMO_AGG_CALLBACK, ARMO_SRC_FLUX2},
};
use armo_proto::{msg::*, records};

use crate::{
    daemon::{Client, Daemon, Sink},
    flux2,
    owner::{Owner, topic},
    probes::Scope,
    progs::Set,
    sel::Sel,
    stats::{delta, quantile_upper_ns},
};

/// Names from the records: the registered symbol, else the topic the callback took from.
#[derive(Default)]
struct Names {
    callbacks: HashMap<(u32, u64), String>,
    rcl_topic: HashMap<(u32, u64), String>,
    rclcpp_rcl: HashMap<(u32, u64), u64>,
    rmw_topic: HashMap<(u32, u64), String>,
    taken: HashMap<u32, u64>,
    flux2: HashMap<(u32, u64, u32), String>,
}

impl Names {
    fn record(&mut self, r: &records::Record) {
        let pid = r.pid;
        let s = |i: usize| -> String {
            r.strings()
                .nth(i)
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default()
        };
        match r.header.event {
            ARMO_EV_RCLCPP_CALLBACK_REGISTER => {
                let e: armo_ev_rclcpp_callback_register = r.payload();
                let sym = s(0);
                if !sym.is_empty() && sym != "DISABLED__demangle_symbol" {
                    self.callbacks.insert((pid, e.callback), sym);
                }
            }
            ARMO_EV_RCL_SUBSCRIPTION_INIT => {
                let e: armo_ev_rcl_topic_init = r.payload();
                self.rcl_topic.insert((pid, e.handle), s(0));
                self.rmw_topic.insert((pid, e.rmw_handle), s(0));
            }
            ARMO_EV_RCLCPP_SUBSCRIPTION_INIT => {
                let e: armo_ev_rclcpp_subscription_init = r.payload();
                self.rclcpp_rcl
                    .insert((pid, e.subscription), e.subscription_handle);
            }
            ARMO_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED => {
                let e: armo_ev_callback_added = r.payload();
                let topic = self
                    .rclcpp_rcl
                    .get(&(pid, e.handle))
                    .and_then(|h| self.rcl_topic.get(&(pid, *h)))
                    .cloned();
                if let Some(t) = topic {
                    self.callbacks.entry((pid, e.callback)).or_insert(t);
                }
            }
            ARMO_EV_RMW_SUBSCRIPTION_TOPIC => {
                let e: armo_ev_rmw_topic = r.payload();
                self.rmw_topic.insert((pid, e.rmw_handle), s(0));
            }
            ARMO_EV_RMW_TAKE => {
                let e: armo_ev_rmw_take = r.payload();
                if e.taken != 0 {
                    self.taken.insert(r.tid, e.rmw_subscription);
                }
            }
            ARMO_EV_CALLBACK_START => {
                let e: armo_ev_callback_start = r.payload();
                let taken = self.taken.remove(&r.tid);
                if e.is_intra_process == 0
                    && let Some(t) = taken.and_then(|h| self.rmw_topic.get(&(pid, h)))
                {
                    self.callbacks.entry((pid, e.callback)).or_insert(t.clone());
                }
            }
            ARMO_EV_FLUX2_CALLBACK_END => {
                let e: armo_ev_flux2_callback_end = r.payload();
                let segment = s(0);
                if !segment.is_empty() {
                    self.flux2
                        .entry((pid, e.executor, e.entry))
                        .or_insert_with(|| flux2::topic(&segment, pid));
                }
            }
            _ => {}
        }
    }
}

fn us(ns: u64) -> f64 {
    ns as f64 / 1000.0
}

fn quantile(hist: &[u64], q: f64) -> f64 {
    quantile_upper_ns(hist.try_into().expect("armod's own BUCKETS"), q).map_or(0.0, us)
}

/// Runs until killed. `pid` None watches every process.
pub fn run(mut d: Daemon, pid: Option<u32>, interval: f64, libs: &[std::path::PathBuf]) -> ! {
    let sel = pid.map_or(Sel::All, |p| Sel::pids([p]));
    if let Some(p) = pid {
        d.k.scan(Some(&BTreeSet::from([p])));
    }
    for lib in libs {
        if let Err(e) = d.k.add_lib(lib, pid) {
            eprintln!("armod: {e}");
        }
    }
    let mut c = Client::new(Sink::Local {
        messages: Vec::new(),
        batches: Vec::new(),
    });
    c.sets.insert(Set::Flux1, sel.clone());
    let id = d.add(c);
    let req = |op| ArmodRequest { id: 0, op };
    d.request(
        id,
        req(ArmodOp::Hello(Hello {
            proto: PROTO,
            client: "armod-text".into(),
            pid: std::process::id(),
        })),
    );
    let selection = Selection {
        all: pid.is_none(),
        pids: pid.into_iter().collect(),
    };
    d.request(
        id,
        req(ArmodOp::Attach(Attach {
            selection,
            sets: vec![ProbeSet::Discovery, ProbeSet::Hot],
            user_words: Default::default(),
        })),
    );
    d.request(
        id,
        req(ArmodOp::Stream {
            on: true,
            pids: pid.into_iter().collect(),
        }),
    );
    d.request(id, req(ArmodOp::Window { window_s: interval }));
    let mut names = Names::default();
    let mut printed: BTreeSet<(String, Scope)> = BTreeSet::new();
    let mut flux_before = HashMap::new();
    loop {
        d.tick();
        for (mapped, scope) in d.k.attached_files() {
            if printed.insert((mapped.clone(), scope)) {
                match scope {
                    Scope::Pid(p) => eprintln!("armod: attached to {mapped} (pid {p})"),
                    Scope::All => eprintln!("armod: attached to {mapped}"),
                }
            }
        }
        let (messages, batches) = match &mut d.clients.get_mut(&id).expect("ours").sink {
            Sink::Local { messages, batches } => {
                (std::mem::take(messages), std::mem::take(batches))
            }
            Sink::Socket { .. } => unreachable!(),
        };
        for b in &batches {
            for r in records::positioned(b).flatten() {
                names.record(&r);
            }
        }
        for m in messages {
            if let ArmodMessage::Window(w) = m {
                let secs = w.window_ns as f64 / 1e9;
                let (stats, fnames, channels, flux_lost) = d.k.flux1();
                println!(
                    "-- window {secs:.2} s, lost: {} flux {flux_lost}",
                    w.lost
                        .iter()
                        .filter(|(_, v)| **v > 0)
                        .map(|(k, v)| format!("{k} {v}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                print_window(&w, &names, secs);
                print_flux1(&stats, &flux_before, &fnames, &channels, secs);
                flux_before = stats;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn header() {
    println!(
        "{:>8} {:>5} {:>18} {:>8} {:>9} {:>10} {:>10} {:>10}  name",
        "pid", "src", "callback", "count", "rate/s", "mean_us", "p50<=us", "p99<=us"
    );
}

fn print_window(w: &ArmodWindow, names: &Names, secs: f64) {
    header();
    let mut rows: Vec<&Aggregate> = w
        .aggregates
        .iter()
        .filter(|a| a.kind == ARMO_AGG_CALLBACK)
        .collect();
    rows.sort_by_key(|a| (a.pid, a.src, a.key, a.entry));
    // A callback that ran on several threads has one row per thread; the table sums them.
    let mut merged: Vec<Aggregate> = Vec::new();
    for a in rows {
        match merged.last_mut() {
            Some(m) if (m.pid, m.src, m.key, m.entry) == (a.pid, a.src, a.key, a.entry) => {
                m.count += a.count;
                m.sum += a.sum;
                for (h, v) in m.hist.iter_mut().zip(&a.hist) {
                    *h += v;
                }
            }
            _ => merged.push(a.clone()),
        }
    }
    for Aggregate {
        pid,
        src,
        key,
        entry,
        count,
        sum,
        hist,
        ..
    } in merged
    {
        let (label, id, name) = if src == ARMO_SRC_FLUX2 {
            (
                "flux2",
                format!("{key:#x}:{entry}"),
                names
                    .flux2
                    .get(&(pid, key, entry))
                    .cloned()
                    .unwrap_or_default(),
            )
        } else {
            (
                "ros",
                format!("{key:#x}"),
                names
                    .callbacks
                    .get(&(pid, key))
                    .cloned()
                    .unwrap_or_default(),
            )
        };
        println!(
            "{:>8} {:>5} {:>18} {:>8} {:>9.1} {:>10.1} {:>10.1} {:>10.1}  {}",
            pid,
            label,
            id,
            count,
            count as f64 / secs,
            us(sum) / count as f64,
            quantile(&hist, 0.5),
            quantile(&hist, 0.99),
            name
        );
    }
}

fn print_flux1(
    now: &HashMap<CallbackKey, armo_common::CallbackStat>,
    before: &HashMap<CallbackKey, armo_common::CallbackStat>,
    names: &HashMap<CallbackKey, String>,
    channels: &HashMap<CallbackKey, u64>,
    secs: f64,
) {
    let mut owners: HashMap<u32, Option<Owner>> = HashMap::new();
    let mut rows: Vec<_> = now
        .iter()
        .filter(|(k, _)| k.kind == KIND_FLUX)
        .map(|(k, v)| (k, delta(v, before.get(k))))
        .filter(|(_, d)| d.count > 0)
        .collect();
    rows.sort_by_key(|(k, _)| (k.tgid, k.callback, k.entry));
    for (k, d) in rows {
        let owner = owners
            .entry(k.tgid)
            .or_insert_with(|| Owner::read(k.tgid))
            .as_ref();
        let name = channels.get(k).map_or_else(String::new, |&channel| {
            let ck = CallbackKey {
                tgid: k.tgid,
                kind: KIND_FLUX_CHANNEL,
                callback: channel,
                entry: 0,
                pad: 0,
            };
            // A name the probe saw is newer than the owner file's record for a reused address,
            // unless it was cut at NAME_LEN.
            let segment = match names.get(&ck) {
                Some(p) if p.len() < NAME_LEN - 1 => Some(p.as_str()),
                p => owner
                    .and_then(|o| o.segment(channel))
                    .or(p.map(String::as_str)),
            };
            segment
                .filter(|s| !s.is_empty())
                .map_or_else(String::new, |s| topic(s, owner))
        });
        let q = |p| quantile_upper_ns(&d.hist, p).map_or(0.0, us);
        println!(
            "{:>8} {:>5} {:>18} {:>8} {:>9.1} {:>10.1} {:>10.1} {:>10.1}  {}",
            k.tgid,
            "flux",
            format!("{:#x}:{}", k.callback, k.entry),
            d.count,
            d.count as f64 / secs,
            us(d.sum_ns) / d.count as f64,
            q(0.5),
            q(0.99),
            name
        );
    }
}
