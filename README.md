# orma(Observation, Recording, Measurement, Analysis)

orma는 ROS 2 시스템을 tracing하고 monitoring하는 도구다. flux를 쓰는 시스템도 볼 수 있다. 관측하는 process는 고치지 않는다.

지원 distro는 Humble과 Jazzy다. orma는 두 distro에 없는 tracepoint(Kilted부터 tracetools에 있는 rmw client, service tracepoint 등)에는 붙지 않는다.

## 개요

| 기능 | 보는 것 | 명령 |
| --- | --- | --- |
| monitoring | topic과 flux channel의 rate, latency, loss, callback duration, node별 CPU, 장비 상태 | `orma node`, `orma topic`, `orma span`, `orma top`, rqt plugin |
| tracing | message가 늦어진 segment(transport, wakeup, dispatch, callback, scheduling) | `orma record`, `orma analyze` |
| chain, reaction time | topic 순서의 단계별 latency와 e2e, test message에 대한 첫 출력까지의 시간 | `orma record --chain`, `orma analyze --chain`, `orma react`, rqt |
| 경보 | window마다 threshold 규칙 검사, `/diagnostics` 발행 | `orma_diagnostics` node |

- ROS topic과 flux channel을 같은 latency segment로 잰다.
- collection method는 셋(`preload`, `lttng`, `uprobe`)이다. 어느 method를 써도 같은 event를 받아 같은 segment를 낸다.
- 추가 수집(opt-in)은 CUDA driver API 호출(`--gpu`), DDS write(`--dds`), futex 대기(`--futex`)다. 코드에 표시한 구간(span)은 늘 수집한다.

| 문서 | 내용 |
| --- | --- |
| [`docs/install.md`](docs/install.md) | 빌드, ormad service, orma-collectd |
| [`docs/usage.md`](docs/usage.md) | 명령별 사용법과 출력 |
| [`docs/troubleshooting.md`](docs/troubleshooting.md) | 증상별 원인과 조치 |
| [`docs/instrumentation.md`](docs/instrumentation.md) | execution flow별 instrumentation point, latency segment, distro와 method별 차이 |

## 빌드

```bash
rustup toolchain install nightly-2026-09-28 --profile minimal --component rust-src
# bpf-linker needs LLVM 23, which Ubuntu 24.04 does not package; the release binary carries it.
curl -LO https://github.com/aya-rs/bpf-linker/releases/download/v0.11.1/bpf-linker-$(uname -m)-unknown-linux-musl.tar.zst
tar -xpf bpf-linker-$(uname -m)-unknown-linux-musl.tar.zst -C ~/.local/bin
sudo apt install liblttng-ust-dev lttng-tools babeltrace

cd ~/ros2_ws   # the colcon workspace with orma under src/
colcon build --symlink-install --packages-up-to orma_cli rqt_orma orma_daemon orma_collectd orma_preload orma_lttng orma_rclpy orma_diagnostics
```

- 아키텍처: `aarch64`, `x86_64`. bpf-linker는 두 아키텍처 모두 release binary를 낸다. 측정과 테스트는 aarch64(Jetson Orin)에서만 했다.
- `liblttng-ust-dev`: `orma_lttng` 빌드에 필요
- CUDA toolkit은 필요 없다. orma가 시간을 재는 CUDA 함수의 prototype은 `orma_preload` 안에 있다.
- `lttng-tools`, `babeltrace`(1.5), `setpriv`: `lttng` method 전용
- ormad service 설치와 capability: [`docs/install.md`](docs/install.md)

## 수집 대상

