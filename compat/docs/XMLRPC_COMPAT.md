# rsupervisord: XML-RPC 兼容需求 (XMLRPC_COMPAT.md)

| Document Version | Status | Target Language | Scope |
| :--- | :--- | :--- | :--- |
| **v1.1.0** | Draft / For Review | Rust (Edition 2024) | XML-RPC 线协议、Fault 错误码、`supervisor.*` / `system.*` 方法面、逐方法映射与优先级,目标:标准 `supervisorctl` 直连;可执行基准见 §12 / [`../compat/README.md`](../compat/README.md) |

---

## 1. 目的与范围

回答:**要让标准 `supervisorctl`(Python 4.2.5)直连 rsupervisord,需要实现哪些 XML-RPC 能力、达成什么协议行为。**

- **权威基准**:Python Supervisor **4.2.5** 的 `supervisor/rpcinterface.py`(方法面)与 `supervisor/xmlrpc.py`(线协议/Faults/命名空间/`multicall`)。
- **现状基线**:`src/server/api.rs`(REST 路由 + `AppState`)、`src/server/auth.rs`(Basic auth)、`src/manager/supervisor.rs`(`ManagerHandle` + `ManagerCommand`)。
- **关联**:CLI 侧参数/退出码见 [`CLI_COMPAT.md`](./CLI_COMPAT.md);日志字节偏移降级见 [`SUPERVISORD_COMPAT.md`](./SUPERVISORD_COMPAT.md) §7 #8;进程组/事件见 §7 #4/#6。

**核心结论(预览)**:XML-RPC 是**纯适配层**——新的 `src/server/xrpc.rs` 挂在现有 Axum 路由的 `/RPC2`,复用 `AppState`/`ManagerHandle`/Basic auth,把 XML-RPC 调用翻译为既有的 `ManagerCommand`。**无需改动执行核心**;主要工作是**编解码、Fault 映射、名称语义(namespec/组)、日志降级**。

---

## 2. 优先级定义

| 级别 | 含义 |
| :--- | :--- |
| **P0** | 让 stock `supervisorctl` 的日常命令直连可用(status/start/stop/restart/signal/tail/maintail/version/shutdown/reread/update/all)。 |
| **P1** | 完整覆盖其余只读/辅助方法(clear、read*、avail/getAllConfigInfo、system.* 内省、group 级 signal)。 |
| **P2 / 搁置** | 依赖 daemon 重启语义(`restart`),或尚无对应基础能力的项。 |
| **不支持 / 降级** | 复杂或与架构冲突,明确声明降级行为。 |

---

## 3. 线协议要求(wire protocol)

| 项 | Python 行为 | rsupervisord 要求 |
| :--- | :--- | :--- |
| 端点 | HTTP `POST` 到路径 **`/RPC2`** | 新增路由 `/RPC2`,并入现有 `build_router`(同 UDS/TCP、同进程) |
| Content-Type | 请求/响应均 `text/xml` | 一致;兼容缺失 `Content-Type` 的宽松客户端 |
| 请求体 | `<methodCall><methodName>ns.m</methodName><params><param><value>…</value></param></params></methodCall>` | 一致;允许**无 `<params>`** 的零参调用 |
| 响应体 | `<methodResponse><params><param><value>…</value></param></params></methodResponse>` | 一致 |
| Fault | `<methodResponse><fault><value><struct>{faultCode:int,faultString:string}</struct></value></fault></methodResponse>`,HTTP 仍 **200** | 一致;Fault 码见 §4 |
| 鉴权 | HTTP **Basic**(`[inet_http_server]` username/password);UDS 亦适用 | 复用 `src/server/auth.rs` 的 `BasicAuthConfig` / `inet_http_auth_middleware` |
| 整数 | 32-bit `i4`;时间戳经 `capped_int` 饱和到 `MININT/MAXINT`(2038 问题) | 必须饱和;`getProcessInfo.start/stop/now` 用 `i4` |
| 布尔 | `<boolean>1</boolean>` / `0` | 一致 |
| base64 | 用于 `sendProcessStdin` 的二进制 chars | 一致 |
| dateTime.iso8601 | 解析支持(入站) | 入站可选;出站不产生 |
| 命名空间 | `supervisor.*` 与 `system.*`;**方法名必须恰有 2 段点分**(CVE-2017-11610 防护) | 严格 2 段;拒绝 `_` 前缀方法与属性遍历 |
| 未知方法 | `Fault(1 UNKNOWN_METHOD)` | 一致 |
| 参数错误 | `Fault(2 INCORRECT_PARAMETERS)` | 一致 |
| 长操作 | 返回**回调函数**(`NOT_DONE_YET`),由 select 循环分块续跑(如 `startProcess(wait=True)`) | **降级**:同步 `await` 后一次性返回;语义结果一致,不模拟分块 |
| `system.multicall` | 顺序执行,逐项返回结果或 fault;禁止递归 multicall | 一致(递归拒绝返回 `INCORRECT_PARAMETERS`) |

