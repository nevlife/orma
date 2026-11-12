# 사용법

설치는 [install.md](install.md), 문제 해결은 [troubleshooting.md](troubleshooting.md)에 있다. 모든 명령은 워크스페이스를 source한 셸에서 실행한다.

## 1. 상태 확인

```bash
orma status
orma check
```

`orma status`의 둘째 줄이 수집원의 상태다.

```text
orma-collectd: orma-collectd 0.0.0, /run/user/2002/orma/collectd.sock
ormad: connected, lttng: available, preload processes: 0
monitor: idle, window 1s, selection all, sched
recordings: none
executor probes: 4021 (camera_node) stock, 4107 (python3) rcl_executor
delivery probes: 4021 (camera_node) loan+zenoh_shm, 4107 (python3) broker+loan+zenoh_shm+rclpy
```

| 값 | 뜻 |
| --- | --- |
| `ormad: connected` | uprobe, sched, eBPF 비용 사용 가능 |
| `ormad: absent` | ormad가 실행되지 않아 `preload`, `lttng`만 동작한다 |
| `ormad: denied` | 사용자가 그룹 `orma`에 없다. 다시 로그인한다([install.md](install.md) 2절) |
| `ormad: lost` | 연결이 끊겼다. 재연결 시 같은 요청을 다시 보낸다 |
| `lttng: absent` | PATH에 `lttng`가 없다 |
| `preload processes` | `preload`나 `lttng` 방식으로 실행한 프로세스 수 |
| `executor probes` | ROS 프로세스마다 executor에 부착된 uprobe(아래) |
| `delivery probes` | ROS 프로세스마다 부착된 전달 경로 probe: `broker`, `loan`, `zenoh_shm`, `rclpy`([instrumentation.md](instrumentation.md) 2.13) |

`executor probes`의 값:

- `stock`: `rcl_wait`와 rclcpp executor tracepoint
- `rcl_executor`: `stock`에 `rcl_executor` core의 anchor를 추가한 것([instrumentation.md](instrumentation.md) 2.12)

`orma check`:

- 이 장비에서 orma의 가정이 맞는지 검사한다(기본 10 s, `--duration S`).
- 일부 항목은 검사 중에 노드를 새로 실행해야 나온다.
- 행마다 `pass`, `fail`, `unknown` 중 하나가 나오고, `fail`이 하나라도 있으면 exit code는 1이다.
- `unknown`은 판단할 근거가 부족하다는 뜻이고, 이유를 옆에 출력한다.
- ormad가 있으면 추가되는 행은 [troubleshooting.md](troubleshooting.md)의 `orma check` 항목에 설명이 있다.

## 2. 방식 선택

세 방식은 같은 함수를 보고 같은 형식으로 낸다. 부착 방법과 비용만 다르다.

| 방식 | 부착 시점 | 관측 대상 비용(1 kHz C++ chain, p99) | 용도 |
| --- | --- | --- | --- |
| `preload` | 프로세스 시작 시 `LD_PRELOAD` | +0-5 us | 실시간 시스템의 기본 |
| `lttng` | 프로세스 시작 시 `LD_PRELOAD`. 세션은 orma-collectd가 관리한다 | +10-22 us, 코어의 7-11% | ros2_tracing 도구로 읽을 CTF가 필요한 경우 |
| `uprobe` | 실행 중인 프로세스에 즉시 | +30-80 us, 코어의 14-30% | 재시작 불가능한 프로세스를 잠깐 보는 경우 |

- 비용은 Orin, 6.8 RT kernel에서 잰 값이다. 측정 조건은 [README](../README.md)의 실시간 영향 절에 있다.
- uprobe는 같은 함수를 호출하는 스레드끼리 커널 안에서 직렬화한다. 1 kHz 이상의 RT 부하에는 `preload`를 사용한다.
- uprobe는 probe 하나의 부착에 약 0.25 s, 해제에 약 1.1 s가 걸린다. 대상을 고른 뒤 첫 값이 나오기까지 수 초가 걸린다.
- 콜백 안 CPU와 scheduling latency(`sched`)를 재려면 방식과 관계없이 ormad가 필요하다.
- malloc attribution은 `preload`에서만 동작한다.

선택 수집은 활성화해야 동작한다.

| 수집 | 활성화 옵션 | `preload` | `lttng` | `uprobe` | 나오는 곳 |
| --- | --- | --- | --- | --- | --- |
| CUDA driver API 호출 | `--gpu` | O | O | O | `orma node cb`의 CUDA 열, rqt의 GPU tab |
| DDS write 호출 | `--dds` | O | O | O | `orma topic delay`의 DDS 열 |
| futex 대기 | `--futex` | ormad | ormad | ormad | `orma node cb`의 futex 열, rqt의 Threads tab |
| span(`liborma_span`) | 항상 | O | O | O | `orma span`(8절) |
| 장비 상태(clock, 온도, 네트워크, GPU) | 항상 | 방식과 무관 | 방식과 무관 | 방식과 무관 | `orma top` |

