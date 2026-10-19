# 사용법

설치는 [install.md](install.md), 문제 해결은 [troubleshooting.md](troubleshooting.md)에 있다. 모든 명령은 워크스페이스를 source한 셸에서 돌린다.

## 1. 처음 확인

```bash
armo status
armo check
```

`armo status`의 둘째 줄이 출처 상태다.

```text
armo-collectd: armo-collectd 0.0.0, /run/user/2002/armo/collectd.sock
armod: connected, lttng: available, preload processes: 0
monitor: idle, window 1s, selection all, sched
recordings: none
```

| 값 | 뜻 |
| --- | --- |
| `armod: connected` | uprobe, sched, eBPF 비용을 쓸 수 있다 |
| `armod: absent` | armod가 없다. preload와 LTTng만 돈다 |
| `armod: denied` | 그룹 `armo`가 아니다. 다시 로그인한다(install.md 2절) |
| `armod: lost` | 붙어 있다가 끊겼다. 다시 오면 같은 요청을 다시 보낸다 |
| `lttng: absent` | `lttng`가 PATH에 없다 |
| `preload processes` | preload나 LTTng 방식으로 띄운 프로세스 수 |

`armo check`는 이 장비에서 armo의 가정이 맞는지 본다(기본 10초, `--duration S`). 검사 동안 노드를 새로 띄워야 나오는 항목이 있다. 행마다 `pass`, `fail`, `unknown`이고 `fail`이 하나라도 있으면 종료 코드가 1이다. `unknown`은 판단할 재료가 없었다는 뜻이고 이유가 옆에 있다.

armod가 있으면 다음 행이 더 나온다.

| 행 | fail의 뜻 |
| --- | --- |
| `bpf load` | 없다. 올린 프로그램 수와 verifier가 처리한 명령어 수. 붙기는 `probes ...` 행이 따로 보인다 |
| `lockdown` | `confidentiality`. tracefs가 닫혀 sched 프로브가 붙지 않는다 |
| `bpf_jit_enable`, `bpf_jit_harden` | 인터프리터(0)나 상수 블라인딩(2)이라 잰 비용이 맞지 않는다 |
| `perf_event_paranoid`, `kptr_restrict`, `unprivileged_bpf_disabled`, `BTF` | 값만 보인다. armod는 권한으로 동작하고 BTF를 쓰지 않는다 |
| `proc hidepid` | fail은 없다. 켜져 있으면 armo-collectd는 자기 사용자의 프로세스만 본다 |
| `preload pid N` | `LD_PRELOAD`가 armo 라이브러리를 가리키는데 올라오지 않았다. `auxv`의 `AT_SECURE`가 1이면 이유를 적는다(setuid, 파일 capability, LSM 전환) |
| `time namespace pid N` | 다른 time namespace이고 monotonic 오프셋이 0이 아니다. 그 프로세스의 preload, LTTng 레코드 시각이 그만큼 어긋난다 |
| `rcl_wait returns pid N` | 한 스레드의 진입과 반환 수가 2 이상 다르다. uretprobe가 반환을 놓쳤다 |

## 2. 방식 고르기

세 방식은 같은 함수를 보고 같은 형식으로 낸다. 붙는 방법과 비용만 다르다.

| 방식 | 붙이는 때 | 관측 대상의 비용(1 kHz C++ 체인, p99) | 쓰는 경우 |
| --- | --- | --- | --- |
| `preload` | 프로세스를 띄울 때 `LD_PRELOAD` | +0~5 us | 실시간 시스템의 기본 |
| `lttng` | 프로세스를 띄울 때 `LD_PRELOAD`, 세션은 armo-collectd가 켠다 | +10~22 us, CPU 코어의 7~11% | ros2_tracing 도구로도 읽을 CTF가 필요할 때 |
| `uprobe` | 이미 돌고 있는 프로세스에 바로 | +30~80 us, CPU 코어의 14~30% | 재시작할 수 없는 프로세스를 잠깐 볼 때 |

- 비용은 Orin, 6.8 RT 커널에서 잰 값이다(설계 문서 8.6).
- uprobe는 같은 함수를 부르는 스레드끼리 커널 안에서 직렬화된다. 1 kHz 이상의 RT 부하에는 preload를 쓴다.
- uprobe를 붙이는 데 프로브당 약 0.25 s, 떼는 데 약 1.1 s가 걸린다. 처음 고른 뒤 첫 값이 나오기까지 몇 초 걸릴 수 있다.
- 콜백 안 CPU와 CPU 대기(sched)는 방식과 무관하게 armod가 있어야 나온다.
- malloc 귀속은 preload에만 있다.

추가 수집은 켜야 돈다. 방식마다 받는 것이 다르다.

