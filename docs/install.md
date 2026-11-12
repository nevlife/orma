# 설치

지원하는 ROS 2 배포판: Humble, Jazzy

| 프로세스 | 권한 | 실행 |
| --- | --- | --- |
| `ormad` | eBPF 권한을 가진 시스템 서비스(사용자 `orma`) | systemd system unit `ormad.service`. 부팅 시 시작한다 |
| `orma-collectd` | 사용자 권한, 사용자당 하나 | `orma` 명령이나 rqt 플러그인의 첫 연결 시 자동 시작한다. systemd user unit도 가능하다 |
| 관측 대상 | 그대로 | 사용자가 직접 실행한다. `preload`, `lttng` 방식은 `orma run`이나 `OrmaTrace`로 실행한다([usage.md](usage.md) 3절) |

## 1. 빌드

```bash
rustup toolchain install nightly-2026-09-28 --profile minimal --component rust-src
curl -LO https://github.com/aya-rs/bpf-linker/releases/download/v0.11.1/bpf-linker-$(uname -m)-unknown-linux-musl.tar.zst
tar -xpf bpf-linker-$(uname -m)-unknown-linux-musl.tar.zst -C ~/.local/bin
sudo apt install liblttng-ust-dev lttng-tools babeltrace

cd ~/ros2_ws   # the colcon workspace with orma under src/
colcon build --symlink-install --packages-up-to orma_cli rqt_orma orma_daemon orma_collectd orma_preload orma_lttng orma_rclpy orma_diagnostics
source install/setup.bash
```

- nightly toolchain과 `bpf-linker`는 eBPF 프로그램 빌드에 사용한다.
- `bpf-linker`에는 LLVM 23이 필요하다. Ubuntu 24.04에는 그 패키지가 없으므로 LLVM 23을 포함한 릴리스 바이너리를 사용한다.
- `lttng-tools`, `babeltrace`(1.5)는 `lttng` 방식에서만 사용한다.

## 2. ormad 서비스

워크스페이스를 source한 셸에서 실행한다.

```bash
src/orma/daemon/orma_daemon/systemd/install.sh
sudo systemctl enable --now ormad
```

`install.sh`가 하는 일:

1. `ormad`를 `/usr/local/sbin/ormad`에 복사한다.
2. 사용자와 그룹 `orma`를 생성한다(`/etc/sysusers.d/orma.conf`).
3. unit을 `/etc/systemd/system/ormad.service`에 설치한다.
4. 현재 사용자를 그룹 `orma`에 추가한다.

- 실행 파일을 복사하는 이유: 서비스 사용자 `orma`는 홈 디렉터리(`0750`) 아래의 워크스페이스에 접근 불가능하다.
- 그룹은 다음 로그인부터 적용된다. 모든 세션에서 로그아웃한 뒤 다시 로그인한다. 그 전에는 `orma status`에 `ormad: denied`가 나온다.
- orma-collectd를 `sg orma`로 실행하지 않는다. 주 그룹이 바뀌어 같은 사용자 프로세스의 `smaps_rollup`도 읽지 못하므로, rqt의 `pss` 체크를 활성화해도 PSS가 `-`로 나온다.

서비스의 capability:

| capability | 용도 |
| --- | --- |
| `CAP_BPF`, `CAP_PERFMON` | eBPF 프로그램 로드, uprobe와 tracepoint 부착 |
| `CAP_SYS_ADMIN` | eBPF 실행 시간 통계(`BPF_ENABLE_STATS`). Ubuntu 7.0 커널은 이것 없이는 uprobe의 `perf_event_open`도 거부한다(`EACCES`) |
| `CAP_SYS_PTRACE` | 다른 사용자 프로세스의 `/proc/<pid>/maps`, `/proc/<pid>/root`. 없으면 probe 부착 불가능 |
| `CAP_DAC_READ_SEARCH` | tracefs(`/sys/kernel/tracing`)의 이벤트 `format`, `id`. 없으면 sched probe 비활성화 |

unit의 sandbox:

- 파일 시스템은 `/run/orma`를 제외하고 읽기 전용이다. 소켓은 unix 소켓만 사용 가능하고, namespace는 생성 불가능하다.
- 허용하는 시스템 호출은 `@system-service`에 `bpf`, `perf_event_open`을 추가한 목록이다. 그 밖의 시스템 호출은 `EPERM`으로 실패한다.
- `ProtectProc=invisible`, `ProcSubset=pid`는 다른 사용자의 `/proc/<pid>/maps`를 가리므로 사용하지 않는다.
- `LimitMEMLOCK=infinity`를 설정한다. 5.11 미만 커널(JetPack 5)은 map 메모리를 `RLIMIT_MEMLOCK`에 넣어 세기 때문이다.
- `install.sh`는 마지막에 `systemd-analyze security ormad.service` 점수(3.4)를 출력한다.

확인:

```bash
systemctl status ormad
orma status            # ormad: connected
orma check             # exit code 0 when nothing fails
```

### 다시 빌드한 뒤

서비스는 복사한 실행 파일을 사용하므로 다시 복사하고 재시작한다.

```bash
src/orma/daemon/orma_daemon/systemd/install.sh
sudo systemctl restart ormad
```

### 제거

```bash
sudo systemctl disable --now ormad
sudo rm /etc/systemd/system/ormad.service /usr/local/sbin/ormad /etc/sysusers.d/orma.conf
sudo systemctl daemon-reload
sudo userdel orma
```

## 3. orma-collectd

따로 설치하지 않는다.

- 시작: `orma` 명령이나 rqt 플러그인의 연결 시 orma-collectd가 없으면 자동으로 시작한다.
- 종료: 기록도 클라이언트도 없는 상태가 5분 이어지면 자동으로 종료된다.
- 소켓: `$XDG_RUNTIME_DIR/orma/collectd.sock`. `XDG_RUNTIME_DIR`이 없는 셸(예: `docker exec --user`)에서는 사용 불가능하다.
- 설정 파일: `~/.config/orma/collectd.toml`. 기록 한도의 상한을 여기서 정한다([usage.md](usage.md) 5절).

systemd user unit으로 등록하려면:

```bash
systemctl --user link install/orma_collectd/share/orma_collectd/systemd/orma-collectd.service
```

unit이 있으면 클라이언트가 `systemctl --user start orma-collectd`로 시작한다.