| source | instrumentation point |
| --- | --- |
| ROS tracetools | `libtracetools.so`의 `ros_trace_*`(rcl, rclcpp, rmw가 호출) |
| tracepoint 없는 ROS 함수 | `librcl.so`의 `rcl_wait`(entry, return), `rcl_timer_fini`. `librclcpp.so`의 C++ 함수(callback group, executor 소속, intra-process 연결). DDS serialized size |
| rcl_executor(ROS 2 overhaul) | `librcl_executor.so`의 `rcl_executor_trace_*`와 `rcl_executor_next_excluding`. 이 core를 매핑한 process에만, `uprobe` method에서만([`docs/instrumentation.md`](docs/instrumentation.md) 2.12) |
| 전달 경로(ROS 2 overhaul) | `librcl.so`의 broker anchor(`rcl_ipc_trace_*`)와 loan 함수, `libzenohc.so`의 `z_bytes_from_shm_mut`, rclpy의 `rclpy_trace_*`. export된 process에만, 세 method 모두([`docs/instrumentation.md`](docs/instrumentation.md) 2.13) |
| rclpy | rclpy가 부르지 않는 callback event를 `orma_rclpy`가 대신 호출 |
| flux | `libflux_ffi.so`의 `flux_trace_*`(trace contract 2) |
| span | `liborma_span.so`의 `orma_span_begin`, `orma_span_end`(C, C++ `orma::Span`, Python `@span`) |
| CUDA(opt-in) | `libcuda.so`의 driver 함수: kernel과 graph launch, 동기·비동기 copy, stream·context·event synchronize, 할당과 해제. Jetson의 `nvidia/`, `tegra/` 아래 library 포함 |
| DDS write(opt-in) | Cyclone DDS의 `dds_write`, `dds_write_ts`, `dds_writecdr`, `dds_forwardcdr`. Fast DDS의 `DataWriter::write`(`libfastrtps.so` 2.x, `libfastdds.so` 3.x) |
| kernel | `sched_switch`, `sched_wakeup`. futex 대기(opt-in): `sys_enter_futex`/`sys_exit_futex`, `futex_waitv` |

| method | 부착 방법 | 부착 시점 |
| --- | --- | --- |
| `preload` | `liborma_preload.so`가 같은 이름의 함수로 가로챈 뒤 원래 함수 호출 | process 시작 시 `LD_PRELOAD` |
| `lttng` | `liborma_lttng.so`가 같은 방법으로 가로채 LTTng event 생성 | process 시작 시 `LD_PRELOAD`. session은 orma-collectd가 관리 |
| `uprobe` | kernel이 함수 입구에 breakpoint를 걸고 eBPF 프로그램이 받는다 | 실행 중인 process에 즉시 |

```text
ROS/flux process --uprobe-----------------> ormad --/run/orma/ormad.sock--> orma-collectd --> orma, rqt_orma, orma_diagnostics
ROS/flux process --shared memory, LTTng-------------------------------> orma-collectd --> recording directory
kernel           --sched_switch, sched_wakeup, futex--> ormad
```

| process | 역할 |
| --- | --- |
| `ormad` | system service. eBPF probe를 붙이고, record와 window별 aggregate를 orma-collectd에 넘긴다. 시스템은 바꾸지 않는다 |
| `orma-collectd` | 사용자마다 하나 뜨는 daemon. ormad, shared memory 파일, LTTng에서 event를 모아 window마다 aggregate하고 recording을 저장한다. client가 닫혀도 recording은 이어진다 |
| `orma`, `rqt_orma`, `orma_diagnostics` | orma-collectd의 client |

## 사용

workspace를 source한 shell에서 실행한다. 옵션과 출력은 [`docs/usage.md`](docs/usage.md)에 있다.

### 상태 확인

```bash
orma status     # orma-collectd, ormad, selection, recordings
orma check      # this machine's assumptions; exit code 1 on any fail
```

### 관측 대상 실행

```bash
orma run -- ros2 run demo_nodes_cpp talker               # preload (default)
orma run --method lttng -- ros2 launch my_pkg my.launch.py
eval "$(orma env)"                                       # into this shell
```

launch 파일에서는 `OrmaTrace` action을 node보다 앞에 둔다.

```python
from orma_cli.launch import OrmaTrace
from launch import LaunchDescription
from launch_ros.actions import Node


def generate_launch_description():
    return LaunchDescription([
        OrmaTrace(method="preload", record=True, max_duration_s=600),
        Node(package="demo_nodes_cpp", executable="talker"),
    ])
```

