# rsupervisord: Event Listener 兼容需求 (EVENTLISTENER_COMPAT.md)

| Document Version | Status | Target Language | Scope |
| :--- | :--- | :--- | :--- |
| **v1.0.0** | Draft / For Review | Rust (Edition 2024) | `[eventlistener:x]` 配置面、`READY`/`RESULT` 线协议、事件封装与事件类型 payload、池缓冲/分发/生命周期语义,目标:drop-in 兼容 superlance 等外部监听工具;可执行基准见 §12 / [`../compat/tests/test_eventlistener.py`](../compat/tests/test_eventlistener.py) |

---

## 1. 目的与范围

回答:**要让 stock supervisord 生态里的事件监听程序(如 superlance `memmon`/`httpok`)无改动接入 rsupervisord,需要实现哪些能力、达成什么协议行为。**

- **权威基准**:Python Supervisor **4.2.5** 的实现:
  - `supervisor/options.py`(`[eventlistener:x]` 段解析与 `EventListenerPoolConfig`);
  - `supervisor/process.py::EventListenerPool`(池、缓冲、封装、分发、serial);
  - `supervisor/dispatchers.py::PEventListenerDispatcher`(握手状态机与 `RESULT` 解析);
  - `supervisor/events.py`(事件类型与 `payload()`);
  - `supervisor/rpcinterface.py::sendRemoteCommEvent`。
- **现状基线**:rsupervisord 内部事件总线 `src/manager/event.rs`(`EventHub` / `SystemEvent` / `LogEntry`,tokio broadcast,**面向内部**,非线协议);INI 适配器当前**丢弃** `[eventlistener:*]` 段(`src/compat/ini/adapter.rs:115-118`);XML-RPC `sendRemoteCommEvent` 返回 `FAILED`(`src/compat/xmlrpc/supervisor.rs:276`)。
- **关联**:功能编号 #6 见 [`SUPERVISORD_COMPAT.md`](./SUPERVISORD_COMPAT.md) §7(条件性启用);`[eventlistener:x]` 字段继承 `[program:x]` 见 [`INI_COMPAT.md`](./INI_COMPAT.md);`sendRemoteCommEvent` 见 [`XMLRPC_COMPAT.md`](./XMLRPC_COMPAT.md) §9 P2。

**核心结论(预览)**:这是一个**双向线协议子系统**,不能只靠内部 `EventHub` 广播替代——daemon 必须与监听子进程建立 **stdout(事件流)/stdin(命令流)** 通道,实现 `READY`/`RESULT` 握手、长度前缀封装、每池事件缓冲与 serial 管理。**建议独立旁挂"Event Listener 子系统"**,由 `EventHub` → 事件适配层 → 监听池分发,不改写现有执行核心。

---

## 2. 优先级定义

