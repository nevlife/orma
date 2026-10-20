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
```

| 값 | 뜻 |
| --- | --- |
| `ormad: connected` | uprobe, sched, eBPF 비용 사용 가능 |
| `ormad: absent` | ormad 미실행. `preload`, `lttng`만 동작 |
| `ormad: denied` | group `orma` 미포함. 다시 로그인([install.md](install.md) 2절) |
| `ormad: lost` | 연결이 끊긴 상태. 다시 연결되면 같은 요청 재전송 |
| `lttng: absent` | PATH에 `lttng` 부재 |
| `preload processes` | `preload`나 `lttng` method로 실행한 process 수 |

`orma check`:

- 이 장비에서 orma의 가정이 맞는지 검사(기본 10초, `--duration S`)
- 일부 항목은 검사 중 node를 새로 실행해야 나온다
- 행마다 `pass`, `fail`, `unknown`. `fail`이 하나라도 있으면 exit code 1
- `unknown`: 판단 근거 부족. 이유는 옆에 출력
- ormad가 있을 때 추가되는 행은 [troubleshooting.md](troubleshooting.md)의 `orma check` 항목

## 2. method 선택

세 method는 같은 함수를 보고 같은 형식으로 낸다. 부착 방법과 비용만 다르다.

| method | 부착 시점 | 관측 대상 비용(1 kHz C++ chain, p99) | 용도 |
| --- | --- | --- | --- |
| `preload` | process 시작 시 `LD_PRELOAD` | +0-5 us | 실시간 시스템의 기본 |
| `lttng` | process 시작 시 `LD_PRELOAD`. session은 orma-collectd가 관리 | +10-22 us, 코어의 7-11% | ros2_tracing 도구로 읽을 CTF가 필요할 때 |
| `uprobe` | 실행 중인 process에 즉시 | +30-80 us, 코어의 14-30% | 재시작할 수 없는 process를 잠깐 볼 때 |

- 비용: Orin, 6.8 RT kernel 측정값. 조건은 [README](../README.md)의 실시간 영향 절
- uprobe는 같은 함수를 부르는 thread끼리 kernel 안에서 직렬화. 1 kHz 이상 RT 부하에는 `preload`
- uprobe 부착 probe당 약 0.25 s, 해제 약 1.1 s. 선택 후 첫 값까지 수 초
- callback 안 CPU와 scheduling latency(`sched`): method와 무관하게 ormad 필요
- malloc attribution: `preload` 전용

추가 수집은 켜야 동작한다.

| 수집 | 켜는 옵션 | `preload` | `lttng` | `uprobe` | 나오는 곳 |
| --- | --- | --- | --- | --- | --- |
| CUDA driver API 호출 | `--gpu` | O | O | O | `orma node cb`의 CUDA 열, rqt의 GPU tab |
| DDS write 호출 | `--dds` | O | O | O | `orma topic delay`의 DDS 열 |
| futex 대기 | `--futex` | ormad | ormad | ormad | `orma node cb`의 futex 열, rqt의 Threads tab |
| span(`liborma_span`) | 항상 | O | O | O | `orma span`(8절) |
| 장비 상태(clock, 온도, network, GPU) | 항상 | method와 무관 | method와 무관 | method와 무관 | `orma top` |

- futex 대기: method와 무관하게 ormad가 kernel tracepoint로 측정. ormad가 없으면 열이 `-`
- CUDA: `libcuda.so.1` 호출 시간. GPU 안의 kernel 실행 시간은 재지 않는다. 비동기 launch는 바로 반환하고, 기다린 시간은 sync 열과 동기 copy에 나타난다

## 3. preload, lttng method로 실행

```bash
orma run -- ros2 run demo_nodes_cpp talker               # preload (default)
orma run --method lttng -- ros2 launch my_pkg my.launch.py
eval "$(orma env)"                                       # into this shell
```

`orma run`과 `orma env`가 설정하는 환경변수:

| 변수 | 값 |
| --- | --- |
| `LD_PRELOAD` | `liborma_preload.so` 또는 `liborma_lttng.so`. 기존 값은 뒤에 유지 |
| `PYTHONPATH` | `orma_rclpy` autoload. rclpy가 부르지 않는 trace 함수 호출용 |
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
| `record` | 참이면 launch 시작 시 recording 시작. 기본 거짓 |
| `stop_on_shutdown` | launch 종료 시 recording 정지. 기본 참. 거짓이면 한도까지 지속 |
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
orma topic list|info|hz|bw|delay ...  # see orma topic below
```

