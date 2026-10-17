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
| 그 밖 | 4절 기록 옵션의 필드 이름(`kind`, `dir`, `name`, `max_duration_s`, `max_bytes`, `split_bytes`, `split_duration_s`, `cache_bytes`, `compression`, `bandwidth`, `sched`, `malloc`) |

## 4. 모니터링

```bash
armo top <보기> [--node RE] [--topic RE] [--exclude RE] [--pid PID]... [--method M]...
         [--window S] [--pss] [--infra] [--json]
```

무엇을 볼지는 노드, 토픽, 프로세스 중 편한 것으로 고른다.

| 옵션 | 고르는 것 | 붙는 곳(비용) |
| --- | --- | --- |
| `--node RE` | 이름이 정규식 전체에 맞는 노드 | 그 노드가 있는 프로세스 |
| `--topic RE` | 이름이 맞는 토픽, 그 토픽을 발행하거나 구독하는 노드 | 발행자와 구독자 프로세스 모두(홉 지연은 양 끝이 필요하다) |
| `--exclude RE` | 맞는 노드와 토픽을 뺀다 | 노드가 모두 빠진 프로세스는 붙지 않는다. 단독으로 주면 모든 프로세스를 볼 때처럼 uprobe를 붙이지 않고 화면에서만 뺀다 |
| `--pid PID` | 그 프로세스 | 그 프로세스 |

```bash
armo top cb --node /talker
armo top sub --topic '/camera/.*'
armo top node --exclude '/rviz.*'
```

- 정규식은 이름 전체에 맞춘다. `/cam`은 `/camera`에 맞지 않고 `/cam.*`이 맞는다.
- 노드와 토픽은 armo가 이름을 아는 것만 고를 수 있다. 새로 뜨는 노드는 창마다 다시 맞춰 들어온다. armo보다 먼저 떠 있던 노드는 이름을 몰라 맞지 않는다. 그 프로세스를 `--pid`로 고르면 콜백이 돈 뒤 몇 초 안에 이름이 붙는다(아래 `unlinked`).
- 비용은 프로세스 단위로 든다. 컨테이너 안의 노드 하나를 골라도 uprobe는 그 프로세스 전체에 붙는다. 화면에는 고른 노드와 토픽만 보인다.
- 아무것도 고르지 않으면 모든 프로세스를 본다. 이때는 uprobe를 붙이지 않는다. 모든 프로세스에 uprobe 비용을 물리지 않기 위해서다. 값은 preload, LTTng로 띄운 프로세스에서만 나오고, 나머지는 방식, Hz, 지연이 `-`다. 시작할 때 `[info] select_all` 줄이 이를 알린다.
- `--method`는 한 번 주면 모든 pid에, pid마다 한 번 주면 순서대로 짝짓는다. 주지 않으면 프로세스마다 쓸 수 있는 방식을 preload, lttng, uprobe 순으로 고른다. preload나 LTTng로 띄우지 않은 프로세스는 uprobe가 된다.
- `--window`는 창 길이(초)다. 기본 1초다.
- RSS는 `statm`으로 읽는다. PSS는 `--pss`(rqt에서는 `pss` 체크)를 켰을 때만 5초마다 `smaps_rollup`으로 읽는다. 그 읽기는 관측 대상의 페이지 표 전체를 대상의 mmap_lock을 쥔 채 훑는다. 그동안 대상의 page fault와 mmap이 기다린다. 끄면 `proc`의 PSS 열이 `-`다.
- 선택과 창 길이는 armo-collectd에 하나다. 다른 터미널이나 rqt의 선택도 함께 바뀐다.
- Ctrl-C로 끝난다. 아무 클라이언트도 없으면 armo-collectd가 hot 프로브를 뗀다.

보기는 다섯이다.

