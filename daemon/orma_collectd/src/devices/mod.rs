//! The host's devices over a window (design 20.3): clocks, temperatures, throttling, network and
//! GPUs, from counters the kernel keeps and from the GPU driver. Read once per window while
//! windows are produced, never from the processes being watched.

mod nvml;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use orma_proto::msg::{Gpu, HostWindow, NetIf, Thermal, Udp, Window};

use nvml::Nvml;

const UDP: [&str; 4] = ["InErrors", "RcvbufErrors", "SndbufErrors", "NoPorts"];
/// `device/of_node/name` of a Jetson's integrated GPU under devfreq, as jetson-stats finds it.
const JETSON: [&str; 4] = ["gv11b", "gp10b", "ga10b", "gpu"];

fn number<T: FromStr>(path: &Path) -> Option<T> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// The entries `<prefix><n>` of a directory, by `n`.
fn numbered(dir: &Path, prefix: &str) -> Vec<(u32, PathBuf)> {
    let mut v: Vec<(u32, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let e = e.ok()?;
            let n = e.file_name().to_str()?.strip_prefix(prefix)?.parse().ok()?;
            Some((n, e.path()))
        })
        .collect();
    v.sort_by_key(|(n, _)| *n);
    v
}

fn cpu_mhz(root: &Path) -> Vec<Option<u32>> {
    let cpus = numbered(&root.join("sys/devices/system/cpu"), "cpu");
    let mut mhz = vec![None; cpus.last().map_or(0, |(n, _)| *n as usize + 1)];
    for (n, dir) in cpus {
        mhz[n as usize] =
            number::<u32>(&dir.join("cpufreq/scaling_cur_freq")).map(|khz| khz / 1000);
    }
    mhz
}

fn thermal(root: &Path) -> Vec<Thermal> {
    numbered(&root.join("sys/class/thermal"), "thermal_zone")
        .into_iter()
        .filter_map(|(_, dir)| {
            Some(Thermal {
                name: fs::read_to_string(dir.join("type"))
                    .ok()?
                    .trim()
                    .to_string(),
                temp_c: number::<i64>(&dir.join("temp"))? as f64 / 1000.0,
            })
        })
        .collect()
}

/// `core` summed over the CPUs; `package` once per package, since each of its CPUs shows it.
fn throttle(root: &Path) -> BTreeMap<String, u64> {
    let mut sum = BTreeMap::new();
    let mut packages = Vec::new();
    for (_, dir) in numbered(&root.join("sys/devices/system/cpu"), "cpu") {
        if let Some(v) = number::<u64>(&dir.join("thermal_throttle/core_throttle_count")) {
            *sum.entry("core".to_string()).or_default() += v;
        }
        let Some(package) = number::<i64>(&dir.join("topology/physical_package_id")) else {
            continue;
        };
        if !packages.contains(&package)
            && let Some(v) = number::<u64>(&dir.join("thermal_throttle/package_throttle_count"))
        {
            packages.push(package);
            *sum.entry("package".to_string()).or_default() += v;
        }
    }
    sum
}

/// Per interface: rx bytes, packets, errors, drops, then the same for tx.
fn net_dev(text: &str) -> BTreeMap<String, [u64; 8]> {
    text.lines()
        .skip(2)
        .filter_map(|l| {
            let (name, rest) = l.split_once(':')?;
            let f: Vec<u64> = rest
                .split_whitespace()
                .map_while(|v| v.parse().ok())
                .collect();
            let g = |i: usize| f.get(i).copied();
            let v = [g(0)?, g(1)?, g(2)?, g(3)?, g(8)?, g(9)?, g(10)?, g(11)?];
            Some((name.trim().to_string(), v))
        })
        .collect()
}

fn snmp_udp(text: &str) -> Option<[u64; 4]> {
    let mut rows = text.lines().filter_map(|l| l.strip_prefix("Udp:"));
    let names: Vec<&str> = rows.next()?.split_whitespace().collect();
    let values: Vec<&str> = rows.next()?.split_whitespace().collect();
    let get = |k: &str| {
        values
            .get(names.iter().position(|n| *n == k)?)?
            .parse()
            .ok()
    };
    Some([get(UDP[0])?, get(UDP[1])?, get(UDP[2])?, get(UDP[3])?])
}

