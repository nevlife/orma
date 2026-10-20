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

- nightly toolchain, `bpf-linker`: eBPF 프로그램 빌드용
- `bpf-linker`: LLVM 23 필요. Ubuntu 24.04에 package가 없어 release binary 사용
- `lttng-tools`, `babeltrace`(1.5): `lttng` method 전용

## 2. ormad service

workspace를 source한 shell에서 실행한다.

```bash
src/orma/daemon/orma_daemon/systemd/install.sh
sudo systemctl enable --now ormad
```

`install.sh`가 하는 일:

1. `ormad`를 `/usr/local/sbin/ormad`에 복사
2. 사용자와 group `orma` 생성(`/etc/sysusers.d/orma.conf`)
3. unit을 `/etc/systemd/system/ormad.service`에 설치
4. 현재 사용자를 group `orma`에 추가

- 복사 이유: service 사용자 `orma`는 홈 디렉터리(`0750`) 아래 workspace에 접근 불가
- group은 다음 로그인부터 적용. 모든 session에서 로그아웃 후 다시 로그인한다. 그 전에는 `orma status`가 `ormad: denied`
- `sg orma`로 orma-collectd 실행 금지. 주 group이 바뀌어 같은 사용자 process의 `smaps_rollup`도 막히고, rqt의 `pss` 체크를 켜도 PSS가 `-`로 나온다

service의 capability:

| capability | 용도 |
| --- | --- |
| `CAP_BPF`, `CAP_PERFMON` | eBPF 프로그램 로드, uprobe와 tracepoint 부착 |
| `CAP_SYS_ADMIN` | eBPF 실행 시간 통계(`BPF_ENABLE_STATS`). Ubuntu 7.0 kernel은 이것 없이 uprobe의 `perf_event_open`도 거부(`EACCES`) |
| `CAP_SYS_PTRACE` | 다른 사용자 process의 `/proc/<pid>/maps`, `/proc/<pid>/root`. 없으면 probe 부착 불가 |
| `CAP_DAC_READ_SEARCH` | tracefs(`/sys/kernel/tracing`)의 event `format`, `id`. 없으면 sched probe 비활성 |

unit의 sandbox:

- file system은 읽기 전용(`/run/orma` 제외), socket은 unix만, namespace 생성 금지
- system call은 `@system-service`에 `bpf`, `perf_event_open`만 더한 목록. 그 밖은 `EPERM`
- `ProtectProc=invisible`, `ProcSubset=pid`는 쓰지 않는다. 다른 사용자의 `/proc/<pid>/maps`를 가린다
- `LimitMEMLOCK=infinity`: 5.11 미만 kernel(JetPack 5)은 map memory를 `RLIMIT_MEMLOCK`에 단다
- `install.sh`가 끝에 `systemd-analyze security ormad.service` 점수를 출력(3.4, sandbox 전 7.2)

확인:

```bash
systemctl status ormad
orma status            # ormad: connected
orma check             # exit code 0 when nothing fails
```

### 다시 빌드한 뒤

service는 복사한 실행 파일을 쓰므로 복사와 재시작을 다시 한다.

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

- 시작: `orma` 명령이나 rqt plugin이 붙을 때 없으면 자동 시작
- 종료: recording도 client도 없는 상태가 5분 지속되면 자동 종료
- socket: `$XDG_RUNTIME_DIR/orma/collectd.sock`. `XDG_RUNTIME_DIR`이 없는 shell(예: `docker exec --user`)에서는 사용 불가
- 설정 파일: `~/.config/orma/collectd.toml`. recording 한도의 상한([usage.md](usage.md) 5절)

systemd user unit으로 두려면:

```bash
systemctl --user link install/orma_collectd/share/orma_collectd/systemd/orma-collectd.service
```

unit이 있으면 client가 `systemctl --user start orma-collectd`로 시작한다.
