# 문제 해결

먼저 `orma status`와 `orma check`를 확인한다([usage.md](usage.md) 1절).

## ormad

- `ormad: absent`
  - 원인: ormad가 실행되고 있지 않다.
  - 조치: `systemctl status ormad`로 확인하고, 정지되어 있으면 `sudo systemctl start ormad`를 실행한다.
- `ormad: denied`
  - 원인: 사용자가 그룹 `orma`에 없다.
  - 조치: `id`에 `orma`가 없으면 모든 세션에서 로그아웃한 뒤 다시 로그인한다. 이미 실행 중이던 orma-collectd는 예전 그룹으로 실행되고 있으므로 `pkill -x orma-collectd`로 종료한다. 다음 `orma` 명령이 다시 시작한다.
- `ormad: lost`
  - 원인: ormad가 종료되었거나 재시작했다.
  - 조치: 서비스는 `Restart=on-failure`로 다시 시작하고, orma-collectd가 다시 연결한다. 종료 이유는 `journalctl -u ormad`에서 확인한다.
- `sudo ormad`가 "another ormad serves it"로 종료된다
  - 원인: 서비스가 이미 실행 중이다.
  - 조치: `sudo systemctl stop ormad`로 정지한 뒤 실행한다.
- 다시 빌드한 ormad가 반영되지 않는다
  - 원인: 서비스는 `/usr/local/sbin/ormad`에 복사한 사본을 사용한다.
  - 조치: `daemon/orma_daemon/systemd/install.sh`를 실행한 뒤 `sudo systemctl restart ormad`를 실행한다.
- 로그에 `the scheduler probes stay off`
  - 원인: tracefs를 읽지 못했다.
  - 조치: 서비스 unit에 `CAP_DAC_READ_SEARCH`가 있는지 확인한다.
- 방식과 Hz가 `-`
  - 원인: 노드를 지정하지 않은 모니터링에서는 uprobe를 부착하지 않는다([usage.md](usage.md) 4절).
  - 조치: 노드 이름이나 `--pid`로 고른다. 그래도 값이 없으면 `journalctl -u ormad`에서 `not attached to` 줄과 그 이유를 찾는다. 그 줄도 없으면 `CAP_SYS_PTRACE`가 없어 다른 사용자 프로세스의 `/proc/<pid>/maps`를 읽지 못한 경우다.
- `bpf stats`가 `fail`
  - 원인: 커널이 `BPF_ENABLE_STATS`를 거부했다(`CAP_SYS_ADMIN`).
  - 영향: eBPF 실행 시간만 0으로 나오고, 다른 기능은 그대로 동작한다.

## orma-collectd

- `XDG_RUNTIME_DIR is not set`
  - 원인: 로그인 세션이 아닌 셸(`docker exec`, `sudo -u`)이다.
  - 조치: 로그인 세션의 셸에서 실행한다.
- `orma status`가 "not running"
  - 원인: `orma status`는 orma-collectd를 시작하지 않는다.
  - 조치: 다른 `orma` 명령이나 rqt로 시작한다.
- PSS가 `-`
  - 원인: PSS를 활성화하지 않았거나, orma-collectd가 다른 주 그룹으로 실행되었다(`sg orma`).
  - 조치: rqt의 `pss` 체크를 활성화한다. `sg orma`로 실행했다면 종료하고 다시 시작한다.
- `method lttng needs lttng, lttng-sessiond, babeltrace and setpriv on PATH`
  - 원인: LTTng 도구가 없다.
  - 조치: `sudo apt install lttng-tools babeltrace`
- 노드가 `unlinked`, `unlinked (cold)`
  - 원인: orma가 노드 생성 시점을 놓쳤다.
  - 조치: rclcpp 프로세스는 `--pid`로 고르면 수 초 뒤에 노드가 채워진다. 그래도 남으면 [usage.md](usage.md) 4절의 예외에 해당하므로, 클라이언트를 먼저 연결하고 노드를 다시 실행한다.
