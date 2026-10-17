# 설치

지원하는 ROS 2 배포판은 Humble과 Jazzy다.

armo는 프로세스 셋으로 돈다.

| 프로세스 | 권한 | 띄우는 것 |
| --- | --- | --- |
| `armod` | eBPF 권한을 가진 시스템 서비스(사용자 `armo`) | systemd 시스템 유닛 `armod.service`. 부팅할 때 켠다 |
| `armo-collectd` | 사용자 권한. 사용자마다 하나 | `armo` 명령이나 rqt 플러그인이 처음 붙을 때 띄운다. systemd 사용자 유닛으로 둘 수도 있다 |
| 관측 대상 | 그대로 | 사용자가 띄운다. preload와 LTTng 방식은 띄울 때 환경변수를 넣는다([usage.md](usage.md)) |

```text
ROS 프로세스 --uprobe--> armod --/run/armo/armod.sock--> armo-collectd --> armo, rqt_armo
ROS 프로세스 --공유 메모리, LTTng-----------------------> armo-collectd
```

## 1. 빌드

eBPF 프로그램을 빌드하려면 nightly 툴체인과 `bpf-linker`가 필요하다.

```bash
rustup toolchain install nightly-2026-09-28 --profile minimal --component rust-src
curl -LO https://github.com/aya-rs/bpf-linker/releases/download/v0.11.1/bpf-linker-aarch64-unknown-linux-musl.tar.zst
tar -xpf bpf-linker-aarch64-unknown-linux-musl.tar.zst -C ~/.local/bin
sudo apt install liblttng-ust-dev lttng-tools babeltrace

cd ~/flux_devel/flux_ws
colcon build --symlink-install --packages-up-to armo_cli rqt_armo armo_daemon armo_collectd armo_preload armo_lttng armo_rclpy
source install/setup.bash
```

- `bpf-linker`는 LLVM 23이 필요하다. Ubuntu 24.04에는 그 패키지가 없어 릴리스 바이너리를 쓴다.
- `lttng-tools`, `babeltrace`(1.5)는 LTTng 방식을 쓸 때만 필요하다.

## 2. armod 서비스

워크스페이스를 source한 셸에서 돌린다.

```bash
src/armo/daemon/armo_daemon/systemd/install.sh
sudo systemctl enable --now armod
```

`install.sh`가 하는 일:

1. `armod`를 `/usr/local/sbin/armod`에 복사한다.
2. 사용자와 그룹 `armo`를 만든다(`/etc/sysusers.d/armo.conf`).
3. 유닛을 `/etc/systemd/system/armod.service`에 둔다.
4. 지금 사용자를 그룹 `armo`에 넣는다.

- 복사하는 이유: 홈 디렉터리(`0750`) 아래의 워크스페이스에는 서비스 사용자 `armo`가 들어가지 못한다.
- 그룹은 다음 로그인부터 적용된다. 모든 세션에서 로그아웃한 뒤 다시 로그인한다. 그 전에는 armo-collectd가 armod에 붙지 못한다(`armo status`의 `armod: denied`).
- `sg armo`로 armo-collectd를 띄우면 armod에는 붙지만 쓰지 않는다. 주 그룹이 바뀌어 커널이 같은 사용자 프로세스의 `smaps_rollup`도 막고, `--pss`를 켜도 PSS가 `-`로 나온다.

서비스가 받는 권한은 다섯이다. 설계 문서 5.1에 근거가 있다.

| 권한 | 쓰는 곳 |
| --- | --- |
| `CAP_BPF`, `CAP_PERFMON` | eBPF 프로그램을 올리고 uprobe와 트레이스포인트에 붙인다 |
| `CAP_SYS_ADMIN` | eBPF 실행 시간 통계(`BPF_ENABLE_STATS`). Ubuntu 7.0 커널은 이것 없이 uprobe의 `perf_event_open`도 거부했다(`EACCES`) |
| `CAP_SYS_PTRACE` | 다른 사용자 프로세스의 `/proc/<pid>/maps`, `/proc/<pid>/root`. 없으면 아무 프로브도 붙지 않는다 |
| `CAP_DAC_READ_SEARCH` | tracefs(`/sys/kernel/tracing`)의 이벤트 `format`, `id`. 없으면 sched 프로브가 꺼진다 |

유닛은 armod를 가둔다. 파일 시스템은 읽기 전용(`/run/armo` 제외)이고, 소켓은 unix만, namespace 생성 금지, 시스템 호출은 `@system-service`에 `bpf`, `perf_event_open`만 더한 목록이다(그 밖은 `EPERM`). `ProtectProc=invisible`, `ProcSubset=pid`는 다른 사용자의 `/proc/<pid>/maps`를 가리므로 쓰지 않는다. 5.11 미만 커널(JetPack 5)은 맵 메모리를 `RLIMIT_MEMLOCK`에 달아서 `LimitMEMLOCK=infinity`를 둔다. `install.sh`가 끝에 `systemd-analyze security armod.service`의 점수를 찍는다(3.4, 가두기 전 7.2).

확인:

```bash
systemctl status armod
armo status            # armod: connected
armo check         # fail이 없으면 종료 코드 0
```

### 다시 빌드한 뒤

서비스는 복사한 실행 파일을 쓴다. armod를 다시 빌드하면 복사와 재시작을 다시 한다.

```bash
src/armo/daemon/armo_daemon/systemd/install.sh
sudo systemctl restart armod
```

### 개발 중

- 서비스가 떠 있으면 `sudo armod`는 "another armod serves it"로 끝난다. 같은 소켓을 두 armod가 나눠 쓰지 않게 하기 위해서다.
- 빌드 트리의 armod를 쓰려면 서비스를 끄고 전경에서 띄운다.

```bash
sudo systemctl stop armod
sudo install/armo_daemon/bin/armod
```

- 다른 소켓에 하나 더 띄울 수도 있다. 테스트가 쓰는 방법이다.

```bash
sudo install/armo_daemon/bin/armod --socket /tmp/armod.sock --group "$(id -gn)"
```

- 통합 테스트(`armo_integration`)는 `/run/armo/armod.sock`에 이미 armod가 있으면 그것을 쓴다. 서비스가 떠 있으면 복사한 실행 파일로 시험하게 되므로, armod를 고친 뒤에는 위의 "다시 빌드한 뒤"를 먼저 한다.

### 지우기

```bash
sudo systemctl disable --now armod
sudo rm /etc/systemd/system/armod.service /usr/local/sbin/armod /etc/sysusers.d/armo.conf
sudo systemctl daemon-reload
sudo userdel armo
```

## 3. armo-collectd

따로 설치하지 않아도 된다. `armo` 명령이나 rqt 플러그인이 붙을 때 없으면 띄운다. 기록도 클라이언트도 없는 상태가 5분 이어지면 스스로 끝난다.

systemd 사용자 유닛으로 두려면:

```bash
systemctl --user link install/armo_collectd/share/armo_collectd/systemd/armo-collectd.service
```

유닛이 있으면 클라이언트는 `systemctl --user start armo-collectd`로 띄운다.

- 소켓은 `$XDG_RUNTIME_DIR/armo/collectd.sock`이다. `XDG_RUNTIME_DIR`이 없는 셸(예: `docker exec --user`)에서는 붙지 못한다.
- 설정 파일은 `~/.config/armo/collectd.toml`이다. 기록 한도의 상한을 둔다([usage.md](usage.md)의 기록 절).
