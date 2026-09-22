# rsupervisord: Full Supervisor Compatibility Feasibility Analysis (SUPERVISORD_COMPAT.md)

| Document Version | Status | Target Language | Scope |
| :--- | :--- | :--- | :--- |
| **v1.3.0** | Draft / For Review | Rust (Edition 2024) | Protocol & Feature Coverage Strategy vs. Python Supervisor / `ochinchina/supervisord`;Feature Upgrade Definition (骨头→皮,含三态裁决);关联 [`CLI_COMPAT.md`](./CLI_COMPAT.md) / [`INI_COMPAT.md`](./INI_COMPAT.md) / [`XMLRPC_COMPAT.md`](./XMLRPC_COMPAT.md);可执行契约见 [`../compat/README.md`](../compat/README.md) |

---

## 1. Background & Goal

`rsupervisord` 是一个全新的现代进程编排引擎(YAML 配置 + UDS/TCP JSON REST API + SSE + Web UI)。
它与传统 supervisord 生态(含 Python Supervisor 与 Go `ochinchina/supervisord`)**
不共享任何协议与配置格式**。

本分析回答一个核心问题:

> **如果要完全覆盖 supervisord 的功能,是否只需要实现旧协议(XML-RPC)的转译即可?现有骨架能否完全不动?**

### 1.1 核验事实(基于当前代码基线 `80bfa97`)

| 事实 | 位置 | 影响 |
| :--- | :--- | :--- |
| 子进程 `stdin` 为 `Stdio::null()` | `src/program/process.rs:680` | 无 stdin 注入通道 → `sendProcessStdin` 无法转译 |
| 进程日志为**行级内存 ring buffer**(非落盘) | `src/program/*`、`src/server/api.rs:631` | 无字节偏移语义 → `tailProcessLog`(offset 模式)无法转译 |
| Schema 显式拒绝 `numprocs`(有测试断言报错) | `src/config/schema.rs:403-413` | 无 multi-instance/group 领域模型 |
| `EventHub` 仅对内广播(SSE/REST 消费) | `src/manager/*` | 无 daemon→program 事件监听协议通道 |

---

## 2. Methodology:三层分解

XML-RPC 只是**控制面/查询面**。supervisord 的功能一半长在配置语义与 daemon↔program
通道上,那部分没有领域模型就无法转译。因此将"完全覆盖"拆为三层:

- **Layer A — 纯协议转译层**:XML-RPC 编解码与现有 API/manager 的映射,骨架零改动。
- **Layer B — 配置表层**:INI 解析、numprocs 展开、信号序列等,动 config 层,执行骨架不动。
- **Layer C — 真实模型缺口**:必须新增子系统的部分,与 XML-RPC 转译无关。

**结论预览**:Layer A 直接可做;Layer B 只需 config 层增量;Layer C 的四处缺口**
必须新增模型**,但均可在不侵入现有 actor 热路径的前提下**旁挂增量**实现。

---

## 3. Layer A — 纯协议转译(骨架完全不动,成本最低)

新增一个 `server/xrpc` 适配模块:XML-RPC 编解码、`名称/参数 ↔ 现有 API/manager` 映射、
Basic auth 兼容(`[inet_http_server]` username/password)。可覆盖的方法清单:

| 方法组 | 可覆盖方法 | 现有支撑 |
| :--- | :--- | :--- |
| 状态查询 | `getProcessInfo` / `getAllProcessInfo` | 现有 status 快照 |
| 控制 | `startProcess` / `stopProcess` / `restartProcess` / `signalProcess` | 已有 manager 命令 |
| 日志读取 | `readProcessStdoutLog` / `readProcessStderrLog` / `clearProcessLogs` / `tailProcessLog`(行级) | 现有行级读取与清除 |
| 版本/标识 | `getVersion` / `getPID` / `getIdentification` | 琐碎映射 |
| 组控制 | `startProcessGroup` / `stopProcessGroup` / `signalProcessGroup` | 依赖 Layer B 的 group 概念后可用 |

> 注意:`tailProcessLog` 的字节偏移模式(P→O 指针参数)无支撑,见 Layer C-4。

---

