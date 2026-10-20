# 문제 해결

먼저 `orma status`와 `orma check`를 본다([usage.md](usage.md) 1절).

## ormad

| 증상 | 원인 | 조치 |
| --- | --- | --- |
| `ormad: absent` | ormad 미실행 | `systemctl status ormad`. 꺼져 있으면 `sudo systemctl start ormad` |
| `ormad: denied` | group `orma`가 아니다 | `id`에 `orma`가 없으면 모든 session에서 로그아웃 후 다시 로그인. 이미 떠 있던 orma-collectd는 예전 group이므로 `pkill -x orma-collectd`(다음 `orma` 명령이 다시 시작) |
| `ormad: lost` | ormad 종료나 재시작 | service는 `Restart=on-failure`로 다시 시작하고 orma-collectd가 다시 연결. 이유는 `journalctl -u ormad` |
| `sudo ormad`가 "another ormad serves it"로 종료 | service가 실행 중 | `sudo systemctl stop ormad` 후 실행 |
| 다시 빌드한 ormad가 반영되지 않는다 | service는 `/usr/local/sbin/ormad` 사본 사용 | `daemon/orma_daemon/systemd/install.sh` 후 `sudo systemctl restart ormad` |
| 로그에 `the scheduler probes stay off` | tracefs 읽기 실패 | service unit에 `CAP_DAC_READ_SEARCH`가 있는지 확인 |
| method와 Hz가 `-` | node를 지정하지 않은 monitoring은 uprobe 미부착([usage.md](usage.md) 4절) | node 이름이나 `--pid`로 선택. 그래도 없으면 `journalctl -u ormad`에 `not attached to` 줄 확인(이유가 함께 나온다). 그 줄도 없으면 `CAP_SYS_PTRACE` 부재로 다른 사용자 process의 `/proc/<pid>/maps`를 못 읽은 것 |
| `bpf stats`가 `fail` | `BPF_ENABLE_STATS` 거부(`CAP_SYS_ADMIN`) | eBPF 실행 시간만 0. 다른 기능은 그대로 |

## orma-collectd

| 증상 | 원인 | 조치 |
| --- | --- | --- |
| `XDG_RUNTIME_DIR is not set` | 로그인 session이 아닌 shell(`docker exec`, `sudo -u`) | 로그인 session의 shell에서 실행 |
| `orma status`가 "not running" | `orma status`는 orma-collectd를 시작하지 않는다 | 다른 `orma` 명령이나 rqt로 시작 |
| PSS가 `-` | PSS를 켜지 않았다. 켰다면 orma-collectd가 다른 주 group으로 실행됨(`sg orma`) | rqt의 `pss` 체크. `sg orma`였다면 종료 후 다시 시작 |
| `method lttng needs lttng, lttng-sessiond, babeltrace and setpriv on PATH` | LTTng 도구 부재 | `sudo apt install lttng-tools babeltrace` |
| node가 `unlinked`, `unlinked (cold)` | orma가 node 생성 시점을 놓쳤다 | rclcpp process는 `--pid`로 선택하면 수 초 뒤 보완. 그래도 남으면 [usage.md](usage.md) 4절의 예외. client를 먼저 열고 node를 다시 실행 |
| node나 topic 이름으로 골랐는데 아무것도 안 나온다 | orma가 그 node를 모른다(위 행) | ormad를 막 시작했다면 init probe가 붙는 수 초 뒤에 node 실행. `orma node`, `orma topic`은 이름 그대로 매칭. `orma record`의 `--node`, `--topic`은 이름 전체에 맞는 정규식(`/cam`이 아니라 `/cam.*`) |
| `orma run`으로 실행했는데 `preload processes: 0` | process가 `LD_PRELOAD`를 받지 못했다 | launch 파일이면 `OrmaTrace`가 node보다 앞에 있는지, `orma env`를 node를 실행할 shell에서 `eval`했는지 확인 |

## recording

| 증상 | 원인 | 조치 |
| --- | --- | --- |
| 시작 거부(디스크) | 여유 공간이 하한 + 1 GiB 미만 | 공간 확보, 또는 `-o`로 다른 파일시스템 |
| `max_duration_s ... above the cap` | `~/.config/orma/collectd.toml`의 `max_duration_s` 초과 | 길이를 줄이거나 설정 상향 |
| `state: truncated` | orma-collectd가 마무리 전에 종료(SIGKILL, 전원) | 다음 시작이나 `orma record show`가 개수를 다시 세어 복구 |
| `record show`의 `lost`가 0이 아니다 | event 전달 전에 buffer 넘침 | `meta.json`의 `drops`에서 위치 확인(shared memory ring, ormad ring buffer, LTTng) |

## 추가 수집과 chain

