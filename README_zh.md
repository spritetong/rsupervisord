# rsupervisord

[![License: MPL 2.0](https://img.shields.io/badge/License-MPL_2.0-brightgreen.svg)](LICENSE)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](https://www.rust-lang.org)
[![Platform: Linux | Windows | macOS](https://img.shields.io/badge/Platform-Linux%20%7C%20Windows%20%7C%20macOS-lightgrey.svg)]
[![Tests](https://img.shields.io/badge/Tests-174%2F174%20Passing-brightgreen.svg)]

**rsupervisord** 是使用现代化 Rust 语言开发的下一代高性能、跨平台进程编排与监控守护引擎。

[English](README.md) | [简体中文](README_zh.md)

---

## 目录

- [rsupervisord](#rsupervisord)
  - [目录](#目录)
  - [开发目的与背景](#开发目的与背景)
  - [核心特性](#核心特性)
    - [1. Windows 原生支持：直接取代 winsw 与 NSSM](#1-windows-原生支持直接取代-winsw-与-nssm)
    - [2. 真正的静默零轮询 (0% CPU 闲置消耗)](#2-真正的静默零轮询-0-cpu-闲置消耗)
    - [3. Linux / Unix 系统级可靠控制](#3-linux--unix-系统级可靠控制)
    - [4. 零停机配置热重载 (Zero-Downtime Hot Reload)](#4-零停机配置热重载-zero-downtime-hot-reload)
    - [5. 单文件自包含嵌入式 Web 控制台](#5-单文件自包含嵌入式-web-控制台)
    - [6. 解析边界统一路径转译 (Parse-Boundary Path Translation)](#6-解析边界统一路径转译-parse-boundary-path-translation)
    - [7. 双协议安全通信与极简单线程模式](#7-双协议安全通信与极简单线程模式)
  - [系统架构](#系统架构)
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
    - [全量生产级配置示例 (supervisord.yaml)](#全量生产级配置示例-supervisordyaml)
    - [各配置节详细参数说明](#各配置节详细参数说明)
      - [1. 顶层与 `server` 节 (通信与基础设置)](#1-顶层与-server-节-通信与基础设置)
      - [2. `logging` 节 (守护进程自身日志)](#2-logging-节-守护进程自身日志)
      - [3. `metrics` 节 (资源指标监控)](#3-metrics-节-资源指标监控)
      - [4. `programs.<name>` 节 (托管程序详细属性)](#4-programsname-节-托管程序详细属性)
  - [原版 INI 配置基本示例](#原版-ini-配置基本示例)
    - [基本配置示例 (`supervisord.conf`)](#基本配置示例-supervisordconf)
  - [Web 控制台](#web-控制台)
  - [质量与性能验证](#质量与性能验证)
  - [开源许可证](#开源许可证)

---

## 开发目的与背景

在容器化、微服务部署、边缘计算以及 Windows 主机环境中，进程管理工具至关重要。长期以来，行业内主要依赖以下方案，但它们存在显著的痛点：

1. **Python 原版 Supervisor 的分发与平台困境**：
   - 原版 [Supervisor (Python)](http://supervisord.org/) 依赖完整的 Python 运行环境、`setuptools` 及一系列第三方依赖库，在生产环境中打包、交叉编译以及制作单文件自包含镜像极为繁琐。
   - 官方实现缺乏原生 Windows 支持，无法直接在 Windows 环境中作为生产级服务使用。
2. **Go 语言社区版本 `supervisord` 的局限性与 Windows 平台缺陷**：
   - 社区中虽然出现了基于 Go 语言的重写版本 `supervisord`，但其功能并不完整，且在 Windows 平台上的实现存在诸多严重缺陷。
   - **子进程泄漏问题**：缺乏系统级进程树生命周期控制，严重依赖模拟信号或外部 `taskkill.exe`，频繁出现主进程或服务退出后后台孙进程/子进程残留成为孤儿进程、占用端口无法回收的情况。
   - **高空闲开销**：在无任何报警与探针的静默监控状态下，持续轮询和运行时 GC 带来了不可忽视的 CPU 与内存额外开销。
   - **路径与命令处理失真**：在 Windows 平台对路径边界、相对路径解析及批处理脚本调用的处理存在边缘错误。

**`rsupervisord` 的诞生目标**：
基于现代 Rust 异步技术栈（Tokio + 操作系统原生内核通知），提供**单文件静态链接、零外部运行时依赖、真 0% CPU 闲置开销、高可靠进程树回收**的下一代全功能进程守护引擎。

---

## 核心特性

### 1. Windows 原生支持：直接取代 winsw 与 NSSM

- **内置 Windows Service (SCM) 服务驱动**：无需再使用 `winsw` 或 `NSSM` 等第三方工具进行外壳包装。通过 `supervisord service install` 即可将自身直接注册为自启动的 Windows 服务。
- **Win32 Job Objects 保证 100% 连根回收（无孤儿进程泄漏）**：每个托管进程均绑定至 Windows 原生 `Job Object` 并配置 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`。无论是多层调用的 Node.js、Python、Go 进程，还是 `.bat` / `.cmd` 派生的所有孙进程树，当服务停止或进程被重启时，均由 Windows 内核级强制回收，从根源上杜绝端口占用与僵尸进程。
- **超越单服务包装的多进程拓扑编排**：`winsw` / `NSSM` 仅能托管单一二进制文件，而 `rsupervisord` 可统一管理数十个微服务与批处理进程，提供有向无环图（DAG）依赖启动、健康检查、周期 Cron 任务、文件热监控与可视化控制台。

### 2. 真正的静默零轮询 (0% CPU 闲置消耗)

- **纯事件驱动内核通知**：
  - **Windows**：采用 Win32 内核事件通知机制（`RegisterWaitForSingleObject` 绑定进程句柄退出事件）。
  - **Linux / Unix**：采用 `pidfd` 与 POSIX 信号管道驱动。
- **动态计算运行时长**：按需从 `started_at` 计算在线运行时间，彻底废除传统实现每 2 秒一次的无意义后台全局 Tick 轮询。未配置主动健康探针的程序在 Tokio Reactor 中完全休眠，**零定时器唤醒，闲置 CPU 占用严格保持 0.00%**。

### 3. Linux / Unix 系统级可靠控制

- 独立的进程组隔离（`setpgid`）与组信号投送（`killpg`），支持一键安装并管理 systemd 系统服务。
- 采用 `tokio_util::sync::DropGuard` 与 `scopeguard` 实现严格的 RAII 资源生命周期保障，杜绝套接字与文件句柄泄漏。

### 4. 零停机配置热重载 (Zero-Downtime Hot Reload)

- 独创的三路（3-Way）配置差分算法：执行配置重载时，**未被修改的进程保持原有 PID 稳定运行，业务连接零中断**；仅按需平滑启停新增、修改或删除的程序。

### 5. 单文件自包含嵌入式 Web 控制台

- 单一二进制文件中内置基于 Vue 3 的轻量级单页面应用（SPA），**无需 Node.js、NPM 或外部 CDN**。
- 支持实时 SSE 流式日志抽屉（实时滚动刷新与暂停锁定）、多选批量启停操作、状态指示灯、内存/CPU 资源占用监控与配置差异比对视图。

### 6. 解析边界统一路径转译 (Parse-Boundary Path Translation)

- 遵循零扩散原则（Zero-Diffusion）：在配置文件解析阶段完成相对路径转绝对路径锚定（基于配置文件所在目录 `config_dir`），彻底消除了跨平台环境下子进程 CWD 与可执行程序查找规则的差异。
- Windows 平台原生支持 `.bat` 与 `.cmd` 脚本透明封装，自动剔除 `\\?\` 长路径前缀，确保控制台工具无缝调用。

### 7. 双协议安全通信与极简单线程模式

- 支持 cross-platform Unix Domain Socket（Windows 10/11 原生 `AF_UNIX` 及具名管道 `\\.\pipe\`）与带鉴权的 TCP HTTP REST API。
- 可选轻量级单线程模式（`worker_threads: 1` 或 `--worker-threads 1`），内存基线低至 **2~4MB**，极致适配嵌入式网关与边缘设备。

---

## 系统架构

```text
               +-------------------------------------------------------+
               |   CLI: supervisorctl   |   Web 控制台 / REST API      |
               +-------------------------------------------------------+
                                           |
                                [AF_UNIX / Named Pipe / HTTP]
                                           |
               +-------------------------------------------------------+
               |             Axum API 引擎 & 嵌入式 Vue 3 SPA           |
               +-------------------------------------------------------+
                                           | (MPSC 异步通道)
                                           v
               +-------------------------------------------------------+
               |          SupervisorManager (DAG 编排 & 事件总线)        |
               +-------------------------------------------------------+
                             |                            |
                 (PlatformProcessGuard)       (PlatformProcessGuard)
                             v                            v
                  [Process: MySQL/Redis]       [Process: Backend API]
                             |                            |
                  +--------------------+       +--------------------+
                  | Windows Job Object |       | Linux Process Group|
                  +--------------------+       +--------------------+
```

---

## 快速开始

### 1. 源码编译

编译环境要求：**Rust 1.85+ (Edition 2024)**。

```bash
# 克隆仓库
git clone https://github.com/spritetong/rsupervisord.git
cd rsupervisord

# 编译优化发布版（开启 LTO、去除符号表）
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

- **UDS 通信套接字**：
  - **Linux / Unix**：`/var/run/<cmd_name>.sock`。
  - **Windows**：优先使用 `<配置文件目录>/<cmd_name>.sock` 或 Windows 命名管道 `\\.\pipe\<cmd_name>`（非特权账户即可正常通信）。
- **守护进程自身日志**（未配置 `logging.file` 时）：
  - **Linux / Unix**：`/var/log/<cmd_name>/<cmd_name>.log`。
  - **Windows**：`<配置文件目录>/logs/<cmd_name>.log`。

---

### 3. 守护进程运行

```bash
# 1. 自动探测配置文件启动守护进程
./target/release/supervisord

# 2. 显式指定配置文件启动
./target/release/supervisord -c /etc/supervisord/supervisord.yaml

# 3. 指定极简单线程模式启动（极致低内存消耗）
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

# 停止 Windows 服务（安全回收全部子孙进程树）
supervisord.exe service stop

# 重启服务
supervisord.exe service restart

# 卸载服务
supervisord.exe service uninstall
```

> [!NOTE]
> 当注册为 Windows 服务时，操作系统 Service Control Manager (SCM) 启动进程并自动附加 `--service` 标志。`rsupervisord` 在收到 SCM 停机请求时，会执行协作式停机，确保所有任务安全收尾。

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
| `--nodaemon` | `-n` | `true` | 前台运行（当前版本默认前台运行） |
| `--worker-threads <N>` | - | CPU 核心数 | Tokio 运行时工作线程数。设为 `1` 时激活 `current_thread` 单线程运行时 |
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
| `--server <URL>` | `-s` | 自动推导 | 连接目标端点（支持 `unix:///path/to.sock` 或 `http://127.0.0.1:9001`） |
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

# 3. 零停机热重载与配置维护
supervisorctl config reload                    # 【推荐】零停机热重载，未修改进程不重启、连接不中断
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
supervisorctl signal HUP api-server            # 发送指定 POSIX 信号（如 HUP, TERM, KILL, INT）
supervisorctl stdin api-server "reload\n"      # 向指定程序的标准输入写入字符
supervisorctl events                           # 实时订阅并输出系统事件流

# 6. 关闭守护进程
supervisorctl shutdown                         # 优雅关闭远程 supervisord 服务
```

---

## YAML 配置项全量参考规范

### 全量生产级配置示例 (supervisord.yaml)

```yaml
# ==============================================================================
# rsupervisord 全量配置参考模板 (supervisord.yaml)
# 支持 ${ENV_VAR} 或 ${ENV_VAR:-default_value} 形式的环境变量动态展开
# ==============================================================================

# Tokio 运行时工作线程数。省略时默认为系统 CPU 核心数。
# 设置为 1 启用 ultra-lightweight 单线程模式 (仅 2~4MB 内存开销)。
worker_threads: 2

# ------------------------------------------------------------------------------
# 1. 服务通信与 HTTP API 监听配置
# ------------------------------------------------------------------------------
server:
  # 本地 IPC 套接字路径 (Unix Domain Socket / Windows 具名管道)
  # Linux 默认: /var/run/supervisord.sock
  # Windows 默认: <配置文件目录>/supervisord.sock 或 \\.\pipe\supervisord
  uds_path: "/var/run/supervisord.sock"

  # 可选：TCP HTTP 监听地址 (同时支持 REST API 与内置 Web 控制台)
  # 格式支持: "127.0.0.1:9001", "0.0.0.0:9001", ":9001", "9001"
  http_bind: "127.0.0.1:9001"

  # 可选：HTTP API 的 Bearer Token 访问凭据 (留空表示不启用)
  auth_token: "${SUPERVISORD_TOKEN:-}"

  # 可选：HTTP Basic 认证用户名与密码 (兼容原版 Supervisor)
  # 密码支持明文或 {SHA} 前缀的 SHA-1 哈希值
  username: "admin"
  password: "{SHA}82ab876d1387bfafe46cc1c8a2ef074eae50cb1d"

  # 可选：服务唯一标识名 (默认取主机名)
  identifier: "supervisor-node-01"

  # 路径自动转译开关 (默认: true)
  # 为 true 时，在配置解析边界将所有相对路径基于配置文件所在目录转换为绝对路径；
  # 为 false 时，保留原样并相对于执行时的进程当前工作目录 (CWD)。
  path_translation: true

  # 守护进程以特权 (root/管理员) 身份运行且对外提供本地 IPC 时，是否允许非特权客户端连接 (默认: false)
  allow_unelevated: false

  # 可选：本地 IPC 端点权限模式（别名: chmod）。八进制字符串，必须加引号。
  # Unix socket: bind 后调用 set_permissions；Windows 命名管道: 写入首个实例的
  # SECURITY_ATTRIBUTES（无竞态窗口）；Windows 文件型 UDS: SetNamedSecurityInfoW
  # 保护性 DACL。未配置时的默认值: allow_unelevated=false → "0700"，true → "0777"；
  # 显式配置永远优先。解析 fail-fast（0700 / 0o700 / 700）。
  # uds_chmod: "0700"

# ------------------------------------------------------------------------------
# 2. 守护进程自身日志配置
# ------------------------------------------------------------------------------
logging:
  # 是否开启守护进程日志输出 (默认: true)
  enabled: true

  # 守护进程日志文件路径。省略时默认输出到标准错误/控制台
  file: "logs/supervisord.log"

  # 日志级别: trace, debug, info, warn, error, off (默认: info)
  level: "info"

  # 单个日志文件轮转大小上限 (支持 B, KB, MB, GB, 默认: 20MB)
  max_bytes: "20MB"

  # 历史保留备份数量 (默认: 3)
  backups: 5

# ------------------------------------------------------------------------------
# 3. 资源监控指标 (CPU 与内存 RSS)
# ------------------------------------------------------------------------------
metrics:
  # 是否开启性能指标采集 (默认: true)
  enabled: true

  # 闲置超时时长 (秒，默认: 30)
  # 当超过指定时长无 CLI 或 Web 客户端连接时，自动暂停后台指标采集，保持 0% CPU
  idle_timeout_secs: 30

  # 活跃状态下的指标刷新采样间隔 (秒，默认: 2)
  interval_secs: 2

# ------------------------------------------------------------------------------
# 4. 全局程序默认属性模板 (所有 program 默认继承)
# ------------------------------------------------------------------------------
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
    max_bytes: "20MB"
    backups: 3
    redirect_stderr: false

# ------------------------------------------------------------------------------
# 5. 进程分组定义 (支持按组批量操作)
# ------------------------------------------------------------------------------
groups:
  web-cluster:
    programs:
      - api-server
      - frontend-server
    priority: 80

# ------------------------------------------------------------------------------
# 6. 被托管程序定义
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

    # 二进制更新自动平滑重载
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
# 7. 事件监听器 (原版 Supervisor 事件协议兼容)
# ------------------------------------------------------------------------------
event_listeners:
  memmon:
    command: "python3 -m supervisor.memmon -a 200MB -m admin@example.com"
    events:
      - "TICK_60"
    buffer_size: 10
```

---

### 各配置节详细参数说明

#### 1. 顶层与 `server` 节 (通信与基础设置)

- **`worker_threads`** (*整型*, 默认: `None` / CPU 核心数): Tokio 异步线程池核心线程数。设为 `1` 时进入超轻量单线程模式。
- **`server.uds_path`** (*路径字符串*): 本地 IPC 监听套接字路径。Windows 下原生支持 `AF_UNIX` 文件路径或 `\\.\pipe\name` 命名管道。
- **`server.http_bind`** (*字符串*, 默认: 无): TCP 监听网络地址与端口。提供 REST API 与 Web 控制台。未配置则不开放 TCP 端口。
- **`server.auth_token`** (*字符串*, 可选): REST/XMLRPC/SSE 访问令牌（`Authorization: Bearer <token>` 或 `?token=`）。与 basic 凭据同时配置时二者均可通过（OR 语义），TCP 与 IPC 监听器同时生效。
- **`server.username` / `server.password`** (*字符串*, 可选): HTTP Basic Auth 访问凭据。密码支持 `{SHA}` 前缀哈希值。IPC 使用 `uds_username`/`uds_password`（缺省时从本对自动填充）。Web UI 通过登录弹窗换取 HttpOnly 会话 Cookie，浏览器中不保存任何明文凭据。
- **`server.identifier`** (*字符串*, 可选): 守护节点名称，默认为主机名。
- **`server.path_translation`** (*布尔值*, 默认: `true`): 是否在解析配置时统一将所有相对路径转为绝对路径（以配置文件所在目录为基准）。
- **`server.allow_unelevated`** (*布尔值*, 默认: `false`): 当守护进程以 root 或 Administrator 特权身份运行时，是否允许非特权客户端连接本地 IPC（Unix UDS / Windows 命名管道与 AF_UNIX）。默认关闭以保障安全基线。
- **`server.uds_chmod`** (*字符串* / 别名 `chmod`, 可选): 本地 IPC 端点八进制权限模式（授权层，区别于 `allow_unelevated` 的 peer-credential 鉴权层）。Unix: bind 后 `set_permissions`；Windows 命名管道: 首个实例 `SECURITY_ATTRIBUTES`；Windows 文件型 UDS: `SetNamedSecurityInfoW` 保护性 DACL。未配置默认: `allow_unelevated=false` → `"0700"`，`true` → `"0777"`；显式配置永远优先。接受 `0700` / `0o700` / `700`，解析 fail-fast。

#### 2. `logging` 节 (守护进程自身日志)

- **`logging.enabled`** (*布尔值*, 默认: `true`): 是否输出日志。
- **`logging.file`** (*路径字符串*, 可选): 日志落盘文件路径。省略时输出到控制台。
- **`logging.level`** (*字符串*, 默认: `"info"`): 日志等级（`trace`, `debug`, `info`, `warn`, `error`, `off`）。
- **`logging.max_bytes`** (*字符串*, 默认: `"20MB"`): 日志轮转大小限制。
- **`logging.backups`** (*整型*, 默认: `3`): 历史备份日志保留份数。

#### 3. `metrics` 节 (资源指标监控)

- **`metrics.enabled`** (*布尔值*, 默认: `true`): 是否开启 CPU 与内存 RSS 数据采样。
- **`metrics.idle_timeout_secs`** (*整型*, 默认: `30`): 客户端闲置自动挂起超时时间。设为 `0` 表示不休眠。
- **`metrics.interval_secs`** (*整型*, 默认: `2`): 活跃状态下的采样时间间隔。

#### 4. `programs.<name>` 节 (托管程序详细属性)

- **基础执行控制**：
  - **`command`** (*字符串*, 必须): 启动命令行指令。支持参数分词与路径自动转译。
  - **`args`** (*字符串列表*, 可选): 额外的命令行参数列表。
  - **`directory`** (*路径字符串*, 可选): 子进程运行时的工作目录（CWD）。缺省回退至配置文件目录。
  - **`user`** (*字符串*, 可选，仅 Unix): 执行子进程的系统账户名。
  - **`umask`** (*整型*, 可选，仅 Unix): 子进程文件掩码（如 `022`）。
  - **`environment`** (*键值对映射*, 可选): 子进程独立环境变量注入，支持 `${VAR}` 展开。
  - **`autostart`** (*布尔值*, 默认: `true`): 是否在守护进程启动时自动启动该程序（配置 `cron` 时默认为 `false`）。
  - **`autorestart`** (*枚举*, 默认: `"unexpected"`): 自动重启策略：
    - `"unexpected"`: 仅在进程异常退出（退出码不在 `exit_codes` 中）时重启；
    - `"always"`: 进程退出后无条件重启；
    - `"never"`: 进程退出后绝不重启。
  - **`exit_codes`** (*整型列表*, 默认: `[0]`): 判定为正常退出的返回码集合。
  - **`start_secs`** (*整型*, 默认: `1`): 启动后需持续稳定运行的秒数，达标后状态方转为 `RUNNING`。
  - **`start_retries`** (*整型*, 默认: `3`): 启动失败进入 `BACKOFF` 状态的最大连续重试次数。
  - **`priority`** (*整型 0..99*, 默认: `50`): 启停优先级。数值越小越早启动、越晚停止。
  - **`depends_on`** (*字符串列表*, 可选): 依赖的前置程序列表。系统自动根据依赖拓扑分层按序并行启动。
  - **`group`** (*字符串*, 可选): 所属逻辑分组名。
- **停止与清理**：
  - **`stop_signal`** (*字符串*, Unix 默认 `"TERM"`, Windows 默认 `"CTRL_BREAK"`): 优雅停止信号。支持 `TERM`, `INT`, `QUIT`, `KILL`, `HUP`, `CTRL_C`, `CTRL_BREAK` 等。
  - **`stop_wait_secs`** (*整型*, 默认: `10`): 等待优雅退出的最长超时秒数。超时后触发系统级强制终止。
- **日志管道 (`logs`)**：
  - **`logs.enabled`** (*布尔值*, 默认: `true`): 是否捕获日志。设为 `false` 则以 `Stdio::null()` 启动，无管道损耗。
  - **`logs.stdout` / `logs.stderr`** (*路径字符串*): 目标日志文件路径。设为 `NONE`、`OFF`、`NULL`、`/dev/null` 禁用落盘。
  - **`logs.redirect_stderr`** (*布尔值*, 默认: `false`): 是否将 stderr 汇聚到 stdout 输出。
  - **`logs.max_bytes`** (*字符串*, 默认: `"20MB"`): 日志滚动大小阈值。
  - **`logs.backups`** (*整型*, 默认: `3`): 历史归档保留数。
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
  - **`cron_stop`** (*Cron 表达式*): 强制停机 Cron 规则。
  - **`pre_start`** (*字符串*): 进程启动前执行的前置脚本命令。
  - **`pre_start_ignore_failure`** (*布尔值*, 默认: `false`): 前置脚本报错时是否忽略并继续启动业务程序。
  - **`pre_stop`** (*字符串*): 进程停止前执行的收尾脚本命令（失败时自动平滑降级，确保业务进程一定会被停止）。
  - **`hook_timeout_secs`** (*整型*, 默认: `15`): 钩子脚本最大允许执行的超时时间。
- **多副本集群扩展**：
  - **`numprocs`** (*整型*, 默认: `1`): 进程并行副本启动数。
  - **`numprocs_start`** (*整型*, 默认: `0`): 副本编号起始偏移值。
  - **`process_name`** (*字符串*): 副本名称渲染模板，如 `%(program_name)s_%(process_num)02d`。
- **动态文件监控热重载**：
  - **`restart_when_binary_changed`** (*布尔值*, 默认: `false`): 自动监控可执行文件二进制本身的修改时间/哈希并触发更新。
  - **`restart_directory_monitor`** (*路径字符串*): 动态监控的配置文件或静态资产目录。
  - **`restart_file_pattern`** (*Glob 表达式*, 例如 `"*.json"`): 目录监控文件的匹配通配符。
  - **`restart_signal_when_file_changed`** (*信号*, 例如 `SIGHUP`): 文件变更时发送的重载信号。
  - **`restart_cmd_when_file_changed`** (*命令*): 文件变更时执行的外部重载指令。
  - **`restart_debounce_secs`** (*整型*, 默认: `5`): 文件修改防抖时间窗口（秒）。

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
nodaemon = false

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

## 质量与性能验证

`rsupervisord` 实施严格的跨平台双矩阵自动化验证：

| 验证维度 | 测试目标 | 实测结果 |
| :--- | :--- | :--- |
| **静默 0% CPU 占用** | 无健康探针托管状态下零轮询唤醒 | ✅ 实测 CPU 占用 < 0.01% (Reactor 深度休眠) |
| **Windows 进程树回收** | Win32 Job Objects 回收多层子孙进程 | ✅ 100% 回收，无任何孤儿进程残留 |
| **自动化测试套件** | 单元测试、集成测试与平台契约测试 | ✅ 174/174 测试通过 (100% Pass) |
| **代码规范与质量** | 启用严格编译器警告 (`-D warnings`) | ✅ 0 Clippy 警告 |
| **代码格式标准** | 遵循标准 Rust 格式化规范 | ✅ `cargo fmt --check` 0 差异 |
| **跨平台基线** | Windows 11 MSVC 原生环境与 Linux (Ubuntu 22.04 LTS) | ✅ 双平台矩阵全功能一致性通过 |

---

## 开源许可证

本项目基于 **[Mozilla Public License 2.0 (MPL-2.0)](LICENSE)** 协议开源。
欢迎提交 Issue 与 Pull Request！