| 명령 | 열 |
| --- | --- |
| `orma top` | CPU clock(최소, 평균, 최대 MHz와 CPU 수), thermal zone 온도, throttle counter 증가분(x86), interface별 rx·tx byte rate, packet rate, drop, error, UDP error counter 증가분, `gpus` 표(번호, 이름, 사용률, memory(사용/전체), SM clock, 온도, 전력, clock 제한 이유) |
| `orma node cpu` | node 이름, pid, CPU%, CPU source, busy%(wall time), callback Hz, RSS. RSS는 process 값이라 한 process에 node가 여럿이면 `(shared)`. 아래에 process 경고(표 넘침, flux GPU wedged) |
| `orma node cb` | callback, node, pid, Hz, expected Hz, callback duration p50, p90, p99, max, dispatch latency p99. `--gpu`면 `cuda api`(실행 한 번당 CUDA 호출 시간), `cuda sync`(그중 synchronize 시간), `--futex`면 `futex`(실행 한 번당 대기 시간) 열 추가. CB를 주면 이름에 CB가 들어간 callback만 |
| `orma node sub` | (topic, subscription node)마다 한 행: topic, node, pid, take Hz, loss%, loss 수, wakeup latency p50, p90, p99, max. TOPIC을 주면 그 topic만 |
| `orma span` | span 이름, 감싼 span, 감싼 callback, node, pid, Hz, 길이 p50, p90, p99, max. NAME을 주면 그 이름만(8절) |

```bash
orma node cpu
orma node cb /talker
orma node cb /camera/driver 'sub(/image)'
orma node sub /listener /chatter
```

| 옵션 | 뜻 |
| --- | --- |
| `--pid PID` | orma보다 먼저 떠 있던 process 선택. ormad가 uprobe 부착 |
| `--method M` | 한 번 주면 모든 pid에, pid마다 주면 순서대로 대응 |
| `--window S` | window 길이(초). 기본 1초 |
| `--infra` | 숨긴 infrastructure topic과 service 표시 |

- 이름은 그대로 매칭(정규식 아님). node 이름은 `ros2 node list`와 같은 `/ns/name`
- node를 지정하면 그 node의 process를 선택. 비용은 process 단위. container 안 node 하나만 골라도 uprobe는 process 전체에 부착
- node를 지정하지 않는 명령(`node list`, `node cpu`, `span`): 모든 process 표시, uprobe 미부착. 값은 `orma run`(`preload`, `lttng`)으로 실행한 process만 나온다. 시작 시 `[info] select_all` 출력. 보일 것이 없으면 window 줄 아래에 `none`
- orma보다 먼저 떠 있던 process: node 이름을 몰라 이름으로 선택 불가. `--pid`로 고르면 callback이 돈 뒤 수 초 안에 이름이 붙는다(아래 `unlinked`)
- process 단위 값(process CPU, PSS)은 CLI에 없다. rqt의 Processes tree와 recording(`orma export --metric processes`)에 있다. PSS는 rqt의 `pss` 체크를 켰을 때만 5초마다 `smaps_rollup`으로 읽는다. 이 읽기는 대상의 page table 전체를 대상의 mmap_lock을 쥔 채 훑어, 그동안 대상의 page fault와 mmap이 기다린다
- `--gpu`, `--futex`(`node cb`), `--dds`(`topic delay`): 2절의 추가 수집을 켠다. 끄는 것은 rqt의 Selection tab
- 선택, window 길이, 추가 수집은 orma-collectd에 하나. 다른 terminal이나 rqt와 공유
- client가 하나도 없으면 orma-collectd가 hot probe 해제
- tab 완성: workspace를 source하면 켜진다(bash, zsh, argcomplete). 명령과 옵션 외에 node, topic, node의 callback과 subscription topic, span 이름, recording id를 완성. 이름은 orma-collectd의 다음 window에서 읽으므로 최대 window 길이만큼 걸린다. orma-collectd가 없으면 이름 후보 없음

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
| `timer cb_fn` | timer callback. 대상이 없어 callback을 정의한 함수 이름. 인자 목록, template 인자, namespace, lambda 표시는 뺀다 |
| `timer Talker` | `Talker` class 생성자 안의 lambda. rclpy의 `Talker.__init__.<locals>.<lambda>`도 같다 |
| `timer Talker.on_timer` | rclpy method. Python의 `__qualname__` |
| `flux(/image)` | flux channel callback |
| `<timer 0xaaaacbc8a330>` | 함수 이름 미확인. callback 객체 주소 |
| `... #a7e448` | 한 node 안에 같은 이름이 둘 이상. 주소 끝 여섯 자리로 구분 |