| 수집 | 켜는 옵션 | preload | lttng | uprobe | 보이는 곳 |
| --- | --- | --- | --- | --- | --- |
| CUDA driver API 호출 | `--gpu` | 받는다 | 받는다 | 받는다 | `armo node cb`의 CUDA 열, rqt의 GPU 탭 |
| DDS write 호출 | `--dds` | 받는다 | 받는다 | 받는다 | `armo topic delay`의 DDS 열 |
| futex 대기 | `--futex` | armod | armod | armod | `armo node cb`의 futex 열, rqt의 Threads 탭 |
| span(`libarmo_span`) | 늘 켜져 있다 | 받는다 | 받는다 | 받는다 | `armo span`(8절) |
| 장치 상태(클럭, 온도, 네트워크, GPU) | 늘 켜져 있다 | 방식과 무관 | 방식과 무관 | 방식과 무관 | `armo top` |

- futex 대기는 방식과 무관하게 armod가 커널 tracepoint로 잰다. armod가 없으면 열이 `-`다.
- CUDA는 `libcuda.so.1`의 호출 시간이다. GPU 안의 커널 실행 시간은 재지 않는다. 비동기 launch는 바로 돌아오고, 기다린 시간은 sync 열과 동기 복사에 나타난다.

## 3. preload, LTTng 방식으로 띄우기

```bash
armo run -- ros2 run demo_nodes_cpp talker               # preload(기본)
armo run --method lttng -- ros2 launch my_pkg my.launch.py
eval "$(armo env)"                                       # 지금 셸에 넣기
```

`armo run`과 `armo env`가 넣는 환경변수:

| 변수 | 값 |
| --- | --- |
| `LD_PRELOAD` | `libarmo_preload.so` 또는 `libarmo_lttng.so`. 이미 있던 값은 뒤에 붙인다 |
| `PYTHONPATH` | `armo_rclpy`의 autoload. rclpy가 부르지 않는 트레이스 함수를 부르게 한다 |
| `LTTNG_HOME` | lttng만. `$XDG_RUNTIME_DIR/armo/lttng`. armo-collectd의 세션 데몬이 여기 있다 |

launch 파일에서는 `ArmoTrace`를 노드보다 앞에 둔다. 그 뒤에 실행되는 프로세스만 환경변수를 받는다.

```python
from armo_cli.launch import ArmoTrace
from launch import LaunchDescription
from launch_ros.actions import Node


def generate_launch_description():
    return LaunchDescription([
        ArmoTrace(method="preload", record=True, max_duration_s=600),
        Node(package="demo_nodes_cpp", executable="talker"),
    ])
```

| 인자 | 뜻 |
| --- | --- |
| `method` | `preload`(기본), `lttng`, `uprobe`. uprobe는 환경변수 없이 기록만 시작한다 |
| `record` | 참이면 launch가 시작할 때 기록을 시작한다. 기본 거짓 |
| `stop_on_shutdown` | launch가 끝날 때 기록을 멈춘다. 기본 참. 거짓이면 한도까지 이어진다 |
| 그 밖 | 5절 기록 옵션의 필드 이름(`kind`, `dir`, `name`, `max_duration_s`, `max_bytes`, `split_bytes`, `split_duration_s`, `cache_bytes`, `compression`, `bandwidth`, `sched`, `malloc`, `gpu`, `dds`, `futex`, `chains`, `latest_nodes`). `chains`는 `{"name": ..., "topics": [...], "mode": "lineage"}`의 목록이다 |

## 4. 모니터링

`ros2 node`, `ros2 topic`처럼 대상을 이름으로 지정해 본다. 실시간 명령은 창(기본 1초)마다 표를 찍고 Ctrl-C로 끝난다.

```bash
armo top                              # 장비: CPU 클럭, 온도, 네트워크, GPU
armo node list
armo node info NODE                   # pid, RSS, 발행·구독 토픽, 콜백 (한 번)
armo node cpu [NODE...]               # CPU%, busy%, 콜백 Hz, RSS
armo node cb NODE [CB] [--gpu] [--futex]
armo node sub NODE [TOPIC]
armo span [NAME...]
armo topic list|info|hz|bw|delay ...  # 아래 armo topic
```

| 명령 | 열 |
| --- | --- |
| `armo top` | CPU 클럭(최소, 평균, 최대 MHz와 CPU 수), thermal zone 온도, 제한 카운터 증가분(x86), 인터페이스별 받기와 보내기 바이트율, 패킷율, 버림, 오류, UDP 오류 카운터 증가분, `gpus` 표(번호, 이름, 사용률, 메모리(사용/전체), SM 클럭, 온도, 전력, 클럭 제한 이유) |
| `armo node cpu` | 노드 이름, pid, CPU%, CPU 출처, busy%(벽시계), 콜백 Hz, RSS. RSS는 프로세스 값이라, 한 프로세스에 노드가 여럿이면 `(shared)`가 붙는다. 아래에 프로세스 경고(표 넘침, flux GPU wedged). wedged인 프로세스의 GPU 채널 claim, loan, take는 실패하고 토픽의 발행 버림으로 센다 |
| `armo node cb` | 콜백, 노드, pid, Hz, 기대 Hz, 실행 시간 p50, p90, p99, max, dispatch 대기 p99. `--gpu`로 CUDA 값이 생기면 `cuda api`(한 번 실행당 CUDA 호출 시간), `cuda sync`(그중 synchronize 시간), `--futex`로 futex 값이 생기면 `futex`(한 번 실행당 대기 시간) 열이 붙는다. CB를 주면 이름에 CB가 들어간 콜백만 보인다 |
| `armo node sub` | (토픽, 구독 노드)마다 한 행: 토픽, 노드, pid, 받음 Hz, 손실%, 손실 수, 깨움 지연(발행에서 구독 쪽 깨움까지) p50, p90, p99, max. TOPIC을 주면 그 토픽만 |
| `armo span` | span 이름, 감싼 span, 감싼 콜백, 노드, pid, Hz, 길이 p50, p90, p99, max. NAME을 주면 그 이름만(8절) |

