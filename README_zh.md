# rsupervisord

[![License: MPL 2.0](https://img.shields.io/badge/License-MPL_2.0-brightgreen.svg)](LICENSE)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](https://www.rust-lang.org)
[![Platform: Linux | Windows | macOS](https://img.shields.io/badge/Platform-Linux%20%7C%20Windows%20%7C%20macOS-lightgrey.svg)]

**rsupervisord** 是一个跨平台的进程编排与监控守护进程，以单个二进制文件运行在 Linux、Windows 与 macOS 上。

[English](README.md) | [简体中文](README_zh.md)

---

## 目录

- [rsupervisord](#rsupervisord)
  - [目录](#目录)
  - [开发目的与背景](#开发目的与背景)
  - [核心特性](#核心特性)
    - [1. Windows 服务支持](#1-windows-服务支持)
    - [2. 空闲时无轮询](#2-空闲时无轮询)
    - [3. Linux / Unix 进程控制](#3-linux--unix-进程控制)
    - [4. 配置热重载](#4-配置热重载)
    - [5. 内置 Web 控制台](#5-内置-web-控制台)
    - [6. 路径与命令处理](#6-路径与命令处理)
    - [7. 本地通信与运行模式](#7-本地通信与运行模式)
  - [快速开始](#快速开始)
    - [1. 源码编译](#1-源码编译)
    - [2. 配置文件探测规则与默认路径](#2-配置文件探测规则与默认路径)
    - [3. 守护进程运行](#3-守护进程运行)
  - [系统服务管理 (Windows Service 与 Linux Systemd)](#系统服务管理-windows-service-与-linux-systemd)
    - [Windows Service 管理 (SCM 原生支持)](#windows-service-管理-scm-原生支持)
    - [Linux Systemd 服务管理](#linux-systemd-服务管理)
  - [命令行参数与操作示例](#命令行参数与操作示例)
    - [守护进程 supervisord](#守护进程-supervisord)
      - [参数说明](#参数说明)
      - [运行示例](#运行示例)
    - [控制端客户端 supervisorctl](#控制端客户端-supervisorctl)
      - [全局通用选项](#全局通用选项)
      - [常用子命令与操作示例](#常用子命令与操作示例)
  - [YAML 配置项全量参考规范](#yaml-配置项全量参考规范)
    - [全量配置示例 (supervisord.yaml)](#全量配置示例-supervisordyaml)
    - [各配置节详细参数说明](#各配置节详细参数说明)
      - [1. 顶层与 `server` 节 (通信与基础设置)](#1-顶层与-server-节-通信与基础设置)
      - [2. `ctl` 节 (supervisorctl 客户端连接默认值)](#2-ctl-节-supervisorctl-客户端连接默认值)
      - [3. `logging` 节 (守护进程自身日志)](#3-logging-节-守护进程自身日志)
      - [4. `metrics` 节 (资源指标监控)](#4-metrics-节-资源指标监控)
      - [5. `program_defaults` 节 (全局程序默认值)](#5-program_defaults-节-全局程序默认值)
      - [6. `groups` 节 (进程分组)](#6-groups-节-进程分组)
      - [7. `programs.<name>` 节 (托管程序详细属性)](#7-programsname-节-托管程序详细属性)
      - [8. `event_listeners.<name>` 节 (事件监听器)](#8-event_listenersname-节-事件监听器)
  - [原版 INI 配置基本示例](#原版-ini-配置基本示例)
    - [基本配置示例 (`supervisord.conf`)](#基本配置示例-supervisordconf)
  - [Web 控制台](#web-控制台)
  - [开源许可证](#开源许可证)

---

## 开发目的与背景

在容器化、微服务部署、边缘计算以及 Windows 主机环境中，都需要进程管理工具。现有方案存在以下问题：

1. **Python 原版 Supervisor**：
   - 原版 [Supervisor (Python)](http://supervisord.org/) 依赖完整的 Python 运行环境、`setuptools` 及一系列第三方依赖库，制作自包含镜像或独立安装包需要额外处理。
   - 不支持 Windows。
2. **Go 语言社区版 `supervisord`**：
   - 功能不完整。
   - Windows 上通过调用 `taskkill.exe` 结束进程，服务停止或重启后，子进程与孙进程经常残留为孤儿进程并占用端口。

`rsupervisord` 使用 Rust 开发，以单个二进制文件分发，无外部运行时依赖，可在 Linux、Windows 与 macOS 上完成进程启动顺序、健康检查、定时任务、日志收集与配置重载。

---

## 核心特性

### 1. Windows 服务支持

- `supervisord service install` 将 `rsupervisord` 注册为自启动的 Windows 服务，`start`、`stop`、`restart`、`uninstall` 从命令行管理服务。
- 停止或重启服务时，会结束被托管程序的整个进程树，包括 `.bat` / `.cmd` 脚本派生的子孙进程，不调用 `taskkill.exe`。
- 与 `winsw`、`NSSM` 这类单服务包装工具不同，一个守护进程可以托管多个程序，并提供依赖顺序启动、健康检查、定时任务、文件监控与控制台。

### 2. 空闲时无轮询

- 进程退出由操作系统上报，不对运行中的进程做定时轮询。
- 未配置主动健康探针的程序在空闲时不会被周期性唤醒。

### 3. Linux / Unix 进程控制

- 每个程序运行在独立的进程组中，停止信号发送给整个进程组。
- `supervisord service install` 生成并管理守护进程的 systemd 单元。

### 4. 配置热重载

- 重载时配置未变化的程序保持原有 PID 与已有连接继续运行，只对新增、修改或删除的程序执行启动、重启或停止。

### 5. 内置 Web 控制台

- 控制台内置于二进制文件中，无需 Node.js、NPM，也不访问外部 CDN。
- 支持实时 SSE 日志流（自动滚动锁定）、复选框批量启停重启、健康状态指示、CPU/RSS 资源曲线与配置差异对比。

### 6. 路径与命令处理

- 配置中的相对路径以配置文件所在目录为基准解析，不受守护进程工作目录影响。
- Windows 下 `.bat`、`.cmd` 脚本通过 `cmd.exe` 执行，路径中的长路径前缀（`\\?\`）会被去除。

### 7. 本地通信与运行模式

- 本地 IPC 走 Unix Domain Socket（Unix 使用套接字文件，Windows 支持 `AF_UNIX` 文件路径与命名管道 `\\.\pipe\`），或走带认证的 HTTP REST API。
- 单线程模式（`worker_threads: 1` 或 `--worker-threads 1`），适用于边缘设备等资源受限环境。

---

## 快速开始

### 1. 源码编译

编译环境要求：**Rust 1.85+ (Edition 2024)**。

```bash
# 克隆仓库
git clone https://github.com/spritetong/rsupervisord.git
cd rsupervisord

# 编译发布版
cargo build --release

# 编译输出可执行程序位于：
#   target/release/supervisord      (守护进程引擎)
#   target/release/supervisorctl    (命令行控制端)
```

> [!TIP]
> `supervisord` 与 `supervisorctl` 支持单二进制共用：当可执行文件名以 `ctl` 结尾（如通过硬链接或符号链接命名为 `supervisorctl`），或者通过 `supervisord ctl <命令>` 调用时，均会自动无缝进入控制端客户端模式。

---

### 2. 配置文件探测规则与默认路径

当未通过 `-c / --config` 显式指定路径时，程序按以下优先级顺序探测配置文件：

1. 环境变量：`<大写程序名>_CONFIG`（例如 `SUPERVISORD_CONFIG`、`MYD_CONFIG`）。
2. 可执行文件同级目录：`<可执行文件所在目录>/<cmd_name>.yaml`。
3. 可执行文件专用子目录：`<可执行文件所在目录>/<cmd_name>/<cmd_name>.yaml`。
4. 可执行文件同名目录通用配置：`<可执行文件所在目录>/<cmd_name>/config.yaml`。
5. 当前工作目录（CWD）：`<CWD>/<cmd_name>.yaml` 或 `<CWD>/config.yaml`。
6. 用户级配置目录（XDG / Windows AppData）：
   - **Linux / macOS**：`~/.config/<cmd_name>/config.yaml`。
   - **Windows**：`%APPDATA%/<cmd_name>/config.yaml`。
7. 系统全局目录：
   - **Linux**：`/etc/<cmd_name>/config.yaml`。

**缺省运行时路径约定**：

- **IPC 端点**（未配置 `server.uds_path` 时）：
  - **Linux / Unix**：`/var/run/<cmd_name>.sock`。
  - **Windows**：命名管道 `\\.\pipe\<cmd_name>`。
- **守护进程自身日志**（未配置 `logging.file` 且日志开启时）：
  - **Linux / Unix**：`/var/log/<cmd_name>/<cmd_name>.log`。
  - **Windows**：`<配置文件目录>/logs/<cmd_name>.log`。
- **程序日志**（未配置 `logs.stdout` 且日志开启时）：
  - **Linux / Unix**：`/var/log/<cmd_name>/<program_name>.log`。
  - **Windows**：`<配置文件目录>/logs/<program_name>.log`。

---

### 3. 守护进程运行

```bash
# 1. 自动探测配置文件启动守护进程
./target/release/supervisord

# 2. 显式指定配置文件启动
./target/release/supervisord -c /etc/supervisord/supervisord.yaml

# 3. 以单线程模式启动（内存占用更低）
./target/release/supervisord -c config.yaml --worker-threads 1
```

---

## 系统服务管理 (Windows Service 与 Linux Systemd)

`rsupervisord` 提供了与操作系统服务体系原生集成的 `service` 统一子命令。`supervisord` 与 `supervisorctl` 均可直接执行此命令。

### Windows Service 管理 (SCM 原生支持)

请以**管理员身份（Administrator）**运行 PowerShell 或终端：

```powershell
# 安装为 Windows 自动启动服务（省略 -c 时将使用默认探测路径）
supervisord.exe service install

# 安装并固化指定配置文件路径
supervisord.exe service install -c C:\rsupervisord\supervisord.yaml

# 启动 Windows 服务
supervisord.exe service start

# 停止 Windows 服务（结束全部托管程序的进程树）
supervisord.exe service stop

# 重启服务
supervisord.exe service restart

# 卸载服务
supervisord.exe service uninstall
```

> [!NOTE]
> 当注册为 Windows 服务时，操作系统 Service Control Manager (SCM) 启动进程并自动附加 `--service` 标志。`rsupervisord` 在收到 SCM 停机请求时，按顺序收尾各项任务后再报告停止状态。

### Linux Systemd 服务管理

在 Linux 系统中，使用 `sudo` 执行：

```bash
# 安装并启用 systemd 服务（自动生成并注册 /etc/systemd/system/supervisord.service）
sudo supervisord service install -c /etc/supervisord/supervisord.yaml

# 通过内置命令管理服务生命周期
sudo supervisord service start
sudo supervisord service stop
sudo supervisord service restart
sudo supervisord service uninstall

# 也可以直接使用 Linux 原生 systemctl 管理
sudo systemctl status supervisord
sudo systemctl restart supervisord
```

---

## 命令行参数与操作示例

### 守护进程 supervisord

#### 参数说明

| 选项 / 子命令 | 缩写 | 默认值 | 详细说明 |
| :--- | :--- | :--- | :--- |
| `--config <PATH>` | `-c` | 自动探测 | 指定配置文件路径（支持 `.yaml`、`.yml`、`.conf`、`.ini`） |
| `--loglevel <LEVEL>` | `-l` | `info` | 指定日志输出级别：`trace`, `debug`, `info`, `warn`, `error`, `off` |
| `--nodaemon` | `-n` | `false` | 前台运行标志，为兼容原版保留。当前版本始终以前台方式运行。 |
| `--worker-threads <N>` | - | CPU 核心数 | 运行时工作线程数。设为 `1` 时启用单线程模式 |
| `--service` | - | `false` | 系统服务分发模式（由 Windows SCM 或服务调度器自动隐式调用） |
| `--allow-unelevated` | - | `false` | 守护进程以特权身份运行时，允许非特权客户端连接本地 IPC |
| `service <OP>` | - | - | 系统服务生命周期管理：`install`, `uninstall`, `start`, `stop`, `restart` |
| `ctl <CMD...>` | - | - | 直接调用控制端功能，等同于调用 `supervisorctl <CMD...>` |

#### 运行示例

```bash
# 以前台调试模式启动，输出 debug 级日志
supervisord -c config.yaml -l debug

# 以单线程模式运行在资源受限设备上
supervisord -c config.yaml --worker-threads 1

# 允许普通用户连接特权守护进程
supervisord -c config.yaml --allow-unelevated
```

---

### 控制端客户端 supervisorctl

#### 全局通用选项

| 选项 | 缩写 | 默认值 | 详细说明 |
| :--- | :--- | :--- | :--- |
| `--server <URL>` | `-s` | 自动推导 | 连接目标端点（支持 `unix:///path/to.sock`、命名管道或 `http://127.0.0.1:9001`） |
| `--config <PATH>` | `-c` | 自动探测 | 配置文件路径（从配置中读取套接字或 HTTP 绑定与凭证） |
| `--key <TOKEN>` | `-k` | 空 | HTTP REST API 的 Bearer Token 认证密钥 |
| `--user <USER>` | `-u` | 空 | HTTP Basic Auth 认证用户名 |
| `--password <PASS>` | `-p` | 空 | HTTP Basic Auth 认证密码 |

#### 常用子命令与操作示例

```bash
# 1. 状态查看 (展示进程运行状态、PID、运行时长、CPU 与内存利用率)
supervisorctl status
supervisorctl status api-server redis

# 2. 进程启动 / 停止 / 重启 (支持同步阻塞等待与异步触发)
supervisorctl start api-server                 # 同步等待就绪确认（默认超时 30 秒）
supervisorctl stop api-server                  # 同步等待优雅停止
supervisorctl restart api-server               # 同步重启
supervisorctl start all                        # 启动全部程序
supervisorctl restart web:*                    # 按进程组批量重启
supervisorctl start api-server --async         # 异步触发立即返回

# 3. 配置热重载与维护
supervisorctl config reload                    # 【推荐】重载配置，未修改进程不重启
supervisorctl reread                           # 重新读取配置文件并展示变更差异（不操作进程）
supervisorctl update                           # 应用配置差异（按需增删改）
supervisorctl reload                           # 兼容原版模式：平滑停止全部服务后重载守护进程

# 4. 日志实时跟踪与清理
supervisorctl tail -f api-server               # 跟踪程序的实时 stdout 流输出
supervisorctl tail -f -n 100 api-server stderr # 查看 stderr 历史最后 100 行并持续跟踪
supervisorctl maintail -f                      # 实时跟踪守护进程自身日志
supervisorctl clear api-server                 # 清理程序日志文件与内存环形缓冲

# 5. 进程信号与交互控制
supervisorctl pid api-server                   # 查询指定程序的进程 PID
supervisorctl signal HUP api-server            # 发送指定信号（如 HUP, TERM, KILL, INT）
supervisorctl stdin api-server "reload\n"      # 向指定程序的标准输入写入字符
supervisorctl events                           # 实时订阅并输出系统事件流

# 6. 分组与会话管理
supervisorctl avail                            # 列出已配置的程序与分组
supervisorctl add web                          # 激活待生效的进程组
supervisorctl remove web                       # 停用进程组
supervisorctl open http://127.0.0.1:9001       # 切换本会话的连接端点
supervisorctl fg api-server                    # 前台模式：实时输出日志并转发标准输入

# 7. 关闭守护进程
supervisorctl shutdown                         # 优雅关闭远程 supervisord 服务
```

---

## YAML 配置项全量参考规范

### 全量配置示例 (supervisord.yaml)

```yaml
# ==============================================================================
# rsupervisord 配置参考模板 (supervisord.yaml)
# 支持 ${ENV_VAR} 或 ${ENV_VAR:-default_value} 形式的环境变量动态展开
# ==============================================================================

# 异步运行时工作线程数。省略时默认为系统 CPU 核心数。
# 设置为 1 启用单线程模式。
# 取值优先级：本字段 > --worker-threads > $TOKIO_WORKER_THREADS。
worker_threads: 2

# 可选：守护进程 PID 文件（守护进程退出时自动删除）
# pidfile: "/var/run/supervisord.pid"

# 可选：启动时提升守护进程的软资源限制（仅 Unix，尽力而为）
# minfds: 1024
# minprocs: 512

# 可选：守护进程自身的环境变量，托管程序会继承。
# environment:
#   APP_HOME: "/opt/app"

# ------------------------------------------------------------------------------
# 1. 服务通信与 HTTP API 监听配置
# ------------------------------------------------------------------------------
server:
  # 本地 IPC 端点 (Unix 为套接字文件，Windows 为命名管道 / AF_UNIX)
  # Linux 默认: /var/run/supervisord.sock
  # Windows 默认: \\.\pipe\supervisord
  uds_path: "/var/run/supervisord.sock"

  # 可选：TCP HTTP 监听地址 (同时支持 REST API 与内置 Web 控制台)
  # 格式支持: "127.0.0.1:9001", "0.0.0.0:9001", ":9001", "9001"
  # 省略则不开放 TCP 监听。
  http_bind: "127.0.0.1:9001"

  # 可选：HTTP API 的 Bearer Token 访问凭据 (留空表示不启用)
  auth_token: "${SUPERVISORD_TOKEN:-}"

  # 可选：HTTP Basic 认证用户名与密码 (兼容原版 Supervisor)
  # 密码支持明文或 {SHA} 前缀的 SHA-1 哈希值
  username: "admin"
  password: "{SHA}82ab876d1387bfafe46cc1c8a2ef074eae50cb1d"

  # 可选：事件协议 / XMLRPC 上报的服务标识
  # (缺省值: rsupervisord-compat)
  identifier: "supervisor-node-01"

  # 路径解析开关 (默认: true)
  # 为 true 时，配置中的相对路径基于配置文件所在目录解析；
  # 为 false 时，基于守护进程的当前工作目录解析。
  # 对 INI (Python 兼容) 配置强制为 false。
  path_translation: true

  # 守护进程以特权 (root/管理员) 身份运行时，是否允许非特权客户端连接本地 IPC (默认: false)
  # 对 INI (Python 兼容) 配置强制为 true。
  allow_unelevated: false

  # 可选：本地 IPC 端点权限模式（别名: chmod）。八进制字符串，推荐值: "0700"、"0770"、"0777"。
  # 未配置时的默认值: allow_unelevated=true 时为 "0777"；否则 Unix 为 "0700"、Windows 为 "0770"。
  # 支持 "0700" / "0o700" / "700" 格式，显式配置优先。
  # uds_chmod: "0700"

  # 为 true（默认）时，加载配置时用本 server 节回填空的 `ctl` 字段。
  # 对 INI (Python 兼容) 配置强制为 false。
  ctl_defaults: true

# ------------------------------------------------------------------------------
# 2. supervisorctl 客户端连接默认值（守护进程不消费本节）
# ------------------------------------------------------------------------------
# YAML 键 `ctl`（别名 `supervisorctl`）；INI 映射 `[supervisorctl]`。
# 字段相互独立：-s 只覆盖 serverurl，-k 只覆盖 auth_token；
# -u/-p 成对覆盖。本节存在但省略 serverurl 时默认
# http://localhost:9001（对齐 Python）。
ctl:
  # serverurl: "http://127.0.0.1:9001"   # 或 unix:///path/to.sock
  # username: "admin"
  # password: "{SHA}..."
  # auth_token: "${SUPERVISORD_TOKEN:-}"

# ------------------------------------------------------------------------------
# 3. 守护进程自身日志配置
# ------------------------------------------------------------------------------
logging:
  # 守护进程日志模式: on、off、in_memory_only (也接受 true/false)。
  # 默认: on
  enabled: true

  # 守护进程日志文件路径。省略时使用平台默认路径
  # (Linux: /var/log/supervisord/supervisord.log，Windows: <配置文件目录>/logs/supervisord.log)
  file: "logs/supervisord.log"

  # 日志级别: trace, debug, info, warn, error, off (默认: info)
  level: "info"

  # 单个日志文件轮转大小上限 (支持 B, KB, MB, GB, 默认: 50MB)
  max_bytes: "50MB"

  # 历史保留备份数量 (默认: 10)
  backups: 10

  # 供 tail / Web 流式查看使用的内存环形缓冲大小 (默认: 1MB)
  # buffer_size: "1MB"

  # 轮转后的日志文件名追加时间戳 (默认: false)
  # timestamp_suffix: false

  # 关闭控制台输出，日志文件仍然写入 (默认: false)
  # silent: false

# ------------------------------------------------------------------------------
# 4. 资源监控指标 (CPU 与内存 RSS)
# ------------------------------------------------------------------------------
metrics:
  # 是否开启性能指标采集 (默认: true)
  enabled: true

  # 闲置超时时长 (秒，默认: 30)
  # 当超过指定时长无 CLI 或 Web 客户端连接时，暂停后台指标采集。
  # 设为 0 表示持续采集。
  idle_timeout_secs: 30

  # 活跃状态下的指标刷新采样间隔 (秒，默认: 2)
  interval_secs: 2

# ------------------------------------------------------------------------------
# 5. 全局程序默认属性模板 (所有 program 默认继承)
# ------------------------------------------------------------------------------
# 仅下列字段可被继承。command、args、directory、user、umask、environment、
# depends_on、exit_codes、group、cron、cron_stop 必须逐个 program 配置。
program_defaults:
  autostart: true
  autorestart: unexpected
  start_secs: 1
  start_retries: 3
  stop_signal: "TERM"
  stop_wait_secs: 10
  priority: 50
  logs:
    enabled: true
    max_bytes: "50MB"
    backups: 10
    redirect_stderr: false

# ------------------------------------------------------------------------------
# 6. 进程分组定义 (支持按组批量操作)
# ------------------------------------------------------------------------------
groups:
  web-cluster:
    programs:
      - api-server
      - frontend-server
    # 分组优先级，0..999 (默认: 999)
    priority: 80

# ------------------------------------------------------------------------------
# 7. 被托管程序定义
# ------------------------------------------------------------------------------
programs:
  # 基础服务示例：数据库服务
  database:
    command: "redis-server --port 6379"
    priority: 10
    autostart: true
    autorestart: always
    start_secs: 2
    health_check:
      type: tcp
      endpoint: "127.0.0.1:6379"
      interval_secs: 10
      timeout_secs: 2
      failure_threshold: 3
    logs:
      stdout: "logs/redis.log"
      redirect_stderr: true

  # 核心服务示例：后端 API 服务 (依赖 database，含 HTTP 探针)
  api-server:
    command: "./bin/api-server"
    args:
      - "--port"
      - "8080"
    directory: "./services/api"
    priority: 20
    depends_on:
      - "database"

    # Unix 平台专属权限与运行身份 (Windows 平台忽略)
    # user: "www-data"
    # umask: 022

    environment:
      APP_ENV: "production"
      DATABASE_URL: "${DATABASE_URL:-postgres://postgres:secret@127.0.0.1:5432/app}"

    autostart: true
    autorestart: unexpected
    exit_codes: [0]
    start_secs: 3
    start_retries: 5
    stop_signal: TERM
    stop_wait_secs: 15

    # HTTP 健康检查探针
    health_check:
      type: http
      url: "http://127.0.0.1:8080/health"
      expected_status: 200
      interval_secs: 15
      timeout_secs: 3
      failure_threshold: 3
      initial_delay_secs: 5

    logs:
      stdout: "logs/api-server.log"
      stderr: "logs/api-server.err"
      max_bytes: "50MB"
      backups: 5

  # 定时任务与生命周期钩子示例
  nightly-backup:
    command: "python3 backup.py --full"
    autostart: false
    cron: "0 2 * * *"              # 每天凌晨 02:00 自动触发启动
    cron_stop: "0 4 * * *"         # 如果到 04:00 仍在运行则强制停机

    # 生命周期执行钩子
    pre_start: "sh -c 'echo Preparing backup snapshot...'"
    pre_start_ignore_failure: false # 前置钩子失败则拒绝启动业务进程
    pre_stop: "sh -c 'echo Cleaning temporary files...'"
    hook_timeout_secs: 15

    logs:
      stdout: "logs/backup.log"
      redirect_stderr: true

  # 文件与二进制热监控示例 (文件或二进制更新时自动热重载)
  gateway:
    command: "./bin/gateway"
    priority: 30
    autostart: true

    # 可执行文件变更时重载；未配置信号则执行完整重启
    restart_when_binary_changed: true
    restart_signal_when_binary_changed: SIGHUP

    # 监控配置目录文件变动
    restart_directory_monitor: "./config"
    restart_file_pattern: "*.json"
    restart_signal_when_file_changed: SIGHUP
    restart_debounce_secs: 5

    logs:
      stdout: "logs/gateway.log"
      redirect_stderr: true

# ------------------------------------------------------------------------------
# 8. 事件监听器 (原版 Supervisor 事件协议兼容)
# ------------------------------------------------------------------------------
event_listeners:
  memmon:
    command: "python3 -m supervisor.memmon -a 200MB -m admin@example.com"
    # 事件订阅列表，至少配置一项
    events:
      - "TICK_60"
    # 事件队列长度 (默认: 10)
    buffer_size: 10
```

---

### 各配置节详细参数说明

#### 1. 顶层与 `server` 节 (通信与基础设置)

- **顶层字段**：
  - **`worker_threads`** (*整型*, 默认: CPU 核心数): 异步运行时工作线程数，`1` 表示单线程模式。取值优先级：本字段 > `--worker-threads` > `$TOKIO_WORKER_THREADS`。
  - **`nodaemon`** (*布尔值*, 默认: `false`): 前台运行标志，为兼容原版保留。当前版本始终以前台方式运行。
  - **`environment`** (*键值对映射*, 默认: `{}`): 守护进程自身的环境变量，托管程序会继承。
  - **`pidfile`** (*路径字符串*, 可选): 写入守护进程 PID 的文件，守护进程退出时自动删除。
  - **`minfds`** (*整型*, 可选，仅 Unix): 启动时将文件描述符软限制提升到该值（尽力而为）。
  - **`minprocs`** (*整型*, 可选，仅 Unix): 启动时将进程数软限制提升到该值（尽力而为）。
- **`server.uds_path`** (*路径字符串*, 默认: Unix 为 `/var/run/<cmd_name>.sock`，Windows 为 `\\.\pipe\<cmd_name>`): 本地 IPC 端点。Unix 使用套接字文件；Windows 支持命名管道 (`\\.\pipe\name`) 或 `AF_UNIX` 文件路径。
- **`server.http_bind`** (*字符串*, 默认: 无): TCP 监听网络地址与端口。提供 REST API 与 Web 控制台，省略则不开放 TCP 监听。`":9001"`、`"*:9001"`、`"9001"` 会归一化为 `"0.0.0.0:9001"`。
- **`server.auth_token`** (*字符串*, 可选): REST/XMLRPC/SSE 访问令牌（`Authorization: Bearer <token>` 或 `?token=`）。与 basic 凭据同时配置时二者均可通过（OR 语义），TCP 与 IPC 监听器同时生效。
- **`server.username` / `server.password`** (*字符串*, 可选; 别名 `http_username` / `http_password`): HTTP Basic Auth 访问凭据。密码支持 `{SHA}` 前缀哈希值。IPC 使用 `uds_username`/`uds_password`（缺省时从本对自动填充）。Web UI 通过登录弹窗换取 HttpOnly 会话 Cookie，浏览器中不保存任何明文凭据。
- **`server.uds_username` / `server.uds_password`** (*字符串*, 可选): 本地 IPC 端点校验的凭据，YAML 中省略时默认取 `username` / `password`。
- **`server.identifier`** (*字符串*, 默认: `rsupervisord-compat`): 事件协议与 XMLRPC 上报的服务标识。
- **`server.path_translation`** (*布尔值*, 默认: `true`): 为 `true` 时，配置中的相对路径基于配置文件所在目录 (`config_dir`) 解析；为 `false` 时，基于守护进程的当前工作目录解析。对 INI（Python 兼容）配置强制为 `false`。
- **`server.allow_unelevated`** (*布尔值*, 默认: `false`): 当守护进程以 root 或 Administrator 特权身份运行时，是否允许非特权客户端连接本地 IPC。对 INI（Python 兼容）配置强制为 `true`。
- **`server.uds_chmod`** (*字符串* / 别名 `chmod`, 可选): 本地 IPC 端点权限模式。有效值推荐为 `"0700"`、`"0770"` 或 `"0777"`（支持 `"0700"` / `"0o700"` / `"700"` 格式）。未配置时的默认值：`allow_unelevated: true` 时默认为 `"0777"`；否则 Unix 默认为 `"0700"`，Windows 默认为 `"0770"`。显式配置优先。
- **`server.ctl_defaults`** (*布尔值*, 默认: `true`): 为 `true` 时，加载配置时用 server 节回填**已存在** `ctl` 节中的空字段。对 INI（Python 兼容）配置强制为 `false`。

#### 2. `ctl` 节 (supervisorctl 客户端连接默认值)

仅客户端使用的连接配置（别名 `supervisorctl`；INI 映射 `[supervisorctl]`）。**守护进程不消费本节**，它不会改变 `server.uds_*` / `server.username`。

- **本节存在** → 控制端只连接这一个端点，省略 `serverurl` 时回退到 `http://localhost:9001`。
- **本节不存在 + `ctl_defaults: true`** → 端点由 `server` 节推导：优先本地 IPC，`http_bind` 存在时再加 TCP（凭据与令牌同步复制）。
- **本节不存在 + `ctl_defaults: false`**（INI）→ 报错，因为原版要求存在 `[supervisorctl]`。
- **无配置文件** → 优先本地端点，其次 `http://localhost:9001`。

字段说明：

- **`ctl.serverurl`** (*字符串*, 可选): 未给 `-s` 时使用的端点。支持 `http://…`、`tcp://…`、`unix://…`、命名管道与裸 IPC 路径。
- **`ctl.username` / `ctl.password`** (*字符串*, 可选): 该端点的 HTTP Basic Auth。CLI `-u`/`-p` 成对覆盖（缺侧变为 `""`）。
- **`ctl.auth_token`** (*字符串*, 可选): Bearer 令牌；CLI `-k` 只覆盖本字段。

显式 `-c` 指定的配置文件加载失败始终报错。

#### 3. `logging` 节 (守护进程自身日志)

- **`logging.enabled`** (*模式*, 默认: `on`): `on`、`off` 或 `in_memory_only`（也接受布尔值）。`off` 关闭日志；`in_memory_only` 只保留内存日志，供 `tail`/`maintail` 与 Web 界面读取。
- **`logging.file`** (*路径字符串*, 可选): 日志落盘路径。省略且日志开启时使用平台默认路径（见[缺省运行时路径约定](#2-配置文件探测规则与默认路径)）。除非设置 `silent`，控制台输出同样保留。
- **`logging.level`** (*字符串*, 默认: `"info"`): 日志等级（`trace`, `debug`, `info`, `warn`, `error`, `off`）。设置 `RUST_LOG` 时以其为准。
- **`logging.max_bytes`** (*字符串*, 默认: `"50MB"`): 日志轮转大小限制（支持 `B`、`KB`、`MB`、`GB`）。
- **`logging.backups`** (*整型*, 默认: `10`): 历史备份日志保留份数。
- **`logging.buffer_size`** (*字符串*, 默认: `"1MB"`): 供 `maintail` 与 Web 界面使用的内存环形缓冲大小。
- **`logging.timestamp_suffix`** (*布尔值*, 默认: `false`): 轮转后的日志文件名追加时间戳。
- **`logging.silent`** (*布尔值*, 默认: `false`): 关闭控制台输出，日志文件仍然写入。

#### 4. `metrics` 节 (资源指标监控)

- **`metrics.enabled`** (*布尔值*, 默认: `true`): 是否开启 CPU 与内存 RSS 数据采样。
- **`metrics.idle_timeout_secs`** (*整型*, 默认: `30`): 客户端闲置自动挂起超时时间。设为 `0` 表示持续采集。
- **`metrics.interval_secs`** (*整型*, 默认: `2`): 活跃状态下的采样时间间隔。

#### 5. `program_defaults` 节 (全局程序默认值)

每个字段均为可选，只在 program 未配置对应字段时生效。支持的字段：

`autostart`、`autorestart`、`start_secs`、`start_retries`、`restart_pause_secs`、`stop_signal`、`stop_wait_secs`、`priority`、`kill_wait_secs`、`stop_as_group`、`kill_as_group`、`env_files`、`logs`、`health_check`、`pre_start`、`pre_stop`、`pre_start_ignore_failure`、`hook_timeout_secs`、`numprocs`、`numprocs_start`、`process_name`、`restart_when_binary_changed`、`restart_signal_when_binary_changed`、`restart_cmd_when_binary_changed`、`restart_directory_monitor`、`restart_file_pattern`、`restart_signal_when_file_changed`、`restart_cmd_when_file_changed`、`restart_debounce_secs`。

`command`、`args`、`directory`、`user`、`umask`、`environment`、`depends_on`、`exit_codes`、`group`、`cron`、`cron_stop` **不支持继承**，必须逐个 program 配置。

#### 6. `groups` 节 (进程分组)

- **`groups.<name>.programs`** (*字符串列表*, 默认: `[]`): 属于该分组的程序名，必须都存在于 `programs` 中。
- **`groups.<name>.priority`** (*整型 0..999*, 默认: `999`): 分组优先级，数值越小越早启动、越晚停止。

#### 7. `programs.<name>` 节 (托管程序详细属性)

- **基础执行控制**：
  - **`command`** (*字符串*, 必须): 启动命令行指令。支持参数分词与路径解析。
  - **`args`** (*字符串列表*, 可选): 额外的命令行参数列表。为空时 `command` 会被拆分为程序与参数。
  - **`directory`** (*路径字符串*, 可选): 子进程运行时的工作目录（CWD）。缺省为守护进程的工作目录。
  - **`user`** (*字符串*, 可选，仅 Unix): 执行子进程的系统账户名。
  - **`umask`** (*整型*, 可选，仅 Unix): 子进程文件掩码（如 `022`）。
  - **`environment`** (*键值对映射*, 可选): 子进程独立环境变量注入，支持 `${VAR}` 展开。
  - **`env_files`** (*路径列表*, 可选): 先于 `environment` 加载的环境变量文件，`environment` 中的同名值优先。
  - **`autostart`** (*布尔值*, 默认: `true`): 是否在守护进程启动时自动启动该程序（配置 `cron` 时默认为 `false`）。
  - **`autorestart`** (*枚举*, 默认: `"unexpected"`): 自动重启策略：
    - `"unexpected"`: 仅在进程异常退出（退出码不在 `exit_codes` 中）时重启；
    - `"always"`: 进程退出后无条件重启；
    - `"never"`: 进程退出后绝不重启。
  - **`exit_codes`** (*整型列表*, 默认: `[0]`): 判定为正常退出的返回码集合。
  - **`start_secs`** (*整型*, 默认: `1`): 启动后需持续稳定运行的秒数，达标后状态方转为 `RUNNING`。
  - **`start_retries`** (*整型*, 默认: `3`): 启动失败后的最大重试次数。计数从 0 开始，进程进入 `RUNNING` 后清零，因此默认允许 1 次启动 + 3 次重试，超过后进入 `FATAL`。
  - **`restart_pause_secs`** (*整型*, 默认: `0`): `> 0` 时启动失败重试前固定等待该秒数；否则退避为 `2^n` 秒（上限 `32`）。不延迟已进入 `RUNNING` 后的自动重启。
  - **`priority`** (*整型 0..999*, 默认: `50`): 启停优先级。数值越小越早启动、越晚停止。
  - **`depends_on`** (*字符串列表*, 可选): 依赖的前置程序列表。启动按拓扑排序分层并行执行。
  - **`group`** (*字符串*, 可选): 所属逻辑分组名。缺省取第一个包含该程序的 `groups` 条目，否则取程序名本身。
- **停止与清理**：
  - **`stop_signal`** (*字符串*, Unix 默认 `"TERM"`, Windows 默认 `"CTRL_BREAK"`): 优雅停止信号。支持 `TERM`, `INT`, `QUIT`, `KILL`, `HUP`, `CTRL_C`, `CTRL_BREAK` 等。
  - **`stop_wait_secs`** (*整型*, 默认: `10`): 等待优雅退出的最长超时秒数，超时后强制终止。
  - **`kill_wait_secs`** (*整型*, 默认: `2`): 强制终止信号发出后，等待进程消失的秒数。
  - **`stop_as_group`** (*布尔值*, 默认: `false`): 将 `stop_signal` 发送给整个进程组。
  - **`kill_as_group`** (*布尔值*, 默认: 同 `stop_as_group`): 强制终止整个进程组。`stop_as_group: true` 且 `kill_as_group: false` 属于配置错误。
- **日志管道 (`logs`)**：
  - **`logs.enabled`** (*模式*, 默认: `on`): `on`、`off` 或 `in_memory_only`（接受布尔值）。`off` 时子进程的 stdout/stderr 接空设备，不采集日志。
  - **`logs.stdout`** (*路径字符串*): 目标日志文件路径。`logs.enabled` 为 `on` 且省略时使用平台默认程序日志路径（见[缺省运行时路径约定](#2-配置文件探测规则与默认路径)）。设为 `NONE`、`OFF`、`NULL`、`/dev/null` 禁用落盘。
  - **`logs.stderr`** (*路径字符串*): stderr 目标路径。未配置时不写文件。
  - **`logs.redirect_stderr`** (*布尔值*, 默认: `false`): 是否将 stderr 汇聚到 stdout 输出。
  - **`logs.max_bytes`** (*字符串*, 默认: `"50MB"`): 日志滚动大小阈值。
  - **`logs.backups`** (*整型*, 默认: `10`): 历史归档保留数。
  - **`logs.stdout_max_bytes` / `logs.stderr_max_bytes`** (*字符串*, 默认: 取 `logs.max_bytes`): 单流轮转大小。
  - **`logs.stdout_backups` / `logs.stderr_backups`** (*整型*, 默认: 取 `logs.backups`): 单流保留份数。
  - **`logs.buffer_size`** (*字符串*, 默认: `"1MB"`): 供 `tail` 与 Web 界面使用的内存环形缓冲大小。
  - **`logs.stdout_timestamp_suffix` / `logs.stderr_timestamp_suffix`** (*布尔值*, 默认: `false`): 轮转文件名追加时间戳。
  - **`logs.stdout_syslog` / `logs.stderr_syslog`** (*布尔值*, 默认: `false`, 仅 Unix): 同时将该流输出到 syslog。
  - **`logs.syslog_facility` / `logs.syslog_tag`** (*字符串*, 可选): syslog 设施与标签。
  - **`logs.syslog_stdout_priority` / `logs.syslog_stderr_priority`** (*字符串*, 可选): 两个流的 syslog 优先级。
  - **`logs.stdout_events_enabled` / `logs.stderr_events_enabled`** (*布尔值*, 默认: `false`): 是否发出 `PROCESS_LOG_STDOUT` / `PROCESS_LOG_STDERR` 事件；`logs.*` 未设置时取程序级同名字段。
- **主动健康探针 (`health_check`)**：
  - **`health_check.type`** (*枚举*): 探针类型，支持 `"http"`、`"tcp"`、`"exec"`。
  - **`health_check.url`** (*URL*): HTTP 探针目标地址。
  - **`health_check.expected_status`** (*整型*, 默认: `200`): HTTP 探针判定健康的返回码。
  - **`health_check.endpoint`** (*地址*): TCP 探针检测的主机与端口（例如 `"127.0.0.1:3306"`）。
  - **`health_check.command`** (*字符串*): 脚本探针指令（退出码为 0 视为健康）。
  - **`health_check.interval_secs`** (*整型*, 默认: `10`): 探测执行周期。
  - **`health_check.timeout_secs`** (*整型*, 默认: `2`): 单次检测超时时间。
  - **`health_check.failure_threshold`** (*整型*, 默认: `3`): 连续失败触发自动重启的判定阈值。
  - **`health_check.initial_delay_secs`** (*整型*, 默认: `0`): 进程就绪后首次探测的静默等待时间。
- **定时任务与扩展生命周期**：
  - **`cron`** (*Cron 表达式*): 启动调度 Cron 规则（如 `"0 2 * * *"`）。
  - **`cron_stop`** (*Cron 表达式*, 别名 `stop_cron`): 强制停机 Cron 规则。
  - **`pre_start`** (*字符串*, 别名 `pre_start_hook`): 进程启动前执行的前置脚本命令。
  - **`pre_start_ignore_failure`** (*布尔值*, 默认: `false`): 前置脚本报错时是否忽略并继续启动业务程序。
  - **`pre_stop`** (*字符串*, 别名 `pre_stop_hook`): 进程停止前执行的收尾脚本命令（失败不会阻止停止）。
  - **`hook_timeout_secs`** (*整型*, 默认: `15`): 钩子脚本最大允许执行的超时时间。
- **多副本集群扩展**：
  - **`numprocs`** (*整型*, 默认: `1`): 进程并行副本启动数。
  - **`numprocs_start`** (*整型*, 默认: `0`): 副本编号起始偏移值。
  - **`process_name`** (*字符串*): 副本名称渲染模板，如 `%(program_name)s_%(process_num)02d`。
- **动态文件监控热重载**：
  - **`restart_when_binary_changed`** (*布尔值*, 默认: `false`): 监控可执行文件本身，变更时触发重载。
  - **`restart_signal_when_binary_changed`** (*信号*): 可执行文件变更时发送的信号，未配置则执行完整重启。
  - **`restart_cmd_when_binary_changed`** (*命令*): 可执行文件变更时执行的命令，配置后优先于信号。
  - **`restart_directory_monitor`** (*路径字符串*): 动态监控的配置文件或静态资产目录，未配置则不监控目录。
  - **`restart_file_pattern`** (*Glob 表达式*, 例如 `"*.json"`): 目录监控文件的匹配通配符，未配置则匹配全部文件。
  - **`restart_signal_when_file_changed`** (*信号*, 例如 `SIGHUP`): 文件变更时发送的重载信号，未配置则执行完整重启。
  - **`restart_cmd_when_file_changed`** (*命令*): 文件变更时执行的命令，配置后优先于信号。
  - **`restart_debounce_secs`** (*整型*, 默认: `5`): 文件修改防抖时间窗口（秒）。

#### 8. `event_listeners.<name>` 节 (事件监听器)

- **`command`** (*字符串*, 必须): 监听器的启动命令行指令。
- **`args`** (*字符串列表*, 可选): 额外的命令行参数。
- **`events`** (*字符串列表*, 必须): 事件订阅列表（如 `TICK_60`、`PROCESS_STATE`、`PROCESS_LOG`），至少配置一项。
- **`buffer_size`** (*整型*, 默认: `10`): 单个监听器的事件队列长度。
- **`result_handler`** (*字符串*, 默认: `supervisor.dispatchers:default_handler`): 事件协议中上报的结果处理器名称。
- **`priority`** (*整型*, 默认: `0`，负值按 `0` 处理): 监听器启停优先级。
- **`numprocs`** (*整型*, 默认: `1`): 监听器实例数。
- **`numprocs_start`** (*整型*, 默认: `0`): 实例编号起始偏移值。
- **`process_name`** (*字符串*): `numprocs > 1` 时的实例名称渲染模板。
- **`autostart`** (*布尔值*, 默认: `true`): 随守护进程自动启动。
- **`autorestart`** (*枚举*, 默认: `"unexpected"`): 取值同 program。
- **`start_secs`** (*整型*, 默认: `1`): 视为启动成功所需的最短运行秒数。
- **`start_retries`** (*整型*, 默认: `3`): 启动失败后的最大重试次数。
- **`stop_signal`** (*字符串*, Unix 默认 `"TERM"`, Windows 默认 `"CTRL_BREAK"`): 优雅停止信号。
- **`stop_wait_secs`** (*整型*, 默认: `10`): 等待优雅退出的最长超时秒数。
- **`directory`** (*路径字符串*, 可选): 工作目录，缺省为守护进程的工作目录。
- **`user`** (*字符串*, 可选，仅 Unix): 执行账户名。
- **`environment`** (*键值对映射*, 可选): 监听器进程的环境变量。
- **`umask`** (*整型*, 可选，仅 Unix): 文件掩码。
- **`env_files`** (*路径列表*, 可选): 加载到监听器环境中的变量文件。
- **`stderr_logfile`** (*路径字符串*, 可选): 监听器 stderr 输出路径。
- **`stop_as_group` / `kill_as_group`** (*布尔值*, 默认: `false`): 与 program 相同的进程组语义。

> [!NOTE]
> 事件监听器的 stdout 通道用于传输事件协议，因此 `stdout_logfile` 会被忽略，`redirect_stderr` 也不能设为 `true`。

---

## 原版 INI 配置基本示例

`rsupervisord` 原生兼容原版 Supervisor 的 INI / Conf 配置语法，可直接加载运行。

### 基本配置示例 (`supervisord.conf`)

```ini
[unix_http_server]
file = /var/run/supervisord.sock
chmod = 0700

[inet_http_server]
port = 127.0.0.1:9001
username = admin
password = adminpassword

[supervisord]
logfile = /var/log/supervisord.log
logfile_maxbytes = 50MB
logfile_backups = 10
loglevel = info
nodaemon = true

[rpcinterface:supervisor]
supervisor.rpcinterface_factory = supervisor.rpcinterface:make_main_rpcinterface

[supervisorctl]
serverurl = unix:///var/run/supervisord.sock

[program:web]
command = python3 app.py --port 8000
directory = /srv/www
autostart = true
autorestart = unexpected
redirect_stderr = true
stdout_logfile = /var/log/web.log
```

> [!NOTE]
> 关于原版 INI 格式的所有具体配置项说明与完整语法指南，请查阅原版官方文档：
> 📖 **[Supervisor Configuration File Documentation](http://supervisord.org/configuration.html)**

---

## Web 控制台

当配置了 `server.http_bind`（如 `127.0.0.1:9001`）时，`rsupervisord` 会自动启动嵌入式 Web 控制面板。在浏览器中访问：

```text
http://127.0.0.1:9001/
```

- **全状态仪表盘**：实时展示各进程当前运行状态、PID、运行时长、CPU 占用百分比、物理内存（RSS）大小与健康探针结果。
- **交互式批量控制**：支持通过复选框多选，一键执行批量启动、停止与重启。
- **实时 SSE 流式日志抽屉**：点击终端图标即可滑出实时日志查看器，支持实时流式跟踪（Follow）、历史行数拉取与滚动条位置自动锁定。
- **可视化配置 Diff 审查**：在执行配置热重载时，直观展示新增、移除与变更的配置项，降低误操作风险。

---

## 开源许可证

本项目基于 **[Mozilla Public License 2.0 (MPL-2.0)](LICENSE)** 协议开源。
欢迎提交 Issue 与 Pull Request！