- 노드나 토픽 이름으로 골랐는데 아무것도 나오지 않는다
  - 원인: orma가 그 노드를 알지 못한다(위 항목).
  - 조치: ormad를 막 시작했다면 init probe의 부착 시까지 수 초 기다린 뒤 노드를 실행한다. `orma node`, `orma topic`은 이름을 그대로 비교하고, `orma record`의 `--node`, `--topic`은 이름 전체에 맞는 정규식이다(`/cam`이 아니라 `/cam.*`).
- `orma run`으로 실행했는데 `preload processes: 0`
  - 원인: 프로세스가 `LD_PRELOAD`를 받지 못했다.
  - 조치: launch 파일이면 `OrmaTrace`가 노드보다 앞에 있는지 확인한다. `orma env`를 사용했다면 노드를 실행할 셸에서 `eval`했는지 확인한다.

## 기록

| 증상 | 원인 | 조치 |
| --- | --- | --- |
| 시작 거부(디스크) | 여유 공간이 하한 + 1 GiB보다 적다 | 공간을 비우거나, `-o`로 다른 파일 시스템에 기록한다 |
| `max_duration_s ... above the cap` | 길이가 `~/.config/orma/collectd.toml`의 `max_duration_s`를 넘는다 | 길이를 줄이거나 설정값을 올린다 |
| `state: truncated` | orma-collectd가 마무리 전에 종료되었다(SIGKILL, 전원) | orma-collectd의 다음 시작 시나 `orma record show`가 개수를 다시 세어 복구한다 |
| `record show`의 `lost`가 0이 아니다 | 이벤트가 전달되기 전에 버퍼가 넘쳤다 | `meta.json`의 `drops`에서 넘친 곳(공유 메모리 ring, ormad ring buffer, LTTng)을 확인한다 |

## 선택 수집과 chain

- `orma top`의 `gpus` 표가 비었다
  - 원인: NVML(`libnvidia-ml.so.1`)이나 GPU가 없다. orma-collectd는 NVML을 링크하지 않고 시작 시 찾는다.
  - 조치: 외장 GPU에는 NVIDIA driver의 NVML이 필요하다. Jetson에서는 sysfs에서 사용률과 clock만 읽는다.
- `orma node cb`의 CUDA 열, rqt의 CUDA 표가 없다
  - 원인: `--gpu`(rqt의 `gpu`)를 활성화하지 않았다.
  - 조치: `--gpu`를 활성화한다. cudart를 정적으로 링크한 프로그램(nvcc 기본)도 수집된다. cudart가 `cuGetProcAddress`로 driver 함수를 얻으면 `preload`와 `lttng`이 그 함수도 가로채 주소를 바꾼다. uprobe는 함수 주소에 부착하므로 이 경로와 관계없다.
- `orma node cb`의 futex 열이 `-`
  - 원인: futex 대기는 방식과 관계없이 ormad가 잰다.
  - 조치: `orma status`가 `ormad: connected`인지 확인한다.
- `orma topic delay`에 DDS 열이 없다
  - 원인: `--dds`를 활성화하지 않았거나, 그 토픽의 발행과 DDS write를 같은 스레드에서 짝짓지 못했다.
  - 조치: `--dds`를 활성화한다. 짝이 없는 write는 버린다.
- `orma span`이 비었다
  - 원인: 프로세스가 `liborma_span.so`를 링크하지 않았거나 선택 대상에 없다.
  - 조치: 이름은 문자열 literal로 넘긴다. 주소가 다르면 begin과 end가 이어지지 않는다.
- chain의 broken이 크다
  - 원인: chain의 토픽을 발행하는 프로세스 중 관측하지 않은 것이 있다.
  - 조치: 그 프로세스도 선택한다. `first` 방식이면 첫 토픽의 주기가 chain latency보다 긴지 확인한다.
- `orma react`가 `no subscriber on ...`으로 종료된다
  - 원인: 10 s 안에 입력 토픽에 구독자가 연결되지 않았다.
  - 조치: 토픽 이름, `ROS_DOMAIN_ID`, QoS(구독이 transient local을 요구하는지)를 확인한다.
- `orma react`가 `orma-collectd does not see the publisher`로 종료된다
  - 원인: 발행 프로세스가 `preload`로 시작되지 않았다.
  - 조치: `orma env`가 출력하는 `LD_PRELOAD`의 파일이 있는지(`orma_preload`를 빌드했는지) 확인한다. 선택에 노드나 토픽 조건이 있으면 발행 프로세스가 빠질 가능성이 있으므로, 선택을 pid나 모든 프로세스로 바꾼 뒤 다시 실행한다.