```bash
armo node cpu
armo node cb /talker
armo node cb /camera/driver 'sub(/image)'
armo node sub /listener /chatter
```

- 이름은 그대로 맞춘다. 노드 이름은 `ros2 node list`와 같은 `/ns/name`이다.
- 노드를 지정하면 그 노드가 있는 프로세스를 고른다. 비용은 프로세스 단위로 든다. 컨테이너 안의 노드 하나를 골라도 uprobe는 그 프로세스 전체에 붙는다.
- 노드를 지정하지 않는 명령(`node list`, `node cpu`, `span`)은 모든 프로세스를 본다. 이때는 uprobe를 붙이지 않는다. 값은 `armo run`(preload, LTTng)으로 띄운 프로세스에서만 나온다. 시작할 때 `[info] select_all` 줄이 이를 알린다. 보일 것이 없으면 창 줄 아래에 `none`만 찍힌다.
- armo보다 먼저 떠 있던 프로세스는 노드 이름을 몰라 이름으로 고를 수 없다. `--pid PID`로 고르면 armod가 uprobe를 붙이고, 콜백이 돈 뒤 몇 초 안에 이름이 붙는다(아래 `unlinked`). `--method`는 한 번 주면 모든 pid에, pid마다 한 번 주면 순서대로 짝짓는다.
- `--window`는 창 길이(초)다. 기본 1초다.
- 프로세스 단위 값(CPU, PSS)은 CLI에 없다. armo-collectd는 노드에 나누지 못한 CPU를 내려고 관측 대상 프로세스의 `/proc/<pid>/stat`을 창마다 읽고, rqt의 Processes 트리와 기록(`armo export --metric processes`)에 남긴다. PSS는 rqt의 `pss` 체크를 켰을 때만 5초마다 `smaps_rollup`으로 읽는다. 그 읽기는 대상의 페이지 표 전체를 대상의 mmap_lock을 쥔 채 훑는다.
- `--gpu`, `--futex`(`node cb`), `--dds`(`topic delay`)는 2절의 추가 수집을 켠다. 켜기만 하고, 끄는 것은 rqt의 Selection 탭에서 한다.
- 선택, 창 길이, 추가 수집은 armo-collectd에 하나다. 다른 터미널이나 rqt의 선택도 함께 바뀐다.
- 아무 클라이언트도 없으면 armo-collectd가 hot 프로브를 뗀다.
- 탭 완성은 워크스페이스를 source하면 켜진다(bash, zsh). `ros2`처럼 argcomplete를 쓴다. 명령과 옵션 말고도 노드, 토픽, 노드의 콜백과 구독 토픽, span 이름, 기록 id를 완성한다. 이름은 armo-collectd의 다음 창에서 읽으므로 최대 창 길이(기본 1초)만큼 걸리고, armo-collectd가 떠 있지 않으면 이름 후보가 없다. span 이름은 그 프로세스가 선택에 들어 있을 때만 창에 있다(`armo span`을 한 번 띄우면 들어간다).

```text
callbacks
  callback       node       pid     Hz   expected  p50      p90      p99      max      dispatch p99
  sub(/chatter)  /listener  393740  1.0  -         69.1us   69.1us   69.1us   69.1us   88.4us
  timer Talker   /talker    393739  1.0  1.0       185.8us  185.8us  185.8us  185.8us  30.9us
```

콜백 이름:

| 형식 | 뜻 |
| --- | --- |
| `sub(/chatter)` | 구독 콜백. 괄호 안이 토픽이다 |
| `srv(/add)` | 서비스 콜백 |
| `timer cb_fn` | 타이머 콜백. 대상이 없어 콜백을 정의한 함수 이름을 붙인다. 인자 목록, 템플릿 인자, 네임스페이스, 람다 표시는 뺀다 |
| `timer Talker` | `Talker` 클래스 생성자 안의 람다. rclpy의 `Talker.__init__.<locals>.<lambda>`도 같다 |
| `timer Talker.on_timer` | rclpy 메서드. Python의 `__qualname__`이다 |
| `flux(/image)` | flux 채널 콜백 |
| `<timer 0xaaaacbc8a330>` | 함수 이름을 찾지 못했다. 콜백 객체의 주소다 |
| `... #a7e448` | 한 노드 안에 같은 이름이 둘 이상이다. 주소의 끝 여섯 자리로 구분한다 |