| 보기 | 열 |
| --- | --- |
| `proc` | pid, comm, 방식, CPU%, RSS, PSS, 공용 CPU%(노드에 나누지 못한 몫), 끝남. 아래에 armo 자기 비용 한 줄. flux2가 GPU를 wedged로 표시한 프로세스는 CUresult와 함께 경고 한 줄이 붙는다. 그 뒤로 GPU 채널의 claim, loan, take는 실패하고 토픽의 발행 버림으로 센다 |
| `node` | 노드 이름, pid, CPU%, CPU 출처, busy%(벽시계), 콜백 Hz |
| `cb` | 콜백, 노드, pid, Hz, 기대 Hz, 실행 시간 p99, p99.9, max, p50, dispatch 대기 p99 |
| `topic` | 토픽, 전송(`ros`, `flux`, `flux2`), 발행 Hz, 기대 Hz, 대역폭, 버림, 홉 지연 p99, p99.9, max, p50 |
| `sub` | 토픽, 구독 노드, pid, 받음 Hz, 손실%, 손실 수, 깨움 지연 p99, p99.9, max, p50 |

```text
callbacks
  callback                node       pid     Hz   expected  p99      p99.9    max      p50      dispatch p99
  sub(/chatter)           /listener  393740  1.0  -         69.1us   69.1us   69.1us   69.1us   88.4us
  <timer 0xaaaacbc8a330>  /talker    393739  1.0  1.0       279.4us  279.4us  279.4us  279.4us  47.8us
```

콜백 이름:

| 형식 | 뜻 |
| --- | --- |
| `sub(/chatter)` | 구독 콜백. 괄호 안이 토픽이다 |
| `srv(/add)` | 서비스 콜백 |
| `timer cb_fn` | 타이머 콜백. 대상이 없어 함수 이름을 붙인다 |
| `flux2(/image)` | flux2 채널 콜백 |
| `<timer 0xaaaacbc8a330>` | 함수 이름을 찾지 못했다. 콜백 객체의 주소다 |
| `... #a7e448` | 한 노드 안에 같은 이름이 둘 이상이다. 주소의 끝 여섯 자리로 구분한다 |

- 노드 열은 `/ns/name`, pid 열은 그 프로세스다.
- 노드를 모르는 콜백과 구독은 노드 열이 `unlinked`이고 맨 아래에 모인다. armo가 그 프로세스에 나중에 붙어 노드를 만드는 순간을 못 봤으면 `unlinked (cold)`다.
- uprobe로 재는 rclcpp 프로세스는 armo가 executor에서 노드를 읽어 채운다. 노드를 모르는 콜백이 처음 돈 뒤 보통 2~3초 안에 노드가 붙는다. 그동안만 콜백마다 uprobe 하나(약 1.25 us)가 더 든다. 10초 안에 못 찾으면 그만둔다.
- 그래도 남는 것: rclpy 노드, Humble의 `StaticSingleThreadedExecutor`와 `EventsExecutor`, intra-process로만 받는 구독, 타이머만 든 callback group의 타이머. 늦게 붙은 서비스는 이름 없이 `srv`와 함수 이름만 보인다.
- 이것들을 처음부터 보려면 `armo top`이나 rqt를 먼저 열어 둔 채 노드를 띄운다. 클라이언트가 붙어 있는 동안 init 이벤트를 받는 프로브가 모든 프로세스에 있다.
- `/rosout`, `/parameter_events`, `ros_discovery_info`와 노드마다 rclcpp가 만드는 파라미터 서비스(`get_parameters` 등), 그 콜백은 기본으로 숨긴다. `--infra`를 주면 보인다.

- 지연 열은 꼬리부터 찍는다(p99, p99.9, max, p50).
- `-`는 재지 않았다는 뜻이고 `0`은 잰 값이 0이라는 뜻이다.
- 노드의 CPU 출처가 `sched`이면 armod가 잰 on-CPU이고, `wall_bound`이면 sched 없이 콜백 벽시계 시간으로 낸 상한이다.
- 창 줄 끝의 `behind`는 armo-collectd가 창 하나를 창 길이 안에 모으지 못했다는 뜻이다.

`proc` 보기의 표 아래 줄은 armo 자신의 비용이다.

