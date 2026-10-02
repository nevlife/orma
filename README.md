# armo

Tracing, monitoring and benchmarking for ROS 2 systems, including those that carry data over flux. Early work: `armod` loads eBPF probes on rclcpp's tracetools functions and flux's `flux_trace_*` functions and prints, per callback, how often it ran and how long it took. The counting happens in the kernel; the processes being watched are not changed or restarted.

| Directory | Contents |
| --- | --- |
| `armo_common` | Map keys and values shared by the eBPF programs and `armod` (`no_std`) |
| `armo_ebpf` | The eBPF programs (Rust, aya-ebpf, pinned nightly) |
| `armo_daemon` | `armod` (Rust stable, aya), the colcon package |

## Building

```bash
rustup toolchain install nightly-2026-09-28 --profile minimal --component rust-src
# bpf-linker needs LLVM 23, which Ubuntu 24.04 does not package; the release binary carries it.
curl -LO https://github.com/aya-rs/bpf-linker/releases/download/v0.11.1/bpf-linker-aarch64-unknown-linux-musl.tar.zst
tar -xpf bpf-linker-aarch64-unknown-linux-musl.tar.zst -C ~/.local/bin

cd ~/ros2_ws   # the colcon workspace with armo under src/
colcon build --symlink-install --packages-select armo_daemon
```

## Running

The probes need `CAP_BPF` and `CAP_PERFMON`:

```bash
sudo install/armo_daemon/bin/armod [--pid PID] [--interval 1] [--lib PATH]...
```

armod finds the libraries itself: it reads `/proc/<pid>/maps` and attaches to each `libtracetools.so` and `libflux_tracetools.so` a watched process has loaded, so a process that loads a build-tree copy, another install's or a container's is traced through that copy. With `--pid` it looks every window, so a library loaded later is picked up; without, every process is looked at every 5 s. `--lib` attaches to a library before any process has loaded it. A library rebuilt while the process runs is reported and skipped: its path now names a file the process does not execute.

Each row is one callback: `src` is `ros` or `flux`.

- A ROS callback is keyed by its address. A subscription callback is named by its topic, read from the message rclcpp took just before it, so it is named even when armod attached after the node started. Timer and service callbacks have no name.
- A flux callback is keyed by `executor:entry`, which stays the same when its publisher restarts, and named by the key of the channel it last delivered from. When armod attached after the channel was opened, the name comes from the process's flux owner file in `/dev/shm`.

armod reads only a `libflux_tracetools.so` that exports `flux_trace_contract_2`. A library from another flux version carries other event arguments, and armod refuses it rather than misreading them.

## Checks

`pre-commit run --all-files`, and after a build, as a user with sudo, in a shell that has sourced the workspace:

```bash
armo_daemon/test/accuracy.py build/armo_daemon/cargo/release/armod
```

It runs callers a known number of times under armod, which is given no library path, and fails unless armod counted each callback exactly once and named it:

- the tracetools functions called directly;
- flux callbacks on a channel opened before armod attached and one opened after, the second dispatched inside the first, in a process that maps a private copy of `libflux_tracetools.so`;
- the `demo_nodes_cpp` listener, started before armod, named `/chatter`.

## License

MIT ([`LICENSE`](LICENSE)), except `armo_ebpf`, which is MIT or GPL-2.0 at your choice ([`LICENSE-GPL2`](LICENSE-GPL2)). The kernel lets only GPL-compatible programs call helpers such as `bpf_probe_read_user_str`, and the programs declare `Dual MIT/GPL` to it.
