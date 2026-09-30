# armo

Tracing, monitoring and benchmarking for ROS 2 systems, including those that carry data over flux. Early work: `armod` loads eBPF probes on rclcpp's tracetools functions and prints, per callback, how often it ran and how long it took. The counting happens in the kernel; the processes being watched are not changed or restarted.

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

The probes need `CAP_BPF` and `CAP_PERFMON`. `sudo` drops `AMENT_PREFIX_PATH`, so name the library:

```bash
sudo install/armo_daemon/bin/armod --lib /opt/ros/jazzy/lib/libtracetools.so [--pid PID] [--interval 1]
```

Without `--pid` the probes fire in every process that maps the library.

## Checks

`pre-commit run --all-files`, and after a build, as a user with sudo:

```bash
armo_daemon/test/accuracy.py build/armo_daemon/cargo/release/armod
```

It calls the tracetools functions a known number of times and fails unless `armod` counted each one exactly once.

## License

MIT ([`LICENSE`](LICENSE)), except `armo_ebpf`, which is MIT or GPL-2.0 at your choice ([`LICENSE-GPL2`](LICENSE-GPL2)). The kernel lets only GPL-compatible programs call helpers such as `bpf_probe_read_user_str`, and the programs declare `Dual MIT/GPL` to it.