- futex 대기는 방식과 관계없이 ormad가 커널 tracepoint로 잰다. ormad가 없으면 열이 `-`로 나온다.
- CUDA 열은 `libcuda.so.1` 호출에 걸린 시간이고, GPU 안의 kernel 실행 시간은 재지 않는다. 비동기 launch는 즉시 반환하므로, 기다린 시간은 sync 열과 동기 copy에 나타난다.

## 3. preload, lttng 방식으로 실행

```bash
orma run -- ros2 run demo_nodes_cpp talker               # preload (default)
orma run --method lttng -- ros2 launch my_pkg my.launch.py
eval "$(orma env)"                                       # into this shell
```

`orma run`과 `orma env`가 설정하는 환경 변수:

| 변수 | 값 |
| --- | --- |
| `LD_PRELOAD` | `liborma_preload.so` 또는 `liborma_lttng.so`. 기존 값은 그 뒤에 유지된다 |
| `PYTHONPATH` | rclpy가 호출하지 않는 trace 함수를 대신 호출하는 `orma_rclpy`를 자동으로 로드한다 |
| `LTTNG_HOME` | `lttng` 전용. `$XDG_RUNTIME_DIR/orma/lttng`(orma-collectd의 session daemon 위치) |

launch 파일에서는 `OrmaTrace`를 노드보다 앞에 넣는다. 그 뒤에 실행되는 프로세스만 환경 변수를 받는다.

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

| 인자 | 뜻 |
| --- | --- |
| `method` | `preload`(기본), `lttng`, `uprobe`. `uprobe`는 환경 변수 없이 기록만 시작한다 |
| `record` | 참이면 launch 시작 시 기록을 시작한다. 기본값은 거짓 |
| `stop_on_shutdown` | 참(기본)이면 launch 종료 시 기록을 멈추고, 거짓이면 기록이 한도까지 계속된다 |

그 밖의 인자는 5절 기록 옵션의 필드 이름이다: `kind`, `dir`, `name`, `max_duration_s`, `max_bytes`, `split_bytes`, `split_duration_s`, `cache_bytes`, `compression`, `bandwidth`, `sched`, `malloc`, `gpu`, `dds`, `futex`, `chains`, `latest_nodes`. `chains`는 `{"name": ..., "topics": [...], "mode": "lineage"}`의 목록이다.

## 4. 모니터링

`ros2 node`, `ros2 topic`처럼 대상을 이름으로 지정한다. 실시간 명령은 window(기본 1 s)마다 표를 출력하고 Ctrl-C로 종료된다.

```bash
orma top                              # the machine: CPU clocks, temperatures, network, GPUs
orma node list
orma node info NODE                   # pid, RSS, topics and callbacks (once)
orma node cpu [NODE...]               # CPU%, busy%, callback Hz, RSS
orma node cb NODE [CB] [--gpu] [--futex]
orma node sub NODE [TOPIC]
orma span [NAME...]
orma topic list|info|hz|bw|delay|path ...  # see orma topic below
```

명령별 열:

- `orma top`: CPU clock(최소, 평균, 최대 MHz와 CPU 수), thermal zone 온도, throttle counter 증가분(x86), interface별 rx와 tx의 초당 bytes, 초당 패킷 수, drop, error, UDP error counter 증가분, `gpus` 표(번호, 이름, 사용률, 메모리(사용/전체), SM clock, 온도, 전력, clock 제한 이유)
- `orma node cpu`: 노드 이름, pid, CPU%, CPU source, busy%(wall time), 콜백 Hz, RSS.
  - RSS는 프로세스 단위 값이라, 한 프로세스에 노드가 여럿이면 `(shared)`로 표시한다.
  - 표 아래에 프로세스 경고(표 넘침, flux GPU wedged)가 나온다.
- `orma node cb`: 콜백, 노드, pid, Hz, expected Hz, 콜백 실행 시간 p50, p90, p99, max, dispatch latency p99.
  - `--gpu`면 `cuda api`(실행 한 번당 CUDA 호출 시간)와 `cuda sync`(그중 synchronize 시간), `--futex`면 `futex`(실행 한 번당 대기 시간) 열이 추가된다.
  - CB를 주면 이름에 CB가 들어간 콜백만 보인다.
- `orma node sub`: (토픽, 구독 노드)마다 한 행. 토픽, 노드, pid, take Hz, loss%, loss 수, wakeup latency p50, p90, p99, max. TOPIC을 주면 그 토픽만 보인다.
- `orma span`: span 이름, 감싼 span, 감싼 콜백, 노드, pid, Hz, 길이 p50, p90, p99, max. NAME을 주면 그 이름만 보인다(8절).

```bash
orma node cpu
orma node cb /talker
orma node cb /camera/driver 'sub(/image)'
orma node sub /listener /chatter
```

| 옵션 | 뜻 |
| --- | --- |
| `--pid PID` | orma보다 먼저 실행 중이던 프로세스를 고른다. ormad가 uprobe를 부착한다 |
| `--method M` | 한 번 주면 모든 pid에 적용하고, pid마다 주면 순서대로 짝짓는다 |
| `--window S` | window 길이(s). 기본 1 s |
| `--infra` | 숨긴 infrastructure 토픽과 service 표시 |