#[derive(Default)]
struct Counters {
    at_ns: u64,
    net: BTreeMap<String, [u64; 8]>,
    udp: Option<[u64; 4]>,
    throttle: BTreeMap<String, u64>,
}

impl Counters {
    fn take(root: &Path, at_ns: u64) -> Counters {
        let read = |p: &str| fs::read_to_string(root.join(p)).unwrap_or_default();
        Counters {
            at_ns,
            net: net_dev(&read("proc/net/dev")),
            udp: snmp_udp(&read("proc/net/snmp")),
            throttle: throttle(root),
        }
    }

    /// Rates and increases from `self` to `end`; what `self` lacks is left out.
    fn until(&self, end: &Counters, host: &mut HostWindow) {
        let s = end.at_ns.saturating_sub(self.at_ns).max(1) as f64 / 1e9;
        host.net = end
            .net
            .iter()
            .filter_map(|(name, e)| {
                let a = self.net.get(name)?;
                let d = |i: usize| e[i].saturating_sub(a[i]);
                Some(NetIf {
                    name: name.clone(),
                    rx_bps: d(0) as f64 / s,
                    rx_pps: d(1) as f64 / s,
                    rx_errors: d(2),
                    rx_drops: d(3),
                    tx_bps: d(4) as f64 / s,
                    tx_pps: d(5) as f64 / s,
                    tx_errors: d(6),
                    tx_drops: d(7),
                })
            })
            .collect();
        host.udp = end.udp.zip(self.udp).map(|(e, a)| {
            let d = |i: usize| e[i].saturating_sub(a[i]);
            Udp {
                in_errors: d(0),
                rcvbuf_errors: d(1),
                sndbuf_errors: d(2),
                no_ports: d(3),
            }
        });
        host.throttle = end
            .throttle
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.saturating_sub(*self.throttle.get(k)?))))
            .collect();
    }
}

/// A Jetson's integrated GPU: `load` in per mille and the devfreq `cur_freq` in Hz.
struct Jetson {
    name: String,
    load: PathBuf,
    freq: PathBuf,
}

fn jetson(root: &Path) -> Vec<Jetson> {
    let mut found: Vec<Jetson> = fs::read_dir(root.join("sys/class/devfreq"))
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let dir = e.ok()?.path();
            let name = fs::read_to_string(dir.join("device/of_node/name")).ok()?;
            let name = name.trim_matches(['\0', '\n']).to_string();
            JETSON.contains(&name.as_str()).then(|| Jetson {
                name,
                load: dir.join("device/load"),
                freq: dir.join("cur_freq"),
            })
        })
        .collect();
    found.sort_by(|a, b| a.freq.cmp(&b.freq));
    found
}

fn jetson_gpus(found: &[Jetson]) -> Vec<Gpu> {
    found
        .iter()
        .zip(0..)
        .map(|(j, index)| Gpu {
            index,
            name: j.name.clone(),
            util_pct: number::<u32>(&j.load).map(|l| f64::from(l) / 10.0),
            mem_used_bytes: None,
            mem_total_bytes: None,
            clock_mhz: number::<u64>(&j.freq).map(|hz| (hz / 1_000_000) as u32),
            temp_c: None,
            power_w: None,
            throttle: Vec::new(),
        })
        .collect()
}

enum Gpus {
    Nvml(Nvml),
    Jetson(Vec<Jetson>),
}

pub struct Devices {
    root: PathBuf,
    gpus: Option<Gpus>,
    prev: Option<Counters>,
}

impl Devices {
    /// `root` is `/` but in tests.
    pub fn new(root: impl Into<PathBuf>) -> Devices {
        Devices {
            root: root.into(),
            gpus: None,
            prev: None,
        }
    }

    /// NVML's init can take long, so it happens when windows begin, once.
    fn gpus(&mut self) -> &mut Gpus {
        let root = &self.root;
        self.gpus.get_or_insert_with(|| match Nvml::open() {
            Some(n) if !n.is_empty() => Gpus::Nvml(n),
            _ => Gpus::Jetson(jetson(root)),
        })
    }

    /// Takes the sample the first window's rates start from. Without it, as when ormad's window
    /// comes before the tick that would take it, the first window has no rates.
    pub fn start(&mut self, at_ns: u64) {
        if self.prev.is_none() {
            self.gpus();
            self.prev = Some(Counters::take(&self.root, at_ns));
        }
    }

