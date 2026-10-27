# 사용법

설치는 [install.md](install.md), 문제 해결은 [troubleshooting.md](troubleshooting.md)에 있다. 모든 명령은 workspace를 source한 shell에서 실행한다.

## 1. 상태 확인

```bash
orma status
orma check
```

`orma status`의 둘째 줄이 source 상태다.

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
| `ormad: absent` | ormad가 돌지 않는다. `preload`, `lttng`만 동작한다 |
| `ormad: denied` | 사용자가 group `orma`에 없다. 다시 로그인한다([install.md](install.md) 2절) |
| `ormad: lost` | 연결이 끊겼다. 다시 연결되면 같은 요청을 다시 보낸다 |
| `lttng: absent` | PATH에 `lttng`가 없다 |
| `preload processes` | `preload`나 `lttng` method로 실행한 process 수 |
| `executor probes` | ROS process마다 executor에 붙는 uprobe. `stock`: `rcl_wait`와 rclcpp executor tracepoint. `rcl_executor`: `stock`에 overhaul core의 anchor를 더한 것([instrumentation.md](instrumentation.md) 2.12) |
| `delivery probes` | ROS process마다 붙는 전달 경로 probe. `broker`, `loan`, `zenoh_shm`, `rclpy`([instrumentation.md](instrumentation.md) 2.13) |

`orma check`:

- 이 장비에서 orma의 가정이 맞는지 검사한다(기본 10초, `--duration S`).
- 일부 항목은 검사 중에 node를 새로 실행해야 나온다.
- 행마다 `pass`, `fail`, `unknown` 중 하나가 나온다. `fail`이 하나라도 있으면 exit code는 1이다.
- `unknown`은 판단할 근거가 부족하다는 뜻이다. 이유는 옆에 출력한다.
- ormad가 있을 때 더해지는 행은 [troubleshooting.md](troubleshooting.md)의 `orma check` 항목에 설명이 있다.

## 2. method 선택

세 method는 같은 함수를 보고 같은 형식으로 낸다. 부착 방법과 비용만 다르다.

| method | 부착 시점 | 관측 대상 비용(1 kHz C++ chain, p99) | 용도 |
| --- | --- | --- | --- |
| `preload` | process 시작 시 `LD_PRELOAD` | +0-5 us | 실시간 시스템의 기본 |
| `lttng` | process 시작 시 `LD_PRELOAD`. session은 orma-collectd가 관리 | +10-22 us, 코어의 7-11% | ros2_tracing 도구로 읽을 CTF가 필요할 때 |
| `uprobe` | 실행 중인 process에 즉시 | +30-80 us, 코어의 14-30% | 재시작할 수 없는 process를 잠깐 볼 때 |

- 비용은 Orin, 6.8 RT kernel에서 잰 값이다. 측정 조건은 [README](../README.md)의 실시간 영향 절에 있다.
- uprobe를 쓰면 같은 함수를 부르는 thread끼리 kernel 안에서 직렬화된다. 1 kHz 이상의 RT 부하에는 `preload`를 쓴다.
- uprobe는 probe 하나를 붙이는 데 약 0.25 s, 떼는 데 약 1.1 s가 걸린다. 대상을 고른 뒤 첫 값이 나오기까지 수 초가 걸린다.
- callback 안 CPU와 scheduling latency(`sched`)를 재려면 method와 상관없이 ormad가 있어야 한다.
- malloc attribution은 `preload`에서만 된다.

추가 수집은 켜야 동작한다.

| 수집 | 켜는 옵션 | `preload` | `lttng` | `uprobe` | 나오는 곳 |
| --- | --- | --- | --- | --- | --- |
| CUDA driver API 호출 | `--gpu` | O | O | O | `orma node cb`의 CUDA 열, rqt의 GPU tab |
| DDS write 호출 | `--dds` | O | O | O | `orma topic delay`의 DDS 열 |
| futex 대기 | `--futex` | ormad | ormad | ormad | `orma node cb`의 futex 열, rqt의 Threads tab |
| span(`liborma_span`) | 항상 | O | O | O | `orma span`(8절) |
| 장비 상태(clock, 온도, network, GPU) | 항상 | method와 무관 | method와 무관 | method와 무관 | `orma top` |

- futex 대기는 method와 상관없이 ormad가 kernel tracepoint로 잰다. ormad가 없으면 열이 `-`로 나온다.
- CUDA 열은 `libcuda.so.1` 호출에 걸린 시간이다. GPU 안의 kernel 실행 시간은 재지 않는다. 비동기 launch는 바로 반환하고, 기다린 시간은 sync 열과 동기 copy에 나타난다.