`uprobe` method에는 환경변수가 필요 없다. 실행 중인 process를 `--pid`, `--node`, `--topic`으로 고르면 probe가 붙는다.

### monitoring

```bash
orma node cpu                       # CPU, busy time, callback rate and RSS per node
orma node cb /talker                # callbacks of a node
orma node sub /listener /chatter    # subscriptions of a node
orma topic hz /scan                 # publish rate, measured in the publishers
orma topic delay /scan --dds        # publish to take, and to the DDS write
orma topic path /scan               # takes and latency per delivery path
orma span                           # spans marked in code
orma top                            # the machine: CPU clocks, temperatures, network, GPUs
```

- 대상은 `ros2 node`, `ros2 topic`처럼 이름으로 지정한다.
- window(기본 1초)마다 표를 출력한다.

### recording과 분석

```bash
orma record --node '/camera/.*' --max-duration 60
orma record list
orma analyze ~/.local/share/orma/records/orma-<start time>
orma export ~/.local/share/orma/records/orma-<start time> -o out.csv
```

- `orma record`는 event 전부(`--kind events`)나 window snapshot만(`--kind windows`) 기록한다. 길이와 크기 한도는 반드시 준다.
- `orma analyze`는 latency를 transport, wakeup, dispatch, callback segment로 나눠 tail 요약과 CSV를 낸다.

### chain과 reaction time

```bash
orma record --max-duration 60 --chain perception=/camera/image,/detections,/tracks
orma react --input /obstacle --type my_msgs/msg/Obstacle --msg '{x: 1.0}' --output /trajectory
```

- chain은 원천 topic의 publish부터 뒤 topic의 publish까지 걸린 시간을 단계마다 잰다. 마지막 단계가 e2e다.
- `orma react`는 입력 topic에 test message를 publish하고, 출력 topic마다 첫 publish까지 걸린 시간을 잰다. orma에서 topic에 publish하는 명령은 이것뿐이다.

### span

```cpp
#include <orma_span/span.h>

orma::Span s("preprocess");   // begin at construction, end at destruction
```

- callback 안의 구간을 코드에 표시하면 `orma span`에 그 길이가 나온다.
- orma가 붙지 않은 process에서는 빈 함수라 호출 비용만 든다.

### /diagnostics

```bash
ros2 launch orma_diagnostics orma_diagnostics.launch.py rules:=/path/rules.yaml
```

window마다 threshold 규칙(YAML)을 검사해 `/diagnostics`를 publish한다.

### rqt

```bash
rqt --standalone rqt_orma.plugin.Orma
```

CLI와 같은 orma-collectd에 붙으므로 선택, recording, 상태를 공유한다.

### ormad 단독

orma-collectd 없이 callback 표만 볼 때 쓴다. window마다 callback 하나에 한 줄을 출력한다.

```bash
sudo install/orma_daemon/bin/ormad --pid PID [--interval 1] [--lib PATH]...
```

## LTTng

`liborma_lttng.so`는 `liborma_preload.so`와 같은 정적 library로 만들어 같은 함수를 가로챈다. 그리고 호출마다 LTTng event를 낸다.

| provider | event |
| --- | --- |
| `orma` | ROS 함수와 tracepoint 없는 rcl·rclcpp 함수의 orma event 전부, span(`span_begin`, `span_end`). CUDA driver 호출(`cuda_call`)과 DDS write(`dds_write`)는 client가 켰을 때만 낸다. payload는 64비트 word(`p`)와 문자열(`s0`, `s1`)이다. `orma:missing`은 원래 함수를 찾지 못한 point를 알린다 |
| `flux` | trace contract 2의 `flux_trace_*` 함수마다 event 하나. 함수 인자가 field가 된다 |
| `ros2` | `libtracetools.so`에 LTTng가 없는 Humble에서만 쓴다. tracetools 4.1 layout이라 ros2_tracing 도구로 읽을 수 있다 |

`orma run --method lttng`과 `orma env --method lttng`이 하는 일:

- `LD_PRELOAD`에 `liborma_lttng.so`를 더한다.
- `LTTNG_HOME=$XDG_RUNTIME_DIR/orma/lttng`을 설정한다. orma-collectd는 이 위치에 session daemon을 자식 process로 띄운다.
- session은 하나이고 orma-collectd와 수명이 같다. 이름을 주는 event(node, topic, callback 등록)는 늘 켜 둔다. 나머지 event는 그 process가 monitoring이나 recording의 대상일 때만 켠다. process는 시작할 때 이름을 낸다. 그래서 session을 그 뒤에 만들면 orma는 node와 topic을 모른다.
- 나머지 event를 켠 동안에는 window마다 session을 rotate한다.
- rotate한 chunk는 `lttng` method로 도는 recording이 있으면 그 recording의 `ctf/`에 복사한다. 그리고 babeltrace로 읽어 monitoring에 쓴 뒤 지운다.
- tracer가 버린 event는 channel 통계에서 센다.

orma-collectd 없이 직접 session을 만들어도 된다.

```bash
lttng create t && lttng enable-event --userspace 'orma:*,flux:*,ros2:*' && lttng start
LD_PRELOAD=install/orma_lttng/lib/liborma_lttng.so ros2 run <package> <node>
lttng stop && babeltrace ~/lttng-traces/t-*
```

flux를 정적으로 링크한 프로그램은 함수를 직접 호출하므로 보이지 않는다.

## 실시간 영향

아래 값은 Orin(6.8 RT kernel)에서 SCHED_FIFO 부하(rclcpp Pub -> Relay -> Sink chain, component container, fanout, rclpy chain)를 돌려, 아무것도 붙이지 않은 상태와 비교한 것이다. latency는 publish stamp부터 Sink callback 진입까지다.

| method | p99 증가 | 부하 process CPU 증가 | 그 밖의 CPU |
| --- | --- | --- | --- |
| `preload` | 0-5 us | 코어의 5% 이하 | - |
| `lttng` | 10-22 us | 코어의 7-11% | session daemon, consumer 1-3% |
| `uprobe` | 30-80 us | 코어의 14-30% | eBPF 실행 2.2-2.8%, ormad 0.1-0.6% |

- 실시간 시스템에는 `preload`를 쓴다. `orma run`, `orma env`의 기본값이다.
- uprobe를 쓰면 같은 함수를 부르는 thread끼리 kernel 안에서 직렬화된다. 그래서 1 kHz 이상의 RT 부하에는 맞지 않는다.
- uprobe는 probe 하나를 붙이는 데 약 0.25 s, 떼는 데 약 1.1 s가 걸린다. 대상을 고른 뒤 첫 값이 나오기까지 수 초가 걸린다.
- 대상을 고르지 않고 모든 process를 볼 때는 uprobe를 붙이지 않는다.
- orma 자체의 비용은 `orma status` 마지막 줄에 나온다(orma-collectd와 ormad의 CPU, RSS, ring loss).

## package

| package | 설치되는 것 |
| --- | --- |
| `orma_daemon` | `ormad`, systemd service 파일 |
| `orma_collectd` | `orma-collectd`, `orma-analyze` |
| `orma_cli` | `orma` 명령, launch action `OrmaTrace` |
| `rqt_orma` | rqt plugin |
| `orma_diagnostics` | `/diagnostics`를 publish하는 node `orma_diagnostics` |
| `orma_span` | `liborma_span.so`, header `orma_span/span.h`, Python module `orma_span` |
| `orma_preload` | `liborma_preload.so` |
| `orma_lttng` | `liborma_lttng.so` |
| `orma_rclpy` | rclpy용 module |

## 라이선스

MIT([`LICENSE`](LICENSE)). `orma_ebpf`만 MIT와 GPL-2.0 중에서 고를 수 있다([`LICENSE-GPL2`](LICENSE-GPL2)). kernel이 `bpf_probe_read_user_str` 같은 helper를 GPL 호환 프로그램에만 허용하기 때문이다.