- 노드 열은 `/ns/name`, pid 열은 그 프로세스다.
- 노드를 모르는 콜백과 구독은 노드 열이 `unlinked`이고 맨 아래에 모인다. armo가 그 프로세스에 나중에 붙어 노드를 만드는 순간을 못 봤으면 `unlinked (cold)`다.
- uprobe로 재는 rclcpp 프로세스는 armo가 executor에서 노드를 읽어 채운다. 노드를 모르는 콜백이 처음 돈 뒤 보통 2~3초 안에 노드가 붙는다. 그동안만 콜백마다 uprobe 하나(약 1.25 us)가 더 든다. 10초 안에 못 찾으면 그만둔다.
- 그래도 남는 것: rclpy 노드, Humble의 `StaticSingleThreadedExecutor`와 `EventsExecutor`, intra-process로만 받는 구독, 타이머만 든 callback group의 타이머. 늦게 붙은 서비스는 이름 없이 `srv`와 함수 이름만 보인다.
- 이것들을 처음부터 보려면 `armo node cpu`나 rqt를 먼저 열어 둔 채 노드를 띄운다. 클라이언트가 붙어 있는 동안 init 이벤트를 받는 프로브가 모든 프로세스에 있다.
- `/rosout`, `/parameter_events`, `ros_discovery_info`와 노드마다 rclcpp가 만드는 파라미터 서비스(`get_parameters` 등), 그 콜백은 기본으로 숨긴다. `--infra`를 주면 보인다.

- 지연 열은 p50, p90, p99, max 순이다. 창 값의 p99.9는 기록과 rqt에 있고 CLI에는 찍지 않는다.
- `-`는 재지 않았다는 뜻이고 `0`은 잰 값이 0이라는 뜻이다.
- 노드의 CPU 출처가 `sched`이면 armod가 잰 on-CPU이고, `wall_bound`이면 sched 없이 콜백 벽시계 시간으로 낸 상한이다.
- 창 줄 끝의 `behind`는 armo-collectd가 창 하나를 창 길이 안에 모으지 못했다는 뜻이다.

`armo status`의 마지막 줄은 armo 자신의 비용이다.

```text
armo: collectd cpu 1.0% rss 6.9MiB, armod cpu 0.4% rss 22.5MiB, shm files 0 ring drops 0, probes missing 6, collect 1.7ms
```

`probes missing`은 붙지 않은 지점 수다. 그 함수가 없는 라이브러리도 센다. 자세한 이유는 `armo check`가 보인다.

preload 프로세스의 표가 넘치면 `armo node cpu`의 표 바로 아래에 경고가 나온다.

```text
warning: pid 4021 (camera_node): 3 samples not aggregated (thread table full)
```

| 경고 | 뜻 |
| --- | --- |
| `threads not traced (thread slots full)` | 프로세스의 추적 스레드가 256개를 넘었다 |
| `samples not aggregated (thread table full)` | 한 스레드가 돌린 콜백 종류가 63개(malloc 귀속을 켜면 31개)를 넘었다 |
| `callbacks not timed (nested deeper than 16)` | 콜백 안 콜백이 16단을 넘었다 |

`armo top --json`은 창 메시지 전체를 한 줄에 하나씩 찍는다. 필드는 설계 문서 12.7이다.

`armo top`의 값은 armo-collectd가 창마다 커널 카운터와 NVML(Jetson은 sysfs)에서 읽는다. NVML이 없거나 GPU가 없으면 `gpus` 표가 비고 나머지는 그대로 나온다.

### `armo topic`

```bash
armo topic list [--infra]
armo topic info TOPIC
armo topic hz TOPIC... [--window S]
armo topic bw TOPIC... [--window S]
armo topic delay TOPIC... [--window S] [--dds]
```

```text
window 381  1s
  topic   pub Hz  expected  drops
  /scan   10.0    10.0      0
  /none   -       -         -
```

| 명령 | 값 |
| --- | --- |
| `list` | 관측 중인 프로세스의 토픽 이름. 창 하나를 받고 끝난다 |
| `info` | 전송(`ros`, `flux`), 발행 노드와 구독 노드. 창 하나를 받고 끝난다 |
| `hz` | 발행 Hz, 기대 Hz, 발행 버림 수 |
| `bw` | 발행 바이트율 |
| `delay` | 홉 지연(발행에서 take까지) p50, p90, p99, max. `--dds`를 주면 `pub to dds p99`(`rclcpp_publish`에서 DDS write 시작까지), `dds write p99`(write 호출 시간) 열이 붙는다 |

- `ros2 topic`과 달리 구독자를 만들어 받는 쪽에서 세지 않는다. 발행자와 구독자 프로세스 안에서 잰다. 그래서 DDS 전송이 새로 생기지 않고 `hz`에 네트워크 손실이 섞이지 않는다. 받은 수는 `armo node sub`의 받음 Hz다.
- `delay`는 `ros2 topic delay`처럼 header stamp를 쓰지 않는다. 같은 메시지의 발행과 take 시각 차다.
- 토픽 이름은 정규식이 아니라 그대로 맞춘다. 그 토픽의 발행자와 구독자 프로세스를 `--topic`처럼 고른다. `list`는 모든 프로세스를 고른다.
- 관측되지 않는 발행자의 값은 `-`다. 첫 창은 창 중간에 고른 것이라 `-`일 수 있다.