## 3. preload, lttng method로 실행

```bash
orma run -- ros2 run demo_nodes_cpp talker               # preload (default)
orma run --method lttng -- ros2 launch my_pkg my.launch.py
eval "$(orma env)"                                       # into this shell
```

`orma run`과 `orma env`가 설정하는 환경변수:

| 변수 | 값 |
| --- | --- |
| `LD_PRELOAD` | `liborma_preload.so` 또는 `liborma_lttng.so`. 기존 값은 뒤에 남는다 |
| `PYTHONPATH` | `orma_rclpy`를 자동으로 load한다. 이 module이 rclpy가 부르지 않는 trace 함수를 부른다 |
| `LTTNG_HOME` | `lttng` 전용. `$XDG_RUNTIME_DIR/orma/lttng`(orma-collectd의 session daemon 위치) |

launch 파일에서는 `OrmaTrace`를 node보다 앞에 둔다. 그 뒤에 실행되는 process만 환경변수를 받는다.

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
| `method` | `preload`(기본), `lttng`, `uprobe`. `uprobe`는 환경변수 없이 recording만 시작 |
| `record` | 참이면 launch가 시작할 때 recording을 시작한다. 기본값은 거짓 |
| `stop_on_shutdown` | launch가 끝날 때 recording을 멈춘다. 기본값은 참. 거짓이면 recording이 한도까지 이어진다 |
| 그 밖 | 5절 recording 옵션의 field 이름(`kind`, `dir`, `name`, `max_duration_s`, `max_bytes`, `split_bytes`, `split_duration_s`, `cache_bytes`, `compression`, `bandwidth`, `sched`, `malloc`, `gpu`, `dds`, `futex`, `chains`, `latest_nodes`). `chains`는 `{"name": ..., "topics": [...], "mode": "lineage"}`의 목록 |

## 4. monitoring

`ros2 node`, `ros2 topic`처럼 대상을 이름으로 지정한다. 실시간 명령은 window(기본 1초)마다 표를 출력하고 Ctrl-C로 끝난다.

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

| 명령 | 열 |
| --- | --- |
| `orma top` | CPU clock(최소, 평균, 최대 MHz와 CPU 수), thermal zone 온도, throttle counter 증가분(x86), interface별 rx·tx byte rate, packet rate, drop, error, UDP error counter 증가분, `gpus` 표(번호, 이름, 사용률, memory(사용/전체), SM clock, 온도, 전력, clock 제한 이유) |
| `orma node cpu` | node 이름, pid, CPU%, CPU source, busy%(wall time), callback Hz, RSS. RSS는 process 단위 값이라 한 process에 node가 여럿이면 `(shared)`로 표시한다. 표 아래에 process 경고(표 넘침, flux GPU wedged)가 나온다 |
| `orma node cb` | callback, node, pid, Hz, expected Hz, callback duration p50, p90, p99, max, dispatch latency p99. `--gpu`면 `cuda api`(실행 한 번당 CUDA 호출 시간), `cuda sync`(그중 synchronize 시간), `--futex`면 `futex`(실행 한 번당 대기 시간) 열이 더해진다. CB를 주면 이름에 CB가 들어간 callback만 보인다 |
| `orma node sub` | (topic, subscription node)마다 한 행: topic, node, pid, take Hz, loss%, loss 수, wakeup latency p50, p90, p99, max. TOPIC을 주면 그 topic만 보인다 |
| `orma span` | span 이름, 감싼 span, 감싼 callback, node, pid, Hz, 길이 p50, p90, p99, max. NAME을 주면 그 이름만 보인다(8절) |

```bash
orma node cpu
orma node cb /talker
orma node cb /camera/driver 'sub(/image)'
orma node sub /listener /chatter
```

| 옵션 | 뜻 |
| --- | --- |
| `--pid PID` | orma보다 먼저 떠 있던 process를 고른다. ormad가 uprobe를 붙인다 |
| `--method M` | 한 번 주면 모든 pid에 적용한다. pid마다 주면 순서대로 짝짓는다 |
| `--window S` | window 길이(초). 기본 1초 |
| `--infra` | 숨긴 infrastructure topic과 service 표시 |