**入站解析注意**:Python 的 `loads` 用 `iterparse` 且对 `array/struct/value` 有特殊反序列化;实现时按标准 XML-RPC 解析即可,但需容忍其宽松点(缺 `<params>`、空 `<value>`)。

---

## 4. Fault 错误码表(`supervisor/xmlrpc.py::Faults`)

所有业务错误必须映射为下表的 `faultCode`/`faultString`(中英文字符串以 Python 为准,便于客户端识别):

| Code | 名称 | 触发场景 | rsupervisord 来源 |
| :--- | :--- | :--- | :--- |
| 1 | `UNKNOWN_METHOD` | 方法不存在/命名空间非法 | 适配层直接返回 |
| 2 | `INCORRECT_PARAMETERS` | 参数个数/类型错(含 TypeError) | 适配层 |
| 3 | `BAD_ARGUMENTS` | 参数值非法 | 适配层(如有) |
| 4 | `SIGNATURE_UNSUPPORTED` | `system.methodHelp/Signature` 未找到 | `system.*` |
| 6 | `SHUTDOWN_STATE` | daemon 处于 SHUTDOWN/RESTARTING,拒绝调用 | 新增 mood/状态位 |
| 10 | `BAD_NAME` | 名称不存在 | `ProgramError::NotFound` / 组不存在 |
| 11 | `BAD_SIGNAL` | 信号名/编号非法 | 信号解析失败 |
| 20 | `NO_FILE` | 日志文件不存在 / stdin EPIPE | `readFile`/日志路径缺失;stdin 写失败 |
| 21 | `NOT_EXECUTABLE` | command 不可执行/权限不足 | `StartFailed`(可执行性) |
| 30 | `FAILED` | 通用失败 | 多数 `ProgramError` 兜底 |
| 40 | `ABNORMAL_TERMINATION` | 等待启动期间非 STARTING/RUNNING | 启动 wait 超时/异常 |
| 50 | `SPAWN_ERROR` | 派生失败(spawnerr) | `StartFailed` |
| 60 | `ALREADY_STARTED` | 已在运行 | `AlreadyRunning` |
| 70 | `NOT_RUNNING` | 未运行 | `NotRunning` |
| 80 | `SUCCESS` | (仅用于批量结果 `status` 字段) | 批量结果 |
| 90 | `ALREADY_ADDED` | 组已存在 | `addProcessGroup` |
| 91 | `STILL_RUNNING` | 删除组时仍有进程运行 | `removeProcessGroup` |
| 92 | `CANT_REREAD` | 配置重载失败 | `reloadConfig` 解析/校验失败 |

> `ProgramError`(`src/error.rs`)到 Fault 的建议映射:`AlreadyRunning→60`、`NotRunning→70`、`NotFound→10`、`StartFailed→50/21`、`Timeout→40`、`InvalidState→40/70`、`ShuttingDown→6`、其余→`30`。

---

## 5. 方法总览

### 5.1 `supervisor` 命名空间(共 29 个入口,含别名)

