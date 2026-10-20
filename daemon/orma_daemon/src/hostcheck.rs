//! `orma check` items about the host and a target process rather than the probes (design 6.7):
//! kernel settings that stop or change the probes, a loader that dropped LD_PRELOAD, and a time
//! namespace that moves CLOCK_MONOTONIC. ormad reads files root alone may read
//! (CAP_DAC_READ_SEARCH) and other users' processes (CAP_SYS_PTRACE).

use std::fs;

use orma_proto::msg::{Check, CheckStatus};

fn check(item: impl Into<String>, status: CheckStatus, detail: impl Into<String>) -> Check {
    Check {
        item: item.into(),
        status,
        detail: detail.into(),
    }
}

fn text(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// The bracketed choice of a sysfs list such as `[none] integrity confidentiality`.
fn chosen(list: &str) -> Option<&str> {
    list.split_whitespace()
        .find_map(|w| w.strip_prefix('[')?.strip_suffix(']'))
}

/// `hidepid=` of the /proc this process sees, from its mountinfo.
fn hidepid(mountinfo: &str) -> Option<String> {
    mountinfo.lines().find_map(|l| {
        let (_, after) = l.split_once(" - ")?;
        let mut f = after.split_whitespace();
        (f.next()? == "proc").then_some(())?;
        f.nth(1)?
            .split(',')
            .find_map(|o| o.strip_prefix("hidepid="))
            .map(str::to_string)
    })
}

/// A sysctl as a row: a value orma works with passes, one that cannot be read is unknown.
fn sysctl(name: &str, path: &str, fail: &[&str], why: &str) -> Check {
    match text(path) {
        None => check(name, CheckStatus::Unknown, format!("{path} unreadable")),
        Some(v) if fail.contains(&v.as_str()) => {
            check(name, CheckStatus::Fail, format!("{v}: {why}"))
        }
        Some(v) => check(name, CheckStatus::Pass, v),
    }
}

pub fn host() -> Vec<Check> {
    let lockdown = match text("/sys/kernel/security/lockdown") {
        None => check("lockdown", CheckStatus::Unknown, "securityfs unreadable"),
        Some(l) => match chosen(&l) {
            Some("confidentiality") => check(
                "lockdown",
                CheckStatus::Fail,
                "confidentiality: tracefs is closed, so the sched probes find no tracepoint ids; \
                 kprobes, perf kernel samples and BPF kernel reads are refused too",
            ),
            Some("integrity") => check(
                "lockdown",
                CheckStatus::Pass,
                "integrity: refuses only bpf_probe_write_user, which orma does not use",
            ),
            Some(mode) => check("lockdown", CheckStatus::Pass, mode),
            None => check("lockdown", CheckStatus::Unknown, l),
        },
    };
    let hide = fs::read_to_string("/proc/self/mountinfo")
        .ok()
        .and_then(|m| hidepid(&m));
    vec![
        lockdown,
        sysctl(
            "perf_event_paranoid",
            "/proc/sys/kernel/perf_event_paranoid",
            &[],
            "",
        ),
        sysctl("kptr_restrict", "/proc/sys/kernel/kptr_restrict", &[], ""),
        sysctl(
            "unprivileged_bpf_disabled",
            "/proc/sys/kernel/unprivileged_bpf_disabled",
            &[],
            "",
        ),
        sysctl(
            "bpf_jit_enable",
            "/proc/sys/net/core/bpf_jit_enable",
            &["0"],
            "the interpreter runs every probe; the measured costs do not hold",
        ),
        sysctl(
            "bpf_jit_harden",
            "/proc/sys/net/core/bpf_jit_harden",
            &["2"],
            "constant blinding lengthens every probe; the measured costs do not hold",
        ),
        match hide.as_deref() {
            None | Some("0" | "off") => check("proc hidepid", CheckStatus::Pass, "off"),
            Some(v) => check(
                "proc hidepid",
                CheckStatus::Unknown,
                format!(
                    "{v}: orma-collectd sees only its own user's processes; ormad sees all \
                     through CAP_SYS_PTRACE"
                ),
            ),
        },
        check(
            "BTF",
            CheckStatus::Pass,
            if fs::metadata("/sys/kernel/btf/vmlinux").is_ok() {
                "present (orma does not need it)"
            } else {
                "absent (orma does not need it)"
            },
        ),
    ]
}

const AT_SECURE: u64 = 23;

/// The value of `key` in a native-endian 64-bit auxv.
fn auxv(bytes: &[u8], key: u64) -> Option<u64> {
    let (pairs, _) = bytes.as_chunks::<16>();
    pairs
        .iter()
        .map(|c| {
            let (k, v) = c.split_at(8);
            let word = |b: &[u8]| u64::from_ne_bytes(b.try_into().expect("8 bytes"));
            (word(k), word(v))
        })
        .find_map(|(k, v)| (k == key).then_some(v))
}

/// A process whose environment asks for an orma shim it does not map: the loader drops
/// LD_PRELOAD under secure-exec (setuid, file capabilities, an LSM transition).
pub fn preload(pid: u32, comm: &str, maps: &str) -> Option<Check> {
    let wanted = crate::procs::environ(pid, "LD_PRELOAD")?;
    let shim = wanted
        .split([' ', ':'])
        .find(|l| l.contains("liborma_preload") || l.contains("liborma_lttng"))?;
    let name = shim.rsplit('/').next().unwrap_or(shim);
    let item = format!("preload pid {pid} ({comm})");
    if maps.contains(name) {
        return Some(check(item, CheckStatus::Pass, format!("{name} mapped")));
    }
    let secure = fs::read(format!("/proc/{pid}/auxv"))
        .ok()
        .and_then(|a| auxv(&a, AT_SECURE));
    Some(check(
        item,
        CheckStatus::Fail,
        match secure {
            Some(1) => format!(
                "LD_PRELOAD names {name} but the loader ran in secure-exec (AT_SECURE): setuid, \
                 file capabilities or an LSM transition"
            ),
            _ => format!("LD_PRELOAD names {name} but it is not mapped"),
        },
    ))
}

/// `monotonic` and `boottime` offsets in ns of a `timens_offsets` file.
fn offsets(text: &str) -> (i64, i64) {
    let mut out = (0, 0);
    for l in text.lines() {
        let mut f = l.split_whitespace();
        let (Some(clock), Some(s), Some(ns)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let v = s.parse::<i64>().unwrap_or(0) * 1_000_000_000 + ns.parse::<i64>().unwrap_or(0);
        match clock {
            "monotonic" | "1" => out.0 = v,
            "boottime" | "7" => out.1 = v,
            _ => {}
        }
    }
    out
}

/// A process in another time namespace: its CLOCK_MONOTONIC, which the LD_PRELOAD and LTTng
/// methods stamp records with, is off from the host clock `bpf_ktime_get_ns` reads.
pub fn timens(pid: u32, comm: &str) -> Option<Check> {
    let ns = |p: &str| fs::read_link(format!("/proc/{p}/ns/time")).ok();
    let theirs = ns(&pid.to_string())?;
    if Some(&theirs) == ns("self").as_ref() {
        return None;
    }
    let (mono, boot) = offsets(&text(&format!("/proc/{pid}/timens_offsets"))?);
    Some(check(
        format!("time namespace pid {pid} ({comm})"),
        if mono == 0 {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        format!(
            "monotonic offset {mono} ns, boottime {boot} ns; a nonzero monotonic offset moves its \
             LD_PRELOAD and LTTng records by that much"
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_values() {
        assert_eq!(chosen("[none] integrity confidentiality"), Some("none"));
        assert_eq!(
            chosen("none [integrity] confidentiality"),
            Some("integrity")
        );
        let mi = "22 28 0:21 / /proc rw,nosuid shared:13 - proc proc rw,hidepid=invisible\n";
        assert_eq!(hidepid(mi).as_deref(), Some("invisible"));
        assert_eq!(hidepid("22 28 0:21 / /proc rw - proc proc rw\n"), None);
        assert_eq!(
            offsets("monotonic 3600 5\nboottime -2 0\n"),
            (3_600_000_000_005, -2_000_000_000)
        );
        assert_eq!(host().len(), 8);
    }

    #[test]
    fn this_process_auxv() {
        let a = fs::read("/proc/self/auxv").unwrap();
        assert_eq!(auxv(&a, AT_SECURE), Some(0));
        assert!(auxv(&a, 6).is_some_and(|page| page >= 4096));
        assert!(timens(std::process::id(), "self").is_none());
        assert!(preload(std::process::id(), "self", "").is_none());
    }
}
