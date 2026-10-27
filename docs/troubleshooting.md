# 문제 해결

먼저 `orma status`와 `orma check`를 본다([usage.md](usage.md) 1절).

## ormad

| 증상 | 원인 | 조치 |
| --- | --- | --- |
| `ormad: absent` | ormad가 돌지 않는다 | `systemctl status ormad`로 본다. 꺼져 있으면 `sudo systemctl start ormad` |
| `ormad: denied` | 사용자가 group `orma`에 없다 | `id`에 `orma`가 없으면 모든 session에서 로그아웃한 뒤 다시 로그인한다. 이미 떠 있던 orma-collectd는 예전 group으로 돌고 있으므로 `pkill -x orma-collectd`로 끝낸다. 다음 `orma` 명령이 다시 시작한다 |
| `ormad: lost` | ormad가 끝났거나 재시작했다 | service는 `Restart=on-failure`로 다시 시작하고, orma-collectd가 다시 연결한다. 끝난 이유는 `journalctl -u ormad`에서 본다 |
| `sudo ormad`가 "another ormad serves it"로 끝난다 | service가 이미 돌고 있다 | `sudo systemctl stop ormad`로 멈춘 뒤 실행한다 |
| 다시 빌드한 ormad가 반영되지 않는다 | service는 `/usr/local/sbin/ormad`에 복사한 사본을 쓴다 | `daemon/orma_daemon/systemd/install.sh`를 돌린 뒤 `sudo systemctl restart ormad` |
| 로그에 `the scheduler probes stay off` | tracefs를 읽지 못했다 | service unit에 `CAP_DAC_READ_SEARCH`가 있는지 확인한다 |
| method와 Hz가 `-` | node를 지정하지 않은 monitoring에서는 uprobe를 붙이지 않는다([usage.md](usage.md) 4절) | node 이름이나 `--pid`로 고른다. 그래도 값이 없으면 `journalctl -u ormad`에서 `not attached to` 줄을 찾는다(이유가 함께 나온다). 그 줄도 없으면 `CAP_SYS_PTRACE`가 없어 다른 사용자 process의 `/proc/<pid>/maps`를 읽지 못한 것이다 |
| `bpf stats`가 `fail` | kernel이 `BPF_ENABLE_STATS`를 거부했다(`CAP_SYS_ADMIN`) | eBPF 실행 시간만 0으로 나온다. 다른 기능은 그대로 동작한다 |

## orma-collectd

| 증상 | 원인 | 조치 |
| --- | --- | --- |
| `XDG_RUNTIME_DIR is not set` | 로그인 session이 아닌 shell(`docker exec`, `sudo -u`) | 로그인 session의 shell에서 실행 |
| `orma status`가 "not running" | `orma status`는 orma-collectd를 시작하지 않는다 | 다른 `orma` 명령이나 rqt로 시작한다 |
| PSS가 `-` | PSS를 켜지 않았다. 켰다면 orma-collectd가 다른 주 group으로 실행됐다(`sg orma`) | rqt의 `pss` 체크를 켠다. `sg orma`로 실행했다면 끝내고 다시 시작한다 |
| `method lttng needs lttng, lttng-sessiond, babeltrace and setpriv on PATH` | LTTng 도구가 없다 | `sudo apt install lttng-tools babeltrace` |
| node가 `unlinked`, `unlinked (cold)` | orma가 node 생성 시점을 놓쳤다 | rclcpp process는 `--pid`로 고르면 수 초 뒤에 node가 채워진다. 그래도 남으면 [usage.md](usage.md) 4절의 예외에 해당한다. client를 먼저 열고 node를 다시 실행한다 |
| node나 topic 이름으로 골랐는데 아무것도 안 나온다 | orma가 그 node를 모른다(위 행) | ormad를 막 시작했다면 init probe가 붙을 때까지 수 초 기다린 뒤 node를 실행한다. `orma node`, `orma topic`은 이름을 그대로 비교한다. `orma record`의 `--node`, `--topic`은 이름 전체에 맞는 정규식이다(`/cam`이 아니라 `/cam.*`) |
| `orma run`으로 실행했는데 `preload processes: 0` | process가 `LD_PRELOAD`를 받지 못했다 | launch 파일이면 `OrmaTrace`가 node보다 앞에 있는지 확인한다. `orma env`를 썼다면 node를 실행할 shell에서 `eval`했는지 확인한다 |

