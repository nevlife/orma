# armo

Tracing, monitoring and benchmarking for ROS 2 systems, including those that carry data over flux. `armod` loads eBPF probes on ROS 2's tracetools functions, the rcl and rclcpp functions that have none, flux2's `flux2_trace_*` functions and the scheduler, and serves the records and per-window aggregates to `armo-collectd` over a socket. The processes being watched are not changed or restarted. The name stands for Analysis, Recording, Measurement, Observation; it is unrelated to ARMO, the company behind Kubescape.

How to install, run and troubleshoot armo, in Korean: [`docs/install.md`](docs/install.md), [`docs/usage.md`](docs/usage.md), [`docs/troubleshooting.md`](docs/troubleshooting.md).

The colcon packages and crates are grouped by where they run; colcon and cargo find them at any depth, so the package names do not carry the group.

| Directory | Contents |
| --- | --- |
| `collect/armo_ebpf` | The eBPF programs (Rust, aya-ebpf, pinned nightly) |
| `collect/armo_preload` | `libarmo_preload.so`, the LD_PRELOAD backend: it intercepts the ROS, rcl, rclcpp and flux2 functions in the process and writes records and aggregates to its shared memory file (C). `include/armo_preload` holds the C headers: `armo_shm.h` and `armo_events.h`, generated from `armo_common` by cbindgen, and `armo_emit.h`, the functions a backend defines |
| `collect/armo_lttng` | `libarmo_lttng.so`, the LTTng backend: armo_preload's interception code with LTTng-UST providers `armo`, `flux2` and, on Humble, `ros2` (C) |
| `collect/armo_rclpy` | Calls the `ros_trace_*` functions rclpy does not call, loaded at Python start-up through `armo run` and `ArmoTrace` |
| `daemon/armo_daemon` | `armod` (Rust stable, aya), the colcon package; `systemd/`, the service files; `test/accuracy.py` and its callers and nodes |
| `daemon/armo_collectd` | `armo-collectd`, the per-user daemon that collects from armod, the shared memory files and LTTng, serves the clients and owns the recordings; `armo-analyze` |
| `client/armo_cli` | The `armo` command, the `ArmoTrace` launch action and the Python client of armo-collectd |
| `client/rqt_armo` | The rqt plugin: a client of armo-collectd |
| `common/armo_common` | Map keys and values shared by the eBPF programs and `armod`; the event records every collection method writes and the LD_PRELOAD shared memory layout (`no_std`) |
| `common/armo_proto` | Frames, JSON messages and their schema (`schema/v1.json`), golden messages, positioned records, and test doubles: `armo-fake-server` and a fake shared memory writer |
| `integration` | The package `armo_integration`: integration tests against the real armod, armo-collectd, shim and LTTng backend, their test nodes (rclcpp and rclpy chains, CPU burners), and `cost/`, the tool that measures each method's cost under ROS 2 load; `scripts/run_root_tests.sh` runs the tests |
| `docs` | How to install, run and troubleshoot armo (Korean) |
| `scripts` | Header generation and the pre-commit checks |

## Building

```bash
rustup toolchain install nightly-2026-09-28 --profile minimal --component rust-src
# bpf-linker needs LLVM 23, which Ubuntu 24.04 does not package; the release binary carries it.
curl -LO https://github.com/aya-rs/bpf-linker/releases/download/v0.11.1/bpf-linker-aarch64-unknown-linux-musl.tar.zst
tar -xpf bpf-linker-aarch64-unknown-linux-musl.tar.zst -C ~/.local/bin

cd ~/ros2_ws   # the colcon workspace with armo under src/
colcon build --symlink-install --packages-select armo_daemon armo_lttng
```

`armo_lttng` needs `liblttng-ust-dev`; the LTTng method needs `lttng-tools`, `babeltrace` (1.5) and `setpriv` at run time.

## Running

The probes need `CAP_BPF` and `CAP_PERFMON`; the eBPF run-time statistics also need `CAP_SYS_ADMIN`, and so does attaching a uprobe on some kernels (Ubuntu 7.0 refused it without). armod serves `armo-collectd` on a unix socket that only root and the members of one group can open:

```bash
sudo install/armo_daemon/bin/armod [--socket /run/armo/armod.sock] [--group armo]
```

To run armod as a system service, as the user `armo` with only the capabilities it needs (`CAP_BPF`, `CAP_PERFMON`, `CAP_SYS_ADMIN`, plus `CAP_SYS_PTRACE` for other users' `/proc/<pid>/maps` and `CAP_DAC_READ_SEARCH` for tracefs), run `daemon/armo_daemon/systemd/install.sh` from a shell that sourced the workspace, then `sudo systemctl enable --now armod`. The unit also sandboxes armod: a read-only file system, no network sockets, no namespaces, a system call allow list (`systemd-analyze security armod` scores it 3.4, from 7.2 without). The script copies armod to `/usr/local/sbin`, so run it again and restart the service after a rebuild. It adds you to the group `armo`, which applies from your next login.

`armo-collectd` asks it for probe sets per process (`discovery` on every process; `hot`, `rclcpp_symbols`, `bandwidth`, `sched` and `names` on the selected ones), for the records of some processes and for a window of aggregates. When the connection closes, armod takes away what that connection asked for. Without the group armod does not start.

| Set | Attaches to |
| --- | --- |
| `discovery` | the init, registration and fini tracepoints of `libtracetools.so`, `rcl_timer_fini` in `librcl.so`, flux2's `channel_init`, `endpoint_init` and `gpu_wedged` (a flux2 build from before GPU wedging lacks the last, and `probes_missing` says so) |
| `hot` | publish, take, callback, executor and rmw service tracepoints, `rcl_wait` entry and return, the other `flux2_trace_*` functions |
| `rclcpp_symbols` | rclcpp's C++ functions that put callbacks in groups and groups in executors, and the intra-process wiring, found by the front of the mangled name for rclcpp 16 or 28 and later |
| `bandwidth` | the serialized size in Fast DDS (`TypeSupport::serialize`) and Cyclone DDS (`serdata_rmw_from_sample`, only with the dbgsym package) |
| `sched` | `sched_switch` and `sched_wakeup`: CPU wait of the selected processes' threads and off-CPU time inside a callback |
| `names` | rclcpp's `Executor::execute_any_executable`, read for the node of each callback; only on selected processes armod attached to after their nodes were made, until their callbacks have nodes |

armod finds the libraries itself: it reads `/proc/<pid>/maps` and attaches to each `libtracetools.so`, `librcl.so`, `librclcpp.so`, `libflux2_ffi.so.<abi>` and DDS library the processes have loaded, through `/proc/<pid>/root`, so a build-tree copy, another install or a container's own file is traced as that file. It decides each process's ROS distro from `ROS_DISTRO` or the library path, and attaches Humble's `ros_trace_rmw_publish(message)` form there. A library rebuilt while the process runs is reported and skipped.

armod attaches only to a `libflux2_ffi.so` that exports `flux2_trace_contract_2`. A library from another flux2 version carries other arguments, and armod refuses it rather than misreading them.

For a quick look without armo-collectd, the text mode prints one row per callback each window:

```bash
sudo install/armo_daemon/bin/armod --pid PID [--interval 1] [--lib PATH]...
```

A ROS callback is named by its registered symbol or by the topic it took from, so it is named even when armod attached after the node started. A flux2 callback is keyed by `executor:entry` and named by the key its channel's signpost holds. flux v1 callbacks appear only here.

## LTTng

`libarmo_lttng.so` catches the same functions as `libarmo_preload.so` (the interception code is one static library) and turns each call into an LTTng event:

| Provider | Events |
| --- | --- |
| `armo` | every armo event of a ROS function and of the rcl and rclcpp functions without one; the payload is the armo record's payload as 64-bit words (`p`) and its strings (`s0`, `s1`), so armo-collectd reads them back without a table per event. `armo:missing` names an intercept point whose original function was not found |
| `flux2` | each `flux2_trace_*` function of trace contract 2, its arguments as fields |
| `ros2` | only where `libtracetools.so` has no LTTng (Humble): the ROS events in tracetools 4.1's own layout, so ros2_tracing's tools read them. Elsewhere `libtracetools.so` emits them |

`armo run --method lttng` and `armo env --method lttng` put the library in `LD_PRELOAD` and set `LTTNG_HOME=$XDG_RUNTIME_DIR/armo/lttng`, where armo-collectd runs its session daemon as a child. armo-collectd keeps at most one session, rotates it once a window, copies each chunk into the `ctf/` directory of every recording with method `lttng`, reads the chunk with babeltrace into armo records for monitoring and deletes it. Events the tracer discarded are counted from the session's channel statistics. `armo-analyze` reads a recording's `ctf/` the same way.

Without armo-collectd, any session works:

```bash
lttng create t && lttng enable-event --userspace 'armo:*,flux2:*,ros2:*' && lttng start
LD_PRELOAD=install/armo_lttng/lib/libarmo_lttng.so ros2 run <package> <node>
lttng stop && babeltrace ~/lttng-traces/t-*
```

A program that links flux2 statically calls the functions directly and is not seen.

## Checks

`pre-commit run --all-files`, and after a build, as a user with sudo, in a shell that has sourced the workspace:

```bash
daemon/armo_daemon/test/accuracy.py [--armod PATH] [--only CASE]... [--humble]
```

`accuracy.py` starts armod on a private socket, talks to it as armo-collectd does, and fails unless every record and aggregate row matches what the test processes did:

- `ros_calls`, `flux2_calls`: every tracepoint and `flux2_trace_*` function called a known number of times with known arguments, from a process that maps a private copy of the library; nested, recursive and out-of-order callbacks on four threads;
- `flux2_real`: the C caller (`flux2_caller.c`) on the installed flux2;
- `cpp_node`, `cpp_fastdds`, `py_node`: `node.cpp` and `node.py` (with `armo_rclpy`) run timers, a subscription and a service a counted number of times; Fast DDS adds the serialized sizes;
- `sched`: callbacks that sleep and callbacks that spin, timed through the uprobe stack and through a shim-style current-callback word;
- `select`: a second process on the same library file has no breakpoint and no records, and detaching removes the breakpoint;
- `selfcheck`: two nodes inside a selfcheck window;
- `humble_cpp`, `humble_py` (`--humble`): the same nodes in the `flux-humble` container, attached through `/proc/<pid>/root`.

`colcon test --packages-select armo_lttng` (with a private session daemon each):

- `lttng_events`: every ROS and flux2 function called a known number of times arrives as often as its armo or flux2 event, and as often again as a ros2 event;
- `lttng_flux2`: the flux2 caller (`daemon/armo_daemon/test/flux2_caller.c`) on the installed flux2;
- `lttng_agreement`: a C++ pair, an rclpy pair with `armo_rclpy` and the flux2 caller, both libraries in each process, recorded by a real armo-collectd with methods `preload` and `lttng` at once; every event count in the window is the same;
- `lttng_lifecycle`: a recording outlives its killed client; a burst's kept and discarded events add up to the calls; after armo-collectd or armo-collectd and its guard are killed the session daemon is gone and the recording truncated; a running process registers again with the next daemon.

`collect/armo_lttng/test/humble_agreement.py IMAGE HUMBLE_WS` runs the agreement in a Humble container (LTTng there, LD_PRELOAD read by the host's armo-collectd).

`cargo test` (or `scripts/cargo.sh test`) also checks the contracts: the committed C headers, `schema/v1.json` and golden frames against what the code generates now (`scripts/gen_headers.sh` rewrites them), every struct's field offsets in C against Rust, `armo_emit.h` against the event catalog, and every golden message through its Rust type and through `common/armo_proto/test/read_golden.py`, which validates it against the schema (`python3-jsonschema`).

## License

MIT ([`LICENSE`](LICENSE)), except `armo_ebpf`, which is MIT or GPL-2.0 at your choice ([`LICENSE-GPL2`](LICENSE-GPL2)). The kernel lets only GPL-compatible programs call helpers such as `bpf_probe_read_user_str`, and the programs declare `Dual MIT/GPL` to it.