- 이름은 정규식이 아니라 문자열 그대로 비교한다. node 이름은 `ros2 node list`와 같은 `/ns/name` 형식이다.
- node를 지정하면 그 node가 든 process를 고른다. 비용은 process 단위로 든다. container 안의 node 하나만 골라도 uprobe는 process 전체에 붙는다.
- node를 지정하지 않는 명령(`node list`, `node cpu`, `span`)은 모든 process를 보이고 uprobe는 붙이지 않는다. 값은 `orma run`(`preload`, `lttng`)으로 실행한 process에서만 나온다. 시작할 때 `[info] select_all`을 출력한다. 보일 것이 없으면 window 줄 아래에 `none`이 나온다.
- orma보다 먼저 떠 있던 process는 orma가 node 이름을 몰라 이름으로 고를 수 없다. `--pid`로 고르면 callback이 돈 뒤 수 초 안에 이름이 붙는다(아래 `unlinked`).
- process 단위 값(process CPU, PSS)은 CLI에 없다. rqt의 Processes tree와 recording(`orma export --metric processes`)에 있다. PSS는 rqt의 `pss` 체크를 켰을 때만 5초마다 `smaps_rollup`으로 읽는다. kernel은 이 값을 만들 때 대상의 mmap_lock을 쥔 채 대상의 page table 전체를 훑는다. 그동안 대상의 page fault와 mmap은 기다린다.
- `--gpu`, `--futex`(`node cb`), `--dds`(`topic delay`)는 2절의 추가 수집을 켠다. 끄려면 rqt의 Selection tab을 쓴다.
- 선택, window 길이, 추가 수집 설정은 orma-collectd에 하나씩만 있다. 다른 terminal이나 rqt도 같은 설정을 쓴다.
- client가 하나도 없으면 orma-collectd는 hot probe를 뗀다.
- tab 완성은 workspace를 source하면 켜진다(bash, zsh, argcomplete). 명령과 옵션 말고도 node, topic, node의 callback과 subscription topic, span 이름, recording id를 완성한다. 이름은 orma-collectd의 다음 window에서 읽으므로 최대 window 길이만큼 걸린다. orma-collectd가 없으면 이름 후보가 나오지 않는다.

```text
callbacks
  callback       node       pid     Hz   expected  p50      p90      p99      max      dispatch p99
  sub(/chatter)  /listener  393740  1.0  -         69.1us   69.1us   69.1us   69.1us   88.4us
  timer Talker   /talker    393739  1.0  1.0       185.8us  185.8us  185.8us  185.8us  30.9us
```

callback 이름:

| 형식 | 뜻 |
| --- | --- |
| `sub(/chatter)` | subscription callback. 괄호 안은 topic |
| `srv(/add)` | service callback |
| `timer cb_fn` | timer callback. 괄호에 넣을 대상이 없어 callback을 정의한 함수 이름을 쓴다. 인자 목록, template 인자, namespace, lambda 표시는 뺀다 |
| `timer Talker` | `Talker` class 생성자 안의 lambda. rclpy의 `Talker.__init__.<locals>.<lambda>`도 같다 |
| `timer Talker.on_timer` | rclpy method. Python의 `__qualname__` |
| `flux(/image)` | flux channel callback |
| `<timer 0xaaaacbc8a330>` | 함수 이름을 알아내지 못했다. 숫자는 callback 객체 주소 |
| `... #a7e448` | 한 node 안에 같은 이름이 둘 이상 있다. 주소 끝 여섯 자리로 구분한다 |

`unlinked`:

- orma가 node를 모르는 callback과 subscription이다. node 열이 `unlinked`이고 표 맨 아래에 나온다.
- `unlinked (cold)`: orma가 늦게 붙어 node 생성을 보지 못한 경우다.
- uprobe로 보는 rclcpp process에서는 orma가 executor에서 node를 읽어 보통 2-3초 안에 채운다. 그동안 callback마다 uprobe 하나(약 1.25 us)가 더 불린다. 10초 안에 못 찾으면 포기한다.
- 채워지지 않는 것: rclpy node, Humble의 `StaticSingleThreadedExecutor`와 `EventsExecutor`, intra-process 전용 subscription, timer만 있는 callback group의 timer. 늦게 붙은 service는 `srv`와 함수 이름만 보인다.
- 처음부터 보려면 `orma node cpu`나 rqt를 먼저 열고 node를 실행한다. client가 붙어 있는 동안은 모든 process에 init probe가 붙어 있다.

표시 규칙:

- 숨기는 것: `/rosout`, `/parameter_events`, `ros_discovery_info`, rclcpp가 node마다 만드는 parameter service(`get_parameters` 등)와 그 callback. `--infra`를 주면 보인다.
- latency 열 순서: p50, p90, p99, max. window 값의 p99.9는 recording과 rqt에만 있다.
- `-`: 미측정. `0`: 측정값 0
- CPU source `sched`: ormad가 잰 on-CPU. `wall_bound`: sched 없이 callback wall time으로 낸 상한
- window 줄 끝의 `behind`: orma-collectd가 window 하나를 window 길이 안에 모으지 못했다는 뜻이다.

`orma status` 마지막 줄은 orma 자체 비용이다.

```text
orma: collectd cpu 1.0% rss 6.9MiB, ormad cpu 0.4% rss 22.5MiB, shm files 0 ring drops 0, probes missing 6, collect 1.7ms
```

`probes missing`은 붙지 않은 point 수다. 그 함수가 없는 library의 point도 센다. 이유는 `orma check`에 나온다.

`preload` process의 표가 넘치면 `orma node cpu` 표 아래에 경고가 나온다.

```text
warning: pid 4021 (camera_node): 3 samples not aggregated (thread table full)
```

| 경고 | 뜻 |
| --- | --- |
| `threads not traced (thread slots full)` | process의 traced thread 256개 초과 |
| `samples not aggregated (thread table full)` | 한 thread의 callback 종류 63개 초과(malloc attribution을 켜면 31개) |
| `callbacks not timed (nested deeper than 16)` | callback 중첩 16단 초과 |

flux GPU가 wedged인 process에는 `CUresult`와 함께 경고 한 줄이 붙는다. 그 process에서는 GPU channel의 claim, loan, take가 실패하고, orma는 이를 topic의 publish drop으로 센다.

`orma top`:

- orma-collectd가 window마다 kernel counter와 NVML(Jetson은 sysfs)에서 읽는다.
- NVML이나 GPU가 없으면 `gpus` 표만 비고 나머지는 그대로 나온다.
- `--json`을 주면 window message 전체를 한 줄에 하나씩 출력한다. field는 `common/orma_proto/schema/v1.json`의 `window`에 있다.

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
| `list` | 관측 중인 process의 topic 이름. window 하나를 받고 종료 |
| `info` | transport(`ros`, `flux`), publisher node, subscription node. window 하나를 받고 종료 |
| `hz` | publish Hz, expected Hz, publish drop 수 |
| `bw` | publish byte rate |
| `delay` | transport latency(publish에서 take까지) p50, p90, p99, max. `--dds`면 `pub to dds p99`(`rclcpp_publish`에서 DDS write 시작까지), `dds write p99`(write 호출 시간) 열 추가 |
| `path` | 표 셋. `paths`: 전달 경로(`broker`, `loan`, `shm`, `net`, `unknown`)별 take Hz와 transport latency. `publishers`: loan을 빌려서 publish까지(`loan hold p99`), rclpy publish의 변환(`py convert p99`). `subscriptions`: subscription마다 받은 경로, loan take 수, take에서 callback 시작까지 p50과 p99, rclpy 변환 p99, broker message나 loan을 잡은 시간 p99([instrumentation.md](instrumentation.md) 2.13) |

- orma는 `ros2 topic`과 달리 subscriber를 만들어 받는 쪽에서 세지 않는다. publisher와 subscriber process 안에서 잰다. 그래서 DDS 전송이 새로 생기지 않고, `hz`에 network loss가 섞이지 않는다. 받은 수는 `orma node sub`의 take Hz로 본다.
- `delay`는 `ros2 topic delay`와 달리 header stamp를 쓰지 않는다. 같은 message의 publish 시각과 take 시각의 차이다.
- topic 이름은 그대로 비교한다. 명령은 그 topic의 publisher와 subscriber process를 고른다. `list`는 모든 process를 고른다.
- 관측하지 않는 publisher의 값은 `-`다. 첫 window는 window 도중에 대상을 골라서 `-`일 수 있다.

## 5. recording

```bash
orma record [--pid PID]... [--method M] [--kind events|windows] [options]
```

- foreground에서는 window마다 상태를 한 줄 출력한다. Ctrl-C를 누르면 recording을 멈추고, 마무리한 뒤 검증 표를 출력한다.
- terminal을 닫거나 client가 죽어도 recording은 한도까지 이어진다. recording은 orma-collectd가 갖고 있기 때문이다.
- `--no-follow`를 주면 시작한 뒤 바로 반환한다.

