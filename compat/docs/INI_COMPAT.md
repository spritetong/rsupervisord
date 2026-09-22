# rsupervisord: INI 配置适配分析 (INI_COMPAT.md)

| Document Version | Status | Target Language | Scope |
| :--- | :--- | :--- | :--- |
| **v1.0.0** | Draft / For Review | Rust (Edition 2024) | Python `supervisord.conf`(INI)到 rsupervisord 配置模型的 section/字段映射、差异、示例与优先级 |

---

## 1. 目的与范围

回答:**要让标准 Python `supervisord.conf`(INI)被 rsupervisord 直接加载,需要实现什么。**

- **权威基准**:Python Supervisor **4.2.5**(`supervisor/skel/sample.conf` + `options.py`)。
- **参考实现**:Go `ochinchina/supervisord` 的 `config/config.go`(commit `7a73369`)。
- **现状基线**:`src/config/schema.rs`(YAML schema)、`src/config/expand.rs`(宏展开)、`src/program/config.rs`(运行期配置)。
- **关联**:section `[supervisorctl]` 的客户端语义见 `CLI_COMPAT.md`;`[eventlistener:x]` 见 `SUPERVISORD_COMPAT.md` #6;`[rpcinterface]` 见 #5。

**核心结论(预览)**:执行骨架无需改动。INI 只是一个**前端解析器**,把 section/字段翻译成现有的 `SupervisorConfig`(raw)→ 复用现有 `validate()` 与 `resolve_programs()`。**宏展开器、numprocs 展开、group 解析、program_defaults 均已存在**。

---

## 2. 优先级定义

| 级别 | 含义 |
| :--- | :--- |
| **P0** | INI 可用性的基础设施(解析器 + 后缀识别 + 值规范化 + 核心 section 映射)。必做。 |
| **P1** | 常用且低成本(`[include]`、`[rpcinterface]` 容忍、`[supervisorctl]`、priority 范围裁决)。 |
| **P2** | 依赖搁置项或成本偏高(daemon 运行面字段、socket 权限、stdout/stderr 独立轮转)。 |
| **不支持** | 成本过高且 Go 版也未实现,或语义已由其它机制覆盖。 |

---

## 3. Python INI 的 section 分类(共 10 类)