| 方法 | 优先级 | Python 签名 | 当前映射 / 实现途径 | 差异 / 降级 |
| :--- | :--- | :--- | :--- | :--- |
| `getAPIVersion` | **P0** | `() → str` | 常量 `"3.0"`(`getVersion` 别名同返) | 直接返回 |
| `getSupervisorVersion` | **P0** | `() → str` | rsupervisord 版本号 | 返回自身版本(非 "4.2.5") |
| `getIdentification` | **P0** | `() → str` | `server`/`[supervisord] identifier`,或默认 `supervisor` | 可配置,默认 `supervisor` |
| `getState` | **P0** | `() → {statecode,statename}` | 新增 mood 状态(RUNNING/SHUTDOWN/RESTARTING/FATAL) | 需引入 supervisor mood 概念 |
| `getPID` | **P0** | `() → int` | `std::process::id()` | 直接 |
| `getAllProcessInfo` | **P0** | `() → [struct]` | `ManagerHandle::get_all_status` → **getProcessInfo 字段表**(§6) | 字段适配 |
| `getProcessInfo` | **P0** | `(name) → struct` | `get_status(name)` + namespec 解析 | 字段/description 适配 |
| `startProcess` | **P0** | `(name, wait=True) → bool` | `ManagerCommand::StartProgram`(namespec→进程/组/`*`) | 同步等待 |
| `startProcessGroup` | **P0** | `(name, wait=True) → [struct]` | `StartGroup` | 结果 struct 适配 |
| `startAllProcesses` | **P0** | `(wait=True) → [struct]` | `StartAll` | 结果 struct 适配 |
| `stopProcess` | **P0** | `(name, wait=True) → bool` | `StopProgram` | 同步等待 |
| `stopProcessGroup` | **P0** | `(name, wait=True) → [struct]` | `StopGroup` | 结果 struct 适配 |
| `stopAllProcesses` | **P0** | `(wait=True) → [struct]` | `StopAll` | 结果 struct 适配 |
| `signalProcess` | **P0** | `(name, signal) → bool` | `SignalProgram` | 信号名↔`StopSignal` |
| `signalProcessGroup` | **P1** | `(name, signal) → [struct]` | 组内逐进程 signal | 结果 struct 适配 |
| `signalAllProcesses` | **P1** | `(signal) → [struct]` | 全量逐进程 signal | 结果 struct 适配 |
| `tailProcessStdoutLog` | **P0** | `(name, offset, length) → [str, int, bool]` | `subscribe/read_logs` + **#8 降级** | **行级降级**:见 §7.1 |
| `tailProcessStderrLog` | **P0** | `(name, offset, length) → [str, int, bool]` | 同上(stderr 合并语义) | 行级降级 |
| `readLog` | **P0** | `(offset, length) → str` | 主日志(`logging.file`)读取 | maintail;见 §7.2 |
| `readProcessStdoutLog` | **P1** | `(name, offset, length) → str` | 程序日志读取 | 行级降级 |
| `readProcessStderrLog` | **P1** | `(name, offset, length) → str` | 程序 stderr 日志 | 行级降级 |
| `clearLog` | **P1** | `() → bool` | 主日志清空/重开 | 见 §7.2 |
| `clearProcessLogs` | **P1** | `(name) → bool` | 程序日志清空 | 轮转重置 |
| `clearAllProcessLogs` | **P1** | `() → [struct]` | 全量清空 | 结果 struct 适配 |
| `reloadConfig` | **P0** | `() → [[added,changed,removed]]` | `ReloadConfig` → `ReloadSummary` | 见 §7.3 |
| `addProcessGroup` | **P1** | `(name) → bool` | `ManagerCommand::AddProcessGroup`(pending 配置激活) | Python 语义:未在源配置 → `BAD_NAME`;已激活 → `ALREADY_ADDED` |
| `removeProcessGroup` | **P1** | `(name) → bool` | `ManagerCommand::RemoveProcessGroup`(仅移除活动组,源配置保留) | Python 语义:未激活 → `BAD_NAME`;运行中 → `STILL_RUNNING` |
| `getAllConfigInfo` | **P1** | `() → [struct]` | 配置(raw)+ `inuse` 计算 | 字段子集,见 §7.4 |
| `sendProcessStdin` | **P1** | `(name, chars) → bool` | `SendStdin`(依赖 **#7**) | #7 未落地前返回 `FAILED`/`NO_FILE` |
| `sendRemoteCommEvent` | **P0** | `(type, data) → bool` | `send_remote_comm_event` → `REMOTE_COMMUNICATION` | 直达事件监听池 |
| `shutdown` | **P0** | `() → bool` | `ManagerCommand::Shutdown` | 直接 |
| `restart` | **P2** | `() → bool` | daemon 重启(非 hot-reload) | **语义差异**:见 §7.5 |
| 别名 `getVersion`/`readMainLog`/`readProcessLog`/`tailProcessLog`/`clearProcessLog` | **P0/P1** | 同对应方法 | 直接转发 | 零成本,必做 |

### 5.2 `system` 命名空间(内省)

| 方法 | 优先级 | 签名 | 说明 |
| :--- | :--- | :--- | :--- |
| `system.listMethods` | **P1** | `() → [str]` | 返回全部可用方法名(含命名空间前缀),字典序排序 |
| `system.methodHelp` | **P1** | `(name) → str` | 返回方法 docstring;未找到 → `SIGNATURE_UNSUPPORTED` |
| `system.methodSignature` | **P1** | `(name) → [rtype, ptype...]` | 从 docstring `@param/@return` 解析 |
| `system.multicall` | **P1** | `(calls) → [result]` | 逐项 `{methodName, params}`,失败项返回 `{faultCode,faultString}`;禁止递归 |

> 内省方法可用**静态注册表**(方法名→(doc, 签名))实现,不必反射 Rust 类型。

---

## 6. `getProcessInfo` 返回字段(必须逐字段对齐)

| 字段 | 类型 | Python 来源 | rsupervisord 来源 / 说明 |
| :--- | :--- | :--- | :--- |
| `name` | string | `config.name` | `ProgramStatus.name` |
| `group` | string | `group.config.name` | `ProgramStatus.group` |
| `start` | int | `laststart`(capped) | 需记录 last start UNIX 秒 |
| `stop` | int | `laststop`(capped) | 需记录 last stop |
| `now` | int | `time.time()`(capped) | 当前时间 |
| `state` | int | `ProcessStates` | **需 0..7 码映射**,见下 |
| `statename` | string | `getProcessStateDescription` | `STARTING/RUNNING/BACKOFF/STOPPING/STOPPED/EXITED/FATAL/UNKNOWN` |
| `spawnerr` | string | `spawnerr or ''` | 启动错误文本(空串默认) |
| `exitstatus` | int | `exitstatus or 0` | `exit_code` 默认 0 |
| `logfile` | string | stdout 路径(兼容别名) | 同 `stdout_logfile` |
| `stdout_logfile` | string | stdout 日志路径 | 空则 `''` |
| `stderr_logfile` | string | stderr 日志路径 | 空则 `''` |
| `pid` | int | `process.pid` | `pid` 默认 0(非 `null`) |
| `description` | string | `_interpretProcessInfo` | 见下 |

**`state` 码映射**(Python `ProcessStates`):`STOPPED=0, STARTING=10, RUNNING=20, BACKOFF=30, STOPPING=40, EXITED=100, FATAL=200, UNKNOWN=1000`。rsupervisord 的 `ProgramState`(`Stopped/Starting/Running/Backoff/Stopping/Exited/Fatal`)需映射到上述码。

**`description` 规则**(`_interpretProcessInfo`):
- RUNNING → `"pid {pid}, uptime {H:MM:SS}"`(uptime = now - start,负值归零)
- FATAL/BACKOFF → `spawnerr`(空则 `unknown error (try "tail {name}")`)
- STOPPED/EXITED → 有 start 则本地时间 `"%b %d %I:%M %p"`;否则 `"Not started"`
- 其它 → `""`

---

## 7. 重点方法的语义与降级

### 7.1 `tail*Log`(P0,依赖 #8 降级)

- **Python 语义**:`tailProcessStdoutLog(name, offset, length) → [bytes, offset, overflow]`。从 `offset` 读至多 `length` 字节;若总长 > `offset+length`,置 `overflow=True` 并把 offset 对齐到日志末尾;返回的 offset 恒为"最后读取位置 +1"。
- **rsupervisord 降级**(与 [`SUPERVISORD_COMPAT.md`](./SUPERVISORD_COMPAT.md) §7 #8 一致):
  - `offset` = 行级 ring buffer 内**行索引**;`length` = **行数**;返回 `[文本, 新行游标, overflow]`。
  - **极大 offset(`0x7fffffffffffffff`,`supervisorctl tail -f` 约定)饱和为"从窗末追"**。
  - 深历史不可达(仅保留窗)、daemon 重启窗口归零——**必须在文档声明**。
- **验收**:真 `supervisorctl tail -f <name>` 可用(浅尾追)。

### 7.2 `readLog` / `clearLog`(maintail)

- `readLog(offset,length)`:读主日志文件(`logging.file`)。无文件 → `NO_FILE`。
- `clearLog`:`supervisorctl maintail` 依赖;实现为截断/重开主日志文件。
- 与程序日志一样走**行级降级**;`maintail -f` 同 `tail -f` 处理。

### 7.3 `reloadConfig`(P0)

- **Python 返回**:`[[added, changed, removed]]`(三段名称数组,注意**外层再包一层数组**)。
- **rsupervisord 映射**:`ManagerHandle::reload_config` 返回 `ReloadSummary` → 拆出 added/changed/removed 的名称列表。
- **解析/校验失败** → `Fault(92 CANT_REREAD, <细节>)`。
- **注意**:与 `supervisorctl reread`(仅检测)/`update`(应用)的关系见 [`CLI_COMPAT.md`](./CLI_COMPAT.md) §5.1.2。

### 7.4 `getAllConfigInfo`(P1,`supervisorctl avail`)

Python 返回每个 program(组被摊平)的**配置快照**,键含:`autostart, directory, uid, command, exitcodes, group, group_prio, inuse, killasgroup, name, process_prio, redirect_stderr, startretries, startsecs, stdout_capture_maxbytes, stdout_events_enabled, stdout_logfile, stdout_logfile_backups, stdout_logfile_maxbytes, stdout_syslog, stopsignal(int), stopwaitsecs, stderr_* , serverurl`,且 `Automatic→'auto'`、`None→'none'`。

- **要求**:返回字段**超集/子集均可能被客户端读取**,至少提供上表存在映射的字段;缺失字段用 `'none'`/默认值填充以免客户端 KeyError。
- `inuse` = 该组当前是否在运行注册表中。

### 7.5 `restart` vs hot-reload(语义差异)

- Python `restart`:置 daemon mood 为 `RESTARTING`,进程退出后由外部(init/systemd/supervisor 自身)**重新拉起**,配置随之生效。
- rsupervisord 的"热重载"是 `reloadConfig` 的应用路径,**不是** `restart`。
- **裁决**:`restart` 归 **P2**,需在实现"进程自重启(exit code 约定 + 外部 supervisor 拉起)"后才提供;在此之前返回 `FAILED` 并提示用 `reloadConfig`/`shutdown`。**不要把 hot-reload 嫁接到 `restart`**(与 `CLI_COMPAT.md` P0 的 `reload` 语义裁决一致)。

### 7.6 `sendProcessStdin` / `sendRemoteCommEvent`

- `sendProcessStdin`:映射 `ManagerCommand::SendStdin`,**完全依赖 #7**(stdin piped + 背压通道)。#7 前应返回 `NOT_RUNNING`/`FAILED`;参数非字符串 → `INCORRECT_PARAMETERS`;EPIPE → `NO_FILE`。
- `sendRemoteCommEvent`:依赖 **#6** EventHub→listener;未落地时 `Fault(UNKNOWN_METHOD)` 或 `FAILED`。

---

## 8. 架构落点

- **新模块**:`src/server/xrpc.rs`,导出 `pub fn xrpc_router() -> Router`,在 `src/server/mod.rs` 注册,并在 `build_router(state)` 中 `merge`(路径 `/RPC2`,与 `/api/v1/*` 并存)。
- **复用**:`AppState { manager: ManagerHandle, basic_auth, .. }`;Basic auth 复用 `src/server/auth.rs`。
- **编解码**:引入纯 Rust XML-RPC 编解码(自实现或轻量 crate),**不引入 Python 依赖**;Faults 用枚举常量(§4)。
- **状态位**:新增 supervisor mood(`RUNNING/SHUTDOWN/RESTARTING/FATAL`)以支撑 `getState`/`restart`/`SHUTDOWN_STATE`。
- **名称语义**:实现 `namespec` 解析(`group:name`、`group:*`、裸名),对齐 [`CLI_COMPAT.md`](./CLI_COMPAT.md) §4.3。

---

## 9. 优先级清单(汇总)

**P0(让 stock supervisorctl 可用)**
1. `/RPC2` 路由 + XML-RPC 编解码 + Basic auth + Faults 映射 + 2 段方法名校验。
2. `getAPIVersion`/`getVersion`、`getSupervisorVersion`、`getIdentification`、`getState`、`getPID`。
3. `getAllProcessInfo`、`getProcessInfo`(§6 全字段)。
4. `startProcess`/`stopProcess` + group + all;`signalProcess`。
5. `tailProcessStdoutLog`/`tailProcessStderrLog` + 别名;`readLog`(maintail)。
6. `reloadConfig`;`shutdown`。

**P1**
7. `readProcessStdoutLog`/`readProcessStderrLog`、`clearLog`/`clearProcessLogs`/`clearAllProcessLogs`。
8. `signalProcessGroup`/`signalAllProcesses`、`getAllConfigInfo`。
9. `sendProcessStdin`(#7 后)、`system.listMethods`/`methodHelp`/`methodSignature`/`multicall`。
10. `addProcessGroup`/`removeProcessGroup`(pending 配置激活/移除,Python 语义)。

**P2 / 搁置**
11. `restart`(daemon 重启语义)。

**不支持 / 降级**
12. 长操作回调分块(`NOT_DONE_YET`)→ **同步返回降级**。
13. `tail`/`read` 深历史 → **行级 ring buffer 降级**(声明式)。

---

## 10. 验收

- **端到端**:未改动的 Python `supervisorctl` 用 `-s unix://…`(或 `http://…` + `-u/-p`)直连 rsupervisord,以下命令 **100% 通过**:
  `status`、`status <name>`、`start/stop/restart <name>`、`start/stop/restart all`、`signal <name> <sig>`、`tail -f <name>`、`maintail`、`version`、`pid`、`reread`、`update`、`shutdown`。
- **Fault 正确性**:错误名 → 期望 `faultCode`(§4)逐项断言。
- **协议健壮性**:无 `<params>`、非法 2 段名、`_` 前缀方法、递归 `multicall` 均被正确拒绝。
- **互操作回归**:REST(`/api/v1/*`)与 XML-RPC(`/RPC2`)共存,现有 67 测试与 clippy `-D warnings` 保持通过。

```bash
supervisorctl -c /etc/supervisord.conf status
supervisorctl -c /etc/supervisord.conf tail -f web
supervisorctl -c /etc/supervisord.conf shutdown
```

---

## 11. 与其它文档的关系

- CLI 参数、退出码、`reload`/`reread`/`update` 语义:见 [`CLI_COMPAT.md`](./CLI_COMPAT.md)。
- INI 段(尤其 `[rpcinterface:supervisor]`、`[inet_http_server]`)加载:见 [`INI_COMPAT.md`](./INI_COMPAT.md)。
- #4 Group、#6 Event Listener、#7 stdin、#8 日志字节偏移:见 [`SUPERVISORD_COMPAT.md`](./SUPERVISORD_COMPAT.md) §7。

---

## 12. 兼容测试基线

本文的契约由 [`../compat/tests/test_xmlrpc.py`](../compat/tests/test_xmlrpc.py)(31 例,直接断言 `supervisor.*` / `system.*` / Fault)与 [`test_cli.py`](../compat/tests/test_cli.py)(27 例,走未改动的 stock `supervisorctl`)作为可执行基准承载,先在 Python **4.2.5** 上全绿。

对编译出的 rsupervisord(默认靶标)先做 `/RPC2` 能力探测:当前**尚未实现**,相关用例统一记为 **`xfail`(59 例)**;`SUPERVISOR_STRICT=1` 时转为硬失败,即 §9 优先级清单的待办全貌。实现 `/RPC2` 后**无需改动测试**,门控会自动放行并开始逐项断言(详见 [`SUPERVISORD_COMPAT.md`](./SUPERVISORD_COMPAT.md) §8)。