## 5. 기록

```bash
armo record [--pid PID]... [--method M] [--kind events|windows] [옵션]
```

전경에서 돌면 창마다 상태 한 줄을 찍는다. Ctrl-C는 기록을 멈추고 마무리를 기다린 뒤 검증 표를 찍는다. 터미널을 닫거나 프로세스가 죽으면 기록은 멈추지 않고 한도까지 이어진다. 기록은 armo-collectd가 가지기 때문이다. `--no-follow`는 시작하고 바로 돌아온다.

| 옵션 | 뜻 | 기본 |
| --- | --- | --- |
| `--all`, `--pid PID`..., `--node RE`, `--topic RE`, `--exclude RE` | 대상 | `--all` |
| `--method auto\|uprobe\|preload\|lttng` | `auto`는 프로세스마다 preload, lttng, uprobe 순 | `auto` |
| `--kind events\|windows` | 이벤트 전부, 또는 창 스냅샷만 | `events` |
| `-o DIR`, `--name NAME` | 위치와 이름 | `~/.local/share/armo/records/armo-<시작 시각>` |
| `--max-duration S` | 길이 한도 | events 30분, windows 24시간 |
| `--max-size BYTES` | 크기 한도 | events 8 GiB, windows 1 GiB |
| `--split-size BYTES`, `--split-duration S` | 이벤트 파일 나누기 | 1 GiB, 나누지 않음 |
| `--cache BYTES` | 쓰기 버퍼 | 16 MiB |
| `--compression none\|zstd` | 이벤트 파일 압축 | `none` |
| `--bandwidth` | DDS 직렬화 크기(uprobe) | 끔 |
| `--sched`, `--no-sched` | 콜백 안 CPU와 CPU 대기 | armod가 있으면 켬 |
| `--malloc` | 콜백별 할당(preload) | 끔 |
| `--gpu`, `--dds`, `--futex` | 2절의 추가 수집 | 꺼져 있다 |
| `--chain NAME=T1,T2,...[:first]` | 잴 체인(7절). 여러 번 준다. `meta.json`에 남는다 | 없다 |
| `--latest-node RE` | 최신 입력으로 잇는 노드(7절). 여러 번 준다 | 없다 |

- 한도는 늘 있다. 0이나 무한은 받지 않는다. 길이 상한은 `~/.config/armo/collectd.toml`의 `max_duration_s`(기본 7일)다.
- 디스크 여유가 하한(기본 max(2 GiB, 파일시스템의 5%), `disk_floor_bytes`)보다 내려가면 기록이 `disk_floor`로 끝난다. 시작할 때 여유가 하한 + 1 GiB보다 작으면 시작하지 않는다.
- 기록은 동시에 4개까지다.
- `--node`, `--topic`, `--exclude`는 이름 전체에 맞추는 정규식이고, rqt Selection 탭의 같은 칸과 뜻이 같다. 기록 중에 새로 뜬 노드도 이름이 맞으면 들어온다. 한 번 들어온 프로세스는 기록이 끝날 때까지 남는다.

```bash
armo record list                 # 진행 중인 기록과 최근 기록
armo record stop <id>
armo record show <id 또는 디렉터리>   # 검증 표: 엔티티별 이벤트, 손실, Hz와 경고
```

기록 디렉터리:

```text
armo-20261008-153012/
  meta.json        대상, 방식, 옵션, 붙은 라이브러리, 손실, 빈 구간, 끝난 이유
  windows.jsonl    창마다 한 줄
  events/000.armoev
  ctf/             LTTng 방식일 때
```

`meta.json`에는 장비 상태도 남는다. 측정값을 다른 장비나 다른 날의 값과 비교할 때 먼저 이것을 맞춘다.

| 필드 | 내용 |
| --- | --- |
| `system_start`, `system_end` | 시작과 끝의 커널 버전과 설정(`HZ`, preempt), cmdline, clocksource, `clock_gettime` 한 번의 비용, 스케줄러와 sched_ext, 격리 CPU, cpufreq governor와 EPP, 끈 idle 상태, THP, 취약점 완화, lockdown, 가상화, 시계 동기 상태, 대상 프로세스의 Seccomp와 `cpu.max`. root만 읽는 값(debugfs)은 빠진다 |
| `interference` | 시작부터 끝까지 늘어난 커널 카운터. CPU별 인터럽트(`LOC`, `RES`, `CAL`, `TLB`, 장치 인터럽트 합 `dev`)와 softirq, vmstat의 reclaim·compaction·major fault·TLB flush, PSI, steal, 대상 스레드별 minor·major fault와 실행·대기 시간 |
| `realtime_steps` | 기록 중 CLOCK_REALTIME이 점프한 시각. 그 근처의 `header.stamp` 나이와 timestamp 짝짓기는 믿지 않는다 |
| `probe_changes` | armod가 프로브를 붙이고 뗀 시각. 그동안 커널이 코드를 고치고 TLB를 비우므로 그 구간의 꼬리는 armo 자신의 몫이다 |