| # | Section | 作用域 | rsupervisord 目标 | 优先级 |
| :--- | :--- | :--- | :--- | :--- |
| 1 | `[unix_http_server]` | daemon UDS 监听 | `server` | **P0** |
| 2 | `[inet_http_server]` | daemon TCP 监听 | `server` | **P0** |
| 3 | `[supervisord]` | daemon 全局 | `logging` + (运行面, #12) | **P0**(日志部分) |
| 4 | `[program:x]` | 被监管程序 | `programs.x` | **P0** |
| 5 | `[group:x]` | 异构进程组 | `groups.x` | **P0** |
| 6 | `[supervisorctl]` | **客户端**连接配置 | CLI 缺省(非 daemon) | **P1** |
| 7 | `[include]` | 配置包含 | 文件合并 | **P1** |
| 8 | `[rpcinterface:supervisor]` | RPC 接口注册 | 容忍/忽略(#5) | **P1** |
| 9 | `[eventlistener:x]` | 事件监听程序 | 条件性(#6) | **不支持**(暂) |
| 10 | `[fcgi-program:x]` | FastCGI 程序 | — | **不支持** |

> 另:Go 版扩展了 `[program-default]`(Python 无此段),rsupervisord 对应 `program_defaults`。可作为扩展一并支持。

---

## 4. 逐 section 字段映射与差异

### 4.1 `[unix_http_server]` → `server`

| INI 字段 | 当前 YAML | 差异 / 动作 |
| :--- | :--- | :--- |
| `file` | `server.uds_path` | 直接映射 |
| `username` | `server.username` | 直接;**注意**:与 `[inet_http_server]` 共享同一字段,Python 允许两段独立凭据 → 差异 |
| `password` | `server.password` | 同上 |
| `chmod` | — | **P2**(Unix socket 权限,Windows 不适用) |
| `chown` | — | **P2** |

**示例**

```ini
[unix_http_server]
file=/tmp/supervisor.sock
username=admin
password=secret
```

```yaml
server:
  uds_path: /tmp/supervisor.sock
  username: admin
  password: secret
```

### 4.2 `[inet_http_server]` → `server`

| INI 字段 | 当前 YAML | 差异 / 动作 |
| :--- | :--- | :--- |
| `port` | `server.http_bind` | 直接;`normalize_http_bind` 已支持 `:9001` / `*:9001` / `9001` |
| `username` | `server.username` | 与 UDS 共享 → 差异 |
| `password` | `server.password` | 同上 |

**示例**

```ini
[inet_http_server]
port=127.0.0.1:9001
username=admin
password=secret
```

```yaml
server:
  http_bind: 127.0.0.1:9001
  username: admin
  password: secret
```

### 4.3 `[supervisord]` → `logging` + 运行面

| INI 字段 | 当前 YAML | 差异 / 动作 |
| :--- | :--- | :--- |
| `logfile` | `logging.file` | 直接 |
| `logfile_maxbytes` | `logging.max_bytes` | 直接;默认 50MB(Python) vs 20MB(ours) |
| `logfile_backups` | `logging.backups` | 直接;默认 10 vs 3 |
| `loglevel` | `logging.level` | 直接 |
| `pidfile` | — | **P2**(#12) |
| `nodaemon` | — | **P2**(#12) |
| `silent` | — | **P2** |
| `minfds` | — | **P2**(#12 rlimit) |
| `minprocs` | — | **P2**(#12 rlimit) |
| `umask` | — | **P2** |
| `user` | — | 不支持/部分(setuid) |
| `identifier` | — | **P2** |
| `directory` | — | **P2** |
| `nocleanup` | — | **不支持** |
| `childlogdir` | — | **P2**(AUTO 子日志目录) |
| `environment` | — | **P2**(daemon 级环境) |
| `strip_ansi` | — | **不支持** |

**示例**

```ini
[supervisord]
logfile=/var/log/supervisord.log
logfile_maxbytes=50MB
logfile_backups=10
loglevel=info
pidfile=/run/supervisord.pid
```

```yaml
logging:
  enabled: true
  file: /var/log/supervisord.log
  max_bytes: 50MB
  backups: 10
  level: info
```

### 4.4 `[program:x]` → `programs.x`(核心)

| INI 字段 | 当前 YAML | 差异 / 动作 |
| :--- | :--- | :--- |
| `command` | `command`(+`args`) | 直接;ours 在 `args` 空时自动 shell-split |
| `process_name` | `process_name` | 直接 |
| `numprocs` | `numprocs` | 直接(**展开已实现**) |
| `numprocs_start` | `numprocs_start` | 直接 |
| `directory` | `directory` | 直接 |
| `umask` | `umask` | 直接 |
| `priority` | `priority` | **范围冲突**:Python 允许 `999`,ours 校验 `[0,99]` → 见 §6 |
| `autostart` | `autostart` | 布尔解析差异(见 §5) |
| `startsecs` | `start_secs` | 命名;默认 1 (与 Python 一致) |
| `startretries` | `start_retries` | 命名 |
| `autorestart` | `autorestart` | **值映射**:`false→never`,`true→always`,`unexpected→unexpected` |
| `exitcodes` | `exit_codes` | 列表解析(`0,2` → `[0,2]`) |
| `stopsignal` | `stop_signal` | 别名 `SIGTERM` 已支持 |
| `stopwaitsecs` | `stop_wait_secs` | 命名 |
| `stopasgroup` | — | **不支持/部分**(Unix 进程组语义) |
| `killasgroup` | — | **不支持/部分** |
| `user` | `user` | 直接 |
| `redirect_stderr` | `logs.redirect_stderr` | 直接 |
| `stdout_logfile` | `logs.stdout` | **`AUTO`/`NONE` 语义**(见 §5) |
| `stdout_logfile_maxbytes` | `logs.max_bytes` | 命名;**ours 单值共享 stdout/stderr → 差异** |
| `stdout_logfile_backups` | `logs.backups` | 同上 |
| `stderr_logfile` | `logs.stderr` | `AUTO`/`NONE` |
| `stderr_logfile_maxbytes` | `logs.max_bytes` | 共享差异 |
| `stderr_logfile_backups` | `logs.backups` | 共享差异 |
| `environment` | `environment` | **格式**:`A="1",B="2"` → map(见 §5) |
| `stdout_capture_maxbytes` | — | **不支持**(capturemode/event) |
| `stdout_events_enabled` | — | **不支持**(依赖 #6) |
| `stdout_syslog` | — | **不支持** |
| `stderr_capture_maxbytes` | — | **不支持** |
| `stderr_events_enabled` | — | **不支持** |
| `stderr_syslog` | — | **不支持** |
| `serverurl` | — | **不支持**(childutils) |

> ours/Go 扩展字段(Python 无):`depends_on`、`cron`、`pre_start`/`pre_stop`、`health_check`、`restart_*`。INI 中若出现,按扩展接受。

**示例**

```ini
[program:web]
command=/usr/bin/gunicorn app:app
process_name=%(program_name)s_%(process_num)02d
numprocs=2
directory=/srv/app
priority=10
autostart=true
startsecs=5
startretries=3
autorestart=unexpected
exitcodes=0,2
stopsignal=QUIT
stopwaitsecs=15
redirect_stderr=true
stdout_logfile=/var/log/web.log
stdout_logfile_maxbytes=10MB
stdout_logfile_backups=5
environment=PORT="8080",DEBUG="false"
```

```yaml
programs:
  web:
    command: /usr/bin/gunicorn app:app
    process_name: "%(program_name)s_%(process_num)02d"
    numprocs: 2
    directory: /srv/app
    priority: 10
    autostart: true
    start_secs: 5
    start_retries: 3
    autorestart: unexpected
    exit_codes: [0, 2]
    stop_signal: QUIT
    stop_wait_secs: 15
    logs:
      redirect_stderr: true
      stdout: /var/log/web.log
      max_bytes: 10MB
      backups: 5
    environment:
      PORT: "8080"
      DEBUG: "false"
```

### 4.5 `[group:x]` → `groups.x`

| INI 字段 | 当前 YAML | 差异 / 动作 |
| :--- | :--- | :--- |
| `programs` | `groups.x.programs` | 直接(逗号/空白分隔列表) |
| `priority` | `groups.x.priority` | 直接 |

**示例**

```ini
[group:web]
programs=frontend,backend
priority=80
```

```yaml
groups:
  web:
    programs: [frontend, backend]
    priority: 80
```

### 4.6 `[supervisorctl]` → CLI 缺省(P1)

| INI 字段 | 目标 | 动作 |
| :--- | :--- | :--- |
| `serverurl` | `rsupervisorctl -s` 缺省 | 由客户端读取 `-c` 文件时解析 |
| `username` | `-u` 缺省 | 同上 |
| `password` | `-p` 缺省 | 同上 |
| `prompt` | — | **不支持**(交互 shell) |
| `history_file` | — | **不支持** |

> 注意:daemon 本身**不消费** `[supervisorctl]`;它只影响客户端。见 `CLI_COMPAT.md` §5.1.4。

### 4.7 `[include]` → 文件合并(P1)

| INI 字段 | 动作 |
| :--- | :--- |
| `files` | 相对本文件解析;空白/换行分隔;支持通配符;被包含文件不可再 include |

**示例**

```ini
[include]
files = conf.d/*.ini
```

### 4.8 `[rpcinterface:supervisor]` → 容忍(P1)

| INI 字段 | 动作 |
| :--- | :--- |
| `supervisor.rpcinterface_factory` | **忽略**(可解析但丢弃);#5 落地后可据此启用 XML-RPC |

> 生产配置**必含**此段,解析器必须接受而非报错。

### 4.9 `[eventlistener:x]` → 不支持(条件性,#6)

字段与 `[program:x]` 基本一致,另加 `events`(必填)、`buffer_size`(默认 10)、`priority` 默认 `-1`、`redirect_stderr` 必须为 false。

**决策**:**暂不支持**;若 #6 启用,再映射为独立模型。INI 解析器遇到该段应给出明确错误或忽略并警告。

### 4.10 `[fcgi-program:x]` → 不支持

FastCGI 程序:额外 `socket` / `socket_owner` / `socket_mode`,并复用 program 字段。

**决策**:**不支持**(需 FastCGI socket 转发子系统,复杂且 Go 版亦未实现)。

---

## 5. 语法与值格式差异(解析器必须处理)

| 项 | Python INI | 当前 YAML | 需求 |
| :--- | :--- | :--- | :--- |
| 注释 | 前导空格 + `;` 或 `#`;`a=b ;comment` 有效,`a=b;comment` 无效 | YAML `#` | INI 注释规则需精确实现 |
| 引号 | 除 `environment=` 外**不支持引号** | YAML 引号 | INI 值按字面取 |
| 布尔 | `true/false/yes/no/1/0/on/off`(大小写不敏感) | `true/false` | 新增宽松布尔解析 |
| `autorestart` | `false` / `unexpected` / `true` | `never/unexpected/always` | 值映射 |
| `exitcodes` | `0,2`(逗号分隔) | `[0,2]` | 列表解析 |
| `environment` | `A="1",B="2"`(逗号分隔、值带引号) | map | 专用解析器 |
| `stdout_logfile` | `AUTO` / `NONE` / 路径 | `None`/路径 | `NONE→禁用`,`AUTO→默认路径` |
| 宏 | `%(ENV_X)s`/`%(here)s`/`%(program_name)s`/`%(process_num)02d`/`%(numprocs)d`/`%(group_name)s`/`%(host_node_name)s` | 同(expand.rs 已实现) | 复用现有 `MacroExpander` |
| 键名 | 大小写不敏感、无下划线风格(`startsecs`) | snake_case(`start_secs`) | 别名表 |
| 多值列表 | `programs`/`files`/`exitcodes` 逗号或空白 | YAML 序列 | 按字段选择分隔规则 |

---

## 6. 默认值差异(Python 4.2.5 vs 当前)

| 字段 | Python 默认 | 当前默认 | 裁决 |
| :--- | :--- | :--- | :--- |
| `priority` | 999(无上限) | 50,校验 `≤99` | **P1**:放宽到 `0..=999` 或明确拒绝超范围并报错 |
| `startsecs` | 1 | 3 | 保持 ours(文档化差异) |
| `startretries` | 3 | 3 | 一致 |
| `stopwaitsecs` | 10 | 10 | 一致 |
| `exitcodes` | `[0]` | `[0]` | 一致 |
| `autorestart` | unexpected | unexpected | 一致 |
| `logfile_maxbytes` | 50MB | 20MB | 文档化差异 |
| `logfile_backups` | 10 | 3 | 文档化差异 |
| `umask` | 022 | None | **P2** |
| `redirect_stderr` | false | false | 一致 |

> 默认值差异不阻塞加载;建议"配置里显式写了就用显式值",未写时用 ours 默认并在文档标注。

---

## 7. 需要实现的功能清单(按优先级)

### P0 — INI 前端基础设施

1. **INI 解析器**(新模块 `src/config/ini.rs`):section、`key=value`、注释规则、行续、大小写不敏感键、逐值宏展开。
2. **后缀识别**:`SupervisorConfig::from_file`(schema.rs:332)按扩展名分派——`.ini`→INI 管线,`.yaml/.yml`→YAML 管线;**两者产出同一 `SupervisorConfig`**。
3. **值规范化层**:宽松布尔、`autorestart` 三态映射、列表解析(`exitcodes`/`programs`/`files`)、`environment` 专用解析、`AUTO`/`NONE` 日志路径。
4. **字段别名表**:`startsecs→start_secs`、`startretries→start_retries`、`stopwaitsecs→stop_wait_secs`、`stopsignal→stop_signal`、`exitcodes→exit_codes`、`stdout_logfile→logs.stdout` 等全量映射。
5. **核心 5 段映射**:`[unix_http_server]`、`[inet_http_server]`、`[supervisord]`(日志子集)、`[program:x]`、`[group:x]`。

### P1

6. `[include]` `files`(glob + 合并)。
7. `[rpcinterface:*]` 容忍(解析并忽略)。
8. `[supervisorctl]` → CLI 缺省(配合 `CLI_COMPAT.md` `-c`)。
9. `priority` 范围裁决(`0..=999`)。

### P2

10. `[supervisord]` 运行面字段(`pidfile`/`nodaemon`/`minfds`/`minprocs`/`umask`/`directory`/`childlogdir`/`identifier`/`environment`/`silent`)→ 依赖 #12。
11. `[unix_http_server]` `chmod`/`chown`。
12. stdout/stderr 独立 `maxbytes`/`backups`(当前 `logs` 单值共享)。

### 不支持

13. `[eventlistener:x]`(条件性,#6)。
14. `[fcgi-program:x]`(复杂,Go 版亦无)。
15. `[program:x]` 的 `stdout_capture_maxbytes`/`stderr_capture_maxbytes`/`*_events_enabled`/`*_syslog`/`serverurl`。
16. `[supervisord]` `nocleanup`/`strip_ansi`。
17. `[supervisorctl]` `prompt`/`history_file`。

---

## 8. 验收

**P0 验收**:标准生产 `supervisord.conf`(含 `[unix_http_server]`/`[supervisord]`/`[rpcinterface:supervisor]`/`[program:x]`/`[group:x]`)可**字面加载**,宏正确展开,`resolve_programs` 输出与等价 YAML 一致。

```bash
rsupervisord -c /etc/supervisord.conf          # INI 加载
rsupervisord -c /etc/rsupervisord.yaml         # YAML 加载(回归)
```

**等价性验收**:同一配置分别以 INI / YAML 表达,`resolve_programs()` 结果逐字段相等。

---

## 9. 与其它文档的关系

- 客户端参数与 `[supervisorctl]` 消费:见 `CLI_COMPAT.md`。
- `[rpcinterface]` 与 XML-RPC 方法面:见 `SUPERVISORD_COMPAT.md` §7 #5。
- `[eventlistener:x]`(条件性):见 `SUPERVISORD_COMPAT.md` §7 #6。
- daemon 运行面(`pidfile`/rlimit 等):见 `SUPERVISORD_COMPAT.md` §7 #12。
