# orma

Tracing, monitoring and benchmarking for ROS 2 systems, including those that carry data over flux. `ormad` loads eBPF probes on ROS 2's tracetools functions, the rcl and rclcpp functions that have none, flux's `flux_trace_*` functions and the scheduler, and serves the records and per-window aggregates to `orma-collectd` over a socket. The processes being watched are not changed or restarted. The name stands for Observation, Recording, Measurement, Analysis.

orma supports ROS 2 Humble and Jazzy. It does not attach to tracepoints those two distros lack, such as the rmw client and service tracepoints that tracetools has from Kilted on.

How to install, run and troubleshoot orma, in Korean: [`docs/install.md`](docs/install.md), [`docs/usage.md`](docs/usage.md), [`docs/troubleshooting.md`](docs/troubleshooting.md).

The colcon packages and crates are grouped by where they run; colcon and cargo find them at any depth, so the package names do not carry the group.

| Directory | Contents |
| --- | --- |
| `collect/orma_ebpf` | The eBPF programs (Rust, aya-ebpf, pinned nightly) |
| `collect/orma_preload` | `liborma_preload.so`, the LD_PRELOAD backend: it intercepts the ROS, rcl, rclcpp and flux functions in the process, the `orma_span` markers and, when a client asks, the CUDA driver calls and DDS writes, and writes records and aggregates to its shared memory file (C). cudart reaches libcuda through `dlsym` on its own handle and `cuGetProcAddress`, so the shim also wraps those two and hands out its CUDA functions in place of libcuda's. `include/orma_preload` holds the C headers: `orma_shm.h` and `orma_events.h`, generated from `orma_common` by cbindgen, `orma_emit.h`, the functions a backend defines, and `cuda_api.h`, the CUDA functions orma times with their prototypes, so that orma builds without the CUDA toolkit |
| `collect/orma_lttng` | `liborma_lttng.so`, the LTTng backend: orma_preload's interception code with LTTng-UST providers `orma`, `flux` and, on Humble, `ros2` (C) |
| `collect/orma_span` | `liborma_span.so` and `orma_span/span.h`: `orma_span_begin(name)`, `orma_span_end(name)` the C++ `orma::Span` and the Python decorator `orma_span.span`, empty functions that mark a stretch inside a callback; the name must be a string with static storage, since its address pairs the begin with its end |
| `collect/orma_rclpy` | Calls the `ros_trace_*` functions rclpy does not call, loaded at Python start-up through `orma run` and `OrmaTrace` |
| `daemon/orma_daemon` | `ormad` (Rust stable, aya), the colcon package; `systemd/`, the service files; `test/accuracy.py` and its callers and nodes |
| `daemon/orma_collectd` | `orma-collectd`, the per-user daemon that collects from ormad, the shared memory files and LTTng, serves the clients and owns the recordings; `orma-analyze` |
| `client/orma_cli` | The `orma` command (including `orma react`, which publishes test messages and times the first outputs), the `OrmaTrace` launch action and the Python client of orma-collectd |
| `client/rqt_orma` | The rqt plugin: a client of orma-collectd |
| `client/orma_diagnostics` | A ROS node and client of orma-collectd that checks each window against threshold rules (YAML) and publishes `/diagnostics` |
| `common/orma_common` | Map keys and values shared by the eBPF programs and `ormad`; the event records every collection method writes and the LD_PRELOAD shared memory layout (`no_std`) |
| `common/orma_proto` | Frames, JSON messages and their schema (`schema/v1.json`), golden messages, positioned records, and test doubles: `orma-fake-server` and a fake shared memory writer |
| `integration` | The package `orma_integration`: integration tests against the real ormad, orma-collectd, shim and LTTng backend, their test nodes (rclcpp and rclpy chains, CPU burners), and `cost/`, the tool that measures each method's cost under ROS 2 load; `scripts/run_root_tests.sh` runs the tests |
| `docs` | How to install, run and troubleshoot orma (Korean) |
| `scripts` | Header generation and the pre-commit checks |

## Building

```bash
rustup toolchain install nightly-2026-09-28 --profile minimal --component rust-src
# bpf-linker needs LLVM 23, which Ubuntu 24.04 does not package; the release binary carries it.
curl -LO https://github.com/aya-rs/bpf-linker/releases/download/v0.11.1/bpf-linker-$(uname -m)-unknown-linux-musl.tar.zst
tar -xpf bpf-linker-$(uname -m)-unknown-linux-musl.tar.zst -C ~/.local/bin

cd ~/ros2_ws   # the colcon workspace with orma under src/
colcon build --symlink-install --packages-select orma_daemon orma_lttng
```

`orma_lttng` needs `liblttng-ust-dev`; the LTTng method needs `lttng-tools`, `babeltrace` (1.5) and `setpriv` at run time.

## Running

The probes need `CAP_BPF` and `CAP_PERFMON`; the eBPF run-time statistics also need `CAP_SYS_ADMIN`, and so does attaching a uprobe on some kernels (Ubuntu 7.0 refused it without). ormad serves `orma-collectd` on a unix socket that only root and the members of one group can open:

```bash
sudo install/orma_daemon/bin/ormad [--socket /run/orma/ormad.sock] [--group orma]
```

To run ormad as a system service, as the user `orma` with only the capabilities it needs (`CAP_BPF`, `CAP_PERFMON`, `CAP_SYS_ADMIN`, plus `CAP_SYS_PTRACE` for other users' `/proc/<pid>/maps` and `CAP_DAC_READ_SEARCH` for tracefs), run `daemon/orma_daemon/systemd/install.sh` from a shell that sourced the workspace, then `sudo systemctl enable --now ormad`. The unit also sandboxes ormad: a read-only file system, no network sockets, no namespaces, a system call allow list (`systemd-analyze security ormad` scores it 3.4, from 7.2 without). The script copies ormad to `/usr/local/sbin`, so run it again and restart the service after a rebuild. It adds you to the group `orma`, which applies from your next login.

`orma-collectd` asks it for probe sets per process (`discovery` on every process; `hot`, `rclcpp_symbols`, `bandwidth`, `sched`, `names`, `gpu`, `dds` and `futex` on the selected ones), for the records of some processes and for a window of aggregates. When the connection closes, ormad takes away what that connection asked for. Without the group ormad does not start.

| Set | Attaches to |
| --- | --- |
| `discovery` | the init, registration and fini tracepoints of `libtracetools.so`, `rcl_timer_fini` in `librcl.so`, flux's `channel_init`, `endpoint_init` and `gpu_wedged` (a flux build from before GPU wedging lacks the last, and `probes_missing` says so) |
| `hot` | publish, take, callback and executor tracepoints, `rcl_wait` entry and return, the other `flux_trace_*` functions, `orma_span_begin` and `orma_span_end` in `liborma_span.so` (the span name is read on the begin) |
| `rclcpp_symbols` | rclcpp's C++ functions that put callbacks in groups and groups in executors, and the intra-process wiring, found by the front of the mangled name for rclcpp 16 or 28 and later |
| `bandwidth` | the serialized size in Fast DDS (`TypeSupport::serialize`) and Cyclone DDS (`serdata_rmw_from_sample`, only with the dbgsym package) |
| `sched` | `sched_switch` and `sched_wakeup`: CPU wait of the selected processes' threads and off-CPU time inside a callback |
| `gpu` | entry and return of the CUDA driver functions in `libcuda.so`: kernel and graph launches, synchronous and asynchronous copies (`_v2` names and their `_ptds`/`_ptsz` variants; the byte count from the arguments, width times height (times depth) for 2D and 3D copies), stream, context and event synchronize, allocation and free; one record per call with its duration, bytes and `CUresult` |
| `dds` | entry and return of Cyclone DDS's `dds_write`, `dds_write_ts`, `dds_writecdr` and `dds_forwardcdr` (`libddsc.so`) and Fast DDS's `DataWriter::write` overloads (`libfastrtps.so` 2.x, `libfastdds.so` 3.x); one record per call with the writer and its duration |
| `futex` | `syscalls:sys_enter_futex`/`sys_exit_futex` and the `futex_waitv` pair, for the selected processes' threads: each wait command (`FUTEX_WAIT`, `FUTEX_WAIT_BITSET`, `FUTEX_LOCK_PI`, `FUTEX_LOCK_PI2`, `FUTEX_WAIT_REQUEUE_PI`) and each `futex_waitv`, recorded at the return with the address, duration, command and return value |
| `names` | rclcpp's `Executor::execute_any_executable`, read for the node of each callback; only on selected processes ormad attached to after their nodes were made, until their callbacks have nodes |

ormad finds the libraries itself: it reads `/proc/<pid>/maps` and attaches to each `libtracetools.so`, `librcl.so`, `librclcpp.so`, `libflux_ffi.so.<abi>`, DDS library, `libcuda.so` (also Jetson's under `nvidia/` or `tegra/`) and `liborma_span.so` the processes have loaded, through `/proc/<pid>/root`, so a build-tree copy, another install or a container's own file is traced as that file. It decides each process's ROS distro from `ROS_DISTRO` or the library path, and attaches Humble's `ros_trace_rmw_publish(message)` form there. A library rebuilt while the process runs is reported and skipped.

ormad attaches only to a `libflux_ffi.so` that exports `flux_trace_contract_2`. A library from another flux version carries other arguments, and ormad refuses it rather than misreading them.

For a quick look without orma-collectd, the text mode prints one row per callback each window:

```bash
sudo install/orma_daemon/bin/ormad --pid PID [--interval 1] [--lib PATH]...
```

A ROS callback is named by its registered symbol or by the topic it took from, so it is named even when ormad attached after the node started. A flux callback is keyed by `executor:entry` and named by the key its channel's signpost holds.

## LTTng

`liborma_lttng.so` catches the same functions as `liborma_preload.so` (the interception code is one static library) and turns each call into an LTTng event:

| Provider | Events |
| --- | --- |
| `orma` | every orma event of a ROS function and of the rcl and rclcpp functions without one, the spans (`span_begin`, `span_end`), and the CUDA driver calls (`cuda_call`) and DDS writes (`dds_write`) while the options word of the process's `.lttng` file asks for them; the payload is the orma record's payload as 64-bit words (`p`) and its strings (`s0`, `s1`), so orma-collectd reads them back without a table per event. `orma:missing` names an intercept point whose original function was not found |
| `flux` | each `flux_trace_*` function of trace contract 2, its arguments as fields |
| `ros2` | only where `libtracetools.so` has no LTTng (Humble): the ROS events in tracetools 4.1's own layout, so ros2_tracing's tools read them. Elsewhere `libtracetools.so` emits them |

`orma run --method lttng` and `orma env --method lttng` put the library in `LD_PRELOAD` and set `LTTNG_HOME=$XDG_RUNTIME_DIR/orma/lttng`, where orma-collectd runs its session daemon as a child. orma-collectd keeps at most one session, rotates it once a window, copies each chunk into the `ctf/` directory of every recording with method `lttng`, reads the chunk with babeltrace into orma records for monitoring and deletes it. Events the tracer discarded are counted from the session's channel statistics. `orma-analyze` reads a recording's `ctf/` the same way.

Without orma-collectd, any session works:

```bash
lttng create t && lttng enable-event --userspace 'orma:*,flux:*,ros2:*' && lttng start
LD_PRELOAD=install/orma_lttng/lib/liborma_lttng.so ros2 run <package> <node>
lttng stop && babeltrace ~/lttng-traces/t-*
```

A program that links flux statically calls the functions directly and is not seen.

## Checks

`pre-commit run --all-files`, and after a build, as a user with sudo, in a shell that has sourced the workspace:

```bash
daemon/orma_daemon/test/accuracy.py [--ormad PATH] [--only CASE]... [--humble]
```

`accuracy.py` starts ormad on a private socket, talks to it as orma-collectd does, and fails unless every record and aggregate row matches what the test processes did:

- `ros_calls`, `flux_calls`: every tracepoint and `flux_trace_*` function called a known number of times with known arguments, from a process that maps a private copy of the library; nested, recursive and out-of-order callbacks on four threads;
- `flux_real`: the C caller (`flux_caller.c`) on the installed flux;
- `cpp_node`, `cpp_fastdds`, `py_node`: `node.cpp` and `node.py` (with `orma_rclpy`) run timers, a subscription and a service a counted number of times; Fast DDS adds the serialized sizes;
- `sched`: callbacks that sleep and callbacks that spin, timed through the uprobe stack and through a shim-style current-callback word;
- `select`: a second process on the same library file has no breakpoint and no records, and detaching removes the breakpoint;
- `gpu_calls`: every driver function of the `gpu` set in a stand-in `libcuda.so.1` (`fake_cuda.c`), with its byte count and result, from two threads;
- `spans`: nested spans through `liborma_span.so` (the built one, else `fake_span.c`);
- `futex`: a thread's `FUTEX_WAIT`s with their results and durations, a `futex_waitv` and a condition variable wait (`futex_caller.c`);
- `dds_cyclone`, `dds_fastdds`: `node.cpp` with a DDS write next to each publish;
- `selfcheck`: two nodes inside a selfcheck window;
- `humble_cpp`, `humble_py` (`--humble`): the same nodes in the `flux-humble` container, attached through `/proc/<pid>/root`.

`colcon test --packages-select orma_lttng` (with a private session daemon each):

- `lttng_events`: every ROS and flux function called a known number of times arrives as often as its orma or flux event, and as often again as a ros2 event; spans, and CUDA and DDS calls into stand-in libraries (`orma_preload/test/cuda_stub.c`, `dds_stub.c`), arrive with their keys, names, sizes and writers, and none before the caller turns them on;
- `lttng_flux`: the flux caller (`daemon/orma_daemon/test/flux_caller.c`) on the installed flux;
- `lttng_agreement`: a C++ pair, an rclpy pair with `orma_rclpy` and the flux caller, both libraries in each process, recorded by a real orma-collectd with methods `preload` and `lttng` at once; every event count in the window is the same;
- `lttng_lifecycle`: a recording outlives its killed client; a burst's kept and discarded events add up to the calls; after orma-collectd or orma-collectd and its guard are killed the session daemon is gone and the recording truncated; a running process registers again with the next daemon.

`collect/orma_lttng/test/humble_agreement.py IMAGE HUMBLE_WS` runs the agreement in a Humble container (LTTng there, LD_PRELOAD read by the host's orma-collectd).

`cargo test` (or `scripts/cargo.sh test`) also checks the contracts: the committed C headers, `schema/v1.json` and golden frames against what the code generates now (`scripts/gen_headers.sh` rewrites them), every struct's field offsets in C against Rust, `orma_emit.h` against the event catalog, and every golden message through its Rust type and through `common/orma_proto/test/read_golden.py`, which validates it against the schema (`python3-jsonschema`).

## License

MIT ([`LICENSE`](LICENSE)), except `orma_ebpf`, which is MIT or GPL-2.0 at your choice ([`LICENSE-GPL2`](LICENSE-GPL2)). The kernel lets only GPL-compatible programs call helpers such as `bpf_probe_read_user_str`, and the programs declare `Dual MIT/GPL` to it.
