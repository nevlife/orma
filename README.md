# armo

Tracing, monitoring and benchmarking for ROS 2 systems, including those that carry data over flux. Early work: `armod` loads eBPF probes on rclcpp's tracetools functions, flux's `flux_trace_*` functions and flux2's `flux2_trace_*` functions and prints, per callback, how often it ran and how long it took. The counting happens in the kernel; the processes being watched are not changed or restarted. The name stands for Analysis, Recording, Measurement, Observation; it is unrelated to ARMO, the company behind Kubescape.

| Directory | Contents |
| --- | --- |
| `armo_common` | Map keys and values shared by the eBPF programs and `armod` (`no_std`) |
| `armo_ebpf` | The eBPF programs (Rust, aya-ebpf, pinned nightly) |
| `armo_daemon` | `armod` (Rust stable, aya), the colcon package |
| `armo_lttng` | `libarmo_flux2_lttng.so`, the LTTng-UST provider for flux2's trace functions (C) |

## Building

```bash
rustup toolchain install nightly-2026-09-28 --profile minimal --component rust-src
# bpf-linker needs LLVM 23, which Ubuntu 24.04 does not package; the release binary carries it.
curl -LO https://github.com/aya-rs/bpf-linker/releases/download/v0.11.1/bpf-linker-aarch64-unknown-linux-musl.tar.zst
tar -xpf bpf-linker-aarch64-unknown-linux-musl.tar.zst -C ~/.local/bin

cd ~/ros2_ws   # the colcon workspace with armo under src/
colcon build --symlink-install --packages-select armo_daemon armo_lttng
```

`armo_lttng` needs `liblttng-ust-dev`; its test needs `lttng-tools` and `babeltrace`.

## Running

The probes need `CAP_BPF` and `CAP_PERFMON`:

```bash
sudo install/armo_daemon/bin/armod [--pid PID] [--interval 1] [--lib PATH]...
```

armod finds the libraries itself: it reads `/proc/<pid>/maps` and attaches to each `libtracetools.so`, `libflux_tracetools.so` and `libflux2_ffi.so.<abi>` a watched process has loaded, so a process that loads a build-tree copy, another install's or a container's is traced through that copy. With `--pid` it looks every window, so a library loaded later is picked up; without, every process is looked at every 5 s. `--lib` attaches to a library before any process has loaded it. A library rebuilt while the process runs is reported and skipped: its path now names a file the process does not execute.

Each row is one callback: `src` is `ros`, `flux` or `flux2`.

- A ROS callback is keyed by its address. A subscription callback is named by its topic, read from the message rclcpp took just before it, so it is named even when armod attached after the node started. Timer and service callbacks have no name.
- A flux callback is keyed by `executor:entry`, which stays the same when its publisher restarts, and named by the key of the channel it last delivered from. When armod attached after the channel was opened, the name comes from the process's flux owner file in `/dev/shm`.

- A flux2 callback is keyed like a flux one and named by the channel it delivered from. Its probe passes the segment name, so it is named even when armod attached late, with no owner file. The name is the key an endpoint announced into the channel's signpost, or the signpost name when none did. The key is read only from a signpost of flux2 layout v6. The probe reads the segment name into 128 bytes and a flux2 key may be 185, so a cut name is completed from the one signpost in `/dev/shm` it begins.

armod reads only a `libflux_tracetools.so` that exports `flux_trace_contract_2` and a `libflux2_ffi.so` that exports `flux2_trace_contract_1`. A library from another flux version carries other event arguments, and armod refuses it rather than misreading them.

## LTTng (flux2)

flux2 emits no LTTng events itself: its `flux2_trace_*` functions are empty. `libarmo_flux2_lttng.so` defines functions of the same names; loaded with `LD_PRELOAD`, it receives libflux2_ffi's calls, which go through its PLT, and turns each into an event of provider `flux2` whose fields are the function's arguments: `channel_init`, `commit`, `take`, `refused`, `wake`, `callback_start`, `callback_end`. A process started without it pays nothing.

```bash
lttng create flux2 && lttng enable-event --userspace 'flux2:*' && lttng start
LD_PRELOAD=install/armo_lttng/lib/libarmo_flux2_lttng.so ros2 run <package> <node>
lttng stop && babeltrace ~/lttng-traces/flux2-*
```

Like armod, the library decodes only a `libflux2_ffi.so.<abi>` that exports `flux2_trace_contract_1`; it asks the library that made the first call, and with another contract it emits nothing and says so once on stderr. A program that links flux2 statically calls the functions directly and is not seen. armo does not read the trace yet.

## Checks

`pre-commit run --all-files`, and after a build, as a user with sudo, in a shell that has sourced the workspace:

```bash
armo_daemon/test/accuracy.py build/armo_daemon/cargo/release/armod
```

`accuracy.py` runs callers a known number of times under armod, which is given no library path, and fails unless armod counted each callback exactly once and named it:

- the tracetools functions called directly;
- flux callbacks on a channel opened before armod attached and one opened after, the second dispatched inside the first, in a process that maps a private copy of `libflux_tracetools.so`;
- flux2 callbacks the same way, from a C caller built against the installed `flux2_ffi` that maps a private copy of `libflux2_ffi.so.<abi>`; one channel is announced and named by its key, the other by its signpost;
- the `demo_nodes_cpp` listener, started before armod, named `/chatter`.

The flux case runs when `libflux_tracetools.so` is under `AMENT_PREFIX_PATH` and the flux2 case when `flux2_ffi` is under `CMAKE_PREFIX_PATH`; a workspace may hold only one of them, and the run lists the case it skipped. The ROS library and `demo_nodes_cpp` are taken from `/opt/ros/$ROS_DISTRO`.

`colcon test --packages-select armo_lttng` runs the same flux2 caller (`armo_daemon/test/flux2_caller.c`) under the LTTng library in a user session and fails unless each of its events arrives with its fields.

## License

MIT ([`LICENSE`](LICENSE)), except `armo_ebpf`, which is MIT or GPL-2.0 at your choice ([`LICENSE-GPL2`](LICENSE-GPL2)). The kernel lets only GPL-compatible programs call helpers such as `bpf_probe_read_user_str`, and the programs declare `Dual MIT/GPL` to it.
