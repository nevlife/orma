# orma(Observation, Recording, Measurement, Analysis)

orma는 ROS 2 시스템의 트레이싱과 모니터링 도구다. flux를 사용하는 시스템도 관측 가능하다. 관측 대상 프로세스는 수정하지 않는다.

지원하는 ROS 2 배포판은 Humble과 Jazzy다. 두 배포판에 없는 tracepoint(Kilted부터 tracetools에 있는 rmw client, service tracepoint 등)에는 부착하지 않는다.

## 개요

| 기능 | 보는 것 | 명령 |
| --- | --- | --- |
| 모니터링 | 토픽과 flux 채널의 빈도, latency, 손실, 콜백 실행 시간, 노드별 CPU, 장비 상태 | `orma node`, `orma topic`, `orma span`, `orma top`, rqt 플러그인 |
| 트레이싱 | 메시지가 늦어진 구간(transport, wakeup, dispatch, callback, scheduling) | `orma record`, `orma analyze` |
| chain, 반응 시간 | 토픽 순서의 단계별 latency와 e2e, 테스트 메시지에 대한 첫 출력까지의 시간 | `orma record --chain`, `orma analyze --chain`, `orma react`, rqt |
| 경보 | window마다 임계값 규칙 검사, `/diagnostics` 발행 | `orma_diagnostics` 노드 |

- ROS 토픽과 flux 채널을 같은 latency 구간으로 잰다.
- 수집 방식은 `preload`, `lttng`, `uprobe` 셋이다. 어느 방식이든 같은 이벤트를 받아 같은 구간을 낸다.
- 선택 수집은 CUDA driver API 호출(`--gpu`), DDS write(`--dds`), futex 대기(`--futex`)다. 코드에 표시한 구간(span)은 항상 수집한다.

| 문서 | 내용 |
| --- | --- |
| [`docs/install.md`](docs/install.md) | 빌드, ormad 서비스, orma-collectd |
| [`docs/usage.md`](docs/usage.md) | 명령별 사용법과 출력 |
| [`docs/troubleshooting.md`](docs/troubleshooting.md) | 증상별 원인과 조치 |
| [`docs/instrumentation.md`](docs/instrumentation.md) | 실행 흐름별 수집 지점, latency 구간, 배포판과 방식별 차이 |

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

- 아키텍처: `aarch64`, `x86_64`. bpf-linker는 두 아키텍처 모두 릴리스 바이너리를 제공한다.
- `liblttng-ust-dev`: `orma_lttng` 빌드에 필요하다.
- CUDA toolkit은 필요하지 않다. orma가 시간을 재는 CUDA 함수의 선언은 `orma_preload` 안에 있다.
- `lttng-tools`, `babeltrace`(1.5), `setpriv`: `lttng` 방식에만 필요하다.
- ormad 서비스 설치와 capability: [`docs/install.md`](docs/install.md)

## 수집 대상

| 대상 | 수집 지점 | 조건 |
| --- | --- | --- |
| ROS tracetools | `libtracetools.so`의 `ros_trace_*`(rcl, rclcpp, rmw가 호출) | |
| tracepoint 없는 ROS 함수 | `librcl.so`의 `rcl_wait`(entry, return)와 `rcl_timer_fini`, `librclcpp.so`의 C++ 함수(콜백 그룹, executor 소속, intra-process 연결), DDS 직렬화 크기 | |
| rcl_executor | `librcl_executor.so`의 `rcl_executor_trace_*`와 `rcl_executor_next_excluding` | 이 core를 매핑한 프로세스, `uprobe` 방식만([`docs/instrumentation.md`](docs/instrumentation.md) 2.12) |
| rcl broker 전달 경로 | `librcl.so`의 broker anchor(`rcl_ipc_trace_*`)와 loan 함수, `libzenohc.so`의 `z_bytes_from_shm_mut`, rclpy의 `rclpy_trace_*` | export된 프로세스, 세 방식 모두([`docs/instrumentation.md`](docs/instrumentation.md) 2.13) |
| rclpy | rclpy가 호출하지 않는 콜백 이벤트를 `orma_rclpy`가 대신 호출한다 | |
| flux | `libflux_ffi.so`의 `flux_trace_*`(trace contract 2) | |
| span | `liborma_span.so`의 `orma_span_begin`, `orma_span_end`(C, C++ `orma::Span`, Python `@span`) | |
| CUDA | `libcuda.so`의 driver 함수: kernel과 graph launch, 동기와 비동기 copy, stream, context, event의 synchronize, 할당과 해제. Jetson의 `nvidia/`, `tegra/` 아래 라이브러리 포함 | 선택 수집 |
| DDS write | Cyclone DDS의 `dds_write`, `dds_write_ts`, `dds_writecdr`, `dds_forwardcdr`. Fast DDS의 `DataWriter::write`(`libfastrtps.so` 2.x, `libfastdds.so` 3.x) | 선택 수집 |
| 커널 | `sched_switch`, `sched_wakeup` | |
| futex 대기 | `sys_enter_futex`/`sys_exit_futex`, `futex_waitv` | 선택 수집 |

