//! NVML through `dlopen`, so that orma-collectd has no link dependency on the NVIDIA driver.
#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::{CStr, c_int, c_uint, c_ulonglong, c_void};

use orma_proto::msg::Gpu;

type Device = *mut c_void;
type Ret = c_int;
type Uint = unsafe extern "C" fn(Device, *mut c_uint) -> Ret;
type UintOf = unsafe extern "C" fn(Device, c_int, *mut c_uint) -> Ret;
type Processes = unsafe extern "C" fn(Device, *mut c_uint, *mut ProcessInfo) -> Ret;

const SUCCESS: Ret = 0;
const INSUFFICIENT_SIZE: Ret = 7;
const CLOCK_SM: c_int = 1;
const TEMPERATURE_GPU: c_int = 0;
const NAME_SIZE: usize = 96;
const NOT_AVAILABLE: u64 = u64::MAX;

/// The `nvmlClocksEventReason*` bits of nvml.h (CUDA 13.2).
const REASONS: [(u64, &str); 9] = [
    (0x1, "idle"),
    (0x2, "applications_clocks"),
    (0x4, "power_cap"),
    (0x8, "hw_slowdown"),
    (0x10, "sync_boost"),
    (0x20, "sw_thermal"),
    (0x40, "hw_thermal"),
    (0x80, "hw_power_brake"),
    (0x100, "display_clocks"),
];

pub fn reasons(bits: u64) -> Vec<String> {
    REASONS
        .iter()
        .filter(|(b, _)| bits & b != 0)
        .map(|(_, n)| n.to_string())
        .collect()
}

#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: c_uint,
    _memory: c_uint,
}

#[repr(C)]
#[derive(Default)]
struct Memory {
    total: c_ulonglong,
    _free: c_ulonglong,
    used: c_ulonglong,
}

/// `nvmlProcessInfo_v2_t`, which the `_v3` process lists fill.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct ProcessInfo {
    pid: c_uint,
    used: c_ulonglong,
    _gpu_instance: c_uint,
    _compute_instance: c_uint,
}

struct Api {
    utilization: unsafe extern "C" fn(Device, *mut Utilization) -> Ret,
    memory: unsafe extern "C" fn(Device, *mut Memory) -> Ret,
    clock: UintOf,
    temperature: UintOf,
    power: Uint,
    reasons: unsafe extern "C" fn(Device, *mut c_ulonglong) -> Ret,
    compute: Processes,
    graphics: Processes,
}

pub struct Nvml {
    api: Api,
    devices: Vec<(u32, Device, String)>,
    procs: Vec<ProcessInfo>,
}

// SAFETY: NVML is thread safe, and device handles stay valid until nvmlShutdown, which is never
// called.
unsafe impl Send for Nvml {}

fn sym<F: Copy>(lib: *mut c_void, name: &CStr) -> Option<F> {
    // SAFETY: lib is a handle dlopen returned and name is NUL terminated.
    let p = unsafe { libc::dlsym(lib, name.as_ptr()) };
    // SAFETY: F is the function pointer type nvml.h declares for `name`, the size of a pointer.
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, F>(&p) })
}

fn ok(r: Ret) -> bool {
    r == SUCCESS
}