## overflow 경고

`orma node cpu` 아래의 `warning: pid N (...)`은 `preload` 프로세스의 공유 메모리 표가 찼다는 뜻이다.

- 표 칸이나 깊이를 넘친 sample은 window 집계에서만 제외되고, 기록의 이벤트는 남는다.
- 스레드 칸이 넘치면 그 스레드는 이벤트도 기록되지 않는다.

| 경고 | 한도 | 조치 |
| --- | --- | --- |
| `threads not traced (thread slots full)` | 프로세스당 스레드 256개 | 스레드 풀을 줄인다 |
| `samples not aggregated (thread table full)` | 스레드당 콜백 63개, malloc attribution 활성화 시 31개 | `--malloc`을 비활성화하거나, 콜백을 executor 스레드 여럿에 나눈다 |
| `callbacks not timed (nested deeper than 16)` | 콜백 중첩 16단 | 보통 재귀 호출이 원인이다 |

## `orma check` 항목

- `probes <set>`: 수집 지점마다 부착된 수와 실제로 호출된 수. `not fired`는 검사 중에 그 호출이 없었다는 뜻이다. 검사 중에 노드를 새로 실행하면 init 지점이 호출된다.
- `missing <point>`: 라이브러리가 그 함수를 export하지 않는다. Jazzy에서 uprobe의 `rclcpp_callback_register`(gate)는 항상 `unknown`이다.
- `distro pid N`
  - `fail`: publish ABI를 잘못 판정했다.
  - `unknown`(no publish after a publisher init): ormad가 그 프로세스보다 늦게 시작되어 비교할 init이 없다.
- `cyclone serdata_rmw::m_size offset`: `unknown`이면 Cyclone dbgsym 패키지가 없다. Cyclone bandwidth만 제외된다.
- `flux contract <library>`: `fail`이면 라이브러리의 trace contract 버전이 맞지 않아 orma가 그 라이브러리에 부착하지 않는다. 같은 커밋의 flux로 다시 빌드한다.
- `container pid N`: 컨테이너 안 pid와 호스트 pid의 대응을 검사한다. `fail`이면 orma가 그 컨테이너의 스레드를 다른 프로세스로 셌다.
- `bpf load`: 항상 `pass`. 로드한 program 수와 verifier가 처리한 instruction 수를 보인다. 부착 결과는 `probes <set>` 항목에 있다.
- `lockdown`: `fail`(`confidentiality`)이면 tracefs가 닫혀 sched probe가 부착되지 않는다.
- `bpf_jit_enable`, `bpf_jit_harden`: `fail`이면 interpreter(0)나 constant blinding(2)이 활성화되어 있어 측정한 비용이 맞지 않는다.
- `perf_event_paranoid`, `kptr_restrict`, `unprivileged_bpf_disabled`, `BTF`: 값만 출력한다. ormad는 capability로 동작하고 BTF를 사용하지 않는다.
- `proc hidepid`: `unknown`이면 hidepid가 활성화되어 있고, orma-collectd는 자기 사용자의 프로세스만 본다.
- `preload pid N`: `fail`이면 `LD_PRELOAD`가 orma 라이브러리를 가리키는데 로드되지 않았다. `AT_SECURE`가 1이면 secure-exec(setuid, file capability, LSM 전환)가 이유다.
- `time namespace pid N`: `fail`이면 다른 time namespace이고 monotonic offset이 0이 아니다. 그 프로세스의 `preload`, `lttng` 레코드 시각이 그만큼 어긋난다.
- `rcl_wait returns pid N`: `fail`이면 한 스레드의 진입과 반환 수가 2 이상 다르다. uretprobe가 반환을 놓쳤다.
- `executor probes pid N`: 프로세스에 부착된 executor probe(`stock` 또는 `rcl_executor`). `rcl_executor`이면 core의 wait 수, claim 수, rcl handle을 읽은 claim 수, complete 수를 보인다. `unknown`이면 검사 중에 core의 wait와 claim이 없었다.