- 이름은 정규식이 아니라 문자열 그대로 비교한다. 노드 이름은 `ros2 node list`와 같은 `/ns/name` 형식이다.
- 노드를 지정하면 그 노드가 든 프로세스를 고른다. 비용은 프로세스 단위로 든다. 컨테이너 안의 노드 하나만 골라도 uprobe는 프로세스 전체에 부착된다.
- 노드를 지정하지 않는 명령(`node list`, `node cpu`, `span`)은 모든 프로세스를 보이고 uprobe를 부착하지 않는다.
  - 값은 `orma run`(`preload`, `lttng`)으로 실행한 프로세스에서만 나온다.
  - 시작 시 `[info] select_all`을 출력하고, 보일 것이 없으면 window 줄 아래에 `none`이 나온다.
- orma보다 먼저 실행 중이던 프로세스는 orma가 노드 이름을 알지 못해 이름으로 고르기 불가능하다. `--pid`로 고르면 콜백이 실행된 뒤 수 초 안에 이름이 채워진다(아래 `unlinked`).
- 프로세스 단위 값(프로세스 CPU, PSS)은 CLI에 없고, rqt의 Processes tree와 기록(`orma export --metric processes`)에 있다.
  - PSS는 rqt의 `pss` 체크를 활성화한 경우에만 5 s마다 `smaps_rollup`으로 읽는다.
  - 커널은 이 값의 계산 시 대상의 mmap_lock을 점유한 채 대상의 page table 전체를 훑는다. 그동안 대상의 페이지 폴트와 mmap은 기다린다.
- `--gpu`, `--futex`(`node cb`), `--dds`(`topic delay`)는 2절의 선택 수집을 활성화한다. 비활성화하려면 rqt의 Selection tab을 사용한다.
- 선택, window 길이, 선택 수집 설정은 orma-collectd에 하나씩만 있고, 다른 터미널이나 rqt도 같은 설정을 사용한다.
- 클라이언트가 하나도 없으면 orma-collectd는 hot probe를 해제한다.
- tab 완성은 워크스페이스를 source하면 활성화된다(bash, zsh, argcomplete).
  - 명령과 옵션 외에 노드, 토픽, 노드의 콜백과 구독 토픽, span 이름, 기록 id를 완성한다.
  - 이름은 orma-collectd의 다음 window에서 읽으므로 최대 window 길이만큼 걸린다. orma-collectd가 없으면 이름 후보가 나오지 않는다.

```text
callbacks
  callback       node       pid     Hz   expected  p50      p90      p99      max      dispatch p99
  sub(/chatter)  /listener  393740  1.0  -         69.1us   69.1us   69.1us   69.1us   88.4us
  timer Talker   /talker    393739  1.0  1.0       185.8us  185.8us  185.8us  185.8us  30.9us
```

콜백 이름:

| 형식 | 뜻 |
| --- | --- |
| `sub(/chatter)` | 구독 콜백. 괄호 안은 토픽 |
| `srv(/add)` | service 콜백 |
| `timer cb_fn` | timer 콜백. 괄호에 넣을 대상이 없어 콜백을 정의한 함수 이름을 쓴다(인자 목록, template 인자, namespace, lambda 표시 제외) |
| `timer Talker` | `Talker` class 생성자 안의 lambda. rclpy의 `Talker.__init__.<locals>.<lambda>`도 같다 |
| `timer Talker.on_timer` | rclpy method. Python의 `__qualname__` |
| `flux(/image)` | flux 채널 콜백 |
| `<timer 0xaaaacbc8a330>` | 함수 이름을 알아내지 못한 콜백. 숫자는 콜백 객체 주소 |
| `... #a7e448` | 한 노드 안에 같은 이름이 둘 이상인 콜백. 주소 끝 여섯 자리로 구분한다 |

`unlinked`:

- orma가 노드를 알지 못하는 콜백과 구독이다. 노드 열이 `unlinked`이고 표 맨 아래에 나온다.
- `unlinked (cold)`: orma가 늦게 부착되어 노드 생성을 보지 못한 경우다.
- uprobe로 보는 rclcpp 프로세스에서는 orma가 executor에서 노드를 읽어 보통 2-3 s 안에 채운다. 그동안 콜백마다 uprobe 하나(약 1.25 us)가 더 호출된다. 10 s 안에 찾지 못하면 포기한다.
- 채워지지 않는 것: rclpy 노드, Humble의 `StaticSingleThreadedExecutor`와 `EventsExecutor`, intra-process 전용 구독, timer만 있는 콜백 그룹의 timer. 늦게 부착된 service는 `srv`와 함수 이름만 보인다.
- 처음부터 보려면 `orma node cpu`나 rqt를 먼저 실행하고 노드를 실행한다. 클라이언트가 연결된 동안은 모든 프로세스에 init probe가 부착되어 있다.

표시 규칙:

- 숨기는 것: `/rosout`, `/parameter_events`, `ros_discovery_info`, rclcpp가 노드마다 만드는 parameter service(`get_parameters` 등)와 그 콜백. `--infra`를 주면 보인다.
- latency 열 순서: p50, p90, p99, max. window 값의 p99.9는 기록과 rqt에만 있다.
- `-`: 미측정. `0`: 측정값 0
- CPU source `sched`: ormad가 잰 on-CPU. `wall_bound`: sched 없이 콜백 wall time으로 낸 상한
- window 줄 끝의 `behind`: orma-collectd가 window 하나를 window 길이 안에 모으지 못했다는 뜻이다.