`unlinked`:

- node를 모르는 callback과 subscription. node 열이 `unlinked`이고 맨 아래에 표시
- `unlinked (cold)`: orma가 늦게 붙어 node 생성을 못 본 경우
- uprobe로 보는 rclcpp process: executor에서 node를 읽어 보통 2-3초 안에 보완. 그동안 callback마다 uprobe 하나(약 1.25 us) 추가. 10초 안에 못 찾으면 포기
- 채워지지 않는 것: rclpy node, Humble의 `StaticSingleThreadedExecutor`와 `EventsExecutor`, intra-process 전용 subscription, timer만 있는 callback group의 timer. 늦게 붙은 service는 `srv`와 함수 이름만
- 처음부터 보려면 `orma node cpu`나 rqt를 먼저 열고 node를 실행. client가 붙어 있는 동안 모든 process에 init probe가 있다

표시 규칙:

- 숨김: `/rosout`, `/parameter_events`, `ros_discovery_info`, rclcpp가 node마다 만드는 parameter service(`get_parameters` 등)와 그 callback. `--infra`로 표시
- latency 열 순서: p50, p90, p99, max. window 값의 p99.9는 recording과 rqt에만 있다
- `-`: 미측정. `0`: 측정값 0
- CPU source `sched`: ormad가 잰 on-CPU. `wall_bound`: sched 없이 callback wall time으로 낸 상한
- window 줄 끝 `behind`: orma-collectd가 window 하나를 window 길이 안에 모으지 못한 상태

`orma status` 마지막 줄은 orma 자체 비용이다.

```text
orma: collectd cpu 1.0% rss 6.9MiB, ormad cpu 0.4% rss 22.5MiB, shm files 0 ring drops 0, probes missing 6, collect 1.7ms
```

`probes missing`은 부착되지 않은 point 수다. 함수가 없는 library도 포함한다. 이유는 `orma check`에 나온다.

`preload` process의 표가 넘치면 `orma node cpu` 표 아래에 경고가 나온다.

```text
warning: pid 4021 (camera_node): 3 samples not aggregated (thread table full)
```

| 경고 | 뜻 |
| --- | --- |
| `threads not traced (thread slots full)` | process의 traced thread 256개 초과 |
| `samples not aggregated (thread table full)` | 한 thread의 callback 종류 63개 초과(malloc attribution을 켜면 31개) |
| `callbacks not timed (nested deeper than 16)` | callback 중첩 16단 초과 |

flux GPU가 wedged인 process에는 `CUresult`와 함께 경고 한 줄이 붙는다. 그 process의 GPU channel claim, loan, take는 실패하고 topic의 publish drop으로 센다.

`orma top`:

- orma-collectd가 window마다 kernel counter와 NVML(Jetson은 sysfs)에서 읽는다
- NVML이나 GPU가 없으면 `gpus` 표만 비고 나머지는 그대로
- `--json`: window message 전체를 한 줄에 하나씩 출력. field는 `common/orma_proto/schema/v1.json`의 `window`

### `orma topic`