## recording

| 증상 | 원인 | 조치 |
| --- | --- | --- |
| 시작 거부(디스크) | 여유 공간이 하한 + 1 GiB보다 적다 | 공간을 비우거나, `-o`로 다른 파일시스템에 쓴다 |
| `max_duration_s ... above the cap` | 길이가 `~/.config/orma/collectd.toml`의 `max_duration_s`를 넘는다 | 길이를 줄이거나 설정값을 올린다 |
| `state: truncated` | orma-collectd가 마무리 전에 끝났다(SIGKILL, 전원) | orma-collectd가 다음에 시작할 때나 `orma record show`가 개수를 다시 세어 복구한다 |
| `record show`의 `lost`가 0이 아니다 | event가 전달되기 전에 buffer가 넘쳤다 | `meta.json`의 `drops`에서 어디서 넘쳤는지 확인한다(shared memory ring, ormad ring buffer, LTTng) |

## 추가 수집과 chain

| 증상 | 원인 | 조치 |
| --- | --- | --- |
| `orma top`의 `gpus` 표가 비었다 | NVML(`libnvidia-ml.so.1`)이나 GPU가 없다. orma-collectd는 NVML을 링크하지 않고 시작할 때 찾는다 | dGPU에는 NVIDIA driver의 NVML이 필요하다. Jetson에서는 sysfs에서 사용률과 clock만 읽는다 |
| `orma node cb`의 CUDA 열, rqt의 CUDA 표가 없다 | `--gpu`(rqt의 `gpu`)를 켜지 않았다 | `--gpu`를 켠다. cudart를 정적으로 링크한 프로그램(nvcc 기본)도 잡힌다. cudart가 `cuGetProcAddress`로 driver 함수를 얻으면 `preload`와 `lttng`이 그 함수도 가로채 주소를 바꾼다. uprobe는 함수 주소에 붙으므로 이 경로와 상관없다 |
| `orma node cb`의 futex 열이 `-` | futex 대기는 method와 무관하게 ormad가 잰다 | `orma status`가 `ormad: connected`인지 확인 |
| `orma topic delay`에 DDS 열이 없다 | `--dds`를 켜지 않았거나, 그 topic의 publish와 DDS write를 같은 thread에서 짝짓지 못했다 | `--dds`를 켠다. 짝이 없는 write는 버린다 |
| `orma span`이 비었다 | process가 `liborma_span.so`를 링크하지 않았거나 선택에 없다 | 이름은 문자열 literal로 넘긴다. 주소가 다르면 begin과 end가 이어지지 않는다 |
| chain의 broken이 크다 | chain의 topic을 publish하는 process 중 관측하지 않은 것이 있다 | 그 process도 선택한다. `first` 방식이면 원천 주기가 chain latency보다 긴지 확인한다 |
| `orma react`가 `no subscriber on ...`으로 끝난다 | 10초 안에 입력 topic에 subscriber가 붙지 않았다 | topic 이름, `ROS_DOMAIN_ID`, QoS(subscription이 transient local을 요구하는지)를 확인한다 |
| `orma react`가 `orma-collectd does not see the publisher`로 끝난다 | publish process가 `preload`로 올라오지 않았다 | `orma env`가 출력하는 `LD_PRELOAD`의 파일이 있는지 확인한다(`orma_preload`를 빌드했는지). 선택에 node나 topic 조건이 있으면 publish process가 빠질 수 있다. 선택을 pid나 모든 process로 바꾼 뒤 다시 실행한다 |