| 방식 | 부착 방법 | 부착 시점 |
| --- | --- | --- |
| `preload` | `liborma_preload.so`가 같은 이름의 함수로 가로챈 뒤 원래 함수를 호출한다 | 프로세스 시작 시 `LD_PRELOAD` |
| `lttng` | `liborma_lttng.so`가 같은 방법으로 가로채 LTTng 이벤트를 낸다 | 프로세스 시작 시 `LD_PRELOAD`. 세션은 orma-collectd가 관리한다 |
| `uprobe` | 커널이 함수 입구에 breakpoint를 설정하고 eBPF 프로그램이 받는다 | 실행 중인 프로세스에 즉시 |

```text
ROS/flux process --uprobe-----------------> ormad --/run/orma/ormad.sock--> orma-collectd --> orma, rqt_orma, orma_diagnostics
ROS/flux process --shared memory, LTTng-------------------------------> orma-collectd --> recording directory
kernel           --sched_switch, sched_wakeup, futex--> ormad
```

- `ormad`: 시스템 서비스. eBPF probe를 부착하고, 레코드와 window별 집계를 orma-collectd에 넘긴다. 시스템 설정은 바꾸지 않는다.
- `orma-collectd`: 사용자마다 하나 실행되는 데몬. ormad, 공유 메모리 파일, LTTng에서 이벤트를 모아 window마다 집계하고 기록을 저장한다. 클라이언트가 종료되어도 기록은 계속된다.
- `orma`, `rqt_orma`, `orma_diagnostics`: orma-collectd의 클라이언트

## 사용

워크스페이스를 source한 셸에서 실행한다. 옵션과 출력은 [`docs/usage.md`](docs/usage.md)에 있다.

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

launch 파일에서는 `OrmaTrace` action을 노드보다 앞에 넣는다.

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

`uprobe` 방식에는 환경 변수가 필요하지 않다. 실행 중인 프로세스를 `--pid`, `--node`, `--topic`으로 고르면 probe가 부착된다.

### 모니터링

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
- window(기본 1 s)마다 표를 출력한다.

### 기록과 분석

```bash
orma record --node '/camera/.*' --max-duration 60
orma record list
orma analyze ~/.local/share/orma/records/orma-<start time>
orma export ~/.local/share/orma/records/orma-<start time> -o out.csv
```

- `orma record`는 이벤트 전부(`--kind events`)나 window snapshot만(`--kind windows`) 기록한다. 길이와 크기 한도는 반드시 지정한다.
- `orma analyze`는 latency를 transport, wakeup, dispatch, callback 구간으로 나눠 tail 요약과 CSV를 낸다.

### chain과 반응 시간

```bash
orma record --max-duration 60 --chain perception=/camera/image,/detections,/tracks
orma react --input /obstacle --type my_msgs/msg/Obstacle --msg '{x: 1.0}' --output /trajectory
```

- chain은 첫 토픽의 발행부터 뒤 토픽의 발행까지 걸린 시간을 단계마다 잰다. 마지막 단계가 e2e다.
- `orma react`는 입력 토픽에 테스트 메시지를 발행하고, 출력 토픽마다 첫 발행까지 걸린 시간을 잰다. orma에서 토픽에 발행하는 명령은 이것뿐이다.

### span

```cpp
#include <orma_span/span.h>

orma::Span s("preprocess");   // begin at construction, end at destruction
```

- 콜백 안의 구간을 코드에 표시하면 `orma span`에 그 길이가 나온다.
- orma가 부착되지 않은 프로세스에서는 빈 함수라 호출 비용만 든다.

### /diagnostics

```bash
ros2 launch orma_diagnostics orma_diagnostics.launch.py rules:=/path/rules.yaml
```

window마다 임계값 규칙(YAML)을 검사해 `/diagnostics`를 발행한다.

### rqt

```bash
rqt --standalone rqt_orma.plugin.Orma
```

CLI와 같은 orma-collectd에 연결되므로 선택 대상, 기록, 상태를 공유한다.

### ormad 단독

orma-collectd 없이 콜백 표만 보는 경우에 사용한다. window마다 콜백 하나에 한 줄을 출력한다.

```bash
sudo install/orma_daemon/bin/ormad --pid PID [--interval 1] [--lib PATH]...
```

## LTTng

`liborma_lttng.so`는 `liborma_preload.so`와 같은 정적 라이브러리로 만들어 같은 함수를 가로채고, 호출마다 LTTng 이벤트를 낸다.

provider는 셋이다.