```bash
orma topic list [--infra]
orma topic info TOPIC
orma topic hz TOPIC... [--window S]
orma topic bw TOPIC... [--window S]
orma topic delay TOPIC... [--window S] [--dds]
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

- `ros2 topic`과 달리 subscriber를 만들어 받는 쪽에서 세지 않는다. publisher와 subscriber process 안에서 잰다. DDS 전송이 새로 생기지 않고 `hz`에 network loss가 섞이지 않는다. 받은 수는 `orma node sub`의 take Hz
- `delay`는 `ros2 topic delay`처럼 header stamp를 쓰지 않는다. 같은 message의 publish와 take 시각 차
- topic 이름은 그대로 매칭. 그 topic의 publisher와 subscriber process를 선택. `list`는 모든 process 선택
- 관측되지 않는 publisher의 값은 `-`. 첫 window는 window 중간에 선택해 `-`일 수 있다

## 5. recording

```bash
orma record [--pid PID]... [--method M] [--kind events|windows] [options]
```

- foreground: window마다 상태 한 줄. Ctrl-C는 recording을 멈추고 마무리 후 검증 표 출력
- terminal을 닫거나 client가 죽어도 recording은 한도까지 지속(orma-collectd가 소유)
- `--no-follow`: 시작 후 바로 반환

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

- 한도 필수. 0이나 무한 불가. 길이 상한은 `~/.config/orma/collectd.toml`의 `max_duration_s`(기본 7일)
- 디스크 여유가 하한(기본 max(2 GiB, 파일시스템의 5%), `disk_floor_bytes`) 아래로 내려가면 `disk_floor`로 종료. 시작 시 여유가 하한 + 1 GiB 미만이면 시작 거부
- 동시 recording 최대 4개
- `--node`, `--topic`, `--exclude`: 이름 전체에 맞는 정규식. rqt Selection tab의 같은 칸과 같은 뜻. recording 중 새로 뜬 node도 이름이 맞으면 포함. 한 번 포함된 process는 끝까지 유지

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

- `state: truncated`: orma-collectd가 마무리 전에 죽은 recording. 다음 시작 때 개수를 다시 세어 복구
- `probes_missing`: 부착되지 않은 point. 빈 열이 "일이 없었다"인지 "볼 수 없었다"인지 구분
- `gaps`: ormad 연결이 끊긴 구간 같은 빈 구간
- `system_start`, `system_end`: 시작과 끝의 장비 상태. kernel 버전과 설정(`HZ`, preempt), cmdline, clocksource, `clock_gettime` 한 번의 비용, scheduler와 sched_ext, 격리 CPU, cpufreq governor와 EPP, 끈 idle state, THP, 취약점 완화, lockdown, 가상화, 시계 동기 상태, 대상 process의 Seccomp와 `cpu.max`. root만 읽는 값(debugfs)은 빠진다. 다른 장비나 다른 날의 값과 비교하기 전에 먼저 맞춘다
- `interference`: 시작부터 끝까지 늘어난 kernel counter. CPU별 interrupt(`LOC`, `RES`, `CAL`, `TLB`, device interrupt 합 `dev`)와 softirq, vmstat의 reclaim·compaction·major fault·TLB flush, PSI, steal, 대상 thread별 minor·major fault와 실행·대기 시간
- `realtime_steps`: recording 중 CLOCK_REALTIME이 점프한 시각. 그 근처의 `header.stamp` 나이와 timestamp matching은 믿지 않는다
- `probe_changes`: ormad가 probe를 붙이고 뗀 구간. 그동안 kernel이 코드를 고치고 TLB를 비우므로 그 구간의 tail은 orma의 몫

## 6. 분석과 내보내기

```bash
orma analyze <directory> [--chain NAME=T1,T2,...[:first]]...
orma export <directory> -o out.csv [--metric M]...
```

- `analyze`: event recording을 transport, wakeup, dispatch, callback segment로 나눠 message별, callback별 표와 tail 요약(p50, p90, p99, max) 출력. CSV는 `<directory>/analysis/`. `summary.csv`에는 p99.9도 있다(`n,p50_ns,p90_ns,p99_ns,p999_ns,max_ns`)
- `--gpu`, `--futex`로 기록했으면 `callbacks.csv`에 callback 한 번마다 CUDA driver 호출 시간(`cuda_ns`), 그중 synchronize 시간(`cuda_sync_ns`), futex 대기(`futex_ns`) 열 추가. 요약에 같은 tail 표 추가
- `--dds`로 기록했으면 publish마다 `publishes.csv`(publish에서 DDS write 시작까지, write 시간)
- window에 장비 상태(`host`)가 있으면 `tails.csv`. 표와 entity마다 p99 이상인 sample(최대 10개)에 그 sample이 난 window의 장비 상태를 붙인다. 열은 그 thread가 돈 CPU의 최저 clock(thread를 모르면 전체 CPU), 가장 뜨거운 thermal zone, throttle 횟수, GPU별 clock, 온도, clock 제한 이유. 요약 끝의 `tail context`는 표마다 가장 나쁜 entity의 max를 같은 정보와 함께 한 줄로 출력. 장비 상태는 window 단위 값
- `analyze --chain`: event recording으로 그 chain을 다시 재 `analysis/chains.csv`에 쓴다. 형식은 7절과 같다. recording 때 chain을 주지 않았어도 된다
- `export`: `windows.jsonl`을 CSV로 변환하고 `meta.json` 복사. `--metric`은 `processes`, `threads`, `nodes`, `executors`, `callbacks`, `topics`, `subscriptions`, `flows`, `spans`, `chains`, `host`, `agent` 중 선택. 생략하면 전부

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

- chain의 topic을 publish하는 process가 모두 관측돼야 한다. 관측하지 않은 process를 거치면 broken 증가
- 단계는 원천부터 그 topic의 publish까지. 마지막 단계가 e2e

`--latest-node RE`:

- 입력이 여럿인 node의 출력을 각 입력 topic에서 가장 최근에 받은 message와 잇는다. 이름 전체에 매칭
- stamp가 하나로 맞으면 그 연결이 우선. stamp가 모호하거나 맞지 않을 때만 최신 입력으로 잇는다
- 입력을 모았다가 나중에 처리하는 node는 틀리게 잇는다. 고른 node에만 쓴다

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

1. `orma run`과 같은 `preload` 환경으로 rclpy publish process 실행. orma가 그 publish를 보기 위해서다
2. orma-collectd의 chain 설정에 `first` chain `react-<pid>`(`[입력, 출력...]`) 추가. 선택이 모든 process가 아니면 publish process를 선택에 추가하고, 선택이 없으면 모든 process 선택
3. 입력 topic에 subscriber가 붙고(최대 10초 대기) orma-collectd가 publish process를 보면 publish 시작
4. publish가 끝난 뒤 window 두 개를 더 받고 chain 설정과 선택 복원. Ctrl-C로 끝내도 복원

```text
react-52011 (first): /obstacle -> /trajectory -> /control_cmd, published 10
steps
  step  topic         n   p50     p99     max
  1     /trajectory   10  12.3ms  14.1ms  14.1ms
  2     /control_cmd  10  15.0ms  17.8ms  17.8ms
