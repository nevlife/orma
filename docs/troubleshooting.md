# 문제 해결

먼저 `armo status`와 `armo check`를 본다([usage.md](usage.md) 1절).

## armod

| 증상 | 원인과 조치 |
| --- | --- |
| `armod: absent` | armod가 없다. `systemctl status armod`. 꺼져 있으면 `sudo systemctl start armod` |
| `armod: denied` | 그룹 `armo`가 아니다. `id`에 `armo`가 없으면 모든 세션에서 로그아웃한 뒤 다시 로그인한다. 이미 떠 있던 armo-collectd는 예전 그룹을 가지므로 `pkill -x armo-collectd`로 끝낸다. 다음 `armo` 명령이 다시 띄운다 |
| `armod: lost` | armod가 죽었거나 재시작됐다. 서비스는 실패하면 다시 뜨고(`Restart=on-failure`) armo-collectd가 다시 붙는다. `journalctl -u armod`로 이유를 본다 |
| `sudo armod`가 "another armod serves it"로 끝난다 | 서비스가 떠 있다. 빌드 트리의 armod를 쓰려면 `sudo systemctl stop armod` 뒤에 띄운다 |
| 고친 armod가 반영되지 않는다 | 서비스는 `/usr/local/sbin/armod` 사본을 쓴다. `daemon/armo_daemon/systemd/install.sh` 뒤에 `sudo systemctl restart armod` |
| 로그에 `the scheduler probes stay off` | tracefs를 읽지 못했다. 서비스 유닛의 권한에 `CAP_DAC_READ_SEARCH`가 있는지 본다 |
| 방식과 Hz가 `-`다 | `--pid` 없이 모니터링하면 uprobe를 붙이지 않는다([usage.md](usage.md) 4절). `--pid`로 고른다. 그래도 없으면 `journalctl -u armod`에 `attached to` 줄이 있는지 본다. 없으면 다른 사용자 프로세스의 `/proc/<pid>/maps`를 못 읽은 것이다(`CAP_SYS_PTRACE`) |
| `bpf stats`가 fail | `BPF_ENABLE_STATS`가 거부됐다(`CAP_SYS_ADMIN`). eBPF 실행 시간이 0으로 남는다 |

## armo-collectd

| 증상 | 원인과 조치 |
| --- | --- |
| `XDG_RUNTIME_DIR is not set` | 로그인 세션이 아닌 셸이다(`docker exec`, `sudo -u`). 그 셸에서는 armo를 쓰지 못한다 |
| `armo status`가 "not running" | `armo status`는 armo-collectd를 띄우지 않는다. 다른 `armo` 명령이나 rqt가 띄운다 |
| PSS가 `-` | PSS를 켜지 않았다(rqt의 `pss`). 켰는데도 `-`이면 armo-collectd가 대상과 다른 주 그룹으로 떴다(`sg armo`로 띄운 경우). 끝내고 다시 띄운다 |
| `method lttng needs lttng, lttng-sessiond, babeltrace and setpriv on PATH` | LTTng 도구가 없다. `sudo apt install lttng-tools babeltrace` |
| 노드가 `unlinked`, `unlinked (cold)` | armo가 노드를 만드는 순간을 보지 못했다. rclcpp 프로세스는 `--pid`로 고르면 몇 초 뒤 executor에서 노드를 읽어 채운다. 남으면 [usage.md](usage.md) 4절의 남는 경우다. 클라이언트를 먼저 열어 둔 채 노드를 다시 띄운다 |
| `--node`, `--topic`으로 골랐는데 아무것도 안 나온다 | 그 노드를 armo가 모른다(위 행). armod를 막 띄웠다면 init 프로브가 붙는 몇 초 뒤에 노드를 띄운다. 정규식은 이름 전체에 맞춘다(`/cam`이 아니라 `/cam.*`) |
| `preload processes: 0`인데 `armo run`으로 띄웠다 | 그 프로세스가 `LD_PRELOAD`를 받지 못했다. launch 파일이면 `ArmoTrace`가 노드보다 앞에 있는지 본다. `armo env`를 노드를 띄울 셸에서 `eval`했는지 본다 |

## 기록

| 증상 | 원인과 조치 |
| --- | --- |
| 시작을 거절한다(디스크) | 여유가 하한 + 1 GiB보다 작다. 공간을 비우거나 `-o`로 다른 파일시스템을 고른다 |
| `max_duration_s ... above the cap` | `~/.config/armo/collectd.toml`의 `max_duration_s`보다 길다 |
| `state: truncated` | armo-collectd가 마무리 전에 죽었다(SIGKILL, 전원). 다음 시작이나 `armo record show`가 개수를 다시 세어 고친다 |
| `record show`의 `lost`가 0이 아니다 | 이벤트를 넘겨받기 전에 버퍼가 넘쳤다. `meta.json`의 `drops`가 출처(공유 메모리 링, armod ring buffer, LTTng)를 알려 준다 |