| 옵션 | 뜻 | 기본 |
| --- | --- | --- |
| `--all`, `--pid PID`..., `--node RE`, `--topic RE`, `--exclude RE` | 대상 | `--all` |
| `--method auto\|uprobe\|preload\|lttng` | `auto`는 process마다 `preload`, `lttng`, `uprobe` 순 | `auto` |
| `--kind events\|windows` | event 전부 또는 window snapshot만 | `events` |
| `-o DIR`, `--name NAME` | 위치와 이름 | `~/.local/share/orma/records/orma-<start time>` |
| `--max-duration S` | 길이 한도 | events 30분, windows 24시간 |
| `--max-size BYTES` | 크기 한도 | events 8 GiB, windows 1 GiB |
| `--split-size BYTES`, `--split-duration S` | event 파일 분할 | 1 GiB, 미분할 |
| `--cache BYTES` | write buffer | 16 MiB |
| `--compression none\|zstd` | event 파일 압축 | `none` |
| `--bandwidth` | DDS serialized size(uprobe) | 끔 |
| `--sched`, `--no-sched` | callback 안 CPU와 scheduling latency | ormad가 있으면 켬 |
| `--malloc` | callback별 할당(`preload`) | 끔 |
| `--gpu`, `--dds`, `--futex` | 2절의 추가 수집 | 끔 |
| `--chain NAME=T1,T2,...[:first]` | 잴 chain(7절). 여러 번 지정 가능. `meta.json`에 기록 | 없음 |
| `--latest-node RE` | 최신 입력으로 잇는 node(7절). 여러 번 지정 가능 | 없음 |

- 한도는 반드시 준다. 0이나 무한은 받지 않는다. 길이의 상한은 `~/.config/orma/collectd.toml`의 `max_duration_s`다(기본 7일).
- 디스크 여유 공간이 하한(기본 max(2 GiB, 파일시스템의 5%), `disk_floor_bytes`) 아래로 내려가면 recording은 `disk_floor`로 끝난다. 시작할 때 여유 공간이 하한 + 1 GiB보다 적으면 시작하지 않는다.
- recording은 동시에 4개까지 돌릴 수 있다.
- `--node`, `--topic`, `--exclude`는 이름 전체에 맞는 정규식이다. rqt Selection tab의 같은 칸과 뜻이 같다. recording 중에 새로 뜬 node도 이름이 맞으면 들어간다. 한 번 들어간 process는 끝까지 남는다.

```bash
orma record list                       # active and recent recordings
orma record stop <id>
orma record show <id or directory>     # verification table: events, loss, Hz and warnings per entity
```

recording directory:

```text
orma-20261008-153012/
  meta.json        targets, method, options, attached libraries, loss, gaps, end reason
  windows.jsonl    one line per window
  events/000.ormaev
  ctf/             lttng method only
```

`meta.json`:

- `state: truncated`: 마무리 전에 orma-collectd가 죽은 recording이다. orma-collectd가 다음에 시작할 때 개수를 다시 세어 복구한다.
- `probes_missing`: 붙지 않은 point 목록이다. 빈 열이 "일이 없었다"인지 "볼 수 없었다"인지 가리는 데 쓴다.
- `gaps`: ormad 연결이 끊긴 구간처럼 값이 빈 구간이다.
- `system_start`, `system_end`: 시작과 끝의 장비 상태. kernel 버전과 설정(`HZ`, preempt), cmdline, clocksource, `clock_gettime` 한 번의 비용, scheduler와 sched_ext, 격리 CPU, cpufreq governor와 EPP, 끈 idle state, THP, 취약점 완화, lockdown, 가상화, 시계 동기 상태, 대상 process의 Seccomp와 `cpu.max`. root만 읽을 수 있는 값(debugfs)은 빠진다. 다른 장비나 다른 날의 값과 비교하기 전에 이 상태부터 맞춘다.
- `interference`: recording 시작부터 끝까지 늘어난 kernel counter다. CPU별 interrupt(`LOC`, `RES`, `CAL`, `TLB`, device interrupt 합 `dev`)와 softirq, vmstat의 reclaim·compaction·major fault·TLB flush, PSI, steal, 대상 thread별 minor·major fault와 실행·대기 시간을 담는다.
- `realtime_steps`: recording 중에 CLOCK_REALTIME이 점프한 시각이다. 그 근처의 `header.stamp` 나이와 timestamp matching은 믿지 않는다.
- `probe_changes`: ormad가 probe를 붙이거나 뗀 구간이다. 그동안 kernel이 코드를 고치고 TLB를 비운다. 그래서 그 구간의 tail은 orma가 만든 것으로 본다.