`orma status` 마지막 줄은 orma 자체의 비용이다.

```text
orma: collectd cpu 1.0% rss 6.9MiB, ormad cpu 0.4% rss 22.5MiB, shm files 0 ring drops 0, probes missing 6, collect 1.7ms
```

`probes missing`은 부착되지 않은 수집 지점 수다. 그 함수가 없는 라이브러리의 지점도 센다. 이유는 `orma check`에 나온다.

`preload` 프로세스의 표가 넘치면 `orma node cpu` 표 아래에 경고가 나온다.

```text
warning: pid 4021 (camera_node): 3 samples not aggregated (thread table full)
```

| 경고 | 뜻 |
| --- | --- |
| `threads not traced (thread slots full)` | 프로세스의 traced 스레드 256개 초과 |
| `samples not aggregated (thread table full)` | 한 스레드의 콜백 종류 63개 초과(malloc attribution 활성화 시 31개) |
| `callbacks not timed (nested deeper than 16)` | 콜백 중첩 16단 초과 |

flux GPU가 wedged인 프로세스에는 `CUresult`와 함께 경고 한 줄이 추가된다. 그 프로세스에서는 GPU 채널의 claim, loan, take가 실패하고, orma는 이를 토픽의 publish drop으로 센다.

`orma top`:

- orma-collectd가 window마다 커널 counter와 NVML(Jetson은 sysfs)에서 읽는다.
- NVML이나 GPU가 없으면 `gpus` 표만 비고 나머지는 그대로 나온다.
- `--json`을 주면 window 메시지 전체를 한 줄에 하나씩 출력한다. 필드는 `common/orma_proto/schema/v1.json`의 `window`에 있다.

### `orma topic`

```bash
orma topic list [--infra]
orma topic info TOPIC
orma topic hz TOPIC... [--window S]
orma topic bw TOPIC... [--window S]
orma topic delay TOPIC... [--window S] [--dds]
orma topic path TOPIC... [--window S]
```

```text
window 381  1s
  topic   pub Hz  expected  drops
  /scan   10.0    10.0      0
  /none   -       -         -
```

| 명령 | 값 |
| --- | --- |
| `list` | 관측 중인 프로세스의 토픽 이름. window 하나를 받고 종료 |
| `info` | transport(`ros`, `flux`), 발행 노드, 구독 노드. window 하나를 받고 종료 |
| `hz` | 발행 Hz, expected Hz, publish drop 수 |
| `bw` | 초당 발행 bytes |
| `delay` | transport latency(발행에서 take까지) p50, p90, p99, max. `--dds`면 열 둘 추가(아래) |
| `path` | 표 셋(아래) |

`delay --dds`가 추가하는 열:

- `pub to dds p99`: `rclcpp_publish`에서 DDS write 시작까지
- `dds write p99`: write 호출 시간

`path`의 표([instrumentation.md](instrumentation.md) 2.13):

- `paths`: 전달 경로(`broker`, `loan`, `shm`, `net`, `unknown`)별 take Hz와 transport latency
- `publishers`: loan을 빌려서 발행까지(`loan hold p99`), rclpy 발행의 변환(`py convert p99`)
- `subscriptions`: 구독마다 받은 경로, loan take 수, take에서 콜백 시작까지 p50과 p99, rclpy 변환 p99, broker 메시지나 loan을 보유한 시간 p99

`orma topic`의 동작:

- orma는 `ros2 topic`과 달리 구독자를 만들어 받는 쪽에서 세지 않고, 발행자와 구독자 프로세스 안에서 잰다. 그래서 DDS 전송이 새로 생기지 않고, `hz`에 네트워크 손실이 섞이지 않는다. 받은 수는 `orma node sub`의 take Hz로 본다.
- `delay`는 `ros2 topic delay`와 달리 header stamp를 사용하지 않는다. 같은 메시지의 발행 시각과 take 시각의 차이다.
- 토픽 이름은 그대로 비교한다. 명령은 그 토픽의 발행자와 구독자 프로세스를 고르고, `list`는 모든 프로세스를 고른다.
- 관측하지 않는 발행자의 값은 `-`다. 첫 window는 window 도중에 대상을 골라서 `-`일 가능성이 있다.

## 5. 기록

```bash
orma record [--pid PID]... [--method M] [--kind events|windows] [options]
```

- foreground에서는 window마다 상태를 한 줄 출력한다. Ctrl-C를 누르면 기록을 멈추고, 마무리한 뒤 검증 표를 출력한다.
- 기록은 orma-collectd가 보유하므로, 터미널을 닫거나 클라이언트가 죽어도 기록은 한도까지 계속된다.
- `--no-follow`를 주면 시작한 뒤 즉시 반환한다.