## 추가 수집과 체인

| 증상 | 원인과 조치 |
| --- | --- |
| `armo top`의 `gpus` 표가 비었다 | NVML(`libnvidia-ml.so.1`)이 없거나 GPU가 없다. armo-collectd는 NVML을 링크하지 않고 띄울 때 찾는다. dGPU는 NVIDIA 드라이버의 NVML이 있어야 하고, Jetson은 sysfs에서 사용률과 클럭만 읽는다 |
| rqt의 CUDA 표와 `cb`의 CUDA 열이 없다 | `--gpu`(rqt의 `gpu`)를 켜지 않았다. cudart를 정적으로 링크한 프로그램(nvcc 기본)도 잡힌다. cudart가 `cuGetProcAddress`로 driver 함수를 얻는 경우는 preload와 LTTng가 그 함수도 가로채 주소를 바꿔 준다. uprobe는 함수 주소에 붙으므로 상관없다 |
| `cb`의 futex 열이 `-`다 | futex 대기는 방식과 무관하게 armod가 잰다. `armod: connected`인지 본다 |
| `topic`에 DDS 열이 없다 | `--dds`를 켜지 않았거나 그 토픽의 `rmw_publish`와 DDS write를 같은 스레드에서 짝짓지 못했다. 짝이 없는 write는 버린다 |
| `span` 보기가 비었다 | 프로세스가 `libarmo_span.so`를 링크하지 않았거나 그 프로세스가 선택에 없다. 이름은 문자열 리터럴로 넘긴다. 주소가 다르면 begin과 end가 이어지지 않는다 |
| 체인의 broken이 크다 | 체인의 토픽을 발행하는 프로세스 중 관측하지 않은 것이 있다. `first` 방식이면 원천 주기가 체인 지연보다 짧은지 본다 |
| `armo react`가 `no subscriber on ...`으로 끝난다 | 10초 안에 입력 토픽에 구독자가 붙지 않았다. 토픽 이름과 `ROS_DOMAIN_ID`, QoS(구독이 transient local을 요구하는지)를 본다 |
| `armo react`가 `armo-collectd does not see the publisher`로 끝난다 | 발행 프로세스가 preload로 올라오지 않았다. `armo env`가 찍는 `LD_PRELOAD`의 파일이 있는지(`armo_preload`를 빌드했는지) 본다. 선택에 `--node`, `--topic`이 있으면 발행 프로세스가 맞지 않을 수 있다. 선택을 pid나 모든 프로세스로 바꾼 뒤 다시 돌린다 |

## 넘침 경고

`armo node cpu` 아래의 `warning: pid N (...)`은 preload 프로세스의 공유 메모리 표가 찼다는 뜻이다. 표 칸과 깊이가 넘친 몫은 창 집계에서만 빠지고 기록의 이벤트는 남는다. 스레드 칸이 넘친 스레드는 이벤트도 기록되지 않는다.

| 경고 | 한도 | 조치 |
| --- | --- | --- |
| `threads not traced (thread slots full)` | 프로세스당 스레드 256개 | 스레드 풀을 줄인다 |
| `samples not aggregated (thread table full)` | 스레드당 콜백 63개, malloc 귀속을 켜면 31개 | `--malloc`을 끈다. 콜백을 executor 스레드 여럿에 나눈다 |
| `callbacks not timed (nested deeper than 16)` | 콜백 안 콜백 16단 | 보통 재귀 호출이다 |

## selfcheck 항목

| 항목 | `fail`이나 `unknown`일 때 |
| --- | --- |
| `probes <묶음>` | 지점마다 붙은 수와 실제로 불린 수다. `not fired`는 검사 동안 그 일이 없었다는 뜻이다. 검사 중에 노드를 새로 띄우면 init 지점이 불린다 |
| `missing <지점>` | 그 라이브러리가 함수를 내보내지 않는다. Jazzy의 rmw 서비스 트레이스포인트, uprobe의 `rclcpp_callback_register`(게이트 때문)는 늘 `unknown`이다 |
| `distro pid N` | `fail`이면 publish ABI 판정이 틀렸다. `unknown`(no publish after a publisher init)은 armod가 그 프로세스보다 늦게 떠 비교할 init이 없다는 뜻이다 |
| `cyclone serdata_rmw::m_size offset` | `unknown`이면 Cyclone의 dbgsym 패키지가 없다. Cyclone 대역폭만 빠진다 |
| `flux contract`, `flux2 contract` | `fail`이면 라이브러리의 트레이스 계약 버전이 armo와 다르다. armo는 그 라이브러리에 붙지 않는다. 같은 커밋의 flux, flux2로 다시 빌드한다 |
| `container pid N` | 컨테이너 안 pid와 호스트 pid의 대응을 본다. `fail`이면 armo가 그 컨테이너의 스레드를 다른 프로세스로 셌다는 뜻이다 |