    pub fn stop(&mut self) {
        self.prev = None;
    }

    /// `w.host`, and each process's `gpu_mem_bytes` where NVML lists processes.
    pub fn fill(&mut self, w: &mut Window, at_ns: u64) {
        let (host, mem) = self.sample(at_ns);
        for p in &mut w.processes {
            p.gpu_mem_bytes = mem.as_ref().map(|m| m.get(&p.pid).copied().unwrap_or(0));
        }
        w.host = Some(host);
    }

    fn sample(&mut self, at_ns: u64) -> (HostWindow, Option<BTreeMap<u32, u64>>) {
        let end = Counters::take(&self.root, at_ns);
        let mut host = empty(cpu_mhz(&self.root), thermal(&self.root));
        if let Some(prev) = &self.prev {
            prev.until(&end, &mut host);
        }
        self.prev = Some(end);
        let mem = match self.gpus() {
            Gpus::Nvml(n) => {
                host.gpus = n.gpus();
                Some(n.process_memory())
            }
            Gpus::Jetson(j) => {
                host.gpus = jetson_gpus(j);
                None
            }
        };
        (host, mem)
    }
}

fn empty(cpu_mhz: Vec<Option<u32>>, thermal: Vec<Thermal>) -> HostWindow {
    HostWindow {
        cpu_mhz,
        thermal,
        throttle: BTreeMap::new(),
        net: Vec::new(),
        udp: None,
        gpus: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET_DEV: &str = "Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 8630824   32248    0    0    0     0          0         0  8630824   32248    0    0    0     0       0          0
enp5s0: 1950291396 1486313    1  855    0     0          0      5256 74574776  331729    2    3    0     0       0          0
";

    const SNMP: &str = "Ip: Forwarding DefaultTTL
Ip: 1 64
Udp: InDatagrams NoPorts InErrors OutDatagrams RcvbufErrors SndbufErrors InCsumErrors IgnoredMulti MemErrors
Udp: 84491 724 5 62845 6 7 0 59 0
UdpLite: InDatagrams NoPorts InErrors OutDatagrams RcvbufErrors SndbufErrors InCsumErrors IgnoredMulti MemErrors
UdpLite: 0 0 0 0 0 0 0 0 0
";

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Tree {
            let dir =
                std::env::temp_dir().join(format!("orma_devices_{name}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Tree(dir)
        }
        fn put(&self, rel: &str, text: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn proc_net() {
        let n = net_dev(NET_DEV);
        assert_eq!(n["lo"], [8630824, 32248, 0, 0, 8630824, 32248, 0, 0]);
        assert_eq!(
            n["enp5s0"],
            [1950291396, 1486313, 1, 855, 74574776, 331729, 2, 3]
        );
        assert_eq!(snmp_udp(SNMP), Some([5, 6, 7, 724]));
        assert_eq!(snmp_udp("Ip: Forwarding\nIp: 1\n"), None);
    }

    #[test]
    fn rates_over_the_window() {
        let t = Tree::new("rates");
        t.put("proc/net/dev", NET_DEV);
        t.put("proc/net/snmp", SNMP);
        t.put(
            "sys/devices/system/cpu/cpu0/thermal_throttle/core_throttle_count",
            "4\n",
        );
        t.put(
            "sys/devices/system/cpu/cpu0/thermal_throttle/package_throttle_count",
            "1\n",
        );
        t.put(
            "sys/devices/system/cpu/cpu1/thermal_throttle/core_throttle_count",
            "2\n",
        );
        t.put(
            "sys/devices/system/cpu/cpu0/topology/physical_package_id",
            "0\n",
        );
        let a = Counters::take(&t.0, 1_000_000_000);
        t.put(
            "proc/net/dev",
            &(NET_DEV.replace(
                "enp5s0: 1950291396 1486313    1  855",
                "enp5s0: 1950293396 1486333    1  857",
            ) + "  wg0: 1 1 0 0 0 0 0 0 1 1 0 0 0 0 0 0\n"),
        );
        t.put("proc/net/snmp", &SNMP.replace("84491 724 5", "84491 730 9"));
        t.put(
            "sys/devices/system/cpu/cpu1/thermal_throttle/core_throttle_count",
            "5\n",
        );
        let b = Counters::take(&t.0, 1_500_000_000);
        let mut h = empty(Vec::new(), Vec::new());
        a.until(&b, &mut h);
        assert_eq!(h.net.len(), 2, "wg0 began within the window");
        let e = h.net.iter().find(|n| n.name == "enp5s0").unwrap();
        assert_eq!(
            (e.rx_bps, e.rx_pps, e.rx_drops, e.rx_errors),
            (4000.0, 40.0, 2, 0)
        );
        assert_eq!((e.tx_bps, e.tx_drops), (0.0, 0));
        let u = h.udp.unwrap();
        assert_eq!((u.in_errors, u.no_ports, u.rcvbuf_errors), (4, 6, 0));
        assert_eq!(h.throttle["core"], 3);
        assert_eq!(h.throttle["package"], 0);
        assert!(throttle(&Tree::new("no_throttle").0).is_empty());
    }

    #[test]
    fn package_throttle_once_per_package() {
        let t = Tree::new("packages");
        for cpu in 0..8 {
            let dir = format!("sys/devices/system/cpu/cpu{cpu}");
            t.put(
                &format!("{dir}/topology/physical_package_id"),
                &format!("{}\n", cpu / 4),
            );
            let tt = format!("{dir}/thermal_throttle");
            t.put(&format!("{tt}/core_throttle_count"), &format!("{cpu}\n"));
            t.put(
                &format!("{tt}/package_throttle_count"),
                if cpu < 4 { "10\n" } else { "7\n" },
            );
        }
        let s = throttle(&t.0);
        assert_eq!(s["core"], 28);
        assert_eq!(s["package"], 17);
    }

    #[test]
    fn clocks_and_temperatures() {
        let t = Tree::new("clocks");
        t.put(
            "sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq",
            "907844\n",
        );
        t.put(
            "sys/devices/system/cpu/cpu2/cpufreq/scaling_cur_freq",
            "4800000\n",
        );
        t.put(
            "sys/devices/system/cpu/cpu10/cpufreq/scaling_cur_freq",
            "1200000\n",
        );
        t.put("sys/devices/system/cpu/cpufreq/boost", "1\n");
        assert_eq!(
            cpu_mhz(&t.0),
            [
                Some(907),
                None,
                Some(4800),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(1200)
            ]
        );
        t.put("sys/class/thermal/thermal_zone10/type", "gpu-thermal\n");
        t.put("sys/class/thermal/thermal_zone10/temp", "51500\n");
        t.put("sys/class/thermal/thermal_zone2/type", "x86_pkg_temp\n");
        t.put("sys/class/thermal/thermal_zone2/temp", "-1000\n");
        t.put("sys/class/thermal/thermal_zone3/type", "broken\n");
        t.put("sys/class/thermal/cooling_device0/type", "Processor\n");
        let z = thermal(&t.0);
        let z: Vec<(&str, f64)> = z.iter().map(|z| (z.name.as_str(), z.temp_c)).collect();
        assert_eq!(z, [("x86_pkg_temp", -1.0), ("gpu-thermal", 51.5)]);
    }

    #[test]
    fn jetson_gpu() {
        let t = Tree::new("jetson");
        t.put(
            "sys/class/devfreq/17000000.gpu/device/of_node/name",
            "gpu\0",
        );
        t.put("sys/class/devfreq/17000000.gpu/device/load", "437\n");
        t.put("sys/class/devfreq/17000000.gpu/cur_freq", "918000000\n");
        t.put(
            "sys/class/devfreq/15340000.vic/device/of_node/name",
            "vic\0",
        );
        let g = jetson_gpus(&jetson(&t.0));
        assert_eq!(g.len(), 1);
        assert_eq!(
            (g[0].name.as_str(), g[0].util_pct, g[0].clock_mhz),
            ("gpu", Some(43.7), Some(918))
        );
    }

    #[test]
    fn this_host() {
        let mut d = Devices::new("/");
        d.start(crate::sys::mono_ns());
        std::thread::sleep(std::time::Duration::from_millis(200));
        let (h, mem) = d.sample(crate::sys::mono_ns());
        println!("{}\n{mem:?}", serde_json::to_string(&h).unwrap());
        assert!(h.net.iter().any(|n| n.name == "lo"));
        assert!(h.udp.is_some());
    }
}