`meta.json`의 `state`가 `truncated`이면 armo-collectd가 마무리하지 못하고 죽은 기록이다. 다음 시작이 개수를 다시 세어 고친다. `probes_missing`은 그 기록에서 붙지 않은 지점이고, 빈 열이 일이 없었던 것인지 볼 수 없었던 것인지 가른다. `gaps`는 armod가 끊긴 구간 같은 빈 구간이다.

## 6. 분석과 내보내기

```bash
armo analyze <디렉터리> [--chain NAME=T1,T2,...[:first]]...
armo export <디렉터리> -o out.csv [--metric M]...
```

- `analyze`는 이벤트 기록을 구간(전송 홉, 깨움, dispatch, 콜백)으로 나눠 메시지별, 콜백별 표와 요약(p50, p90, p99, max)을 찍는다. `summary.csv`에는 p99.9도 있다(`n,p50_ns,p90_ns,p99_ns,p999_ns,max_ns`). CSV는 `<디렉터리>/analysis/`에 쓴다.
- `--gpu`나 `--futex`로 기록했으면 `callbacks.csv`에 콜백 한 번마다 CUDA driver 호출 시간(`cuda_ns`), 그중 동기화 시간(`cuda_sync_ns`), futex 대기(`futex_ns`) 열이 붙고 요약에 같은 꼬리 표가 생긴다. `--dds`로 기록했으면 발행마다 `publishes.csv`(발행에서 DDS write 시작까지, write 시간)를 쓴다.
- 기록의 창에 장치 상태(`host`)가 있으면 `tails.csv`를 쓴다. 표와 엔티티마다 p99 이상인 샘플(최대 10개)에 그 샘플이 난 창의 장치 상태를 붙인다. 열은 그 스레드가 돈 CPU의 가장 낮은 클럭(스레드를 모르면 전체 CPU), 가장 뜨거운 thermal zone, 열 제한 횟수, GPU별 클럭, 온도, 클럭 제한 이유다. 요약 끝의 `tail context`는 표마다 가장 나쁜 엔티티의 최댓값을 같은 정보와 함께 한 줄로 보인다. 장치 상태는 창 단위라 그 창 동안의 값이다.
- `analyze --chain`은 이벤트 기록으로 그 체인을 다시 재 `analysis/chains.csv`를 쓴다. 형식은 7절과 같다. 기록할 때 체인을 주지 않았어도 된다.
- `export`는 `windows.jsonl`을 CSV로 바꾸고 `meta.json`을 함께 복사한다. `--metric`은 `processes`, `threads`, `nodes`, `executors`, `callbacks`, `topics`, `subscriptions`, `flows`, `spans`, `chains`, `host`, `agent` 중에서 고른다. 주지 않으면 전부다.

## 7. 체인과 반응 시간

체인은 토픽의 순서다. 원천 토픽의 발행에서 뒤 토픽의 발행까지 걸린 시간을 단계마다 잰다.

```bash
armo record --max-duration 60 --chain perception=/camera/image,/detections,/tracks
armo analyze <디렉터리> --chain reaction=/obstacle,/trajectory,/control_cmd:first
```

실시간으로는 rqt의 Chains 탭에서 본다. `armo top`에는 체인 보기가 없다.

| 방식 | 잇는 법 | 쓰는 곳 |
| --- | --- | --- |
| `lineage`(기본) | 메시지마다 원천 시각을 붙여 옮긴다. 노드 안에서는 콜백 입력, stamp, 최신 입력 연결로 잇는다 | stamp를 이어 주는 파이프라인 |
| `first`(`:first`) | 원천의 발행마다 다음 토픽의 첫 발행을 차례로 찾는다. 다 끝나기 전에 원천이 다시 발행되면 앞 것은 broken이다 | stamp를 잇지 않는 노드(플래닝 등). 원천 주기가 체인 지연보다 길어야 한다 |

- 체인의 토픽을 발행하는 프로세스가 모두 관측돼야 한다. 관측하지 않은 프로세스를 거치면 broken이 는다.
- 체인의 단계는 원천에서 그 토픽의 발행까지다. 마지막 단계가 e2e다.

`--latest-node RE`는 입력이 여럿인 노드의 출력을 각 입력 토픽에서 가장 최근에 받은 메시지와 잇는다. 이름 전체에 맞춘다. stamp가 하나로 맞으면 그 연결이 먼저이고, stamp가 모호하거나 맞지 않을 때만 최신 입력으로 잇는다. 입력을 모았다가 나중에 처리하는 노드에서는 틀리게 이으므로 고른 노드에만 쓴다.

### `armo react`

시험 메시지를 입력 토픽에 발행하고, 그 뒤 출력 토픽마다 처음 나온 발행까지의 시간을 잰다. armo가 토픽에 발행하는 것은 이 명령뿐이다.