| 옵션 | 뜻 | 기본 |
| --- | --- | --- |
| `--all`, `--pid PID`..., `--node RE`, `--topic RE`, `--exclude RE` | 대상 | `--all` |
| `--method auto\|uprobe\|preload\|lttng` | `auto`는 프로세스마다 `preload`, `lttng`, `uprobe` 순 | `auto` |
| `--kind events\|windows` | 이벤트 전부 또는 window snapshot만 | `events` |
| `-o DIR`, `--name NAME` | 위치와 이름 | `~/.local/share/orma/records/orma-<start time>` |
| `--max-duration S` | 길이 한도 | events 30분, windows 24시간 |
| `--max-size BYTES` | 크기 한도 | events 8 GiB, windows 1 GiB |
| `--split-size BYTES`, `--split-duration S` | 이벤트 파일 분할 | 1 GiB, 미분할 |
| `--cache BYTES` | write buffer | 16 MiB |
| `--compression none\|zstd` | 이벤트 파일 압축 | `none` |
| `--bandwidth` | DDS 직렬화 크기(uprobe) | 비활성화 |
| `--sched`, `--no-sched` | 콜백 안 CPU와 scheduling latency | ormad가 있으면 활성화 |
| `--malloc` | 콜백별 할당(`preload`) | 비활성화 |
| `--gpu`, `--dds`, `--futex` | 2절의 선택 수집 | 비활성화 |
| `--chain NAME=T1,T2,...[:first]` | 잴 chain(7절, 여러 번 지정 가능, `meta.json`에 기록) | 없음 |
| `--latest-node RE` | 최신 입력으로 잇는 노드(7절, 여러 번 지정 가능) | 없음 |

- 한도는 반드시 지정한다. 0이나 무한은 받지 않는다. 길이의 상한은 `~/.config/orma/collectd.toml`의 `max_duration_s`다(기본 7일).
- 디스크 여유 공간이 하한(기본 max(2 GiB, 파일 시스템의 5%), `disk_floor_bytes`) 아래로 내려가면 기록은 `disk_floor`로 종료된다. 시작 시 여유 공간이 하한 + 1 GiB보다 적으면 시작하지 않는다.
- 기록은 동시에 4개까지 실행 가능하다.
- `--node`, `--topic`, `--exclude`는 이름 전체에 맞는 정규식이고, rqt Selection tab의 같은 칸과 뜻이 같다. 기록 중에 새로 시작된 노드도 이름이 맞으면 들어가고, 한 번 들어간 프로세스는 끝까지 남는다.

```bash
orma record list                       # active and recent recordings
orma record stop <id>
orma record show <id or directory>     # verification table: events, loss, Hz and warnings per entity
```

기록 디렉터리:

```text
orma-20261008-153012/
  meta.json        targets, method, options, attached libraries, loss, gaps, end reason
  windows.jsonl    one line per window
  events/000.ormaev
  ctf/             lttng method only
```

`meta.json`:

- `state: truncated`: 마무리 전에 orma-collectd가 죽은 기록이다. orma-collectd의 다음 시작 시 개수를 다시 세어 복구한다.
- `probes_missing`: 부착되지 않은 수집 지점 목록이다. 빈 열이 "호출이 없었다"인지 "볼 수 없었다"인지 가리는 데 사용한다.
- `gaps`: ormad 연결이 끊긴 구간처럼 값이 빈 구간이다.
- `system_start`, `system_end`: 시작과 끝의 장비 상태.
  - 커널 버전과 설정(`HZ`, preempt), cmdline, clocksource, `clock_gettime` 한 번의 비용, scheduler와 sched_ext, 격리 CPU, cpufreq governor와 EPP, 비활성화한 idle state, THP, 취약점 완화, lockdown, 가상화, 시계 동기 상태, 대상 프로세스의 Seccomp와 `cpu.max`를 담는다.
  - root만 읽기 가능한 값(debugfs)은 제외된다.
  - 다른 장비나 다른 날의 값과 비교하기 전에 이 상태부터 맞춘다.
- `interference`: 기록 시작부터 끝까지 늘어난 커널 counter다. CPU별 interrupt(`LOC`, `RES`, `CAL`, `TLB`, device interrupt 합 `dev`)와 softirq, vmstat의 reclaim, compaction, major fault, TLB flush, PSI, steal, 대상 스레드별 minor fault와 major fault, 실행 시간과 대기 시간을 담는다.
- `realtime_steps`: 기록 중에 CLOCK_REALTIME이 점프한 시각이다. 그 근처의 `header.stamp` 나이와 timestamp matching은 신뢰하지 않는다.
- `probe_changes`: ormad가 probe를 부착하거나 해제한 구간이다. 그동안 커널이 코드를 고치고 TLB를 비우므로, 그 구간의 tail은 orma가 만든 것으로 본다.

## 6. 분석과 내보내기

```bash
orma analyze <directory> [--chain NAME=T1,T2,...[:first]]...
orma export <directory> -o out.csv [--metric M]...
```