## 4. Layer B — 配置表层适配(动 config 层,执行骨架不动)

1. **INI 解析器**:`[program:x]` 段 → 内部 `ProgramConfig`。需实现
   `[supervisord]`、`[inet_http_server]`、`[supervisorctl]` 等标准段,并支持
   `%(ENV_xxx)s` / `%(program_name)s` 展开。`deny_unknown_fields` 需对 INI 走宽松路径。
2. **numprocs 展开**:`numprocs: N` 在 `resolve_programs` 期展开为
   `name:0 .. name:N-1` 共 N 个独立 actor,并做 `%(process_num)02d` 替换。
   与现有"DAG 注册 N 个 actor"模型天然契合,**Manager 无需结构改动**。
3. **stopsignal 序列 / stopasgroup / killasgroup**:
   进程组回收本就是默认能力,只需给 stop 状态机与信号数组增加两个配置开关
   (go 版支持多 stopsignal 依次发送,supervisord 传统为单信号)。
4. **exitcodes / startretries 语义对齐**:已验证当前已含 `exit_codes` 与 `start_retries`,
   对齐成本低。

> 本层完成后,`supervisorctl` 日常操作(状态/启停/重启/信号/行级日志)基本全通。

---

## 5. Layer C — 真实模型缺口(必须新增子系统)

| # | 缺口 | 现状 | 所需新增 | 建议 |
| :--- | :--- | :--- | :--- | :--- |
| C-1 | 运行时 `addProcessGroup` / `removeProcessGroup` | **已实现(轻量方案)**:group 为"名称前缀 + 元数据",add/remove = 基于 pending 配置批量注册/注销 actor(依赖 Layer B 的展开产物);`ALREADY_ADDED`/`BAD_NAME`/`STILL_RUNNING` 与 Python 一致 | 轻量方案已落地 |
| C-2 | **事件监听协议(Event Listener)** | `EventHub` 仅对内广播 | 全新 daemon→program 通道:`README`/`RESULT` 三步握手、自定义 `STATE_CHANGED` / `PROCESS_LOG` 等事件序列化、`{FD_2}`/`FD_NUM` 展开;与 XML-RPC 无关 | **二期最大工作量**,独立子系统 |
| C-3 | `sendProcessStdin`(含 F_EVENT) | `Stdio::null()`(`process.rs:680`) | 将 stdin 改为 piped + 新增 manager→actor 写通道 + 管道生存期管理 | 独立子系统 |
| C-4 | `tailProcessLog` 字节偏移 / daemon `getLog` | 行级内存 ring buffer,无持久化 | 进程日志落盘或"字节游标"缓存;否则 P→O 偏移语义无法给出 | 二选一,涉及日志管线 |

---

## 6. Conclusion

- **不是"只转译"**:Layer A + B 覆盖 `supervisorctl` 生产日常约 90% 场景,且现有
  actor 热路径可原样保留;
- **必须新增**的是 Layer C 的四处:事件监听协议(C-2)与 stdin 注入(C-3)是唯二需要
  新写用户态子系统的部分;group 模型(C-1)可用 numprocs 展开 + 批量注册获得轻量支撑;
  字节偏移 tail(C-4)依赖日志持久化改造。
- Horizon:**现有骨架不推翻,增量旁挂四条能力通道**即为当前最优演进路径。

---

## 7. 功能升级定义与落地顺序(先骨后皮)