```bash
armo react --input /obstacle --type my_msgs/msg/Obstacle --msg '{x: 1.0, y: 2.0}' \
           --output /trajectory --output /control_cmd [--count N] [--period S] [-o out.csv]
armo react --input /points --type sensor_msgs/msg/PointCloud2 --bag rosbag/ --output /objects
```

| 옵션 | 뜻 | 기본 |
| --- | --- | --- |
| `--input TOPIC`, `--type PKG/msg/TYPE` | 발행할 토픽과 형 | 꼭 준다 |
| `--msg YAML` | 메시지 내용. `ros2 topic pub`과 같은 형식이고 `header: auto`, `stamp: now`를 받는다. 발행마다 그 시각을 넣는다 | `--bag`과 둘 중 하나 |
| `--bag DIR` | 그 bag의 입력 토픽 메시지를 차례로 발행한다 | `--msg`와 둘 중 하나 |
| `--output TOPIC` | 반응으로 볼 토픽. 여러 번 주면 그 순서로 단계가 된다 | 꼭 준다 |
| `--count N` | 발행 수 | `--msg`는 10, `--bag`은 그 토픽의 메시지 전부 |
| `--period S` | 발행 사이 간격 | 1초 |
| `-o CSV` | 단계별 결과 CSV(`step`, `topic`, `n`, `p50_ns`, `p99_ns`, `max_ns`, `count`, `broken`) | 쓰지 않는다 |

하는 일:

1. `armo run`과 같은 preload 환경으로 rclpy 발행 프로세스를 띄운다. 그래야 armo가 그 발행을 본다.
2. armo-collectd의 체인 설정에 `first` 체인 `react-<pid>`(`[입력, 출력...]`)를 더한다. 선택이 모든 프로세스가 아니면 발행 프로세스를 선택에 더하고, 아무것도 고르지 않았으면 모든 프로세스를 고른다.
3. 입력 토픽에 구독자가 붙고(10초까지 기다린다) armo-collectd가 발행 프로세스를 보면 발행을 시작한다.
4. 발행이 끝난 뒤 창 두 개를 더 받고, 체인 설정과 선택을 원래대로 되돌린다. Ctrl-C로 끝내도 되돌린다.

```text
react-52011 (first): /obstacle -> /trajectory -> /control_cmd, published 10
steps
  step  topic         n   p50     p99     max
  1     /trajectory   10  12.3ms  14.1ms  14.1ms
  2     /control_cmd  10  15.0ms  17.8ms  17.8ms
count 10, broken 0
note: n and max are exact; p50 is the n-weighted mean of the window p50s (an estimate), p99 the largest window p99 (an upper bound)
```

- 결과는 창마다의 체인 행을 모은 것이다. n과 max는 정확하다. p99는 창별 p99의 최댓값이라 전체 p99의 상한이다. p50은 창별 p50을 n으로 가중 평균한 근삿값이다.
- 메시지 내용으로 반응을 판정하지 않는다. 입력 뒤 그 토픽의 첫 발행을 반응으로 본다. 입력 토픽에 다른 발행자가 있으면 그 발행도 원천으로 센다.
- 출력 토픽을 발행하는 프로세스도 관측돼야 한다. preload나 LTTng로 띄우거나, 미리 `armo node cpu --pid PID`나 rqt로 고른다.
- `--period`는 체인 지연보다 길게 준다. 짧으면 앞 발행이 broken이 된다.

## 8. span

콜백 안의 구간을 코드에 표시하면 `armo span`에 그 구간의 길이가 나온다. armo가 정하는 구간이 아니라 사용자가 표시한 구간만 나온다. C, C++는 패키지 `armo_span`의 `libarmo_span.so`를 링크하고 헤더 `armo_span/span.h`를 쓴다. Python은 같은 패키지의 데코레이터를 쓴다.

```c
#include <armo_span/span.h>

armo_span_begin("preprocess");
/* ... */
armo_span_end("preprocess");
```

```cpp
#include <armo_span/span.h>

void Node::on_image(const Image & msg)
{
  armo::Span s("preprocess");   // 생성에서 begin, 소멸에서 end
  // ...
}
```

```cmake
find_package(armo_span REQUIRED)
target_link_libraries(my_node armo_span::armo_span)
```

```python
from armo_span import span


@span("plan")          # 함수를 부를 때마다 span 하나
def plan(self, msg):
    ...
```