- `orma`: ROS 함수와 tracepoint 없는 rcl, rclcpp 함수의 orma 이벤트 전부, span(`span_begin`, `span_end`).
  - CUDA driver 호출(`cuda_call`)과 DDS write(`dds_write`)는 클라이언트가 활성화한 경우에만 낸다.
  - 페이로드는 64-bit word(`p`)와 문자열(`s0`, `s1`)이다.
  - `orma:missing`은 원래 함수를 찾지 못한 수집 지점을 알린다.
- `flux`: trace contract 2의 `flux_trace_*` 함수마다 이벤트 하나. 함수 인자가 필드가 된다.
- `ros2`: `libtracetools.so`에 LTTng가 없는 Humble에서만 사용한다. tracetools 4.1 형식이라 ros2_tracing 도구로 읽기 가능하다.

`orma run --method lttng`과 `orma env --method lttng`이 하는 일:

- `LD_PRELOAD`에 `liborma_lttng.so`를 추가한다.
- `LTTNG_HOME=$XDG_RUNTIME_DIR/orma/lttng`을 설정한다. orma-collectd는 이 위치에 session daemon을 자식 프로세스로 시작한다.
- 세션은 하나이고 orma-collectd와 수명이 같다. 이름을 알리는 이벤트(노드, 토픽, 콜백 등록)는 항상 활성화한다. 나머지 이벤트는 그 프로세스가 모니터링이나 기록의 대상인 동안에만 활성화한다.
- 프로세스는 시작 시 이름을 내므로, 세션을 그 뒤에 만들면 orma는 노드와 토픽을 알지 못한다.
- 나머지 이벤트가 활성화된 동안에는 window마다 세션을 rotate한다.
- rotate한 chunk는 `lttng` 방식으로 실행 중인 기록이 있으면 그 기록의 `ctf/`에 복사하고, babeltrace로 읽어 모니터링에 쓴 뒤 지운다.
- tracer가 버린 이벤트는 채널 통계에서 센다.

orma-collectd 없이 세션을 직접 만들어도 된다.

```bash
lttng create t && lttng enable-event --userspace 'orma:*,flux:*,ros2:*' && lttng start
LD_PRELOAD=install/orma_lttng/lib/liborma_lttng.so ros2 run <package> <node>
lttng stop && babeltrace ~/lttng-traces/t-*
```

flux를 정적으로 링크한 프로그램은 함수를 직접 호출하므로 보이지 않는다.

## 실시간 영향

Orin(6.8 RT kernel)에서 SCHED_FIFO 부하(rclcpp Pub -> Relay -> Sink chain, 컴포넌트 컨테이너, fanout, rclpy chain)를 실행해, 아무것도 부착하지 않은 상태와 비교한 값이다. latency는 발행 stamp부터 Sink 콜백 진입까지다.

| 방식 | p99 증가 | 부하 프로세스 CPU 증가 | 그 밖의 CPU |
| --- | --- | --- | --- |
| `preload` | 0-5 us | 코어의 5% 이하 | - |
| `lttng` | 10-22 us | 코어의 7-11% | session daemon, consumer 1-3% |
| `uprobe` | 30-80 us | 코어의 14-30% | eBPF 실행 2.2-2.8%, ormad 0.1-0.6% |

- 실시간 시스템에는 `preload`를 사용한다. `orma run`, `orma env`의 기본값이다.
- uprobe는 같은 함수를 호출하는 스레드끼리 커널 안에서 직렬화하므로 1 kHz 이상의 RT 부하에는 맞지 않는다.
- uprobe는 probe 하나의 부착에 약 0.25 s, 해제에 약 1.1 s가 걸린다. 대상을 고른 뒤 첫 값이 나오기까지 수 초가 걸린다.
- 대상을 고르지 않고 모든 프로세스를 보는 경우에는 uprobe를 부착하지 않는다.
- orma 자체의 비용은 `orma status` 마지막 줄에 나온다(orma-collectd와 ormad의 CPU, RSS, ring 손실).

## 패키지

| 패키지 | 설치되는 것 |
| --- | --- |
| `orma_daemon` | `ormad`, systemd 서비스 파일 |
| `orma_collectd` | `orma-collectd`, `orma-analyze` |
| `orma_cli` | `orma` 명령, launch action `OrmaTrace` |
| `rqt_orma` | rqt 플러그인 |
| `orma_diagnostics` | `/diagnostics`를 발행하는 노드 `orma_diagnostics` |
| `orma_span` | `liborma_span.so`, 헤더 `orma_span/span.h`, Python 모듈 `orma_span` |
| `orma_preload` | `liborma_preload.so` |
| `orma_lttng` | `liborma_lttng.so` |
| `orma_rclpy` | rclpy용 모듈 |

## 라이선스

MIT([`LICENSE`](LICENSE)). `orma_ebpf`만 MIT와 GPL-2.0 중에서 선택 가능하다([`LICENSE-GPL2`](LICENSE-GPL2)). 커널이 `bpf_probe_read_user_str` 같은 helper를 GPL 호환 프로그램에만 허용하기 때문이다.