> 本节为**功能升级的完整定义**(编号 #1-#12),含**最终三态**_裁决_:
> **立即 / 已实现 / 搁置 / 条件 / 皮**。对应多次讨论结论:numprocs 与 Group 的**配置层已落地**、
> **运行时 add/remove 已按 Python 语义落地**、Event Listener 已落地、
> 日志"绝对字节游标"因旋转日志矛盾被否决。

### 7.1 三态汇总

| 状态 | 编号 | 裁决依据 |
| :--- | :--- | :--- |
| **立即** | #1 载具、#2 INI+宏、#5 XML-RPC 子集+Basic auth、#7 sendProcessStdin | 支撑"supervisorctl 日常可连",且对现有执行核心零侵入 |
| **已实现(配置层)** | #3 numprocs、#4 Group | 见 §7.2:`numprocs` 展开、`group` 归属与校验、`program_defaults` 均已在 `schema.rs` 落地;运行时 add/remove 按 Python 语义落地(见下行) |
| **已实现(运行时面)** | #4 运行时 `addProcessGroup`/`removeProcessGroup`、`group:*` 批量操作 | pending 配置(pending_configs,镜像 `process_group_configs`)激活/移除 + `ALREADY_ADDED`/`BAD_NAME`/`STILL_RUNNING`;移除仅影响活动集,源配置保留,`reloadConfig` 后可复现 |
| **已实现** | #6 Event Listener | READY/RESULT 广播协议、事件分类映射、pool 背压/UNKNOWN 违约、`sendRemoteCommEvent` 已落地,详见 [`EVENTLISTENER_COMPAT.md`](./EVENTLISTENER_COMPAT.md) |
| **可选/顺带** | #8 日志字节偏移 | 纯协议兼容需求,已否决"绝对游标";若硬做原版 log 功能才顺带,块式字节链垫底 |
| **皮** | #9-#12 | 骨完成后铺 |

### 7.2 功能编号定义

| # | 状态 | 功能 | 范围与关键决策点 | 验收标准 |
| :--- | :--- | :--- | :--- | :--- |
| #1 | 立即 | **Service Install/Uninstall/Start/Stop/Restart**(Windows + Systemd) | Windows:SCM 服务,单二进制自承载、`--install`/`--uninstall`、`--username/password` 登录账户、AutoStart 延迟、`SC_ACTION_RESTART` 崩溃恢复;Systemd:生成 `rsupervisord.service`(Restart=always、LimitNOFILE、User/Group、Environment、KillMode 对齐),支持 `--enable/--disable/--start/--stop/--restart`;安装路径与参数回写进 unit/注册表(ExecStart 带绝对 `-c`);uninstall 必须先 stop 再 remove,失败返回非零;入口建议:`rsupervisorctl service ...` 子命令(本机 UDS 鉴权) | Windows/Linux 各跑通 install→start→restart→stop→uninstall;进程树无残留;uninstall 前未停止时返回非零 + 明确提示 |
| #2 | 立即 | **INI 配置解析 + `%()` 宏展开** | 消化 `[program:x]` / `[supervisord]` / `[inet_http_server]` / `[unix_http_server]` / `[eventlistener]` / `[supervisorctl]` 段;宏展开 `%(ENV_x)s` / `%(program_name)s` / `%(process_num)02d`;**与 YAML 并存**(`-c x.ini` 按后缀自动识别),统一喂同一 resolve 管线;`deny_unknown_fields` 对 INI 走宽松路径。**逐段字段映射/值格式/优先级详见 [`INI_COMPAT.md`](./INI_COMPAT.md)**;宏展开器(`ENV_`/`here`/`program_name`/`process_num`/`numprocs`/`group_name`)已存在 | 标准 supervisord 生产配置字面可加载;宏正确展开;YAML/INI 双通路回归测试通过 |
| #3 | 已实现(配置层) | **numprocs 实例展开** | **已在 `src/config/schema.rs` 落地**:`numprocs` / `numprocs_start` / `process_name` 字段存在,`resolve_programs` 展开为 `name:0..N-1` 共 N 个 actor(经 `%(process_num)02d` 与 `MacroExpander` 联动),共享同一 `ProgramConfig` 模板。**剩余**:XML-RPC 实例方法面(`supervisor.process.*` 逐实例)按 #5 铺 | N 实例全部独立启停;状态/日志按实例隔离 |
| #4 | 已实现(配置层 + 运行时) | **Group 元数据模型(轻量)+ 运行时 add/remove** | **配置层已在 `schema.rs` 落地**:`groups:`(programs + priority)、`ProgramConfig.group`、`resolve_programs` 的归属解析与校验齐全;**不做 GroupActor 重模型**。**运行时已按 Python 语义落地**:`ManagerCommand::AddProcessGroup`/`RemoveProcessGroup` 基于 pending 配置(镜像 `process_group_configs`)激活/移除,`ALREADY_ADDED`/`BAD_NAME`/`STILL_RUNNING` 与 stock 一致;移除仅动活动集,源配置保留 | 运行时注册/注销组原子成功;未运行组可移除、运行中组返回 `STILL_RUNNING`、组不存在返回 `BAD_NAME`;无孤儿进程 |
| #5 | 立即 | **XML-RPC 协议适配层**(`server/xrpc`) | 标准 `supervisord.*` / `supervisor.process.*` 方法面 + Basic auth(`[inet_http_server]` username/password);挂在现有 Axum 独立 router(同进程,复用 UDS/端口);长期目标 stock `supervisorctl` 直连;**group 方法面与运行时 add/remove 已落地**;日志方法走 #8 降级映射 | 标准 `supervisorctl` 的 status/start/stop/restart/signal/tail 全部通过 100%;auth 校验正确;`addProcessGroup`/`removeProcessGroup` 返回 Python 一致的 fault 码 |
| #6 | 条件性 | **Event Listener 协议** | 启用前提:drop-in 兼容监听/告警工具(如 superlance)成为硬需求。优先实现 **EventHub→listener 桥**(用 #2 的 INI 读 `[eventlistener:]` 段,桥一个适配器把 EventHub 事件喂给监听程序),原生 `READY`/`RESULT` 握手仅在桥不足以覆盖时再做 | **(启用时)**superlance `memmon` 等接入并收到 state/log 事件 |
| #7 | 立即 | **sendProcessStdin(数据面)** | stdin 从 `Stdio::null()`(`process.rs:680`)改为 piped + manager→actor **Bounded 写通道**(背压:子进程不读时 Bounded+溢出丢弃+告警)+ 写入时机(不在 select! 内阻塞 write_all;独立 writer 任务或非阻塞 drain)+ **管道生存期管理**(wait_exit 后 drop 写端;对已退出进程写入返回错误;Restart 换新写端);F_EVENT 展开可选 | 注入 stdin 可被子进程读取;停止/退出后写通道安全关闭,无泄漏 |
| #8 | 可选/顺带 | **原版 log 功能(字节偏移 / getLog)** | **否决"绝对字节游标"**:日志文件滚动使单调游标与轮转文件矛盾,引入更多问题。**参数面与 Python 版逐字段一致**(`tailProcessStdoutLog(name, offset, length)` → `(bytes, offset, total)`),语义映射到现有行级 ring buffer:offset=保留窗内行索引、length=行数、返回里 offset 推进为行游标、total=窗内行总数;**极大 offset(`0x7fffffffffffffff`,supervisorctl tail -f 约定)饱和为"从窗末追"**;降级行为明示(深历史不可达、重启归零) | 真 `supervisorctl tail -f` 可用(浅尾追);边界行为符合降级声明 |
| #9 | 皮 | **Cron 计划 + Hook(pre_start / pre_stop)** | `cron:` 表达式调度启停;pre_start / pre_stop 脚本勾子;失败语义与事件挂钩 | cron 程序按时启动;hook 按生命周期点执行并可失败降级 |
| #10 | 皮 | **文件/二进制变更触发重启** | filechangemonitor 语义:监控文件/目录模式,命中变化重启并支持自定义重启命令/信号 | 文件变更→程序按配置重启,N 次内收敛 |
| #11 | 皮 | **Prometheus metrics 端点** | 基于现有 activity-aware 采样器导出 `process_*` 指标;`/metrics` 可开关 | `/metrics` 输出符合命名规范;空闲采样自动暂停仍生效 |
| #12 | 皮 | **daemon 运行面补全** | pidfile、minfds / minprocs rlimit、`reload` 与现有 hot-reload 对齐 | 各选项生效且有参数校验;`reload` 语义与 `#5` 的 XML-RPC `reloadConfig` 一致 |

### 7.3 关键决策记录(本次讨论定论)

- **numprocs(#3)/ Group(#4)**:**配置层已实现**(`schema.rs` 的字段、展开、归属与校验);**运行时 add/remove 已按 Python 语义落地**(pending 配置激活/移除,`ALREADY_ADDED`/`BAD_NAME`/`STILL_RUNNING`),`#5` 的 group 方法面与运行时 add/remove 均已接通。
- **INI(#2)**:逐段字段映射、值格式与优先级详见 [`INI_COMPAT.md`](./INI_COMPAT.md);结论是**前端解析器 + 复用现有 resolve 管线**,执行骨架零改动。
- **Event Listener(#6)**:已实现(READY/RESULT、事件分类、pool 背压、违约 UNKNOWN、`sendRemoteCommEvent`),详见 [`EVENTLISTENER_COMPAT.md`](./EVENTLISTENER_COMPAT.md);原生握手即协议本体,已直接落地。
- **日志字节偏移(#8)**:否决绝对游标(旋转日志与之矛盾);改为**参数面一致 + 行级语义降级**,
  极大 tail offset 做饱和映射——保证真 `supervisorctl tail -f` 可用。
- **日志"块式 Bytes 链 + 绝对游标"优化**:随口一提,非必要不实现;仅硬做原版 log 功能时顺带。
- **sendProcessStdin(#7)**:单向 channel 只是传输段,难点在背压、写入时机、管道生存期。

### 7.4 里程碑

| 里程碑 | 内容 | 验收一句话 |
| :--- | :--- | :--- |
| Phase A(载具) | #1 | 系统服务装得上、起得来、卸得净 |
| Phase B(配置/协议入口) | #2 #5(子集+auth,含 tail 降级) | `-c x.ini` 可加载;`supervisorctl` 状态/启停/重启/signal/tail 直连全通 |
| Phase C(数据面) | #7 | stdin 注入可用且无泄漏 |
| Phase D(表面/条件) | #9 #10 #11 #12;#6 按需 | 运维舒适度齐活;监听工具接入按需 |

---

## 8. 兼容测试基线(可执行契约)

兼容度不再只靠文字判断,而由 [`../compat/`](../compat) 下的 pytest 套件作为**可执行契约**驱动。运行方式、前置与明细见 [`../compat/README.md`](../compat/README.md)。

**两个靶标**(`SUPERVISOR_TARGET`):

| 靶标 | 服务端 | 客户端 | 用途 |
| :--- | :--- | :--- | :--- |
| `python`(黄金基准) | stock Python supervisord 4.2.5(INI 夹具) | stock `supervisorctl` + XML-RPC | 定义期望行为,必须全绿 |
| `rsupervisord`(**默认**) | **编译出的 `target/<profile>/rsupervisord`**(YAML 夹具) | `rsupervisorctl`(native)+ stock client(能力门控) | 量出当前差距 |

**运行**(在 WSL 内):

```bash
bash compat/run.sh                              # 默认靶标:编译 bin
SUPERVISOR_TARGET=python bash compat/run.sh     # 黄金基准(4.2.5)
SUPERVISOR_STRICT=1 bash compat/run.sh          # 未实现项 => 硬失败
```

**门控语义**:oracle 用例([`test_cli.py`](../compat/tests/test_cli.py)、[`test_xmlrpc.py`](../compat/tests/test_xmlrpc.py)、`test_zz_daemon.py`)对编译 bin 先做 `/RPC2` 能力探测;缺失时按 `pytest.xfail` 记为 `xfail`(理由指向 §7 #5),`SUPERVISOR_STRICT=1` 时转硬失败。native 用例([`test_native_cli.py`](../compat/tests/test_native_cli.py))仅在 `rsupervisord` 靶标运行。

**当前基线(`target/debug` 本地构建)**:

| 运行 | 结果 |
| :--- | :--- |
| `SUPERVISOR_TARGET=python` | **59 passed, 5 skipped**(定义黄金行为) |
| 默认(编译 bin) | **5 passed(native), 59 xfailed**(XML-RPC 未实现,即 #5) |
| `SUPERVISOR_STRICT=1`(编译 bin) | **5 passed, 59 errors**(= #5 的待办面) |

> **构建修复**:为使 Linux 版可编译,`Cargo.toml` 的 `nix` 依赖补齐了 `hostname` feature(`src/platform/unix/mod.rs::hostname` 依赖 `nix::unistd::gethostname`)。