## overflow 경고

`orma node cpu` 아래의 `warning: pid N (...)`은 `preload` process의 shared memory 표가 찼다는 뜻이다.

- 표 칸이나 깊이를 넘친 sample은 window aggregate에서만 빠진다. recording의 event는 남는다.
- thread 칸이 넘치면 그 thread는 event도 기록되지 않는다.

| 경고 | 한도 | 조치 |
| --- | --- | --- |
| `threads not traced (thread slots full)` | process당 thread 256개 | thread pool을 줄인다 |
| `samples not aggregated (thread table full)` | thread당 callback 63개, malloc attribution을 켜면 31개 | `--malloc`을 끈다. callback을 executor thread 여럿에 나눈다 |
| `callbacks not timed (nested deeper than 16)` | callback 중첩 16단 | 보통 재귀 호출이 원인이다 |

## `orma check` 항목

| 항목 | `fail`이나 `unknown`의 뜻 |
| --- | --- |
| `probes <set>` | point마다 붙은 수와 실제로 불린 수. `not fired`는 검사 중에 그 일이 없었다는 뜻이다. 검사 중에 node를 새로 실행하면 init point가 불린다 |
| `missing <point>` | library가 그 함수를 export하지 않는다. Jazzy에서 uprobe의 `rclcpp_callback_register`(gate)는 항상 `unknown` |
| `distro pid N` | `fail`: publish ABI를 잘못 판정했다. `unknown`(no publish after a publisher init): ormad가 그 process보다 늦게 떠서 비교할 init이 없다 |
| `cyclone serdata_rmw::m_size offset` | `unknown`: Cyclone dbgsym package가 없다. Cyclone bandwidth만 빠진다 |
| `flux contract <library>` | `fail`: library의 trace contract 버전이 맞지 않는다. orma는 그 library에 붙지 않는다. 같은 커밋의 flux로 다시 빌드한다 |
| `container pid N` | container 안 pid와 host pid가 맞게 대응하는지 검사한다. `fail`: orma가 그 container의 thread를 다른 process로 셌다 |
| `bpf load` | 항상 `pass`. 올린 program 수와 verifier가 처리한 instruction 수를 보인다. 부착 결과는 `probes <set>` 행에 있다 |
| `lockdown` | `fail`(`confidentiality`): tracefs가 닫혀 sched probe가 붙지 않는다 |
| `bpf_jit_enable`, `bpf_jit_harden` | `fail`: interpreter(0)나 constant blinding(2)이 켜져 있다. 이때 측정한 비용은 맞지 않는다 |
| `perf_event_paranoid`, `kptr_restrict`, `unprivileged_bpf_disabled`, `BTF` | 값만 출력한다. ormad는 capability로 동작하고 BTF를 쓰지 않는다 |
| `proc hidepid` | `unknown`: 켜져 있다. orma-collectd는 자기 사용자의 process만 본다 |
| `preload pid N` | `fail`: `LD_PRELOAD`가 orma library를 가리키는데 load되지 않았다. `AT_SECURE`가 1이면 secure-exec(setuid, file capability, LSM 전환)가 이유 |
| `time namespace pid N` | `fail`: 다른 time namespace이고 monotonic offset이 0이 아니다. 그 process의 `preload`, `lttng` record 시각이 그만큼 어긋난다 |
| `rcl_wait returns pid N` | `fail`: 한 thread의 진입과 반환 수가 2 이상 다르다. uretprobe가 반환을 놓쳤다 |
| `executor probes pid N` | process에 붙은 executor probe(`stock` 또는 `rcl_executor`). `rcl_executor`이면 core의 wait 수, claim 수, rcl handle을 읽은 claim 수, complete 수를 보인다. `unknown`: 검사 중에 core의 wait와 claim이 없었다 |