count 10, broken 0
note: n and max are exact; p50 is the n-weighted mean of the window p50s (an estimate), p99 the largest window p99 (an upper bound)
```

- 결과는 window별 chain 행의 합. n과 max는 정확한 값. p99는 window별 p99의 최댓값이라 전체 p99의 상한. p50은 window별 p50을 n으로 가중 평균한 근삿값
- message 내용으로 reaction을 판정하지 않는다. 입력 뒤 그 topic의 첫 publish를 reaction으로 본다. 입력 topic에 다른 publisher가 있으면 그 publish도 원천으로 센다
- 출력 topic을 publish하는 process도 관측돼야 한다. `preload`나 `lttng`로 실행하거나 미리 `orma node cpu --pid PID`나 rqt로 선택
- `--period`는 chain latency보다 길게. 짧으면 앞 publish가 broken

## 8. span

callback 안의 구간을 코드에 표시하면 `orma span`에 그 구간의 길이가 나온다. orma가 정하는 구간은 없고 사용자가 표시한 구간만 나온다.

- C, C++: package `orma_span`의 `liborma_span.so`를 링크하고 header `orma_span/span.h` 사용
- Python: 같은 package의 decorator 사용. `package.xml`에 `orma_span`을 `exec_depend`로 추가

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

- 이름은 정적 수명의 문자열(문자열 literal). 주소로 begin과 end를 잇기 때문이다. 같은 이름의 begin과 end에는 같은 문자열을 넘긴다
- orma가 붙지 않은 process에서는 빈 함수. 호출 비용만 든다. 켜는 옵션 없음
- `preload`와 `lttng`은 같은 이름의 함수로 가로채고, `uprobe`는 process가 load한 `liborma_span.so`에 붙는다. 세 method 모두 같은 값
- 행은 같은 process 안의 (이름, 이름 문자열 주소, 감싼 callback, 감싼 span)마다 하나. 같은 callback 안 두 곳에 같은 이름을 쓰면 두 행이 되고 이름 뒤에 주소 끝 여섯 자리(`step #a1b2c3`)가 붙는다. 짝 없는 end는 버린다
- C, C++에서 같은 library 안의 같은 내용 문자열 literal은 compiler와 linker가 하나로 합칠 수 있다. 그러면 두 곳이 한 행으로 합쳐진다. 구별하려면 이름을 다르게 준다. Python decorator는 decorator마다 buffer가 따로라 항상 구별된다
- ROS가 아닌 process도 `orma run`으로 실행하면 span이 나온다. callback 열은 `-`, node 열은 `unlinked`
- Python decorator: 함수 호출 한 번이 span 하나. 함수 안 일부 구간은 그 부분을 함수로 떼어 decorator를 붙인다. 이름은 decorator마다 한 번 buffer에 담아 그 주소를 쓰므로 정적 문자열 제약 없음
- Python decorator 비용: orma가 붙지 않아도 호출마다 약 0.3 us(ctypes 호출 두 번). 1 kHz 이하 callback 안의 함수에 쓴다

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