| 증상 | 원인 | 조치 |
| --- | --- | --- |
| `orma top`의 `gpus` 표가 비었다 | NVML(`libnvidia-ml.so.1`) 부재 또는 GPU 부재. orma-collectd는 NVML을 링크하지 않고 시작 시 찾는다 | dGPU는 NVIDIA driver의 NVML 필요. Jetson은 sysfs에서 사용률과 clock만 읽는다 |
| `orma node cb`의 CUDA 열, rqt의 CUDA 표가 없다 | `--gpu`(rqt의 `gpu`)를 켜지 않았다 | `--gpu`. cudart를 정적으로 링크한 프로그램(nvcc 기본)도 잡힌다. cudart가 `cuGetProcAddress`로 driver 함수를 얻으면 `preload`와 `lttng`이 그 함수도 가로채 주소를 바꾼다. uprobe는 함수 주소에 붙으므로 무관 |
| `orma node cb`의 futex 열이 `-` | futex 대기는 method와 무관하게 ormad가 잰다 | `orma status`가 `ormad: connected`인지 확인 |
| `orma topic delay`에 DDS 열이 없다 | `--dds`를 켜지 않았거나, 그 topic의 publish와 DDS write를 같은 thread에서 짝짓지 못했다 | `--dds`. 짝이 없는 write는 버린다 |
| `orma span`이 비었다 | process가 `liborma_span.so`를 링크하지 않았거나 선택에 없다 | 이름은 문자열 literal로 넘긴다. 주소가 다르면 begin과 end가 이어지지 않는다 |
| chain의 broken이 크다 | chain의 topic을 publish하는 process 중 관측하지 않은 것이 있다 | 그 process도 선택. `first` 방식이면 원천 주기가 chain latency보다 긴지 확인 |
| `orma react`가 `no subscriber on ...`으로 종료 | 10초 안에 입력 topic에 subscriber가 붙지 않았다 | topic 이름, `ROS_DOMAIN_ID`, QoS(subscription이 transient local을 요구하는지) 확인 |
| `orma react`가 `orma-collectd does not see the publisher`로 종료 | publish process가 `preload`로 올라오지 않았다 | `orma env`가 출력하는 `LD_PRELOAD`의 파일이 있는지(`orma_preload` 빌드 여부) 확인. 선택에 node나 topic 조건이 있으면 publish process가 빠질 수 있다. 선택을 pid나 모든 process로 바꾼 뒤 다시 실행 |

## overflow 경고

`orma node cpu` 아래의 `warning: pid N (...)`은 `preload` process의 shared memory 표가 찼다는 뜻이다.

- 표 칸과 깊이 초과분: window aggregate에서만 빠지고 recording의 event는 남는다
- thread 칸 초과: 그 thread는 event도 기록되지 않는다

| 경고 | 한도 | 조치 |
| --- | --- | --- |
| `threads not traced (thread slots full)` | process당 thread 256개 | thread pool 축소 |
| `samples not aggregated (thread table full)` | thread당 callback 63개, malloc attribution을 켜면 31개 | `--malloc` 끄기. callback을 executor thread 여럿에 분산 |
| `callbacks not timed (nested deeper than 16)` | callback 중첩 16단 | 보통 재귀 호출 |

## `orma check` 항목

| 항목 | `fail`이나 `unknown`의 뜻 |
| --- | --- |
| `probes <set>` | point마다 부착 수와 실제 호출 수. `not fired`는 검사 중 그 일이 없었다는 뜻. 검사 중 node를 새로 실행하면 init point가 호출된다 |
| `missing <point>` | library가 그 함수를 export하지 않는다. Jazzy에서 uprobe의 `rclcpp_callback_register`(gate)는 항상 `unknown` |
| `distro pid N` | `fail`: publish ABI 판정 오류. `unknown`(no publish after a publisher init): ormad가 그 process보다 늦게 떠 비교할 init 부재 |
| `cyclone serdata_rmw::m_size offset` | `unknown`: Cyclone dbgsym package 부재. Cyclone bandwidth만 빠진다 |
| `flux contract <library>` | `fail`: library의 trace contract 버전 불일치. orma는 그 library에 붙지 않는다. 같은 커밋의 flux로 다시 빌드 |
| `container pid N` | container 안 pid와 host pid의 대응 검사. `fail`: orma가 그 container의 thread를 다른 process로 센 상태 |
| `bpf load` | 항상 `pass`. 올린 program 수와 verifier가 처리한 instruction 수. 부착은 `probes <set>` 행 |
| `lockdown` | `fail`(`confidentiality`): tracefs가 닫혀 sched probe가 붙지 않는다 |
| `bpf_jit_enable`, `bpf_jit_harden` | `fail`: interpreter(0)나 constant blinding(2). 측정한 비용이 맞지 않는다 |
| `perf_event_paranoid`, `kptr_restrict`, `unprivileged_bpf_disabled`, `BTF` | 값만 출력. ormad는 capability로 동작하고 BTF를 쓰지 않는다 |
| `proc hidepid` | `unknown`: 켜져 있다. orma-collectd는 자기 사용자의 process만 본다 |
| `preload pid N` | `fail`: `LD_PRELOAD`가 orma library를 가리키는데 load되지 않았다. `AT_SECURE`가 1이면 secure-exec(setuid, file capability, LSM 전환)가 이유 |
| `time namespace pid N` | `fail`: 다른 time namespace이고 monotonic offset이 0이 아니다. 그 process의 `preload`, `lttng` record 시각이 그만큼 어긋난다 |
| `rcl_wait returns pid N` | `fail`: 한 thread의 진입과 반환 수가 2 이상 다르다. uretprobe가 반환을 놓쳤다 |
