# Instrumentation

ROS 2와 flux의 주요 execution flow마다 orma가 어느 함수에 붙고 어떤 event를 받는지 적는다. sequence diagram에서 `orma` participant로 가는 화살표가 instrumentation point다. 화살표 이름은 orma event 이름이고, 괄호 안은 orma가 쓰는 인자, 대괄호 안은 uprobe probe set이다.

목차:

- [1. General rules](#1-general-rules)
- [2. ROS execution flows](#2-ros-execution-flows)
  - [2.1 Process and node creation](#21-process-and-node-creation)
  - [2.2 Publisher and subscription creation](#22-publisher-and-subscription-creation)
  - [2.3 Timer, service and client creation](#23-timer-service-and-client-creation)
  - [2.4 Executors and callback groups](#24-executors-and-callback-groups)
  - [2.5 Wait and wakeup](#25-wait-and-wakeup)
  - [2.6 Message publishing](#26-message-publishing)
  - [2.7 Subscription callbacks](#27-subscription-callbacks)
  - [2.8 Intra-process](#28-intra-process)
  - [2.9 Timer and service callbacks](#29-timer-and-service-callbacks)
  - [2.10 rclpy](#210-rclpy)
  - [2.11 Late attach](#211-late-attach)
  - [2.12 rcl_executor](#212-rcl_executor)
  - [2.13 전달 경로](#213-전달-경로)
- [3. flux execution flows](#3-flux-execution-flows)
  - [3.1 Channel and endpoint creation](#31-channel-and-endpoint-creation)
  - [3.2 Publishing](#32-publishing)
  - [3.3 Take and callbacks](#33-take-and-callbacks)
- [4. Scheduling](#4-scheduling)
- [5. Additional collection](#5-additional-collection)
  - [5.1 Spans](#51-spans)
  - [5.2 CUDA driver calls](#52-cuda-driver-calls)
  - [5.3 DDS writes](#53-dds-writes)
  - [5.4 futex waits](#54-futex-waits)
- [6. Latency segments](#6-latency-segments)
- [7. Distro differences](#7-distro-differences)
- [8. Coverage by method](#8-coverage-by-method)
- [9. Limitations](#9-limitations)

## 1. General rules

- instrumentation point 목록은 하나다. 세 method(`preload`, `lttng`, `uprobe`)는 같은 함수에 붙고, 같은 field와 같은 latency segment를 낸다. method별 차이는 8절에 있다.
- event timestamp: `CLOCK_MONOTONIC`
- 함수가 없으면 그 point만 빠지고 `probes_missing`에 기록된다. 빈 열이 "일이 없었다"인지 "볼 수 없었다"인지 가리기 위해서다.
- uprobe method는 probe set 단위로 붙는다.

| probe set | 붙는 process |
| --- | --- |
| `discovery` | 모든 process. init, registration, fini |
| `hot` | 선택한 process. publish, take, callback, executor, `rcl_wait`, 나머지 flux 함수, `orma_span_begin`과 `orma_span_end`. rcl_executor core를 매핑한 process에는 그 anchor도(2.12). 전달 경로 probe(2.13) |
| `rclcpp_symbols` | 선택한 process. rclcpp C++ 함수 |
| `bandwidth` | 선택한 process, opt-in. DDS serialized size |
| `sched` | 선택한 process의 thread. kernel scheduler |
| `gpu` | 선택한 process, opt-in. `libcuda.so`의 driver 함수(5.2) |
| `dds` | 선택한 process, opt-in. DDS write 함수(5.3) |
| `futex` | 선택한 process의 thread, opt-in. futex syscall tracepoint(5.4) |
| `names` | 선택한 process 중 node를 모르는 callback이 있는 것. 2.11 |

일부 tracepoint는 rclcpp가 `ros_trace_enabled_<name>()`이 참일 때만 `ros_trace_do_<name>()`으로 부른다(gated tracepoint). 인자를 만드는 비용이 크기 때문이다. `rclcpp_callback_register`(callback 함수 이름)와 rmw_zenoh의 `rmw_publisher_init`이 그렇다.

| method | gated tracepoint |
| --- | --- |
| `preload` | `ros_trace_enabled_<name>`도 가로채 참을 반환하고, `do_` 쪽을 받는다 |
| `lttng` | 같은 방법으로 받는다 |
| `uprobe` | 받지 못한다(uprobe는 반환값을 바꾸지 못한다). callback 함수 이름은 2.7의 code address로 채운다 |

gate는 이름을 주는 event에만 걸려 있고, timestamp를 재는 event에는 없다. 그래서 측정값은 method와 무관하게 같다.

## 2. ROS execution flows

아래 diagram은 Jazzy rclcpp 기준이다. distro 차이는 7절에 있다.

### 2.1 Process and node creation

`rclcpp::init()`이 `rcl_init()`을 부르고, node 생성자가 `rcl_node_init()`을 부른다. node 이름과 namespace는 여기서만 온다. orma가 이 event를 놓치면 node를 나중에 알아낸다(2.11).

수집하는 것:

- context handle(`rcl_init`)
- node handle, rmw handle, 이름, namespace(`rcl_node_init`)

```mermaid
sequenceDiagram
    participant process
    participant rclcpp
    participant rcl
    participant orma

    process->>rclcpp: rclcpp::init(argc, argv)
    rclcpp->>rcl: rcl_init()
    rcl-->>orma: rcl_init(context_handle) [discovery]
    process->>rclcpp: Node(name, namespace)
    rclcpp->>rcl: rcl_node_init()
    rcl-->>orma: rcl_node_init(node_handle, rmw_handle, name, namespace) [discovery]
```

### 2.2 Publisher and subscription creation

publisher와 subscription은 rclcpp, rcl, rmw 세 층에 handle이 하나씩 있다. orma는 세 handle을 이어 topic 이름을 handle에 붙인다. publish와 take event는 rmw handle만 싣기 때문이다.

수집하는 것:

- rcl handle, node handle, rmw handle, topic 이름, queue depth(`rcl_publisher_init`, `rcl_subscription_init`)
- rmw handle과 gid(`rmw_publisher_init`, `rmw_subscription_init`)
- rclcpp subscription 객체와 callback 객체(`rclcpp_subscription_init`, `rclcpp_subscription_callback_added`)
- callback 함수 이름(`rclcpp_callback_register`, gated)
- publisher 객체(`rclcpp_publisher_base`). rclcpp `PublisherBase` 생성자에 붙고, 안에서 뜨는 `rcl_publisher_init`과 thread로 짝짓는다

```mermaid
sequenceDiagram
    participant node
    participant rclcpp
    participant rcl
    participant rmw
    participant orma

    node->>rclcpp: create_publisher(topic, qos)
    rclcpp-->>orma: rclcpp_publisher_base(publisher) [rclcpp_symbols]
    rclcpp->>rcl: rcl_publisher_init()
    rcl->>rmw: rmw_create_publisher()
    rmw-->>orma: rmw_publisher_init(rmw_handle, gid) [discovery]
    rcl-->>orma: rcl_publisher_init(rcl_handle, node_handle, rmw_handle, topic, depth) [discovery]

    node->>rclcpp: create_subscription(topic, qos, callback)
    rclcpp->>rcl: rcl_subscription_init()
    rcl->>rmw: rmw_create_subscription()
    rmw-->>orma: rmw_subscription_init(rmw_handle, gid) [discovery]
    rcl-->>orma: rcl_subscription_init(rcl_handle, node_handle, rmw_handle, topic, depth) [discovery]
    rclcpp-->>orma: rclcpp_subscription_init(rcl_handle, subscription) [discovery]
    rclcpp-->>orma: rclcpp_subscription_callback_added(subscription, callback) [discovery]
    rclcpp-->>orma: rclcpp_callback_register(callback, symbol) [gated]
```

### 2.3 Timer, service and client creation

수집하는 것:

- timer handle과 period(`rcl_timer_init`), callback 객체(`rclcpp_timer_callback_added`), node(`rclcpp_timer_link_node`)
- timer 소멸(`rcl_timer_fini`). tracepoint가 아니라 `librcl.so`의 함수다. timer를 계속 만들고 지우는 process(예: rviz2)에서 이름 표의 칸을 회수한다
- service handle, node, rmw handle, service 이름(`rcl_service_init`), callback 객체(`rclcpp_service_callback_added`)
- client handle, node, rmw handle, service 이름(`rcl_client_init`)

```mermaid
sequenceDiagram
    participant node
    participant rclcpp
    participant rcl
    participant orma

    node->>rclcpp: create_wall_timer(period, callback)
    rclcpp->>rcl: rcl_timer_init()
    rcl-->>orma: rcl_timer_init(timer_handle, period) [discovery]
    rclcpp-->>orma: rclcpp_timer_callback_added(timer_handle, callback) [discovery]
    rclcpp-->>orma: rclcpp_timer_link_node(timer_handle, node_handle) [discovery]

    node->>rclcpp: create_service(name, callback)
    rclcpp->>rcl: rcl_service_init()
    rcl-->>orma: rcl_service_init(service_handle, node_handle, rmw_handle, name) [discovery]
    rclcpp-->>orma: rclcpp_service_callback_added(service_handle, callback) [discovery]

    Note over rclcpp,rcl: timer destroyed
    rclcpp->>rcl: rcl_timer_fini(timer_handle)
    rcl-->>orma: rcl_timer_fini(timer_handle) [discovery]
```

### 2.4 Executors and callback groups

callback이 어느 callback group과 executor에 속하는지에는 tracepoint가 없다. orma는 rclcpp C++ 함수에 붙어 읽는다. 소속을 알아야 wait가 긴 것과 무거운 callback을 가를 수 있다.

| orma event | rclcpp 함수 | 주는 것 |
| --- | --- | --- |
| `rclcpp_group_add_subscription` | `CallbackGroup::add_subscription` | subscription -> group |
| `rclcpp_group_add_timer` | `CallbackGroup::add_timer` | timer -> group. 뒤따르는 `rclcpp_timer_link_node`와 thread로 matching |
| `rclcpp_executor` | `Executor::Executor` | executor 생성 |
| `rclcpp_collector` | `ExecutorEntitiesCollector::ExecutorEntitiesCollector` | collector -> executor |
| `rclcpp_collector_add_group_to_collection` | `ExecutorEntitiesCollector::add_callback_group_to_collection` | group -> executor |
| `rclcpp_collector_add_group` | `ExecutorEntitiesCollector::add_callback_group` | 위의 수동 API 경로 |
| `rclcpp_executor_add_group` | `Executor::add_callback_group_to_map` | group -> executor(Humble) |

규칙:

- 함수는 mangled name의 앞부분(namespace, class, method)으로 찾는다. 둘 이상이 맞으면 붙이지 않고 `probes_missing`에 기록한다.
- 함수 목록은 rclcpp major 버전(16, 28 이후)마다 따로 있다. rclcpp를 올리면 orma를 다시 빌드한다. `preload`, `lttng`은 같은 이름의 함수를 정의해 가로채기 때문이다.
- caller에 inline된 함수는 어느 method로도 보이지 않는다. `orma check`의 `probes` 행에서 확인한다.

### 2.5 Wait and wakeup

executor는 `rcl_wait()`에서 잠들고, ready인 handle이 생기면 깨어난다. orma는 `librcl.so`의 `rcl_wait` entry와 return을 잡는다. tracepoint가 아니라 함수 자체다. C++와 Python에 공통이고, executor 종류와 상관없이 같은 뜻이다.

수집하는 것:

- wait start(`rcl_wait_enter`)와 wakeup(`rcl_wait_exit`). 같은 thread의 둘 사이가 wait time이다
- rclcpp executor tracepoint(`rclcpp_executor_get_next_ready`, `rclcpp_executor_wait_for_work`, `rclcpp_executor_execute`). executor는 먼저 ready인 executable을 찾고(`get_next_ready`), 없을 때만 wait한다(`wait_for_work`). `rclcpp_executor_execute`가 dispatch start다

```mermaid
sequenceDiagram
    participant Executor
    participant rcl
    participant orma

    Executor-->>orma: rclcpp_executor_get_next_ready [hot]
    opt nothing ready
        Executor-->>orma: rclcpp_executor_wait_for_work(timeout) [hot]
        Executor->>rcl: rcl_wait(wait_set, timeout)
        rcl-->>orma: rcl_wait_enter [hot]
        Note over rcl: sleeps until a handle is ready
        rcl-->>orma: rcl_wait_exit [hot]
        rcl-->>Executor: ret
    end
    Executor-->>orma: rclcpp_executor_execute(handle) [hot]
```

`EventsExecutor`는 `rcl_wait`를 쓰지 않으므로 wait와 wakeup이 없다. orma는 그 칸을 비워 둔다. ROS 2 overhaul의 `RclExecutor`도 `rcl_wait`를 쓰지 않는다. 그 wait와 dispatch는 2.12의 anchor에서 받는다.

### 2.6 Message publishing

`publish()`는 rclcpp, rcl, rmw를 차례로 지난다. orma는 rmw 층의 event를 publish timestamp와 matching key로 쓴다. `rmw_publish`가 DDS의 source timestamp를 싣기 때문이다.

수집하는 것:

- publish 수와 rate(`rmw_publish`의 rmw handle)
- source timestamp(`rmw_publish`). take 쪽과 짝짓는 값이다(2.7)
- message 앞 8 B. message가 `header.stamp`로 시작하면 data age가 된다. orma는 message type을 모르므로, clock 검사를 통과한 값만 쓴다
- `/clock` publisher의 message 앞 8 B. sim time으로 data age를 잴 때 쓴다
- serialized size(`dds_serialized_size`, opt-in). Fast DDS의 `TypeSupport::serialize`, Cyclone DDS의 `serdata_rmw_from_sample`(dbgsym package가 있을 때만)에 붙는다. 이 함수들은 function pointer로만 불리므로 uprobe로만 잰다
- late attach일 때 publisher의 topic 이름(`rmw_publisher_topic`, uprobe). rmw publisher handle에서 처음 한 번 읽는다
- Jazzy rclcpp는 `rclcpp_publish`의 publisher handle 자리에 NULL을 넘긴다. publisher는 `rcl_publish`와 `rmw_publish`의 handle로 안다

```mermaid
sequenceDiagram
    participant node
    participant rclcpp
    participant rcl
    participant rmw
    participant orma

    node->>rclcpp: publish(msg)
    rclcpp-->>orma: rclcpp_publish(NULL, message) [hot]
    rclcpp->>rcl: rcl_publish(publisher_handle, message)
    rcl-->>orma: rcl_publish(publisher_handle, message) [hot]
    rcl->>rmw: rmw_publish(rmw_handle, message)
    rmw-->>orma: dds_serialized_size(size) [bandwidth]
    rmw-->>orma: rmw_publish(rmw_handle, message, timestamp) [hot]
```

`header.stamp` clock 검사:

- `sec`이 10^9 이상 4 x 10^9 미만이면 wall clock stamp로 보고 `CLOCK_REALTIME`과 비교한다.
- 아니면 sim time으로 보고 마지막 `/clock` 값과 비교한다. `/clock` publisher가 없으면 쓰지 않는다.
- stamp가 기준 시각보다 1초 넘게 앞서거나 age가 600초를 넘으면 쓰지 않는다.

### 2.7 Subscription callbacks

executor가 subscription을 고르면 rmw에서 message를 take하고 callback을 부른다. `rmw_take`는 `taken`이 참일 때만 take로 센다.

수집하는 것:

- take 수와 rate, loss(`rmw_take`)
- source timestamp(`rmw_take`). publish 쪽 `rmw_publish`와 `(topic, source timestamp)`로 짝지어 transport latency를 낸다
- message 앞 8 B(`rmw_take`). 2.6과 같은 규칙이다
- callback start와 end(`callback_start`, `callback_end`). `callback_start`는 intra-process 여부를 싣는다
- callback의 code address(`callback_code`). callback이 처음 돌 때 callback 객체 안 `std::function`의 code address를 읽는다. orma-collectd가 `/proc/<pid>/maps`와 ELF symbol로 함수 이름을 푼다. gate 때문에 `rclcpp_callback_register`가 오지 않는 uprobe method와, 이름 문자열이 비는 Humble에서 쓴다
- late attach일 때 subscription의 topic 이름(`rmw_subscription_topic`, uprobe). `rmw_take`의 rmw subscription handle에서 처음 한 번 읽는다

```mermaid
sequenceDiagram
    participant Executor
    participant rclcpp
    participant rcl
    participant rmw
    participant callback
    participant orma

    Executor-->>orma: rclcpp_executor_execute(handle) [hot]
    Executor->>rclcpp: execute_subscription()
    rclcpp->>rcl: rcl_take()
    rcl->>rmw: rmw_take_with_info()
    rmw-->>orma: rmw_take(rmw_handle, message, source_timestamp, taken) [hot]
    rcl-->>orma: rcl_take(message) [hot]
    rclcpp-->>orma: rclcpp_take(message) [hot]
    rclcpp-->>orma: callback_start(callback, is_intra_process) [hot]
    rclcpp-->>orma: callback_code(callback, address) [hot, first call only]
    rclcpp->>callback: callback(msg)
    rclcpp-->>orma: callback_end(callback) [hot]
```

같은 topic에서 publisher 둘이 같은 source timestamp를 내면 잇지 않고 ambiguous로 센다. matching key에 topic을 넣는 이유는 다른 topic의 message가 같은 nanosecond 값을 가질 수 있기 때문이다.

### 2.8 Intra-process

intra-process 전달에는 rmw take가 없다. 그래서 transport latency와 take rate를 잴 수 없다. orma는 `callback_start`의 `is_intra_process`로 전달 경로를 가리고, publish rate는 `rclcpp_intra_publish`로 보인다.

연결 관계는 rclcpp C++ 함수에서 읽는다.

| orma event | rclcpp 함수 | 주는 것 |
| --- | --- | --- |
| `rclcpp_publisher_setup_intra` | `PublisherBase::setup_intra_process` | intra publisher id -> rcl handle |
| `rclcpp_subscription_setup_intra` | `SubscriptionBase::setup_intra_process` | intra subscription id -> rclcpp subscription |
| `rclcpp_ipm_insert_sub_id_for_pub` | `IntraProcessManager::insert_sub_id_for_pub` | process 안 publisher -> subscription |

intra-process buffer tracepoint(`rclcpp_ring_buffer_*`, `rclcpp_buffer_to_ipb`, `rclcpp_ipb_to_subscription`)는 수집하지 않는다.

ROS 2 overhaul의 rcl broker(`RCL_INTRA_PROCESS=1`)는 rclcpp `IntraProcessManager`와 다른 경로다. take가 있어 transport latency와 take rate를 잰다(2.13).

### 2.9 Timer and service callbacks

timer와 service callback도 `callback_start`와 `callback_end` 사이로 잰다. callback 객체는 2.3의 `*_callback_added`가 timer나 service에 이어 둔 것이다.

- timer: expected rate는 `rcl_timer_init`의 period에서 온다. `orma node cb`의 `expected` 열이다.
- service: request와 response latency는 재지 않는다. rmw client, service tracepoint(`rmw_client_init`, `rmw_send_request`, `rmw_take_request`, `rmw_send_response`, `rmw_take_response`)는 Kilted부터 있고 Humble, Jazzy에는 없다. service callback duration만 낸다.

### 2.10 rclpy

rclpy는 `ros_trace_*`를 직접 부르지 않는다. rcl과 rmw를 거치는 event(init, `rcl_publish`, `rmw_publish`, `rmw_take`, `rcl_wait`)는 Python node에서도 나오지만, registration과 callback event는 나오지 않는다. `orma_rclpy`가 그 빈칸을 채운다. 부르는 함수는 C++ node와 같은 `libtracetools.so`의 `ros_trace_*`이므로, 세 method가 C++ node와 똑같이 받는다.

| event | rclpy에서 나오나 | `orma_rclpy`가 하는 일 |
| --- | --- | --- |
| `rcl_*_init`, `rcl_publish`, `rmw_publish`, `rmw_take`, `rcl_wait` | 나온다 | - |
| `rclcpp_subscription_init`, `rclcpp_subscription_callback_added` | 안 나온다 | subscription을 만들 때 부른다. rclcpp subscription 객체 자리에는 Python `Subscription` 객체 주소를 넣는다 |
| `rclcpp_service_callback_added` | 안 나온다 | service를 만들 때 부른다 |
| `rclcpp_timer_callback_added`, `rclcpp_timer_link_node` | 안 나온다 | timer를 만들 때 부른다 |
| `rclcpp_callback_register` | 안 나온다 | callback의 `__qualname__`을 넘겨 부른다. 이 이름에는 gate가 없어 uprobe method도 받는다 |
| `rclcpp_executor_*` | 안 나온다 | 부르지 않는다. wakeup은 `rcl_wait` return으로 잰다 |
| `callback_start`, `callback_end` | 안 나온다 | entity의 `callback` 속성을 감싼 함수가 callback 앞뒤에서 부른다 |

- 감싸는 함수: `rclpy.node.Node`의 `create_subscription`, `create_service`, `create_timer`
- module은 process가 시작할 때 load된다(`orma run`, `OrmaTrace`가 `PYTHONPATH`에 더한다). 이미 실행 중인 Python node는 다시 시작해야 한다. uprobe method도 마찬가지다.
- user site가 꺼진 process(가상환경, `python3 -s`, `-I`)에서는 load되지 않는다.
- 사용자가 `callback` 속성을 다시 바꾼 entity에서는 `callback_*`이 나오지 않는다(예: `create_rate`의 timer).
- coroutine callback은 `await`로 멈춘 시간도 callback duration에 들어간다.

### 2.11 Late attach

orma가 entity보다 늦게 붙으면 2.1-2.3의 event가 없어 callback의 node를 모른다. rcl과 rmw handle에는 node를 가리키는 값이 없고, node는 rclcpp 객체에만 있다. 그래서 uprobe method는 executor가 callback을 돌리는 함수에서 node를 읽는다.

```text
Executor::execute_any_executable(AnyExecutable &)   uprobe, probe set names
  AnyExecutable.subscription -> SubscriptionBase -> node_handle_ -> rcl_node_t -> impl -> fq_name
  AnyExecutable.service      -> ServiceBase      -> node_handle_ -> ...
  AnyExecutable.timer        -> (no node) callback_group -> first subscription, service or client's node_handle_
callback_start(callback)      same thread, one level deep -> rclcpp_callback_owner (once per callback)
```

- `rclcpp_callback_owner` event는 callback, 그 entity, rcl handle, node handle, 종류, node FQN을 싣는다. orma-collectd는 이것을 놓친 init event 대신 쓴다.
- 이 uprobe는 callback마다 불린다(Orin에서 약 1.25 us). 그래서 probe set `names`는 node를 모르는 callback이 처음 돈 지 10초 안인 process에만 붙인다. node를 알면 뗀다. 10초 안에 못 찾으면 포기하고 뗀다.
- subscription의 topic은 `rmw_subscription_topic`(2.7)에서 온다. service 이름은 읽지 않는다. late attach한 service callback은 `srv`와 함수 이름만 보인다.
- timer는 node를 갖지 않는다. 그 callback group의 첫 subscription, service, client의 node를 쓴다. timer만 든 group의 timer는 node를 모른 채 남는다.
- 보이지 않는 것: `execute_any_executable`을 거치지 않는 executor(Humble의 `StaticSingleThreadedExecutor`, `EventsExecutor`), intra-process로만 받는 subscription, rclpy. 이들은 `orma node` 명령의 node 열에 `unlinked`로 남는다.

### 2.12 rcl_executor

ROS 2 overhaul의 executor core `rcl_executor`(`librcl_executor.so`)는 `rcl_wait`를 부르지 않는다. 그 위의 `rclcpp::experimental::executors::RclExecutor`와 `rclpy.experimental.RclExecutor`는 `Executor::execute_any_executable`도 거치지 않는다. 그래서 2.5의 `rcl_wait_*`, `rclcpp_executor_get_next_ready`, `rclcpp_executor_wait_for_work`, `rclcpp_executor_execute`가 나오지 않는다. callback, take, publish event는 그대로 나온다. core는 uprobe용 빈 함수 넷(`rcl_executor/trace.h`의 `rcl_executor_trace_*`)을 export하고, orma는 uprobe method에서 그 함수에 붙는다.

probe 선택:

- process마다 매핑한 파일로 정한다. `librcl_executor.so`가 매핑돼 있고 그 파일이 anchor 넷을 모두 export하면 rcl_executor probe를 `hot` set에 더한다. 버전이나 환경변수는 보지 않는다
- anchor가 하나라도 없는 `librcl_executor.so`는 거절하고 `probes_missing`에 남긴다
- stock probe(`rcl_wait`, rclcpp executor tracepoint)는 그대로 붙는다. 한 process에 stock executor와 `RclExecutor`가 함께 있을 수 있기 때문이다
- core를 매핑하지 않은 process에는 아무것도 더 붙지 않는다. stock ROS 2 process의 비용은 그대로다
- 확인: `orma status`의 `executor probes:` 줄(`stock`, `rcl_executor`), `orma check`의 `executor probes pid N` 행

| core 함수 | orma event | 인자 | 같은 뜻의 stock point |
| --- | --- | --- | --- |
| `rcl_executor_trace_wait_enter(executor, timeout_ns)` | `rcl_wait_enter`, flag `ORMA_RECORD_RCL_EXECUTOR` | `wait_set` = executor, `timeout` = timeout_ns | `rcl_wait` entry |
| `rcl_executor_trace_wait_exit(executor, result)` | `rcl_wait_exit`, flag `ORMA_RECORD_RCL_EXECUTOR` | `wait_set` = executor, `ret` = `rcl_executor_result_t` | `rcl_wait` return |
| `rcl_executor_next_excluding` entry | 없음. thread별로 `rcl_executor_item_t *`만 기억 | - | - |
| `rcl_executor_trace_item_claimed(executor, id, type, group)` | `rcl_executor_claim` | executor, registration, entity, context, group, entity_type, rclcpp | `rclcpp_executor_execute`(dispatch start) |
| `rcl_executor_trace_item_completed(executor, id)` | `rcl_executor_complete` | executor, registration | 없음 |

```mermaid
sequenceDiagram
    participant Client as RclExecutor (rclcpp, rclpy)
    participant core as rcl_executor
    participant orma

    Client->>core: rcl_executor_wait(timeout)
    core-->>orma: rcl_wait_enter(executor, timeout) [hot]
    Note over core: sleeps until an item is ready
    core-->>orma: rcl_wait_exit(executor, result) [hot]
    Client->>core: rcl_executor_next(item)
    core-->>orma: rcl_executor_claim(executor, id, entity, group) [hot]
    Client-->>orma: callback_start, callback_end [hot]
    Client->>core: rcl_executor_complete(token)
    core-->>orma: rcl_executor_complete(executor, id) [hot]
```

event를 고른 규칙:

- wait 두 event는 stock event 번호와 payload를 그대로 쓰고, record header의 flag `ORMA_RECORD_RCL_EXECUTOR`(2)로만 구분한다. wait time, wakeup latency, dispatch latency는 분기 없이 같은 식으로 계산된다(6절)
- core wait의 key는 wait set 대신 executor(`rcl_executor_t *`)다. `ret`는 `rcl_ret_t`가 아니라 `rcl_executor_result_t`(0 `READY`, 1 `TIMEOUT`, 2 `INTERRUPTED`, 3 `STOPPED`, 4 `ERROR`)다
- claim은 새 event다. `rclcpp_executor_execute`는 rclcpp만 부르는 tracepoint이고 rcl handle 하나만 싣는다. core는 언어와 무관하고, claim과 complete를 잇는 데 registration id가 필요하다
- orma-collectd는 `rclcpp`가 1인 claim을 `rclcpp_executor_execute`와 같은 dispatch start로 읽는다. `rclcpp`가 0인 claim은 stock rclpy처럼 executor event 없이 둔다. 그래서 executor 종류(`rclcpp`, `rclpy`)가 stock과 같은 규칙으로 정해진다
- `rclcpp`는 ormad가 그 process의 `librclcpp.so` 매핑 여부로 채운다. rclcpp를 쓰는 C++ extension을 로드한 Python process는 `rclcpp`로 표시된다
- complete는 stock에 없다. claim부터 complete까지가 core가 본 item 실행 시간이다. coroutine callback이면 callback보다 길다

claim의 값과 다른 event의 대응:

| 값 | rclcpp `RclExecutor` | rclpy `RclExecutor` | 같은 값을 싣는 event |
| --- | --- | --- | --- |
| `registration` | core가 매긴 순번(`rcl_executor_add`의 출력) | rclcpp `RclExecutor`와 같다 | `rcl_executor_complete` |
| `entity` | `rcl_subscription_t *`, `rcl_timer_t *`, `rcl_service_t *`, `rcl_client_t *`. waitable(`RCL_EXECUTOR_WORK`)은 0 | `rcl_*_t *`(event, guard condition 포함) | `rcl_subscription_init`, `rcl_timer_init`, `rcl_service_init`, `rcl_client_init`의 handle. service와 timer는 `rclcpp_*_callback_added`의 handle도 |
| `context` | `SubscriptionBase *`, `ServiceBase *`, `ClientBase *`, `TimerBase *`, `Waitable *` | entity 객체의 `id()` | subscription은 `rclcpp_subscription_init`의 subscription(rclpy는 `orma_rclpy`가 같은 `id()`를 넣는다) |
| `group` | `CallbackGroup *` | callback group의 `id()` | `rclcpp_group_add_subscription`, `rclcpp_group_add_timer`의 group |

entity 읽기:

- anchor는 registration id만 넘기고 entity는 넘기지 않는다. core만 아는 id라 orma가 다른 event와 이을 수 없다
- 그래서 `rcl_executor_next_excluding`(`rcl_executor_next`가 넘어가는 곳) 입구에서 `rcl_executor_item_t *`를 thread별로 기억하고, claim anchor에서 `item->entity`(offset 24)와 `item->context`(offset 32)를 읽는다. anchor가 불릴 때 item은 이미 채워져 있다
- `item->registration`(offset 0)이 anchor의 id와 다르면 읽지 않고 0으로 둔다. 같은 thread의 이전 호출이 남긴 item을 읽지 않기 위해서다
- core가 anchor에 entity와 context를 넘기면 이 probe는 빠진다. 그때까지 item 하나에 core 쪽 uprobe가 다섯 번 불린다(wait 둘, next, claim, complete). 모두 entry probe이고 uretprobe는 없다

차이와 빈칸:

- `rcl_executor_spin`은 claim마다 먼저 wait를 부른다. ready인 item이 있으면 바로 `READY`로 돌아오므로 길이 0에 가까운 wait가 섞인다
- rclcpp `RclExecutor`가 `RCL_EXECUTOR_WAIT_SET` policy로 waitable을 실행하면 그 안에서 `rcl_wait(ws, 0)`를 부른다. 이 호출은 stock `rcl_wait_*`로 잡히고, 그 뒤 callback의 dispatch latency는 이 호출의 return부터 잰다
- `preload`, `lttng` method는 anchor를 가로채지 않는다. 그 method로 실행한 `RclExecutor` process에는 wait와 claim이 없다
- late attach(2.11)의 node 읽기는 `execute_any_executable`에 붙으므로 `RclExecutor`에서는 동작하지 않는다. 그런 callback은 `unlinked`로 남는다

### 2.13 전달 경로

ROS 2 overhaul에서는 같은 topic의 message도 subscription마다 다른 경로로 간다. orma는 take마다 경로를 붙이고, 경로별 take 수와 transport latency, take에서 callback까지, rclpy 변환, buffer 보유 시간을 낸다.

| 경로 | 뜻 | 조건 |
| --- | --- | --- |
| `broker` | rcl broker가 같은 process의 reader에게 넘긴 message. rmw를 거치지 않는다 | overhaul rcl, `RCL_INTRA_PROCESS=1` |
| `loan` | publisher가 rmw loan을 빌려 publish한 message | `borrow_loaned_message`를 쓰는 publisher, loan을 지원하는 rmw |
| `shm` | rmw_zenoh가 zenoh shared memory에 직렬화한 payload | rmw_zenoh, SHM 켬, message가 SHM threshold 이상 |
| `net` | rmw가 자기 buffer에 직렬화한 payload | 위 셋이 아닌 rmw publish |
| `unknown` | publish를 보지 못한 take | publisher process를 관측하지 않을 때 |

probe 선택:

- process마다 매핑한 파일의 export로 정한다. 2.12와 같은 규칙이다. 버전이나 환경변수는 보지 않는다
- stock Jazzy process에는 `loan`과 `zenoh_shm`만 붙는다. 둘 다 stock library의 export 함수다
- 확인: `orma status`의 `delivery probes:` 줄
- loan 반환은 rmw 함수에서 잡는다. rcl의 반환 함수도 그것을 부르고, rclpy는 subscription loan을 rmw 함수로 바로 돌려준다
- `preload`, `lttng` method도 같은 point를 가로챈다. rcl과 rclpy가 anchor를 PLT로 부르기 때문이다. 2.12의 core anchor와 다른 점이다

| probe | 붙는 조건 | 붙는 함수 |
| --- | --- | --- |
| `broker` | `librcl.so`가 `rcl_ipc_trace_contract_1`을 export | `rcl_ipc_trace_publish`, `rcl_ipc_trace_take`, `rcl_ipc_trace_release` |
| `loan` | `librcl.so`가 `rcl_borrow_loaned_message`를 export | `librcl.so`의 `rcl_borrow_loaned_message`(entry, return), `rcl_publish_loaned_message`, `rcl_take_loaned_message`. `librmw_implementation.so`의 `rmw_return_loaned_message_from_publisher`, `rmw_return_loaned_message_from_subscription` |
| `zenoh_shm` | `libzenohc.so`가 `z_bytes_from_shm_mut`을 export | `z_bytes_from_shm_mut` |
| `rclpy` | `_rclpy_pybind11.*.so`가 `rclpy_trace_contract_1`을 export | `rclpy_trace_take_converted`, `rclpy_trace_publish_enter` |

| 함수 | orma event | 인자 |
| --- | --- | --- |
| `rcl_ipc_trace_publish(publisher, message, source_timestamp, readers)` | `rcl_ipc_publish` | broker가 같은 process reader `readers`개에 넣은 message. reader가 없으면 불리지 않는다 |
| `rcl_ipc_trace_take(subscription, message, data, source_timestamp, local)` | `rcl_ipc_take` | reader의 take. `message`는 release까지 잡는 `rcl_ipc_message_t *`, 복사로 꺼냈으면 0. `local`이 0이면 rmw가 broker에 넣은 다른 process의 message |
| `rcl_ipc_trace_release(message)` | `rcl_ipc_release` | take한 message의 release |
| `rcl_borrow_loaned_message` return | `rcl_loan_borrow` | publisher, 빌린 loan, `rcl_ret_t` |
| `rcl_publish_loaned_message` | `rcl_loan_publish` | publisher, loan |
| `rmw_return_loaned_message_from_publisher` | `rmw_loan_return` | rmw publisher, loan |
| `rcl_take_loaned_message` | `rcl_loan_take` | subscription. loan은 안의 `rmw_take`가 싣는다 |
| `rmw_return_loaned_message_from_subscription` | `rmw_loan_return` | rmw subscription, loan |
| `z_bytes_from_shm_mut(bytes, shm)` | `zenoh_shm_payload` | bytes 객체 |
| `rclpy_trace_publish_enter(publisher)` | `rclpy_publish_enter` | rclpy publish 시작(loan publish 포함). Python message 변환 전 |
| `rclpy_trace_take_converted(subscription, message)` | `rclpy_take_converted` | take한 message의 Python 변환 끝 |

```mermaid
sequenceDiagram
    participant pub as publisher
    participant rcl
    participant rmw as rmw_zenoh
    participant orma
    participant sub as subscription

    pub->>rcl: rcl_publish(message)
    rcl-->>orma: rcl_publish [hot]
    rcl->>rmw: rmw publish
    rmw->>rcl: broker prepare(source_timestamp)
    rcl-->>orma: rcl_ipc_publish(publisher, source_timestamp, readers) [hot]
    rmw-->>orma: zenoh_shm_payload [hot, SHM일 때]
    rmw-->>orma: rmw_publish(rmw_handle, source_timestamp) [hot, 다른 process reader가 있을 때]
    sub->>rcl: take
    rcl-->>orma: rcl_ipc_take(subscription, message, source_timestamp, local) [hot]
    sub-->>orma: callback_start, callback_end [hot]
    sub->>rcl: rcl_ipc_message_release(message)
    rcl-->>orma: rcl_ipc_release(message) [hot]
```

경로 판정:

- `broker`: `local`이 1인 `rcl_ipc_take`. publish 쪽 key는 `rcl_ipc_publish`의 `(topic, source timestamp)`다. 모든 reader가 broker reader면 rmw_zenoh가 `rmw_publish`를 부르지 않기 때문이다
- 그 밖의 take는 짝지은 publish의 경로를 받는다. publish 경로는 같은 thread의 `rcl_publish`부터 `rmw_publish`까지 본 event로 정한다. `rcl_loan_publish`가 있으면 `loan`, `zenoh_shm_payload`가 있으면 `shm`, 둘 다 없으면 `net`이다
- 한 publish가 broker reader와 다른 process reader를 함께 가지면, 같은 source timestamp의 take가 reader마다 다른 경로로 잡힌다
- `orma topic path`(사용법 4절)와 window의 `paths`가 경로별 값이다. 경로별 hop 표본 수의 합은 topic과 subscription의 hop 표본 수와 같다

segment:

| segment | 정의 |
| --- | --- |
| hop by path | 경로별 transport latency. take - publish(`rmw_publish` 또는 `rcl_ipc_publish`) |
| take -> callback start | take(`rmw_take`, `rcl_ipc_take`) - 그 take의 `callback_start` |
| rclpy take -> converted | `rclpy_take_converted` - 같은 thread의 마지막 take. take -> callback start의 일부 |
| rclpy publish -> rcl | `rcl_publish` - `rclpy_publish_enter`. Python message에서 C message로의 변환 |
| subscription hold | `rcl_ipc_release` 또는 subscription의 `rmw_loan_return` - take. broker message나 loan을 잡은 시간 |
| publisher loan hold | `rcl_loan_publish` 또는 publisher의 `rmw_loan_return` - `rcl_loan_borrow` |

anchor를 둔 이유:

overhaul rcl과 rclpy에는 uprobe용 빈 함수(anchor)를 더했다. 2.12의 core anchor와 같은 방식이고, 인자를 바꾸면 contract 심볼 번호를 올린다. stock 함수로 잡을 수 있는 point(`loan`, `zenoh_shm`)에는 anchor를 두지 않았다.

| point | stock 함수로 안 되는 이유 |
| --- | --- |
| broker publish | broker만 받는 publish에는 `rmw_publish`가 없다. source timestamp는 rmw_zenoh가 만들어 rcl 안의 내부 함수(broker의 prepare)로만 넘긴다 |
| broker take | take 함수가 셋(`rcl_take_ipc_message`, `rcl_take_ipc_adapted`, `rcl_ipc_take_serialized_copy`)이고 source timestamp와 message는 출력 인자다. uprobe로는 함수마다 entry와 return 두 probe가 필요하다. anchor는 entry probe 하나다 |
| broker release | `rcl_ipc_take_copy`가 안에서 message를 지운다. anchor가 그 경로도 release로 센다 |
| rclpy 변환 | `_rclpy_pybind11`은 심볼을 숨긴다. take는 Python executor 말고도 C++ executor(`EventsExecutor`, `RclExecutor`)에서 불려 Python 쪽에서 감쌀 곳이 없다 |

차이와 빈칸:

- rmw_zenoh는 SHM provider를 첫 SHM publish 때 만든다. 그 전의 몇 message는 `net`으로 잡힌다
- `shm`은 publisher가 SHM payload를 만들었다는 뜻이다. subscriber가 SHM을 매핑하지 못해 받은 쪽에서 복사했는지는 보지 않는다
- `rcl_take_loaned_message`는 entry만 본다. take하지 못한 호출 뒤 같은 thread의 다음 take가 loan으로 잡힐 수 있다
- `rcl_take`가 broker message를 사용자 message로 복사하는 경로(`rcl_ipc_take_copy`)는 take 직후 release하므로 hold가 0에 가깝다
- `rclpy` probe는 overhaul rclpy에만 있다. stock rclpy의 변환 시간은 take -> callback start 안에 섞여 있다

## 3. flux execution flows

flux는 `libflux_ffi.so`가 export하는 `flux_trace_*` 함수를 부른다. 몸체는 비어 있고 enabled 검사가 없어, 세 method가 모두 함수 자체에 붙는다. 함수 이름과 인자가 계약이고, 버전은 아무것도 하지 않는 symbol `flux_trace_contract_2`다. orma는 이 symbol을 export하는 library에만 붙는다. 다른 버전의 library는 인자가 다르므로 잘못 읽는 대신 거절한다.

- channel id는 flux channel 공유 상태의 주소이고, re-attach 전까지 고정이다. executor id는 executor control block의 주소다.
- 문자열 인자는 255 B까지 읽는다.
- flux를 정적으로 링크한 프로그램은 함수를 직접 부르므로 `preload`, `lttng`에 보이지 않는다.

### 3.1 Channel and endpoint creation

수집하는 것:

- channel handle, segment(device, inode, 이름), publisher 여부, slot 수와 크기, payload 안 `header.stamp` 위치(`flux_channel_init`). 생성과 re-attach 때 나온다
- endpoint handle, node, key, signpost, fingerprint, role(publisher, subscription), language(C++, Python)(`flux_endpoint_init`). flux_cpp와 flux_py의 `Publisher`, `Subscription` 생성자에서 나온다

```mermaid
sequenceDiagram
    participant node
    participant flux
    participant orma

    node->>flux: Publisher(node, key) / Subscription(node, key, callback)
    flux-->>orma: flux_channel_init(channel, dev, ino, segment, publisher, slot_count, slot_size, stamp_offset) [discovery]
    flux-->>orma: flux_endpoint_init(endpoint, node, key, signpost, fingerprint, role, lang) [discovery]
```

orma가 channel보다 늦게 붙으면 `flux_callback_end`가 싣는 segment 이름으로 channel을 안다. callback 이름은 그 channel의 signpost가 가진 key다.

### 3.2 Publishing

publish에는 두 경로가 있다. copy 경로는 slot을 claim하고 바로 commit한다. loan 경로는 slot을 사용자에게 loan하고, 사용자가 쓴 뒤 commit한다.

수집하는 것:

- slot claim 결과와 실패 이유(`flux_claim`). 실패하면 slot은 `UINT32_MAX`다
- loan start와 end, 결과(`flux_loan`, `flux_loan_end`). commit에 성공하면 `loan_end`는 `commit` 다음에 나온다. 쓰지 않고 버린 loan은 abort로 끝난다
- publish(`flux_commit`). ticket, slot, byte 수, 깨운 subscription 수, `header.stamp`를 싣는다

```mermaid
sequenceDiagram
    participant node
    participant flux
    participant orma

    alt copy
        node->>flux: publish(msg)
        flux-->>orma: flux_claim(channel, slot, result, loan=0) [hot]
        flux-->>orma: flux_commit(channel, ticket, slot, nbytes, woke, stamp) [hot]
    else loan
        node->>flux: loan()
        flux-->>orma: flux_claim(channel, slot, result, loan=1) [hot]
        flux-->>orma: flux_loan(channel, slot) [hot]
        Note over node: writes into the slot
        node->>flux: commit()
        flux-->>orma: flux_commit(channel, ticket, slot, nbytes, woke, stamp) [hot]
        flux-->>orma: flux_loan_end(channel, slot, result) [hot]
    end
```

flux_bag recorder가 frame을 bag에 쓰지 못하고 버리면 `flux_record_drop`이 나온다. orma는 이를 recording loss로 보인다.

sticky CUDA error가 process의 GPU를 멈추면 `flux_gpu_wedged(result)`가 process마다 한 번 나온다. `result`는 그 `CUresult`다. 그 뒤로 GPU channel의 claim, loan, take는 `CLAIM_GPU_WEDGED`, `LOAN_GPU_WEDGED`, `REFUSED_GPU_WEDGED`로 실패하고 orma는 topic의 publish drop으로 센다. `orma node cpu`는 그 process에 경고 한 줄을 붙인다. ormad는 이 함수를 discovery set으로 모든 flux process에 붙인다. 이 함수가 없는 이전 flux build에서는 `probes_missing`에 남는다.

### 3.3 Take and callbacks

flux executor는 wait word에서 잠들고, publisher가 깨우면 callback을 돌린다. `flux_wake`는 무엇이 깨웠는지(source, control word, timeout, already ready)와 깨운 entry 번호를 싣는다. signal(`WAKE_SIGNAL`)은 이전 flux build만 낸다. 지금 flux는 `EINTR`이면 남은 deadline으로 다시 기다린다.

수집하는 것:

- wait start(`flux_wait`). 기다리는 wait word의 수와 timeout을 싣는다
- wakeup과 무엇이 깨웠는지(`flux_wake`)
- callback start와 end(`flux_callback_start`, `flux_callback_end`). start는 endpoint를, end는 delivered 여부, channel, segment를 싣는다
- take(`flux_take`). ticket, slot, 놓친 frame 수, `header.stamp`를 싣는다. publish 쪽 `flux_commit`과 `(segment, ticket)`으로 짝짓는다
- borrow refused와 이유(`flux_refused`)
- lease end(`flux_release`). `held`가 1이면 fence가 실패해 borrow를 쥔 채 남았다는 뜻이다
- Python: GIL wait(`flux_py_gil`, phase 0과 1 사이)와 frame을 Python 객체로 만든 시점(`flux_py_ready`). `py_ready`는 같은 `(channel, ticket)`의 `take`와 짝짓는다

```mermaid
sequenceDiagram
    participant Executor
    participant flux
    participant callback
    participant orma

    Executor-->>orma: flux_wait(executor, sources, timeout_ns) [hot]
    Note over Executor: futex_waitv until a publisher wakes it
    Executor-->>orma: flux_wake(executor, result, entry) [hot]
    Executor-->>orma: flux_callback_start(executor, entry, endpoint) [hot]
    Executor->>flux: take()
    flux-->>orma: flux_take(channel, ticket, slot, lost, stamp) [hot]
    Executor->>callback: callback(frame)
    Note over callback: frame dropped
    flux-->>orma: flux_release(channel, ticket, held) [hot]
    Executor-->>orma: flux_callback_end(executor, entry, delivered, channel, segment) [hot]
```

## 4. Scheduling

orma는 thread가 깨어난 시각(wakeup), CPU를 받은 시각(switch-in), CPU를 놓은 시각(switch-out)을 받는다. kernel tracepoint `sched_switch`와 `sched_wakeup`을 eBPF로 받으므로 ormad가 있어야 한다. collection method와는 상관없다. 대상 thread만 kernel 안에서 거른다.

두 가지에 쓴다.

- scheduling latency: switch-in 시각 - wakeup 시각
- callback 안 off-CPU time: callback start와 end 사이에 thread가 CPU를 놓은 시간. ormad는 그 thread가 지금 어느 callback 안에 있는지 알아야 한다. uprobe method에서는 eBPF 프로그램이 callback stack을 갖고 있다. `preload`와 `lttng`에서는 thread마다 지금 callback을 shared memory 파일에 적고, ormad가 그 파일을 읽는다

orma는 callback 안 on-CPU time(callback duration - callback 안 off-CPU time)을 기준으로 CPU를 node에 나눈다. DDS와 SDK thread처럼 callback 밖에서 도는 몫은 process 공용 몫으로 따로 보인다. `sched`가 없으면 callback wall time으로 낸 상한을 쓰고, `orma node cpu`의 CPU source 열이 `wall_bound`가 된다.

## 5. Additional collection

span은 항상 수집한다. 나머지는 client가 켤 때만 붙는다(`--gpu`, `--dds`, `--futex`, rqt의 Selection tab). event마다 thread와 시각이 있다. orma-collectd는 이 값으로 event를 그 thread에서 지금 도는 callback에 붙인다.

### 5.1 Spans

사용자가 코드에 표시한 구간이다. `liborma_span.so`의 `orma_span_begin(name)`, `orma_span_end(name)`은 몸체가 빈 함수다. C++ `orma::Span`과 Python decorator `orma_span.span`도 이 둘을 부른다.

수집하는 것:

- span start(`span_begin`). 이름 문자열의 주소(key)와 이름. 이름은 begin에서만 읽는다
- span end(`span_end`). key

```mermaid
sequenceDiagram
    participant callback
    participant orma_span
    participant orma

    callback->>orma_span: orma_span_begin("preprocess")
    orma_span-->>orma: span_begin(key, name) [hot]
    Note over callback: the marked stretch
    callback->>orma_span: orma_span_end("preprocess")
    orma_span-->>orma: span_end(key) [hot]
```

- begin과 end는 같은 thread에서 key로 짝짓는다. 짝 없는 end는 버린다
- span 안의 span은 감싼 span을, callback 안의 span은 감싼 callback을 갖는다
- 이름은 정적 수명의 문자열이어야 한다. 주소가 다르면 begin과 end가 이어지지 않는다

### 5.2 CUDA driver calls

`libcuda.so`의 driver 함수 entry와 return이다. 호출 하나가 record 하나다.

| 종류 | 함수 |
| --- | --- |
| launch | kernel launch, graph launch |
| copy | 동기 copy, 비동기 copy(`_v2` 이름과 `_ptds`/`_ptsz` 변형). byte 수는 인자에서 읽는다. 2D, 3D copy는 width x height(x depth) |
| synchronize | stream, context, event synchronize |
| memory | 할당, 해제 |

수집하는 것(`cuda_call`): 호출 시간, byte 수, 종류, `CUresult`

- 재는 것은 host 쪽 호출 시간이다. GPU 안의 kernel 실행 시간은 재지 않는다. 비동기 launch는 바로 반환하고, 기다린 시간은 synchronize와 동기 copy에 나타난다
- cudart는 libcuda를 자기 handle의 `dlsym`과 `cuGetProcAddress`로 얻는다. `preload`와 `lttng`은 이 둘도 가로채 자기 CUDA 함수를 건넨다. 그래서 cudart를 정적으로 링크한 프로그램(nvcc 기본)도 잡힌다. `uprobe`는 함수 주소에 붙으므로 경로와 무관하다
- ormad는 Jetson의 `nvidia/`, `tegra/` 아래 `libcuda.so`에도 붙는다
- orma는 CUDA toolkit 없이 빌드한다. 시간을 재는 함수의 prototype은 `orma_preload`의 `cuda_api.h`에 있다

### 5.3 DDS writes

DDS write 함수의 entry와 return이다. 호출 하나가 record 하나다.

| DDS | 함수 |
| --- | --- |
| Cyclone DDS(`libddsc.so`) | `dds_write`, `dds_write_ts`, `dds_writecdr`, `dds_forwardcdr` |
| Fast DDS(`libfastrtps.so` 2.x, `libfastdds.so` 3.x) | `DataWriter::write` overload |

수집하는 것(`dds_write`): writer, 호출 시간

- orma-collectd는 write를 같은 thread의 진행 중인 `rcl_publish`와 짝지어 topic을 붙인다. 짝이 없는 write는 버린다
- segment 두 개를 낸다(6절): publish에서 DDS write 시작까지, DDS write 호출에 걸린 시간

### 5.4 futex waits

kernel tracepoint `syscalls:sys_enter_futex`/`sys_exit_futex`와 `futex_waitv` 쌍을 eBPF로 받는다. ormad가 있어야 하고 collection method와는 무관하다. 대상 thread만 kernel 안에서 거른다.

- 대상: 대기 command(`FUTEX_WAIT`, `FUTEX_WAIT_BITSET`, `FUTEX_LOCK_PI`, `FUTEX_LOCK_PI2`, `FUTEX_WAIT_REQUEUE_PI`)와 `futex_waitv`
- 수집하는 것(`futex_wait`, return 때): 주소, 대기 시간, command, 반환값
- callback 안 대기: callback start 전에 시작한 대기는 callback start부터 센다

## 6. Latency segments

ROS topic과 flux channel을 같은 segment로 잰다.

| event | ROS | flux |
| --- | --- | --- |
| publish | `rmw_publish`, broker만 받는 publish는 `rcl_ipc_publish`(2.13) | `flux_commit` |
| matching key | `(topic, source timestamp)` | `(segment, ticket)` |
| take | `rmw_take`(`taken`이 참), `rcl_ipc_take` | `flux_take` |
| wait start | `rcl_wait` entry, `rcl_executor_trace_wait_enter`(2.12) | `flux_wait` |
| wakeup | `rcl_wait` return, `rcl_executor_trace_wait_exit` | `flux_wake` |
| dispatch start | `rclcpp_executor_execute`, `rcl_executor_claim` | - |
| callback | `callback_start` -> `callback_end` | `flux_callback_start` -> `flux_callback_end` |

| segment | 정의 |
| --- | --- |
| transport latency | take - publish(같은 message) |
| wait time | wakeup - wait start(같은 thread) |
| wakeup latency | wakeup - publish |
| dispatch latency | callback start - wakeup |
| callback duration | callback end - callback start |
| callback on-CPU time | callback duration - callback 안 off-CPU time |
| scheduling latency | switch-in - wakeup(4절) |
| pub to dds | DDS write start - `rclcpp_publish`(없으면 `rcl_publish`), 같은 thread(5.3) |
| dds write | DDS write 호출 시간(5.3) |
| span duration | `span_end` - `span_begin`(5.1) |
| 전달 경로별 segment | hop by path, take -> callback start, rclpy 변환, hold(2.13) |

- process 안 segment(callback duration, dispatch latency)는 instrumentation point에서 aggregate한다. `lttng`만 orma-collectd에서 aggregate한다.
- process 사이 segment(transport latency, wakeup latency)는 orma-collectd가 matching해서 낸다.
- `orma analyze`는 recording을 이 segment로 나눈다.

## 7. Distro differences

orma는 process마다 distro를 판정한다. 한 host에 Jazzy process와 Humble container가 함께 있을 수 있기 때문이다. 판정은 `/proc/<pid>/environ`의 `ROS_DISTRO`, 없으면 매핑된 `libtracetools.so`의 경로(`/opt/ros/<distro>/`)로 한다. 파일은 모두 `/proc/<pid>/root`를 거쳐 열므로 container 안의 library도 그 container의 파일로 판정한다.

| 항목 | Humble | Jazzy |
| --- | --- | --- |
| `libtracetools.so`의 LTTng | X. `lttng` method에서는 `liborma_lttng.so`의 `ros2` provider가 대신 event를 만든다 | O |
| `rclcpp_callback_register` | gate가 없다. 이름 문자열이 빌 수 있어 `callback_code`로 채운다 | gated(1절) |
| publish ABI | rcl: `ros_trace_rmw_publish(message)`(orma event `rmw_publish_rcl`). publisher key는 `rcl_publish`의 rcl handle | rmw: `ros_trace_rmw_publish(rmw_handle, message, timestamp)` |
| transport latency | take 시각 - source timestamp(wall clock). publish 쪽 timestamp가 없어 matching할 수 없고, 같은 host에서만 맞는 값이다 | matching한 monotonic 시각 차 |
| `rclcpp_intra_publish` | X. intra-process로만 오가는 topic은 행이 생기지 않는다 | O |
| executor 소속 | `Executor::add_callback_group_to_map`(`rclcpp_executor_add_group`) | `ExecutorEntitiesCollector` 함수 |
| rmw client, service tracepoint | X(orma 미부착) | X(orma 미부착) |

## 8. Coverage by method

| 항목 | `uprobe` | `preload` | `lttng` |
| --- | --- | --- | --- |
| 붙이는 때 | 돌고 있는 process | process 시작 | process 시작 |
| ROS tracepoint | gated tracepoint를 뺀 전부 | 전부 | 전부 |
| callback 함수 이름 | `callback_code` | `rclcpp_callback_register` | `rclcpp_callback_register` |
| `rcl_wait`, `rcl_timer_fini`, rclcpp C++ 함수 | O | O | O |
| rcl_executor anchor(2.12) | O | X | X |
| 전달 경로 probe(2.13) | O | O | O |
| late attach일 때 topic과 node(2.11) | O | -(시작부터 부착) | - |
| DDS serialized size | opt-in | 이 uprobe 추가 필요 | 이 uprobe 추가 필요 |
| `header.stamp`, `/clock` | O | O | O |
| flux event | O | O | O |
| span | O | O | O |
| CUDA driver 호출(opt-in) | O | O | O |
| DDS write(opt-in) | O | O | O |
| futex 대기(opt-in) | ormad | ormad | ormad |
| callback 안 CPU, scheduling latency | ormad | ormad가 있을 때 | ormad가 있을 때 |
| malloc attribution(opt-in) | X | O | X |
| 권한 | ormad | 불필요 | 사용자 session daemon |

`liborma_preload.so`와 `liborma_lttng.so`는 함께 넣을 수 있다. 둘 다 원래 함수를 `dlsym(RTLD_NEXT)`로 부른다. 그래서 호출은 앞의 library가 먼저 받아 뒤의 library로 넘기고, 마지막에 원래 함수(uprobe가 붙는 곳)가 불린다.

## 9. Limitations

orma는 ROS 2를 고치지 않는다. 그래서 한계마다 우회 방법을 두거나, 빈칸을 그대로 받아들였다.

| 한계 | 결과 | 대응 |
| --- | --- | --- |
| gated tracepoint(1절) | uprobe는 callback 함수 이름과 zenoh publisher gid를 받지 못한다 | 함수 이름은 `callback_code`로 채운다. 측정값은 같다 |
| rclpy는 callback event를 내지 않는다 | Python callback duration과 소속이 없다 | `orma_rclpy`(2.10). 이미 돌고 있는 Python node는 재시작해야 한다 |
| Humble, Jazzy에는 rmw client, service tracepoint가 없다 | service request와 response latency를 잴 수 없다 | service callback duration만 낸다 |
| publish와 take를 잇는 값은 source timestamp뿐이다 | source timestamp만으로 짝지으면 다른 topic의 message가 섞인다 | matching key에 topic을 넣는다. 같은 topic에서 겹치면 ambiguous로 센다 |
| intra-process 전달에는 take가 없다 | transport latency와 take rate가 없다 | `is_intra_process`로 경로를 가리고 publish rate를 보인다(2.8) |
| executor와 callback group 소속에는 tracepoint가 없다 | 소속 없이는 wait와 무거운 callback을 가르지 못한다 | rclcpp C++ 함수에서 읽는다(2.4). inline된 함수는 보이지 않는다 |
| DDS thread와 heap은 process의 node 전부가 같이 쓴다 | node에 나눌 근거가 없다 | process 공용 몫으로 따로 보인다 |
| memory는 address space 단위다 | node별 memory가 없다 | process RSS, PSS만 낸다. callback별 malloc attribution은 `preload`의 opt-in |
| serialized size는 rmw 내부 함수에만 있다 | `preload`, `lttng`로는 bandwidth를 볼 수 없다 | uprobe opt-in으로만 잰다 |
| late attach하면 init과 registration event를 놓친다 | 이름과 소속이 빈다 | topic은 rmw handle에서, node는 executor에서 읽는다(2.11). 못 찾은 것은 `unlinked`로 둔다 |
| orma는 message type을 모른다 | `header.stamp` 위치를 모른다 | 앞 8 B를 읽고 clock 검사를 통과한 값만 쓴다(2.6) |
| `EventsExecutor`는 `rcl_wait`를 쓰지 않는다 | wait와 wakeup이 없다 | 그 칸을 비워 둔다 |
| overhaul의 `RclExecutor`는 `rcl_wait`와 rclcpp executor tracepoint를 쓰지 않는다 | stock probe로는 wait, wakeup, dispatch start가 없다 | core의 anchor에 붙는다(2.12). anchor가 entity를 넘기지 않아 `rcl_executor_next_excluding`에서 item을 읽는다 |
| overhaul broker만 받는 publish에는 `rmw_publish`가 없다 | stock point로는 broker 전달의 publish와 경로가 없다 | rcl과 rclpy의 anchor에 붙는다(2.13) |
| CUDA는 host 쪽 호출만 보인다 | GPU 안의 kernel 실행 시간이 없다 | synchronize와 동기 copy의 대기 시간으로 본다(5.2) |