## 6. 분석과 내보내기

```bash
orma analyze <directory> [--chain NAME=T1,T2,...[:first]]...
orma export <directory> -o out.csv [--metric M]...
```

- `analyze`는 event recording을 transport, wakeup, dispatch, callback segment로 나눈다. 그리고 message별, callback별 표와 tail 요약(p50, p90, p99, max)을 출력한다. CSV는 `<directory>/analysis/`에 쓴다. `summary.csv`에는 p99.9도 있다(`n,p50_ns,p90_ns,p99_ns,p999_ns,max_ns`).
- message별 표(`deliveries.csv`)에는 전달 경로(`path`), take에서 callback 시작까지(`deliver_ns`), rclpy 변환(`py_convert_ns`) 열이 있다. 요약에는 경로별 transport latency, take -> callback start, rclpy 변환, hold 표가 있다([instrumentation.md](instrumentation.md) 2.13).
- `--gpu`, `--futex`로 기록했으면 `callbacks.csv`에 열이 더해진다. callback 한 번마다 CUDA driver 호출 시간(`cuda_ns`), 그중 synchronize 시간(`cuda_sync_ns`), futex 대기(`futex_ns`)다. 요약에도 같은 tail 표가 더해진다.
- `--dds`로 기록했으면 `publishes.csv`에 publish마다 publish에서 DDS write 시작까지 걸린 시간과 write 시간을 쓴다.
- window에 장비 상태(`host`)가 있으면 `tails.csv`도 쓴다. 표와 entity마다 p99 이상인 sample(최대 10개)을 골라, 그 sample이 나온 window의 장비 상태를 붙인다. 열은 그 thread가 돈 CPU의 최저 clock(thread를 모르면 전체 CPU), 가장 뜨거운 thermal zone, throttle 횟수, GPU별 clock, 온도, clock 제한 이유다. 요약 끝의 `tail context`는 표마다 가장 나쁜 entity의 max를 같은 정보와 함께 한 줄로 출력한다. 장비 상태는 window 단위 값이다.
- `analyze --chain`은 event recording에서 그 chain을 다시 재어 `analysis/chains.csv`에 쓴다. 형식은 7절과 같다. recording 때 chain을 주지 않았어도 된다.
- `export`는 `windows.jsonl`을 CSV로 바꾸고 `meta.json`을 복사한다. `--metric`으로 `processes`, `threads`, `nodes`, `executors`, `callbacks`, `topics`, `subscriptions`, `flows`, `spans`, `chains`, `host`, `agent` 중에서 고른다. 생략하면 전부 내보낸다.

## 7. chain과 reaction time

chain은 topic의 순서다. 원천 topic의 publish부터 뒤 topic의 publish까지 걸린 시간을 단계마다 잰다.

```bash
orma record --max-duration 60 --chain perception=/camera/image,/detections,/tracks
orma analyze <directory> --chain reaction=/obstacle,/trajectory,/control_cmd:first
```

실시간으로는 rqt의 Chains tab에서 본다. CLI에는 chain 실시간 보기가 없다.

| 방식 | 잇는 법 | 용도 |
| --- | --- | --- |
| `lineage`(기본) | message마다 원천 시각을 붙여 옮긴다. node 안에서는 callback 입력, stamp, 최신 입력 연결로 잇는다 | stamp를 이어 주는 pipeline |
| `first`(`:first`) | 원천의 publish마다 다음 topic의 첫 publish를 차례로 찾는다. 다 끝나기 전에 원천이 다시 publish하면 앞 것은 broken | stamp를 잇지 않는 node(planning 등). 원천 주기가 chain latency보다 길어야 한다 |

- chain의 topic을 publish하는 process가 모두 관측돼야 한다. 관측하지 않은 process를 거치면 broken이 늘어난다.
- 각 단계는 원천부터 그 topic의 publish까지다. 마지막 단계가 e2e다.

`--latest-node RE`:

- 입력이 여럿인 node의 출력을 각 입력 topic에서 가장 최근에 받은 message와 잇는다. RE는 node 이름 전체에 맞춘다.
- stamp로 입력 하나가 정해지면 그 연결을 먼저 쓴다. stamp가 모호하거나 맞지 않을 때만 최신 입력으로 잇는다.
- 입력을 모았다가 나중에 처리하는 node에서는 연결이 틀린다. 그래서 고른 node에만 쓴다.

### `orma react`

test message를 입력 topic에 publish하고, 출력 topic마다 그 뒤 첫 publish까지의 시간을 잰다. orma가 topic에 publish하는 명령은 이것뿐이다.