- `match`: 이름 전체에 맞는 정규식. 맞는 대상마다 상태 하나
- `topic_hz_min`: 값이 `warn`보다 작으면 WARN, `error`보다 작으면 ERROR. 나머지 규칙: 값이 `warn`보다 크면 WARN, `error`보다 크면 ERROR
- 맞는 대상이 window에 없으면 그 항목은 STALE 하나. 대상은 있는데 값이 `-`(미측정)여도 STALE
- 상태 이름은 `orma <rule>: <target>`. 값(`value`, `warn`, `error`)을 key-value로 싣는다
- node는 선택을 바꾸지 않는다. 볼 대상은 `orma node` 명령이나 rqt로 선택. 대상 process가 선택에 없으면 STALE
- orma-collectd가 끝나면 node도 종료. launch 파일은 2초 뒤 다시 실행

## 10. rqt

```bash
rqt --standalone rqt_orma.plugin.Orma
```

- rqt 메뉴: Plugins > Introspection > orma
- CLI와 같은 orma-collectd에 붙어 선택, recording, 상태 공유
- plugin을 닫아도 recording 지속
- Monitor tab의 Host, Spans tab: `orma top`, `orma span`과 같은 열. GPU tab에는 process별 CUDA 표도 있다. Processes tree에 CUDA api, CUDA sync, futex 열, Topics 표에 DDS 열, Threads 표에 futex p99 열
- Selection tab의 `gpu`, `dds`, `futex` 체크: 추가 수집 켜기와 끄기. `Chains (--chain)` 칸은 CLI 형식의 chain을 공백으로 나눠 받는다. 비우면 chain 삭제
- recording 시작 form: `--gpu`, `--dds`, `--futex` 체크와 `--chain`, `--latest-node` 칸(공백 구분)

## 11. ormad 단독

orma-collectd 없이 callback 표만 볼 때 쓴다. service와 별개로 실행된다.

```bash
sudo install/orma_daemon/bin/ormad --pid PID [--interval 1]
```