```text
armo: collectd cpu 1.0% rss 6.9MiB, armod cpu 0.4% rss 22.5MiB, shm files 0 ring drops 0, probes missing 6, collect 1.7ms
```

`probes missing`은 붙지 않은 지점 수다. 그 함수가 없는 라이브러리(예: Jazzy의 rmw 서비스 트레이스포인트)도 센다. 자세한 이유는 `armo check`가 보인다.

preload 프로세스의 표가 넘치면 `proc` 보기의 표 바로 아래에 경고가 나온다.

```text
warning: pid 4021 (camera_node): 3 samples not aggregated (thread table full)
```

| 경고 | 뜻 |
| --- | --- |
| `threads not traced (thread slots full)` | 프로세스의 추적 스레드가 256개를 넘었다 |
| `samples not aggregated (thread table full)` | 한 스레드가 돌린 콜백 종류가 63개(malloc 귀속을 켜면 31개)를 넘었다 |
| `callbacks not timed (nested deeper than 16)` | 콜백 안 콜백이 16단을 넘었다 |

`--json`은 창 메시지를 한 줄에 하나씩 찍는다. 보기를 주면 `seq`, `window_ns`와 그 목록만 담는다. 필드는 설계 문서 12.7이다.

## 5. 기록

```bash
armo record [--pid PID]... [--method M] [--kind events|windows] [옵션]
```

전경에서 돌면 창마다 상태 한 줄을 찍는다. Ctrl-C는 기록을 멈추고 마무리를 기다린 뒤 검증 표를 찍는다. 터미널을 닫거나 프로세스가 죽으면 기록은 멈추지 않고 한도까지 이어진다. 기록은 armo-collectd가 가지기 때문이다. `--no-follow`는 시작하고 바로 돌아온다.

| 옵션 | 뜻 | 기본 |
| --- | --- | --- |
| `--all`, `--pid PID`..., `--node RE`, `--topic RE`, `--exclude RE` | 대상(4절) | `--all` |
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

- 한도는 늘 있다. 0이나 무한은 받지 않는다. 길이 상한은 `~/.config/armo/collectd.toml`의 `max_duration_s`(기본 7일)다.
- 디스크 여유가 하한(기본 max(2 GiB, 파일시스템의 5%), `disk_floor_bytes`)보다 내려가면 기록이 `disk_floor`로 끝난다. 시작할 때 여유가 하한 + 1 GiB보다 작으면 시작하지 않는다.
- 기록은 동시에 4개까지다.
- `--node`, `--topic`, `--exclude`는 모니터링과 같은 뜻이다. 기록 중에 새로 뜬 노드도 이름이 맞으면 들어온다. 한 번 들어온 프로세스는 기록이 끝날 때까지 남는다.

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
armo analyze <디렉터리>
armo export <디렉터리> -o out.csv [--metric M]...
```

- `analyze`는 이벤트 기록을 구간(전송 홉, 깨움, dispatch, 콜백)으로 나눠 메시지별, 콜백별 표와 꼬리 요약(p90, p99, p99.9, max)을 찍는다. CSV는 `<디렉터리>/analysis/`에 쓴다.
- `export`는 `windows.jsonl`을 CSV로 바꾸고 `meta.json`을 함께 복사한다. `--metric`은 `processes`, `threads`, `nodes`, `executors`, `callbacks`, `topics`, `subscriptions`, `flows`, `agent` 중에서 고른다. 주지 않으면 전부다.

## 7. rqt

```bash
rqt --standalone rqt_armo.plugin.Armo
```

rqt 메뉴에서는 Plugins > Introspection > armo다. CLI와 같은 armo-collectd에 붙으므로 선택, 기록, 상태가 CLI와 같다. 창을 닫아도 기록은 이어진다.

## 8. armod만으로 보기

armo-collectd 없이 콜백 표만 볼 때 쓴다. 서비스와 따로 돈다.

```bash
sudo install/armo_daemon/bin/armod --pid PID [--interval 1]
```