```bash
orma react --input /obstacle --type my_msgs/msg/Obstacle --msg '{x: 1.0, y: 2.0}' \
           --output /trajectory --output /control_cmd [--count N] [--period S] [-o out.csv]
orma react --input /points --type sensor_msgs/msg/PointCloud2 --bag rosbag/ --output /objects
```

| 옵션 | 뜻 | 기본 |
| --- | --- | --- |
| `--input TOPIC`, `--type PKG/msg/TYPE` | publish할 topic과 type | 필수 |
| `--msg YAML` | message 내용. `ros2 topic pub`과 같은 형식. `header: auto`, `stamp: now`면 publish마다 그 시각을 넣는다 | `--bag`과 택일 |
| `--bag DIR` | 그 bag의 입력 topic message를 차례로 publish | `--msg`와 택일 |
| `--output TOPIC` | reaction으로 볼 topic. 여러 번 주면 그 순서로 단계 | 필수 |
| `--count N` | publish 수 | `--msg`는 10, `--bag`은 그 topic의 message 전부 |
| `--period S` | publish 간격 | 1초 |
| `-o CSV` | 단계별 결과 CSV(`step`, `topic`, `n`, `p50_ns`, `p99_ns`, `max_ns`, `count`, `broken`) | 미출력 |

동작:

1. `orma run`과 같은 `preload` 환경으로 rclpy publish process를 실행한다. orma가 그 publish를 보기 위해서다.
2. orma-collectd의 chain 설정에 `first` chain `react-<pid>`(`[입력, 출력...]`)를 더한다. 선택이 모든 process가 아니면 publish process를 선택에 더한다. 선택이 없으면 모든 process를 고른다.
3. 입력 topic에 subscriber가 붙고(최대 10초 기다린다) orma-collectd가 publish process를 보면 publish를 시작한다.
4. publish가 끝나면 window 두 개를 더 받은 뒤 chain 설정과 선택을 되돌린다. Ctrl-C로 끝내도 되돌린다.

```text
react-52011 (first): /obstacle -> /trajectory -> /control_cmd, published 10
steps
  step  topic         n   p50     p99     max
  1     /trajectory   10  12.3ms  14.1ms  14.1ms
  2     /control_cmd  10  15.0ms  17.8ms  17.8ms
count 10, broken 0
note: n and max are exact; p50 is the n-weighted mean of the window p50s (an estimate), p99 the largest window p99 (an upper bound)
```

- 결과는 window별 chain 행을 합친 것이다. n과 max는 정확한 값이다. p99는 window별 p99의 최댓값이므로 전체 p99의 상한이다. p50은 window별 p50을 n으로 가중 평균한 근삿값이다.
- message 내용으로 reaction을 판정하지 않는다. 입력 뒤 그 topic의 첫 publish를 reaction으로 본다. 입력 topic에 다른 publisher가 있으면 그 publish도 원천으로 센다.
- 출력 topic을 publish하는 process도 관측돼야 한다. 그 process를 `preload`나 `lttng`로 실행하거나, 미리 `orma node cpu --pid PID`나 rqt로 고른다.
- `--period`는 chain latency보다 길게 준다. 짧으면 앞 publish가 broken이 된다.

## 8. span

callback 안의 구간을 코드에 표시하면 `orma span`에 그 구간의 길이가 나온다. orma가 정하는 구간은 없고 사용자가 표시한 구간만 나온다.

- C, C++: package `orma_span`의 `liborma_span.so`를 링크하고 header `orma_span/span.h`를 쓴다.
- Python: 같은 package의 decorator를 쓴다. `package.xml`에 `orma_span`을 `exec_depend`로 더한다.

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