- `analyze`는 이벤트 기록을 transport, wakeup, dispatch, callback 구간으로 나누고, 메시지별, 콜백별 표와 tail 요약(p50, p90, p99, max)을 출력한다. CSV는 `<directory>/analysis/`에 기록한다. `summary.csv`에는 p99.9도 있다(`n,p50_ns,p90_ns,p99_ns,p999_ns,max_ns`).
- 메시지별 표(`deliveries.csv`)에는 전달 경로(`path`), take에서 콜백 시작까지(`deliver_ns`), rclpy 변환(`py_convert_ns`) 열이 있다. 요약에는 경로별 transport latency, take -> callback start, rclpy 변환, hold 표가 있다([instrumentation.md](instrumentation.md) 2.13).
- `--gpu`, `--futex`로 기록했으면 `callbacks.csv`에 콜백 한 번마다 CUDA driver 호출 시간(`cuda_ns`), 그중 synchronize 시간(`cuda_sync_ns`), futex 대기(`futex_ns`) 열이 추가되고, 요약에도 같은 tail 표가 추가된다.
- `--dds`로 기록했으면 `publishes.csv`에 발행마다 발행에서 DDS write 시작까지 걸린 시간과 write 시간을 기록한다.
- window에 장비 상태(`host`)가 있으면 `tails.csv`도 기록한다.
  - 표와 entity마다 p99 이상인 sample(최대 10개)을 골라, 그 sample이 나온 window의 장비 상태를 추가한다.
  - 열은 그 스레드가 실행된 CPU의 최저 clock(스레드를 모르면 전체 CPU), 가장 뜨거운 thermal zone, throttle 횟수, GPU별 clock, 온도, clock 제한 이유다.
  - 요약 끝의 `tail context`는 표마다 가장 나쁜 entity의 max를 같은 정보와 함께 한 줄로 출력한다.
  - 장비 상태는 window 단위 값이다.
- `analyze --chain`은 이벤트 기록에서 그 chain을 다시 재어 `analysis/chains.csv`에 기록한다. 형식은 7절과 같고, 기록 시 chain을 지정하지 않았어도 된다.
- `export`는 `windows.jsonl`을 CSV로 변환하고 `meta.json`을 복사한다. `--metric`으로 `processes`, `threads`, `nodes`, `executors`, `callbacks`, `topics`, `subscriptions`, `flows`, `spans`, `chains`, `host`, `agent` 중에서 고르고, 생략하면 전부 내보낸다.

## 7. chain과 반응 시간

chain은 토픽의 순서다. 첫 토픽의 발행부터 뒤 토픽의 발행까지 걸린 시간을 단계마다 잰다.

```bash
orma record --max-duration 60 --chain perception=/camera/image,/detections,/tracks
orma analyze <directory> --chain reaction=/obstacle,/trajectory,/control_cmd:first
```

실시간으로는 rqt의 Chains tab에서 본다. CLI에는 chain 실시간 보기가 없다.

`orma path`는 두 노드 사이의 경로를 찾아 `--chain`에 바로 사용 가능한 토픽 순서로 보여 준다. 최근 window의 발행자와 구독자를 따라가며, 한 경로에서 같은 노드를 두 번 지나지 않는다.

```bash
orma path /camera_node /control_node
orma path /camera_node /control_node --via /tracker --max-nodes 8 --skip-node /debug --skip-topic /viz
```

```text
1. /camera/image,/detections,/tracks
   /camera_node -> /detector -> /tracker -> /control_node
2. /camera/image,/points,/grid
   /camera_node -> /depth -> /mapper -> /control_node
   2 node(s) with no measured link from input to output
```

- 입력에서 출력으로 이어진다는 flow가 측정되지 않은 노드 수를 함께 적고, 그 수가 적은 경로를 먼저 보여 준다. 같은 노드가 받고 내보내도 서로 무관할 가능성이 있으므로, 잴 경로는 사용자가 고른다.
- flow는 트래픽이 있어야 측정된다. 트래픽이 없던 경로는 후보로만 나온다.
- 경로는 100개까지 보여 준다. 더 있으면 그렇게 알리므로 `--via`나 `--skip-node`로 좁힌다.

chain을 잇는 방식은 둘이다.

- `lineage`(기본): 메시지마다 첫 토픽의 시각을 붙여 전달한다. 노드 안에서는 콜백 입력, stamp, 최신 입력 연결로 잇는다. stamp를 이어 주는 pipeline에 사용한다.
- `first`(`:first`): 첫 토픽의 발행마다 다음 토픽의 첫 발행을 차례로 찾는다. 다 끝나기 전에 첫 토픽이 다시 발행되면 앞 것은 broken이다. stamp를 잇지 않는 노드(planning 등)에 사용하고, 첫 토픽의 주기가 chain latency보다 길어야 한다.

측정 조건:

- chain의 토픽을 발행하는 프로세스가 모두 관측되어야 한다. 관측하지 않은 프로세스를 거치면 broken이 늘어난다.
- 각 단계는 첫 토픽부터 그 토픽의 발행까지다. 마지막 단계가 e2e다.

`--latest-node RE`:

- 입력이 여럿인 노드의 출력을 각 입력 토픽에서 가장 최근에 받은 메시지와 잇는다. RE는 노드 이름 전체에 맞춘다.
- stamp로 입력 하나가 정해지면 그 연결을 먼저 사용하고, stamp가 모호하거나 맞지 않는 경우에만 최신 입력으로 잇는다.
- 입력을 모았다가 나중에 처리하는 노드에서는 연결이 틀리므로, 고른 노드에만 사용한다.