impl Nvml {
    /// `None` without the library, a symbol of driver 510 or later, or a working driver.
    pub fn open() -> Option<Nvml> {
        // SAFETY: the name is NUL terminated; the library stays loaded for the process's life.
        let lib = unsafe {
            libc::dlopen(
                c"libnvidia-ml.so.1".as_ptr(),
                libc::RTLD_NOW | libc::RTLD_LOCAL,
            )
        };
        if lib.is_null() {
            return None;
        }
        let init: unsafe extern "C" fn() -> Ret = sym(lib, c"nvmlInit_v2")?;
        let count: unsafe extern "C" fn(*mut c_uint) -> Ret = sym(lib, c"nvmlDeviceGetCount_v2")?;
        let handle: unsafe extern "C" fn(c_uint, *mut Device) -> Ret =
            sym(lib, c"nvmlDeviceGetHandleByIndex_v2")?;
        let name: unsafe extern "C" fn(Device, *mut u8, c_uint) -> Ret =
            sym(lib, c"nvmlDeviceGetName")?;
        let api = Api {
            utilization: sym(lib, c"nvmlDeviceGetUtilizationRates")?,
            memory: sym(lib, c"nvmlDeviceGetMemoryInfo")?,
            clock: sym(lib, c"nvmlDeviceGetClockInfo")?,
            temperature: sym(lib, c"nvmlDeviceGetTemperature")?,
            power: sym(lib, c"nvmlDeviceGetPowerUsage")?,
            reasons: sym(lib, c"nvmlDeviceGetCurrentClocksThrottleReasons")?,
            compute: sym(lib, c"nvmlDeviceGetComputeRunningProcesses_v3")?,
            graphics: sym(lib, c"nvmlDeviceGetGraphicsRunningProcesses_v3")?,
        };
        // SAFETY: nvmlInit_v2 takes no arguments.
        if !ok(unsafe { init() }) {
            return None;
        }
        let mut n = 0;
        // SAFETY: n is a valid out pointer.
        if !ok(unsafe { count(&mut n) }) {
            return None;
        }
        let devices = (0..n)
            .filter_map(|i| {
                let mut d = std::ptr::null_mut();
                // SAFETY: d is a valid out pointer.
                if !ok(unsafe { handle(i, &mut d) }) {
                    return None;
                }
                let mut buf = [0u8; NAME_SIZE];
                // SAFETY: buf holds the NAME_SIZE bytes the call is told of.
                unsafe { name(d, buf.as_mut_ptr(), NAME_SIZE as c_uint) };
                let name = CStr::from_bytes_until_nul(&buf)
                    .map_or_else(|_| String::new(), |s| s.to_string_lossy().into_owned());
                Some((i, d, name))
            })
            .collect();
        Some(Nvml {
            api,
            devices,
            procs: vec![ProcessInfo::default(); 32],
        })
    }

    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    pub fn gpus(&self) -> Vec<Gpu> {
        let api = &self.api;
        self.devices
            .iter()
            .map(|&(index, d, ref name)| {
                let uint = |f: Uint| {
                    let mut v = 0;
                    // SAFETY: d is a device handle and v a valid out pointer.
                    ok(unsafe { f(d, &mut v) }).then_some(v)
                };
                let uint_of = |f: UintOf, which: c_int| {
                    let mut v = 0;
                    // SAFETY: d is a device handle and v a valid out pointer.
                    ok(unsafe { f(d, which, &mut v) }).then_some(v)
                };
                let mut u = Utilization::default();
                // SAFETY: d is a device handle and u a valid out pointer.
                let util = ok(unsafe { (api.utilization)(d, &mut u) });
                let mut m = Memory::default();
                // SAFETY: d is a device handle and m a valid out pointer.
                let mem = ok(unsafe { (api.memory)(d, &mut m) });
                let mut bits = 0;
                // SAFETY: d is a device handle and bits a valid out pointer.
                let held = ok(unsafe { (api.reasons)(d, &mut bits) });
                Gpu {
                    index,
                    name: name.clone(),
                    util_pct: util.then_some(u.gpu as f64),
                    mem_used_bytes: mem.then_some(m.used),
                    mem_total_bytes: mem.then_some(m.total),
                    clock_mhz: uint_of(api.clock, CLOCK_SM),
                    temp_c: uint_of(api.temperature, TEMPERATURE_GPU).map(f64::from),
                    power_w: uint(api.power).map(|mw| mw as f64 / 1000.0),
                    throttle: if held { reasons(bits) } else { Vec::new() },
                }
            })
            .collect()
    }

    /// Device memory by host pid, summed over the devices. A process with both a compute and a
    /// graphics context is in both lists with one figure, so a device counts it once.
    pub fn process_memory(&mut self) -> BTreeMap<u32, u64> {
        let Nvml {
            api,
            devices,
            procs,
        } = self;
        let mut total: BTreeMap<u32, u64> = BTreeMap::new();
        for &(_, d, _) in devices.iter() {
            let mut device: BTreeMap<u32, u64> = BTreeMap::new();
            for f in [api.compute, api.graphics] {
                for p in processes(f, d, procs) {
                    if p.used != NOT_AVAILABLE {
                        let v = device.entry(p.pid).or_default();
                        *v = (*v).max(p.used);
                    }
                }
            }
            for (pid, used) in device {
                *total.entry(pid).or_default() += used;
            }
        }
        total
    }
}

fn processes(f: Processes, d: Device, buf: &mut Vec<ProcessInfo>) -> &[ProcessInfo] {
    loop {
        let mut n = buf.len() as c_uint;
        // SAFETY: buf holds n entries of the layout nvml.h declares.
        match unsafe { f(d, &mut n, buf.as_mut_ptr()) } {
            SUCCESS => return &buf[..n as usize],
            INSUFFICIENT_SIZE => buf.resize(n as usize + 8, ProcessInfo::default()),
            _ => return &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_reasons() {
        assert!(reasons(0).is_empty());
        assert_eq!(reasons(0x1), ["idle"]);
        assert_eq!(
            reasons(0x4 | 0x20 | 0x8),
            ["power_cap", "hw_slowdown", "sw_thermal"]
        );
        assert_eq!(reasons(0x1ff).len(), 9);
        assert!(reasons(0x200).is_empty());
    }

    /// The machine's NVML: `ORMA_NVML=1 cargo test -p orma-collectd nvml_here -- --nocapture`.
    #[test]
    fn nvml_here() {
        if std::env::var_os("ORMA_NVML").is_none() {
            return;
        }
        let mut nvml = Nvml::open().expect("NVML");
        assert!(!nvml.is_empty());
        for g in nvml.gpus() {
            println!("{}", serde_json::to_string(&g).unwrap());
            assert!(g.mem_total_bytes.is_some() && g.clock_mhz.is_some());
        }
        println!("{:?}", nvml.process_memory());
    }
}