- 이름에는 정적 수명의 문자열(문자열 literal)을 넘긴다. orma가 주소로 begin과 end를 잇기 때문이다. 같은 이름의 begin과 end에는 같은 문자열을 넘긴다.
- orma가 붙지 않은 process에서는 빈 함수라 호출 비용만 든다. 켜는 옵션은 없다.
- `preload`와 `lttng`은 같은 이름의 함수로 가로채고, `uprobe`는 process가 load한 `liborma_span.so`에 붙는다. 세 method 모두 같은 값을 낸다.
- 행은 같은 process 안의 (이름, 이름 문자열 주소, 감싼 callback, 감싼 span)마다 하나다. 같은 callback 안 두 곳에 같은 이름을 쓰면 두 행이 되고, 이름 뒤에 주소 끝 여섯 자리(`step #a1b2c3`)가 붙는다. 짝 없는 end는 버린다.
- C, C++에서 같은 library 안의 같은 내용 문자열 literal은 compiler와 linker가 하나로 합칠 수 있다. 그러면 두 곳이 한 행으로 합쳐진다. 구별하려면 이름을 다르게 준다. Python decorator는 decorator마다 buffer가 따로라 항상 구별된다.
- ROS가 아닌 process도 `orma run`으로 실행하면 span이 나온다. callback 열은 `-`, node 열은 `unlinked`다.
- Python decorator는 함수 호출 한 번을 span 하나로 잰다. 함수 안의 일부 구간만 재려면 그 부분을 함수로 떼어 decorator를 붙인다. decorator는 이름을 한 번 buffer에 담아 그 주소를 쓰므로, 정적 문자열이어야 한다는 제약이 없다.
- Python decorator는 orma가 붙지 않아도 호출마다 약 0.3 us(ctypes 호출 두 번)가 든다. 1 kHz 이하로 도는 callback 안의 함수에 쓴다.

## 9. /diagnostics

`orma_diagnostics` node는 orma-collectd의 client로 붙어 window마다 `/diagnostics`(`diagnostic_msgs/DiagnosticArray`)를 publish한다. rqt_robot_monitor나 diagnostic_aggregator로 본다.

```bash
ros2 launch orma_diagnostics orma_diagnostics.launch.py rules:=/path/rules.yaml
ros2 run orma_diagnostics orma_diagnostics --ros-args -p rules:=/path/rules.yaml
```

규칙 파일은 규칙 이름마다 항목 목록이다. 예시는 package의 `config/rules.yaml`이다.

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
| `callback_p99_us` | callback 이름(`sub(/scan)` 등, 4절) | callback duration p99(us) |
| `topic_hz_min` | topic 이름 | publish Hz. 작을수록 나쁨 |
| `topic_hop_p99_us` | topic 이름 | transport latency p99(us) |
| `sub_loss_pct` | subscription의 topic 이름 | loss% |
| `chain_e2e_p99_us` | chain 이름(7절) | e2e p99(us) |
| `gpu_temp_c` | GPU 번호(`"0"`) | 온도(C) |
| `thermal_c` | thermal zone 이름 | 온도(C) |

- `match`는 이름 전체에 맞는 정규식이다. 맞는 대상마다 상태를 하나 낸다.
- `topic_hz_min`은 값이 `warn`보다 작으면 WARN, `error`보다 작으면 ERROR다. 나머지 규칙은 값이 `warn`보다 크면 WARN, `error`보다 크면 ERROR다.
- 맞는 대상이 window에 없으면 그 항목은 STALE 상태 하나를 낸다. 대상은 있는데 값이 `-`(미측정)여도 STALE이다.
- 상태 이름은 `orma <rule>: <target>`이다. 값(`value`, `warn`, `error`)은 key-value로 싣는다.
- node는 선택을 바꾸지 않는다. 볼 대상은 `orma node` 명령이나 rqt로 고른다. 대상 process가 선택에 없으면 STALE이다.
- orma-collectd가 끝나면 node도 끝난다. launch 파일은 2초 뒤에 node를 다시 실행한다.

## 10. rqt

```bash
rqt --standalone rqt_orma.plugin.Orma
```

- rqt 메뉴: Plugins > Introspection > orma
- CLI와 같은 orma-collectd에 붙으므로 선택, recording, 상태를 공유한다.
- plugin을 닫아도 recording은 이어진다.
- Monitor tab의 Host tab과 Spans tab은 `orma top`, `orma span`과 같은 열을 보인다. GPU tab에는 process별 CUDA 표도 있다. Processes tree에는 CUDA api, CUDA sync, futex 열이, Topics 표에는 DDS 열이, Threads 표에는 futex p99 열이 있다.
- Selection tab의 `gpu`, `dds`, `futex` 체크로 추가 수집을 켜고 끈다. `Chains (--chain)` 칸은 CLI 형식의 chain을 공백으로 나눠 받는다. 비우면 chain을 지운다.
- recording 시작 form에는 `--gpu`, `--dds`, `--futex` 체크와 `--chain`, `--latest-node` 칸이 있다(칸은 공백으로 구분).

## 11. ormad 단독

orma-collectd 없이 callback 표만 볼 때 쓴다. service와 별개로 실행된다.

```bash
sudo install/orma_daemon/bin/ormad --pid PID [--interval 1]
```
