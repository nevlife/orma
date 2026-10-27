# 설치

지원 ROS 2 distro: Humble, Jazzy

| process | 권한 | 실행 |
| --- | --- | --- |
| `ormad` | eBPF 권한을 가진 system service(사용자 `orma`) | systemd system unit `ormad.service`. 부팅 시 시작 |
| `orma-collectd` | 사용자 권한. 사용자당 하나 | `orma` 명령이나 rqt plugin이 처음 붙을 때 자동 시작. systemd user unit도 가능 |
| 관측 대상 | 그대로 | 사용자가 직접 실행. `preload`, `lttng` method는 `orma run`이나 `OrmaTrace`로 실행([usage.md](usage.md) 3절) |

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

- nightly toolchain과 `bpf-linker`는 eBPF 프로그램을 빌드할 때 쓴다.
- `bpf-linker`에는 LLVM 23이 필요하다. Ubuntu 24.04에는 그 package가 없어서, LLVM 23을 담은 release binary를 쓴다.
- `lttng-tools`, `babeltrace`(1.5)는 `lttng` method에서만 쓴다.

## 2. ormad service

workspace를 source한 shell에서 실행한다.

```bash
src/orma/daemon/orma_daemon/systemd/install.sh
sudo systemctl enable --now ormad
```

`install.sh`가 하는 일:

1. `ormad`를 `/usr/local/sbin/ormad`에 복사한다.
2. 사용자와 group `orma`를 만든다(`/etc/sysusers.d/orma.conf`).
3. unit을 `/etc/systemd/system/ormad.service`에 설치한다.
4. 현재 사용자를 group `orma`에 넣는다.

- 실행 파일을 복사하는 이유: service 사용자 `orma`는 홈 디렉터리(`0750`) 아래의 workspace에 들어갈 수 없다.
- group은 다음 로그인부터 적용된다. 모든 session에서 로그아웃한 뒤 다시 로그인한다. 그 전에는 `orma status`에 `ormad: denied`가 나온다.
- orma-collectd를 `sg orma`로 실행하지 않는다. 주 group이 바뀌어 같은 사용자 process의 `smaps_rollup`도 읽지 못한다. 그러면 rqt의 `pss` 체크를 켜도 PSS가 `-`로 나온다.

service의 capability:

| capability | 용도 |
| --- | --- |
| `CAP_BPF`, `CAP_PERFMON` | eBPF 프로그램 로드, uprobe와 tracepoint 부착 |
| `CAP_SYS_ADMIN` | eBPF 실행 시간 통계(`BPF_ENABLE_STATS`). Ubuntu 7.0 kernel은 이것 없이 uprobe의 `perf_event_open`도 거부(`EACCES`) |
| `CAP_SYS_PTRACE` | 다른 사용자 process의 `/proc/<pid>/maps`, `/proc/<pid>/root`. 없으면 probe 부착 불가 |
| `CAP_DAC_READ_SEARCH` | tracefs(`/sys/kernel/tracing`)의 event `format`, `id`. 없으면 sched probe 비활성 |

unit의 sandbox:

- file system은 `/run/orma`를 빼고 읽기 전용이다. socket은 unix socket만 열 수 있고, namespace는 만들 수 없다.
- 허용하는 system call은 `@system-service`에 `bpf`, `perf_event_open`만 더한 목록이다. 그 밖의 system call은 `EPERM`으로 실패한다.
- `ProtectProc=invisible`, `ProcSubset=pid`는 쓰지 않는다. 이 둘은 다른 사용자의 `/proc/<pid>/maps`를 가리기 때문이다.
- `LimitMEMLOCK=infinity`를 둔다. 5.11 미만 kernel(JetPack 5)은 map memory를 `RLIMIT_MEMLOCK`에 넣어 세기 때문이다.
- `install.sh`는 마지막에 `systemd-analyze security ormad.service` 점수를 출력한다. 점수는 3.4이고, sandbox를 두기 전에는 7.2였다.

확인:

```bash
systemctl status ormad
orma status            # ormad: connected
orma check             # exit code 0 when nothing fails
```

### 다시 빌드한 뒤

service는 복사해 둔 실행 파일을 쓴다. 그래서 다시 복사하고 재시작한다.

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

- 시작: `orma` 명령이나 rqt plugin이 붙을 때 orma-collectd가 없으면 자동으로 시작한다.
- 종료: recording도 client도 없는 상태가 5분 이어지면 자동으로 끝난다.
- socket: `$XDG_RUNTIME_DIR/orma/collectd.sock`. `XDG_RUNTIME_DIR`이 없는 shell(예: `docker exec --user`)에서는 쓸 수 없다.
- 설정 파일: `~/.config/orma/collectd.toml`. recording 한도의 상한을 여기서 정한다([usage.md](usage.md) 5절).

systemd user unit으로 두려면:

```bash
systemctl --user link install/orma_collectd/share/orma_collectd/systemd/orma-collectd.service
```

unit이 있으면 client가 `systemctl --user start orma-collectd`로 시작한다.