### `orma react`

테스트 메시지를 입력 토픽에 발행하고, 출력 토픽마다 그 뒤 첫 발행까지의 시간을 잰다. orma가 토픽에 발행하는 명령은 이것뿐이다.

```bash
orma react --input /obstacle --type my_msgs/msg/Obstacle --msg '{x: 1.0, y: 2.0}' \
           --output /trajectory --output /control_cmd [--count N] [--period S] [-o out.csv]
orma react --input /points --type sensor_msgs/msg/PointCloud2 --bag rosbag/ --output /objects
```

| 옵션 | 뜻 | 기본 |
| --- | --- | --- |
| `--input TOPIC`, `--type PKG/msg/TYPE` | 발행할 토픽과 타입 | 필수 |
| `--msg YAML` | 메시지 내용(`ros2 topic pub`과 같은 형식). `header: auto`, `stamp: now`면 발행마다 그 시각을 넣는다 | `--bag`과 택일 |
| `--bag DIR` | 그 bag의 입력 토픽 메시지를 차례로 발행 | `--msg`와 택일 |
| `--output TOPIC` | 반응으로 볼 토픽. 여러 번 주면 그 순서로 단계가 된다 | 필수 |
| `--count N` | 발행 수 | `--msg`는 10, `--bag`은 그 토픽의 메시지 전부 |
| `--period S` | 발행 간격 | 1 s |
| `-o CSV` | 단계별 결과 CSV(`step`, `topic`, `n`, `p50_ns`, `p99_ns`, `max_ns`, `count`, `broken`) | 미출력 |

동작:

1. orma가 발행을 관측하도록 `orma run`과 같은 `preload` 환경으로 rclpy 발행 프로세스를 실행한다.
2. orma-collectd의 chain 설정에 `first` chain `react-<pid>`(`[입력, 출력...]`)를 추가한다. 선택이 모든 프로세스가 아니면 발행 프로세스를 선택에 추가하고, 선택이 없으면 모든 프로세스를 고른다.
3. 입력 토픽에 구독자가 연결되고(최대 10 s 대기) orma-collectd가 발행 프로세스를 보면 발행을 시작한다.
4. 발행이 끝나면 window 두 개를 더 받은 뒤 chain 설정과 선택을 되돌린다. Ctrl-C로 종료해도 되돌린다.

```text
react-52011 (first): /obstacle -> /trajectory -> /control_cmd, published 10
steps
  step  topic         n   p50     p99     max
  1     /trajectory   10  12.3ms  14.1ms  14.1ms
  2     /control_cmd  10  15.0ms  17.8ms  17.8ms
count 10, broken 0
note: n and max are exact; p50 is the n-weighted mean of the window p50s (an estimate), p99 the largest window p99 (an upper bound)
```

- 결과는 window별 chain 행을 합친 값이다. n과 max는 정확한 값이고, p99는 window별 p99의 최댓값이라 전체 p99의 상한이며, p50은 window별 p50을 n으로 가중 평균한 근삿값이다.
- 메시지 내용으로 반응을 판정하지 않고, 입력 뒤 그 토픽의 첫 발행을 반응으로 본다. 입력 토픽에 다른 발행자가 있으면 그 발행도 첫 토픽의 발행으로 센다.
- 출력 토픽을 발행하는 프로세스도 관측되어야 한다. 그 프로세스를 `preload`나 `lttng`로 실행하거나, 미리 `orma node cpu --pid PID`나 rqt로 고른다.
- `--period`는 chain latency보다 길게 지정한다. 짧으면 앞 발행이 broken이 된다.

## 8. span

콜백 안의 구간을 코드에 표시하면 `orma span`에 그 구간의 길이가 나온다. 사용자가 표시한 구간만 나온다.

- C, C++: 패키지 `orma_span`의 `liborma_span.so`를 링크하고 헤더 `orma_span/span.h`를 사용한다.
- Python: 같은 패키지의 decorator를 사용한다. `package.xml`에 `orma_span`을 `exec_depend`로 추가한다.

```c
#include <orma_span/span.h>

orma_span_begin("preprocess");
/* ... */
orma_span_end("preprocess");
```

```cpp
#include <orma_span/span.h>

void Node::on_image(const Image & msg)
{
  orma::Span s("preprocess");   // begin at construction, end at destruction
  // ...
}
```

```cmake
find_package(orma_span REQUIRED)
target_link_libraries(my_node orma_span::orma_span)
```

```python
from orma_span import span


@span("plan")          # one span per call
def plan(self, msg):
    ...
```