| 级别 | 含义 |
| :--- | :--- |
| **P0** | 让协议正确的最小闭环:`[eventlistener:x]` 解析、池进程化、`READY`/`RESULT` 握手、`PROCESS_STATE_*` 事件投递、缓冲与背压。 |
| **P1** | 完整事件面:`PROCESS_LOG_*`、`PROCESS_COMMUNICATION_*`、`REMOTE_COMMUNICATION`、`TICK_*`、`PROCESS_GROUP_*`、`SUPERVISOR_STATE_CHANGE_*`;`result_handler`;协议违规 → `UNKNOWN`。 |
| **P2 / 搁置** | `PROCESS_COMMUNICATION_*` 依赖 stdin 注入(#7)与捕获 token;`sendRemoteCommEvent` 已可随本子系统解锁。 |
| **不支持 / 降级** | 与 Python 运行时耦合的 `result_handler` import spec(见 §4、§13)。 |

---

## 3. 术语与架构定位

| 术语 | 定义 |
| :--- | :--- |
| **监听池(pool)** | 一个 `[eventlistener:x]` 段即一个**同构进程组**,`numprocs` 个监听进程共享同一 `events=` 订阅集。池名 = 段名 `x`。 |
| **监听进程(listener)** | 池内的子进程;stdout 是**协议通道**,stdin 是**命令通道**,stderr 走日志。 |
| **事件(event)** | daemon 内部 `notify(event)` 产生的对象,具有 `payload()` 与 `serial`。 |
| **封装(envelope)** | 发往监听进程的一行头 + 变长 payload(§5.1)。 |
| **`READY`/`RESULT`** | 监听进程侧的两条控制行:宣告可接收、回报处理结果。 |

**架构定位**:新增 daemon→listener 通道,与现有 `ManagerActor`/`ProcessActor` 并列。`EventListenerPool` 不是 `ProgramConfig` 的简单复用:它需要**独立的监听状态机**与**事件缓冲队列**。推荐:

```
EventHub (internal broadcast)
   └─> EventAdapter  (SystemEvent/LogEntry -> stock event payload)
          └─> EventListenerPool(s)  (per-pool buffer + serial + dispatch)
                 └─> ListenerProcess.stdin/stdout  (READY/RESULT wire protocol)
```

---

## 4. 配置面 `[eventlistener:x]`

字段与 `[program:x]` **完全兼容**(继承 `EventListenerConfig(ProcessConfig)`),另加:

| 字段 | 默认 | 约束 | 说明 |
| :--- | :--- | :--- | :--- |
| `command` | — | 必填(继承) | 监听程序命令行。 |
| `events` | — | **必填**;逗号/空白分隔;大写化;未知事件名 → 配置错误 | 订阅集,如 `PROCESS_STATE,PROCESS_LOG,TICK_5`。 |
| `buffer_size` | `10` | 整数 `>= 1`,否则配置错误 | 每池事件缓冲上界(§7)。 |
| `result_handler` | `supervisor.dispatchers:default_handler` | import spec,解析失败 → 配置错误 | 见 §5.3。 |
| `priority` | `-1`(高) | 整数 | 监听者**优先启动、最后停止**。 |
| `redirect_stderr` | `false` | **必须为 false**;置 true → 配置错误 | 混入 stdout 会破坏协议。 |
| `autostart` | `true`(继承) | 布尔 | 池随 daemon 启动。 |
| `numprocs` | `1` | 整数 | 池内监听进程数。 |

> 其它 `[program:x]` 字段(`environment`、`user`、`startsecs`、`stopsignal`、`stdout_logfile` 等)语义一致;`use_stderr` 被**强制为 true**(监听进程 stderr 独立于 stdout)。

---

## 5. 线协议(wire protocol)

### 5.1 事件封装(envelope)

daemon 向监听进程 stdout 写入(UTF-8 字节):

```
ver:3.0 server:<identifier> serial:<global_serial> pool:<pool_name> poolserial:<pool_serial> eventname:<NAME> len:<payload_len>\n<payload>
```

| 字段 | 语义 |
| :--- | :--- |
| `ver` | 固定 `3.0`。 |
| `server` | `[supervisord] identifier`。 |
| `serial` | **全局**单调递增序列(跨所有池),`maxint` 回绕。 |
| `pool` | 池名(`[eventlistener:x]` 的 `x`)。 |
| `poolserial` | **池内**单调递增序列。 |
| `eventname` | §6 的事件名(抽象类型不出现)。 |
| `len` | payload 的**字符数**(Python `len(payload)`)。ASCII 下等于字节数;**含多字节 UTF-8 时按字符计,原版即如此**,监听方按此语义读取。 |
| `<payload>` | 紧随换行的定长 body(§6),无结尾换行要求。 |

### 5.2 握手状态机(监听进程侧)

监听进程 `listener_state` 初值为 `ACKNOWLEDGED`(忙),状态机:

| 当前态 | 收到 | 迁移 | daemon 动作 |
| :--- | :--- | :--- | :--- |
| `ACKNOWLEDGED` | buffer 以 `READY\n` 起始 | → `READY` | 可投递事件 |
| `ACKNOWLEDGED` | 有数据但非 `READY\n` | → **`UNKNOWN`** | 记录 warning,停止投递 |
| `READY` | 任何投机数据 | → **`UNKNOWN`** | 记录 warning,停止投递 |
| `BUSY` | `RESULT <n>\n<n 字节>` | → `ACKNOWLEDGED` | 调用 `result_handler`,成功则继续 |
| `BUSY` | `RESULT` 头非法 | → **`UNKNOWN`** + `EventRejectedEvent` | 事件被丢弃并告警 |
| `BUSY` | `<n>` 不足 | 保持 `BUSY`(续读) | 等待剩余数据 |
| `UNKNOWN` | 任意 | 保持 `UNKNOWN` | 该监听进程**永久**退出接收 |

> `READY` token 为**恰好** `READY\n`(含换行);`RESULT` 前缀为 `RESULT `。

### 5.3 结果处理(`result_handler`)

- 默认 `supervisor.dispatchers:default_handler`:body 必须为 `OK`,否则抛 `RejectEvent`。
- `RejectEvent` → 状态回到 `ACKNOWLEDGED` 并 `notify(EventRejectedEvent)`;池把被拒事件**重新插回缓冲头部**(重投)。
- handler 抛任意异常 → → `UNKNOWN` + `EventRejectedEvent`。

---

## 6. 事件类型与 payload 规范

`payload` 为**单行 k:v 空格分隔**(`PROCESS_LOG_*`/`PROCESS_COMMUNICATION_*`/`REMOTE_COMMUNICATION` 含一个换行后接数据体):

| eventname | payload 格式 |
| :--- | :--- |
| `PROCESS_STATE_STARTING` / `PROCESS_STATE_BACKOFF` | `processname:<n> groupname:<g> from_state:<STATE> tries:<n>` |
| `PROCESS_STATE_RUNNING` / `PROCESS_STATE_STOPPING` / `PROCESS_STATE_STOPPED` | `processname:<n> groupname:<g> from_state:<STATE> pid:<pid>` |
| `PROCESS_STATE_EXITED` | `processname:<n> groupname:<g> from_state:<STATE> expected:<0\|1> pid:<pid>` |
| `PROCESS_STATE_FATAL` / `PROCESS_STATE_UNKNOWN` | `processname:<n> groupname:<g> from_state:<STATE>` |
| `PROCESS_LOG_STDOUT` | `processname:<n> groupname:<g> pid:<pid> channel:stdout\n<data>` |
| `PROCESS_LOG_STDERR` | 同上,`channel:stderr` |
| `PROCESS_COMMUNICATION_STDOUT` / `_STDERR` | `processname:<n> groupname:<g> pid:<pid>\n<data>` |
| `REMOTE_COMMUNICATION` | `type:<type>\n<data>` |
| `TICK_5` / `TICK_60` / `TICK_3600` | `when:<unix 秒整数>` |
| `SUPERVISOR_STATE_CHANGE_RUNNING` / `_STOPPING` | 空串 |
| `PROCESS_GROUP_ADDED` / `PROCESS_GROUP_REMOVED` | `groupname:<g>\n` |

`<STATE>` 为 `getProcessStateDescription` 名(`STOPPED`/`STARTING`/`RUNNING`/`BACKOFF`/`STOPPING`/`EXITED`/`FATAL`/`UNKNOWN`);`expected` 为 `0/1` 整数。

**触发点**:
- `PROCESS_STATE_*`:`[program:x]`/组内进程状态迁移。
- `PROCESS_LOG_*`:进程配置 `stdout_events_enabled` / `stderr_events_enabled=true`,且产生输出。
- `PROCESS_COMMUNICATION_*`:配置 `stdout_capture_maxbytes` / `stderr_capture_maxbytes`,且输出含 `<!--XSUPERVISOR:BEGIN-->…<!--XSUPERVISOR:END-->` 捕获 token。
- `REMOTE_COMMUNICATION`:XML-RPC `sendRemoteCommEvent(type, data)`。
- `TICK_*`:daemon 每 5s / 60s / 3600s 定时。
- `PROCESS_GROUP_ADDED/REMOVED`:运行时 `addProcessGroup`/`removeProcessGroup`(rsupervisord 的 compat shim,见 [`XMLRPC_COMPAT.md`](./XMLRPC_COMPAT.md) §9)。
- `SUPERVISOR_STATE_CHANGE_*`:daemon 进入 RUNNING / 开始 STOPPING。

---

## 7. 池、缓冲与分发语义

1. **每池独立缓冲**:事件先 `_acceptEvent` 入池队列,再在 `transition()` 中按序分发。
2. **serial 分配入队时完成**:`serial`(全局)与 `poolserial`(池内)在首次入队时赋值;重投复用同一 `serial`/`poolserial`。
3. **背压**:仅投递给 `RUNNING` 且 `listener_state == READY` 的监听进程;头一个可用者接收,该进程转 `BUSY`。
4. **重投**:分发失败(含 `RejectEvent`)时事件**插回队首**,并停止本轮后续分发。
5. **溢出**:队列长度 `>= buffer_size` 时**丢弃最旧**事件并记录 error(`pool <name> event buffer overflowed, discarding event <serial>`)。
6. **多监听进程**:同池多进程并发时,每个事件只投递给一个 `READY` 进程。

---

## 8. 生命周期与进程语义

- 池以 `priority` 默认 `-1` **最先启动、最后停止**;daemon 停止时应先停止普通程序、最后停止监听池。
- 监听进程 stdout 仅承载协议;`redirect_stderr=true` 被拒绝。
- 池可被 `startProcess`/`stopProcess`/`signalProcess` 等按普通组操作(池名即 group 名,进程名同池名,故 namespec 为 `listener`、`listener:*`)。

---

## 9. XML-RPC / CLI 交互面

| 方法/命令 | 行为 |
| :--- | :--- |
| `supervisor.getProcessInfo("listener")` / `getAllProcessInfo` | 监听池作为普通组出现(`group == pool name`,进程名 == pool name,`statename` 正常)。 |
| `supervisor.sendRemoteCommEvent(type, data)` | 触发 `REMOTE_COMMUNICATION` 事件,返回 `True`。 |
| `supervisor.getAllConfigInfo` | 监听池配置**在** `process_group_configs` 内(与程序组并列)。 |
| `supervisorctl status` | 显示监听池,与程序组一致。 |

---

## 10. rsupervisord 现状与差距映射

| 能力 | 现状 | 差距 |
| :--- | :--- | :--- |
| `[eventlistener:x]` 解析 | INI 适配器**丢弃**并发 warning(`src/compat/ini/adapter.rs:115-118`) | 需建成池配置(§4) |
| 事件源 | `EventHub`(`src/manager/event.rs`:`SystemEvent`/`LogEntry`,内部) | 需映射为 §6 的 stock payload + serial |
| 线协议 | 无 | 全新 `READY`/`RESULT` 通道 + 状态机(§5) |
| 缓冲/分发 | 无 | 每池队列 + `buffer_size` + 重投/溢出(§7) |
| `sendRemoteCommEvent` | `FAILED`(`src/compat/xmlrpc/supervisor.rs:276`) | 接入本子系统后返回 `True` |
| `PROCESS_COMMUNICATION_*` | 依赖 stdin 注入(#7)与捕获 token | P2,依赖 #7 |

**EventHub → stock 事件的最小映射**(P0):

| 内部 `SystemEvent` | stock 事件 |
| :--- | :--- |
| `StateChanged`(→ Starting/Running/Exited/...) | `PROCESS_STATE_*` |
| `LogEntry`(stdout/stderr) | `PROCESS_LOG_STDOUT` / `PROCESS_LOG_STDERR`(受 `*_events_enabled` 控制) |
| `ConfigReloaded` + 组增删 | `PROCESS_GROUP_ADDED` / `PROCESS_GROUP_REMOVED` |
| `DaemonLifecycle` | `SUPERVISOR_STATE_CHANGE_RUNNING` / `_STOPPING` |
| 定时器 | `TICK_5` / `TICK_60` / `TICK_3600` |
| XML-RPC `sendRemoteCommEvent` | `REMOTE_COMMUNICATION` |

---

## 11. 详细需求条目

约定:**MUST** 必须实现;**SHOULD** 强烈建议;**MAY** 可选。

- **EL-1(MUST)** 解析 `[eventlistener:x]` 为监听池配置:`events` 必填且校验、`buffer_size>=1`、`redirect_stderr` 禁止、`priority` 默认 `-1`;监听池在 `getAllProcessInfo` 中作为组 `<x>` 出现。
- **EL-2(MUST)** 池内监听进程 stdin/stdout 建为管道;stdout 仅承载协议,stderr 独立。
- **EL-3(MUST)** 实现 `READY\n` 握手与 `ACKNOWLEDGED→READY→BUSY→ACKNOWLEDGED` 状态机;`BUSY` 仅在有完整 `RESULT <n>\n` 头与 `<n>` 字节 body 后推进。
- **EL-4(MUST)** 按 §5.1 生成封装,`serial`(全局)与 `poolserial`(池内)单调递增,`len` 为 payload 字符数。
- **EL-5(MUST)** 仅向订阅集内事件(§6)投递;`events=` 未列出的类型不得下发。
- **EL-6(MUST)** P0 至少覆盖 `PROCESS_STATE_*`、`TICK_*`、`REMOTE_COMMUNICATION`;P1 覆盖 `PROCESS_LOG_*`、`PROCESS_GROUP_*`、`SUPERVISOR_STATE_CHANGE_*`;payload 字段与 §6 逐字段一致。
- **EL-7(MUST)** 协议违规(非 `READY` 数据、`READY` 后投机数据、非法 `RESULT` 头)→ 监听进程转入 `UNKNOWN`,记录 warning,并**停止**向其投递。
- **EL-8(MUST)** 每池缓冲与背压:溢出丢最旧并记录 error;分发失败重投队首;`result_handler` 拒绝时重投。
- **EL-9(SHOULD)** `result_handler` 语义对齐(默认 body==`OK`);rsupervisord 可先固定默认 handler。
- **EL-10(MUST)** `sendRemoteCommEvent(type, data)` 返回 `True` 并触发 `REMOTE_COMMUNICATION`。
- **EL-11(SHOULD)** 池默认最高优先级:先启动、最后停止。
- **EL-12(MAY)** `PROCESS_COMMUNICATION_*`(依赖 #7 stdin 注入与捕获 token)。

---

## 12. 验收与基准测试映射

可执行基准:[`../compat/tests/test_eventlistener.py`](../compat/tests/test_eventlistener.py),先在 Python **4.2.5** 上全绿,再对 rsupervisord 门控放行。

| 需求 | 基准用例 |
| :--- | :--- |
| EL-1 | `test_eventlistener_pool_is_running`、`test_process_state_groupname` |
| EL-3/EL-4 | `test_event_envelope_fields`、`test_protocol_violation_marks_listener_unknown` |
| EL-5/EL-6 | `test_process_state_events`、`test_process_log_stdout_events`、`test_tick_event`、`test_remote_communication_event` |
| EL-7 | `test_protocol_violation_marks_listener_unknown` |
| EL-8 | `test_event_buffer_overflow_discards_oldest` |
| EL-10 | `test_remote_communication_event` |

运行:

```bash
SUPERVISOR_TARGET=python bash compat/run.sh -k eventlistener   # 基准(全绿)
bash compat/run.sh -k eventlistener                           # rsupervisord(当前 xfail,实现后放行)
```

> 未实现前,该模块在 rsupervisord 靶标统一记为 **`xfail`**;`SUPERVISOR_STRICT=1` 转为硬失败,即 §11 的待办全貌。

---

## 13. 非目标与降级

- **`result_handler` 的 Python import spec**:rsupervisord 不内嵌 Python 运行时。**降级**:仅支持内置默认 handler(`body == OK`);自定义 `result_handler` 声明为不支持,或提供 Rust 侧等价注册点(MAY)。
- **`PROCESS_COMMUNICATION_*`**:依赖 #7 stdin 注入与捕获 token,列为 P2。
- **不改变现有执行核心**:监听池是旁挂子系统;`EventHub` 仍是内部事实源,线协议是适配层产物。

---

## 14. 参考

- Python Supervisor **4.2.5**:`options.py`(`EventListenerConfig` / `EventListenerPoolConfig`)、`process.py::EventListenerPool`、`dispatchers.py::PEventListenerDispatcher`、`events.py`、`rpcinterface.py::sendRemoteCommEvent`。
- 协议文档:Supervisor 官方 *Event Listeners* / *Events* 章节(events 类型与 `READY`/`RESULT` 示例)。
- Go 参考实现 `ochinchina/supervisord` `events` 包(`EventSysVersion = "3.0"`、`EventListener`、`BaseEvent`、`ProcessStateEvent`、`RemoteCommunicationEvent`)——事件命名/封装的可对照实现。
- 关联文档:[`SUPERVISORD_COMPAT.md`](./SUPERVISORD_COMPAT.md) §7 #6、[`XMLRPC_COMPAT.md`](./XMLRPC_COMPAT.md) §9、[`INI_COMPAT.md`](./INI_COMPAT.md)、[`../compat/README.md`](../compat/README.md)。