- 이름은 정적 수명의 문자열(문자열 리터럴)이어야 한다. 주소로 begin과 end를 잇기 때문이다. 같은 이름의 begin과 end는 같은 문자열을 넘긴다.
- armo가 붙지 않은 프로세스에서는 빈 함수라 호출 비용만 든다. 켜는 옵션이 따로 없다.
- preload와 LTTng는 같은 이름의 함수로 가로채고, uprobe는 프로세스가 올린 `libarmo_span.so`에 붙는다. 세 방식 모두 같은 값을 낸다.
- 행은 같은 프로세스 안의 (이름, 이름 문자열의 주소, 감싼 콜백, 감싼 span)마다 하나다. 같은 콜백 안에서 같은 이름을 두 곳에 쓰면 두 행이 되고, 이름 뒤에 주소의 끝 여섯 자리(`step #a1b2c3`)가 붙는다. 짝이 없는 end는 버린다.
- C, C++에서 같은 라이브러리 안의 같은 내용 문자열 리터럴은 컴파일러와 링커가 하나로 합칠 수 있다. 그러면 두 곳이 한 행으로 합쳐진다. 구별하려면 이름을 다르게 준다. Python 데코레이터는 데코레이터마다 버퍼가 따로라 늘 갈린다.
- ROS가 아닌 프로세스도 `armo run`으로 띄우면 span이 나온다. 콜백 열은 `-`, 노드 열은 `unlinked`다.
- Python 데코레이터는 함수 한 번 부르기가 span 하나다. 함수 안의 일부 구간은 그 부분을 함수로 떼어 데코레이터를 붙인다. 이름은 데코레이터마다 한 번 버퍼에 담아 그 주소를 쓰므로 정적 문자열 제약이 없다. package.xml에 `armo_span`을 exec_depend로 넣는다.
- Python 데코레이터는 armo가 붙지 않아도 호출마다 약 0.3 us가 더 든다(ctypes 호출 두 번, 이 장비에서 잰 값). 1 kHz 이하 콜백 안의 함수에 쓴다.

## 9. /diagnostics

`armo_diagnostics` 노드는 armo-collectd에 클라이언트로 붙어 창마다 `/diagnostics`(`diagnostic_msgs/DiagnosticArray`)를 발행한다. rqt_robot_monitor나 diagnostic_aggregator로 본다.

```bash
ros2 launch armo_diagnostics armo_diagnostics.launch.py rules:=/path/rules.yaml
ros2 run armo_diagnostics armo_diagnostics --ros-args -p rules:=/path/rules.yaml
```

규칙 파일은 규칙 이름마다 항목 목록이다. 예는 패키지의 `config/rules.yaml`이다.

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

| 규칙 | `match`가 맞추는 이름 | 값 |
| --- | --- | --- |
| `callback_p99_us` | 콜백 이름(`sub(/scan)` 등, 4절) | 실행 시간 p99(us) |
| `topic_hz_min` | 토픽 이름 | 발행 Hz. 값이 작을수록 나쁘다 |
| `topic_hop_p99_us` | 토픽 이름 | 홉 지연 p99(us) |
| `sub_loss_pct` | 구독의 토픽 이름 | 손실% |
| `chain_e2e_p99_us` | 체인 이름(7절) | e2e p99(us) |
| `gpu_temp_c` | GPU 번호(`"0"`) | 온도(C) |
| `thermal_c` | thermal zone 이름 | 온도(C) |

- `match`는 이름 전체에 맞추는 정규식이다. 맞는 대상마다 상태 하나를 낸다.
- `topic_hz_min`은 값이 `warn`보다 작으면 WARN, `error`보다 작으면 ERROR다. 나머지 규칙은 값이 `warn`보다 크면 WARN, `error`보다 크면 ERROR다.
- 맞는 대상이 창에 없으면 그 항목은 STALE 하나다. 대상은 있는데 값이 `-`(재지 않음)이어도 STALE이다.
- 상태 이름은 `armo <규칙>: <대상>`이고, 값(`value`, `warn`, `error`)을 key-value로 싣는다.
- 노드는 선택을 바꾸지 않는다. 무엇을 볼지는 `armo node` 명령이나 rqt로 고른다. 대상의 프로세스가 선택에 없으면 STALE이다.
- armo-collectd가 끝나면 노드도 끝난다. launch 파일은 2초 뒤 다시 띄운다.

## 10. rqt

```bash
rqt --standalone rqt_armo.plugin.Armo
```

rqt 메뉴에서는 Plugins > Introspection > armo다. CLI와 같은 armo-collectd에 붙으므로 선택, 기록, 상태가 CLI와 같다. 창을 닫아도 기록은 이어진다.

- Monitor 탭의 Host, Spans 탭은 `armo top`, `armo span`과 열이 같다. GPU 탭에는 프로세스별 CUDA 표도 있다. Processes 트리에는 CUDA api, CUDA sync, futex 열이, Topics 표에는 DDS 열이, Threads 표에는 futex p99 열이 있다.
- Selection 탭의 `gpu`, `dds`, `futex` 체크가 추가 수집을 켜고 끈다. `Chains (--chain)` 칸은 CLI 형식의 체인을 공백으로 나눠 받는다. 비우면 체인을 지운다.
- 기록 시작 폼에는 `--gpu`, `--dds`, `--futex` 체크와 `--chain`, `--latest-node` 칸(공백으로 나눈다)이 있다.

## 11. armod만으로 보기

armo-collectd 없이 콜백 표만 볼 때 쓴다. 서비스와 따로 돈다.

```bash
sudo install/armo_daemon/bin/armod --pid PID [--interval 1]
```