- orma가 주소로 begin과 end를 이으므로, 이름에는 정적 수명의 문자열(문자열 literal)을 넘긴다. 같은 이름의 begin과 end에는 같은 문자열을 넘긴다.
- orma가 부착되지 않은 프로세스에서는 빈 함수라 호출 비용만 든다. 활성화 옵션은 없다.
- `preload`와 `lttng`은 같은 이름의 함수로 가로채고, `uprobe`는 프로세스가 로드한 `liborma_span.so`에 부착한다. 세 방식 모두 같은 값을 낸다.
- 행은 같은 프로세스 안의 (이름, 이름 문자열 주소, 감싼 콜백, 감싼 span)마다 하나다. 같은 콜백 안 두 곳에 같은 이름을 쓰면 두 행이 되고, 이름 뒤에 주소 끝 여섯 자리(`step #a1b2c3`)가 추가된다. 짝 없는 end는 버린다.
- C, C++에서 같은 라이브러리 안의 같은 내용 문자열 literal은 compiler와 linker가 하나로 합칠 수 있고, 그러면 두 곳이 한 행으로 합쳐진다. 구별하려면 이름을 다르게 지정한다. Python decorator는 decorator마다 버퍼가 따로 있어 항상 구별된다.
- ROS가 아닌 프로세스도 `orma run`으로 실행하면 span이 나온다. 콜백 열은 `-`, 노드 열은 `unlinked`다.
- Python decorator는 함수 호출 한 번을 span 하나로 잰다. 함수 안의 일부 구간만 재려면 그 부분을 함수로 분리해 decorator를 적용한다. decorator는 이름을 한 번 버퍼에 담아 그 주소를 사용하므로 정적 문자열 제약이 없다.
- Python decorator는 orma가 부착되지 않아도 호출마다 약 0.3 us(ctypes 호출 두 번)가 든다. 1 kHz 이하로 실행되는 콜백 안의 함수에 사용한다.

## 9. /diagnostics

`orma_diagnostics` 노드는 orma-collectd의 클라이언트로 연결되어 window마다 `/diagnostics`(`diagnostic_msgs/DiagnosticArray`)를 발행한다. rqt_robot_monitor나 diagnostic_aggregator로 본다.

```bash
ros2 launch orma_diagnostics orma_diagnostics.launch.py rules:=/path/rules.yaml
ros2 run orma_diagnostics orma_diagnostics --ros-args -p rules:=/path/rules.yaml
```

규칙 파일은 규칙 이름마다 항목 목록이다. 예시는 패키지의 `config/rules.yaml`이다.

```yaml
callback_p99_us:
  - match: "sub\\(/scan\\)"
    warn: 5000
    error: 20000
topic_hz_min:
  - match: /scan
    warn: 9.0
    error: 5.0
```

| 규칙 | `match`가 매칭하는 이름 | 값 |
| --- | --- | --- |
| `callback_p99_us` | 콜백 이름(`sub(/scan)` 등, 4절) | 콜백 실행 시간 p99(us) |
| `topic_hz_min` | 토픽 이름 | 발행 Hz(작을수록 나쁨) |
| `topic_hop_p99_us` | 토픽 이름 | transport latency p99(us) |
| `sub_loss_pct` | 구독의 토픽 이름 | loss% |
| `chain_e2e_p99_us` | chain 이름(7절) | e2e p99(us) |
| `gpu_temp_c` | GPU 번호(`"0"`) | 온도(C) |
| `thermal_c` | thermal zone 이름 | 온도(C) |

- `match`는 이름 전체에 맞는 정규식이다. 맞는 대상마다 상태를 하나 낸다.
- `topic_hz_min`은 값이 `warn`보다 작으면 WARN, `error`보다 작으면 ERROR다. 나머지 규칙은 값이 `warn`보다 크면 WARN, `error`보다 크면 ERROR다.
- 맞는 대상이 window에 없으면 그 항목은 STALE 상태 하나를 낸다. 대상은 있는데 값이 `-`(미측정)여도 STALE이다.
- 상태 이름은 `orma <rule>: <target>`이고, 값(`value`, `warn`, `error`)은 key-value로 싣는다.
- 노드는 선택을 바꾸지 않는다. 볼 대상은 `orma node` 명령이나 rqt로 고르고, 대상 프로세스가 선택에 없으면 STALE이다.
- orma-collectd가 종료되면 노드도 종료된다. launch 파일은 2 s 뒤에 노드를 다시 실행한다.

## 10. rqt

```bash
rqt --standalone rqt_orma.plugin.Orma
```

- rqt 메뉴: Plugins > Introspection > orma
- CLI와 같은 orma-collectd에 연결되므로 선택, 기록, 상태를 공유한다.
- 플러그인을 닫아도 기록은 계속된다.
- Monitor tab의 Host tab과 Spans tab은 `orma top`, `orma span`과 같은 열을 보인다. GPU tab에는 프로세스별 CUDA 표도 있다. Processes tree에는 CUDA api, CUDA sync, futex 열이, Topics 표에는 DDS 열이, Threads 표에는 futex p99 열이 있다.
- Selection tab의 `gpu`, `dds`, `futex` 체크로 선택 수집을 활성화하고 비활성화한다. `Chains (--chain)` 칸은 CLI 형식의 chain을 공백으로 나눠 받고, 비우면 chain을 지운다.
- 기록 시작 form에는 `--gpu`, `--dds`, `--futex` 체크와 `--chain`, `--latest-node` 칸이 있다(칸은 공백으로 구분).

## 11. ormad 단독

orma-collectd 없이 콜백 표만 보는 경우에 사용한다. 서비스와 별개로 실행된다.

```bash
sudo install/orma_daemon/bin/ormad --pid PID [--interval 1]
```
