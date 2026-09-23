# rsupervisord

[![License: MPL 2.0](https://img.shields.io/badge/License-MPL_2.0-brightgreen.svg)](LICENSE)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](https://www.rust-lang.org)
[![Platform: Linux | Windows | macOS](https://img.shields.io/badge/Platform-Linux%20%7C%20Windows%20%7C%20macOS-lightgrey.svg)]
[![Tests](https://img.shields.io/badge/Tests-174%2F174%20Passing-brightgreen.svg)]

**rsupervisord** is a modern, high-performance, cross-platform process orchestration and monitoring daemon engine written in Rust.

[English](README.md) | [简体中文](README_zh.md)

---

## Table of Contents

- [Background & Motivation](#background--motivation)
- [Key Features](#key-features)
- [System Architecture](#system-architecture)
- [Quick Start](#quick-start)
  - [1. Build from Source](#1-build-from-source)
  - [2. Configuration Search Order & Default Paths](#2-configuration-search-order--default-paths)
  - [3. Running the Daemon](#3-running-the-daemon)
- [System Service Management (Windows Service & Linux Systemd)](#system-service-management-windows-service--linux-systemd)
  - [Windows Service Management (Native SCM)](#windows-service-management-native-scm)
  - [Linux Systemd Service Management](#linux-systemd-service-management)
- [Command-Line Options & Usage Examples](#command-line-options--usage-examples)
  - [Supervisord Daemon (`supervisord`)](#supervisord-daemon-supervisord)
  - [CLI Controller (`supervisorctl`)](#cli-controller-supervisorctl)
- [Complete YAML Configuration Reference](#complete-yaml-configuration-reference)
  - [Full Production Configuration Example (`supervisord.yaml`)](#full-production-configuration-example-supervisordyaml)
  - [Detailed Parameter Reference](#detailed-parameter-reference)
- [Legacy INI Configuration Example](#legacy-ini-configuration-example)
  - [Basic INI Example (`supervisord.conf`)](#basic-ini-example-supervisordconf)
- [Embedded Web Dashboard](#embedded-web-dashboard)
- [Quality & Performance Verification](#quality--performance-verification)
- [License](#license)

---

## Background & Motivation

In containerized environments, microservice deployments, edge devices, and Windows host environments, robust process management tools are essential. For a long time, the industry has relied on existing solutions, but they exhibit significant limitations:

1. **Python Supervisor Distribution Burden & Platform Limitations**:
   - The original [Supervisor (Python)](http://supervisord.org/) depends on a full Python runtime environment, `setuptools`, and multiple third-party libraries. Distributing, cross-compiling, and packaging it into self-contained container images or standalone packages is cumbersome.
   - It lacks native Windows support, preventing direct production adoption on Windows hosts.
2. **Go-based `supervisord` Incompleteness & Windows Platform Defects**:
   - Community Go implementations (`supervisord`) provide standalone binaries, but their feature sets remain incomplete and suffer from severe defects on Windows.
   - **Process Tree Leaks**: They lack system-level process tree lifecycle control and rely on simulated signals or external `taskkill.exe` invocations. When the daemon stops or restarts a service, background child/grandchild processes frequently leak as orphaned processes, leaving ports occupied.
   - **High Idle CPU Consumption**: Even in silent monitoring states without probes or alarms, continuous timer polling and runtime garbage collection incur unnecessary CPU and memory overhead.
   - **Path & Command Distortions**: Edge cases in Windows command parsing, relative path resolution, and batch file execution often lead to runtime lookup failures.

**The Mission of `rsupervisord`**:
Built on a modern Rust async stack (Tokio + native OS kernel notifications), `rsupervisord` delivers a **single static binary, zero external runtime dependencies, true 0% CPU idle overhead, and guaranteed process tree reclamation** across both Unix and Windows.

---

## Key Features

### 1. Native Windows Support: Replacing winsw and NSSM
- **Built-in Windows Service (SCM) Driver**: No need to wrap applications using third-party wrappers like `winsw` or `NSSM`. Run `supervisord service install` to register `rsupervisord` directly as an auto-starting Windows Service.
- **Win32 Job Objects Guarantee 100% Process Tree Reclamation**: Every managed process is bound to a native Win32 `Job Object` configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Whether dealing with complex Node.js, Python, or Go process trees, or `.bat` / `.cmd` scripts spawning descendant tasks, the Windows kernel automatically terminates all descendants upon stopping or restarting—without relying on `taskkill.exe`.
- **Multi-Process DAG Orchestration vs. Single-Service Wrappers**: While `winsw` and `NSSM` manage only a single binary per service, `rsupervisord` unifies dozens of services under Directed Acyclic Graph (DAG) dependency startup, health probes, scheduled cron jobs, hot-reloads, and a visual dashboard.

### 2. True Silent Zero Polling (0% CPU at Idle)
- **Event-Driven Kernel Notifications**:
  - **Windows**: Win32 kernel handle event notifications via `RegisterWaitForSingleObject`.
  - **Linux / Unix**: Linux `pidfd` and POSIX signal pipelines.
- **On-Demand Uptime Calculation**: Calculates uptime on demand from `started_at`, eliminating legacy 2-second background tick loops. Processes without active health probes sleep in the Tokio reactor with **zero periodic timer wakeups and strictly 0.00% idle CPU overhead**.

### 3. Linux / Unix System-Level Reliability
- Isolated process groups (`setpgid`) and group signal dispatching (`killpg`), with automated systemd unit generation and lifecycle management.
- Strict RAII resource lifecycles powered by `tokio_util::sync::DropGuard` and `scopeguard`, eliminating socket and file descriptor leaks.

### 4. Zero-Downtime Hot Reload
- 3-Way configuration diffing: during configuration reloads, **unchanged processes keep running with the same PID and uninterrupted TCP connections**. Only modified, added, or removed programs are safely transitioned.

### 5. Single-Binary Embedded Web Dashboard
- Single binary embeds a lightweight Vue 3 Single Page Application (SPA) with **zero external NPM, Node.js, or CDN dependencies**.
- Features real-time SSE live log streaming (with auto-scroll locking), batch multi-select operations, health indicators, CPU/RSS resource graphs, and configuration diff modals.

### 6. Parse-Boundary Path Translation
- Zero-Diffusion Principle: relative paths are absolutized against `config_dir` at the parse boundary, eliminating discrepancies between child process CWD and executable path resolution across platforms.
- Windows transparently routes `.bat` and `.cmd` scripts via `cmd.exe /C` and strips `\\?\` UNC prefixes for seamless compatibility.

### 7. Dual-Protocol IPC & Ultra-Lightweight Single-Thread Mode
- Supports cross-platform Unix Domain Sockets (Windows 10/11 native `AF_UNIX` and named pipes `\\.\pipe\`) and authenticated HTTP REST APIs.
- Configurable single-threaded mode (`worker_threads: 1` or `--worker-threads 1`) reduces memory footprint to **2~4MB**, ideal for edge devices and resource-constrained environments.

---

## System Architecture

```text
               +-------------------------------------------------------+
               |   CLI: supervisorctl   |   Web Dashboard / REST API   |
               +-------------------------------------------------------+
                                           |
                                [AF_UNIX / Named Pipe / HTTP]
                                           |
               +-------------------------------------------------------+
               |             Axum API Engine & Embedded Vue 3 SPA      |
               +-------------------------------------------------------+
                                           | (MPSC Async Channels)
                                           v
               +-------------------------------------------------------+
               |        SupervisorManager (DAG Engine & Event Hub)     |
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

## Quick Start

### 1. Build from Source

Requirements: **Rust 1.85+ (Edition 2024)**.

```bash
# Clone repository
git clone https://github.com/spritetong/rsupervisord.git
cd rsupervisord

# Build release binaries (LTO optimized, stripped)
cargo build --release

# Output binaries located at:
#   target/release/supervisord      (Daemon engine)
#   target/release/supervisorctl    (CLI client)
```

> [!TIP]
> `supervisord` and `supervisorctl` support unified binary dispatch: if the executable name ends with `ctl` (via symlink or hardlink), or is invoked as `supervisord ctl <command>`, it automatically switches to CLI controller mode.

---

### 2. Configuration Search Order & Default Paths

When `-c / --config` is not explicitly provided, configuration files are discovered in the following order:

1. Environment variable: `<UPPERCASE_CMD_NAME>_CONFIG` (e.g. `SUPERVISORD_CONFIG`, `MYD_CONFIG`).
2. Executable parent directory: `<executable_dir>/<cmd_name>.yaml`.
3. Executable dedicated subdirectory: `<executable_dir>/<cmd_name>/<cmd_name>.yaml`.
4. Subdirectory generic config: `<executable_dir>/<cmd_name>/config.yaml`.
5. Current Working Directory (CWD): `<CWD>/<cmd_name>.yaml` or `<CWD>/config.yaml`.
6. User configuration directory (XDG / Windows AppData):
   - **Linux / macOS**: `~/.config/<cmd_name>/config.yaml`.
   - **Windows**: `%APPDATA%/<cmd_name>/config.yaml`.
7. System global directory:
   - **Linux**: `/etc/<cmd_name>/config.yaml`.

**Default Runtime Paths**:
- **IPC Socket**:
  - **Linux / Unix**: `/var/run/<cmd_name>.sock`.
  - **Windows**: `<config_dir>/<cmd_name>.sock` or named pipe `\\.\pipe\<cmd_name>`.
- **Daemon Self-Log** (when `logging.file` is omitted):
  - **Linux / Unix**: `/var/log/<cmd_name>/<cmd_name>.log`.
  - **Windows**: `<config_dir>/logs/<cmd_name>.log`.

---

### 3. Running the Daemon

```bash
# 1. Start daemon with automatic configuration discovery
./target/release/supervisord

# 2. Start daemon with explicit configuration file
./target/release/supervisord -c /etc/supervisord/supervisord.yaml

# 3. Start in ultra-lightweight single-thread mode (minimal memory consumption)
./target/release/supervisord -c config.yaml --worker-threads 1
```

---

## System Service Management (Windows Service & Linux Systemd)

`rsupervisord` provides a unified `service` subcommand integrated directly into operating system service managers. Both `supervisord` and `supervisorctl` support this command.

### Windows Service Management (Native SCM)

Run PowerShell or Command Prompt as **Administrator**:

```powershell
# Install as an auto-starting Windows Service (uses discovered config if -c is omitted)
supervisord.exe service install

# Install with an explicit configuration file path
supervisord.exe service install -c C:\rsupervisord\supervisord.yaml

# Start the Windows Service
supervisord.exe service start

# Stop the Windows Service (cleanly reclaims all descendant process trees)
supervisord.exe service stop

# Restart the Windows Service
supervisord.exe service restart

# Uninstall the Windows Service
supervisord.exe service uninstall
```

> [!NOTE]
> When executing as a registered Windows Service, the Windows Service Control Manager (SCM) automatically invokes the binary with `--service`. Upon receiving stop requests from SCM, `rsupervisord` coordinates graceful cancellation before reporting stopped state.

### Linux Systemd Service Management

On Linux systems, run with `sudo`:

```bash
# Install and enable systemd service (/etc/systemd/system/supervisord.service)
sudo supervisord service install -c /etc/supervisord/supervisord.yaml

# Manage service lifecycle via built-in subcommands
sudo supervisord service start
sudo supervisord service stop
sudo supervisord service restart
sudo supervisord service uninstall

# Or manage directly via native systemctl
sudo systemctl status supervisord
sudo systemctl restart supervisord
```

---

## Command-Line Options & Usage Examples

### Supervisord Daemon (`supervisord`)

#### Parameter Reference

| Option / Subcommand | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--config <PATH>` | `-c` | Auto-detect | Path to configuration file (`.yaml`, `.yml`, `.conf`, `.ini`) |
| `--loglevel <LEVEL>` | `-l` | `info` | Log verbosity level: `trace`, `debug`, `info`, `warn`, `error`, `off` |
| `--nodaemon` | `-n` | `true` | Run in foreground (default in current version) |
| `--worker-threads <N>` | - | CPU cores | Number of Tokio worker threads (`1` enables single-threaded runtime) |
| `--service` | - | `false` | System service mode (invoked by Windows SCM or service dispatchers) |
| `--allow-unelevated` | - | `false` | When daemon is elevated, permits non-elevated clients to connect to local IPC |
| `service <OP>` | - | - | System service management: `install`, `uninstall`, `start`, `stop`, `restart` |
| `ctl <CMD...>` | - | - | Invoke CLI controller commands directly (equivalent to `supervisorctl <CMD...>`) |

#### Usage Examples

```bash
# Start in debug mode with verbose logging
supervisord -c config.yaml -l debug

# Run in single-threaded mode on resource-constrained devices
supervisord -c config.yaml --worker-threads 1

# Allow non-elevated callers to connect to elevated daemon
supervisord -c config.yaml --allow-unelevated
```

---

### CLI Controller (`supervisorctl`)

#### Global Options

| Option | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--server <URL>` | `-s` | Inferred | Target endpoint (`unix:///path/to.sock`, named pipe, or `http://127.0.0.1:9001`) |
| `--config <PATH>` | `-c` | Auto-detect | Path to configuration file (used to read socket, HTTP bind, and credentials) |
| `--key <TOKEN>` | `-k` | Empty | Bearer Token authentication key for HTTP REST API |
| `--user <USER>` | `-u` | Empty | HTTP Basic Auth username |
| `--password <PASS>` | `-p` | Empty | HTTP Basic Auth password |

#### Subcommands and Practical Examples

```bash
# 1. Inspect status (PID, uptime, state, CPU, and RSS memory)
supervisorctl status
supervisorctl status api-server redis

# 2. Process lifecycle controls (synchronous waiting vs asynchronous triggering)
supervisorctl start api-server                 # Synchronous start (waits up to 30s for verification)
supervisorctl stop api-server                  # Synchronous graceful stop
supervisorctl restart api-server               # Synchronous restart
supervisorctl start all                        # Start all managed programs
supervisorctl restart web:*                    # Batch restart all programs in the 'web' group
supervisorctl start api-server --async         # Fire-and-forget asynchronous start

# 3. Zero-downtime hot reload and configuration updates
supervisorctl config reload                    # [Recommended] Zero-downtime reload; unchanged programs keep running
supervisorctl reread                           # Reread config and display diff (does not touch processes)
supervisorctl update                           # Apply configuration diff (add/remove/restart changed programs)
supervisorctl reload                           # Legacy behavior: gracefully stops all programs and restarts daemon

# 4. Log tailing and buffer management
supervisorctl tail -f api-server               # Follow live stdout stream
supervisorctl tail -f -n 100 api-server stderr # View last 100 stderr lines and follow
supervisorctl maintail -f                      # Follow daemon internal log stream
supervisorctl clear api-server                 # Clear program log files and in-memory ring buffers

# 5. Process signals and interaction
supervisorctl pid api-server                   # Retrieve process PID
supervisorctl signal HUP api-server            # Send POSIX signal (e.g. HUP, TERM, KILL, INT)
supervisorctl stdin api-server "reload\n"      # Send input characters to program stdin
supervisorctl events                           # Stream real-time state and lifecycle events

# 6. Shut down daemon
supervisorctl shutdown                         # Gracefully shut down the supervisord daemon
```

---

## Complete YAML Configuration Reference

### Full Production Configuration Example (`supervisord.yaml`)

```yaml
# ==============================================================================
# rsupervisord Production Reference Configuration (supervisord.yaml)
# Supports environment variable expansion: ${VAR} or ${VAR:-default_value}
# ==============================================================================

# Number of Tokio async runtime worker threads. Defaults to CPU cores if omitted.
# Set to 1 to enable ultra-lightweight single-threaded mode (2~4MB memory baseline).
worker_threads: 2

# ------------------------------------------------------------------------------
# 1. Daemon Server & HTTP API Settings
# ------------------------------------------------------------------------------
server:
  # Local IPC socket path (Unix Domain Socket on Unix, Named Pipe / UDS on Windows)
  # Linux default: /var/run/supervisord.sock
  # Windows default: <config_dir>/supervisord.sock or \\.\pipe\supervisord
  uds_path: "/var/run/supervisord.sock"

  # Optional TCP HTTP listener for REST API and embedded Web dashboard
  # Formats: "127.0.0.1:9001", "0.0.0.0:9001", ":9001", "9001"
  http_bind: "127.0.0.1:9001"

  # Optional Bearer Token for HTTP REST API authentication
  auth_token: "${SUPERVISORD_TOKEN:-}"

  # Optional HTTP Basic Authentication (Supervisor compatible)
  # Passwords support cleartext or SHA-1 hashes prefixed with {SHA}
  username: "admin"
  password: "{SHA}82ab876d1387bfafe46cc1c8a2ef074eae50cb1d"

  # Optional server node identifier (defaults to system hostname)
  identifier: "supervisor-node-01"

  # Parse-boundary path translation (default: true)
  # When true, relative paths in config fields are absolutized against the config file directory.
  # When false, relative paths are resolved relative to the process runtime CWD (Python behavior).
  path_translation: true

  # When daemon runs elevated (root/Administrator), permits non-elevated callers on local IPC (default: false)
  allow_unelevated: false

  # Optional IPC endpoint permission mode (alias: chmod). Quoted octal string.
  # Unix socket: applied via set_permissions after bind. Windows named pipe:
  # baked into first-instance SECURITY_ATTRIBUTES. Windows file-based UDS:
  # applied via SetNamedSecurityInfoW (protected DACL).
  # Defaults when omitted: "0700" if allow_unelevated=false, "0777" if true.
  # An explicit value always wins. Parsed fail-fast (0700 / 0o700 / 700).
  # uds_chmod: "0700"

# ------------------------------------------------------------------------------
# 2. Daemon Logging Settings
# ------------------------------------------------------------------------------
logging:
  # Enable internal daemon logging (default: true)
  enabled: true

  # Daemon log file destination. If omitted, logs to stderr/console
  file: "logs/supervisord.log"

  # Log verbosity level: trace, debug, info, warn, error, off (default: info)
  level: "info"

  # Maximum size before log file rotation (supports B, KB, MB, GB; default: 20MB)
  max_bytes: "20MB"

  # Number of rotated historical log backups to retain (default: 3)
  backups: 5

# ------------------------------------------------------------------------------
# 3. Resource Utilization Metrics (CPU & RSS Memory)
# ------------------------------------------------------------------------------
metrics:
  # Enable resource utilization metrics collection (default: true)
  enabled: true

  # Idle timeout in seconds before pausing metrics sampling when no clients are connected (default: 30)
  # Pauses sampling during inactivity to maintain true 0% idle CPU overhead. Set to 0 to disable sleep.
  idle_timeout_secs: 30

  # Active sampling interval in seconds when clients are connected (default: 2)
  interval_secs: 2

# ------------------------------------------------------------------------------
# 4. Global Program Defaults Template (inherited by all programs)
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
# 5. Process Groups (batch control via <group>:*)
# ------------------------------------------------------------------------------
groups:
  web-cluster:
    programs:
      - api-server
      - frontend-server
    priority: 80

# ------------------------------------------------------------------------------
# 6. Managed Programs
# ------------------------------------------------------------------------------
programs:
  # Infrastructure service example: Redis database
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

  # Core API service: depends on database, environment expansion, HTTP probe
  api-server:
    command: "./bin/api-server"
    args:
      - "--port"
      - "8080"
    directory: "./services/api"
    priority: 20
    depends_on:
      - "database"

    # Unix-only user execution context (ignored on Windows)
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

    # Active HTTP health probe
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

  # Scheduled cron task and lifecycle hooks example
  nightly-backup:
    command: "python3 backup.py --full"
    autostart: false
    cron: "0 2 * * *"              # Trigger start daily at 02:00
    cron_stop: "0 4 * * *"         # Automatically stop if still running at 04:00

    # Lifecycle hooks
    pre_start: "sh -c 'echo Preparing backup snapshot...'"
    pre_start_ignore_failure: false # If pre-start hook fails, abort process start
    pre_stop: "sh -c 'echo Cleaning temporary files...'"
    hook_timeout_secs: 15

    logs:
      stdout: "logs/backup.log"
      redirect_stderr: true

  # File and binary watcher example (auto-reload on changes)
  gateway:
    command: "./bin/gateway"
    priority: 30
    autostart: true

    # Automatically reload when binary executable changes
    restart_when_binary_changed: true
    restart_signal_when_binary_changed: SIGHUP

    # Monitor configuration directory for changes
    restart_directory_monitor: "./config"
    restart_file_pattern: "*.json"
    restart_signal_when_file_changed: SIGHUP
    restart_debounce_secs: 5

    logs:
      stdout: "logs/gateway.log"
      redirect_stderr: true

# ------------------------------------------------------------------------------
# 7. Event Listeners (Supervisor event protocol compatibility)
# ------------------------------------------------------------------------------
event_listeners:
  memmon:
    command: "python3 -m supervisor.memmon -a 200MB -m admin@example.com"
    events:
      - "TICK_60"
    buffer_size: 10
```

---

### Detailed Parameter Reference

#### 1. Top-Level & `server` Section
- **`worker_threads`** (*integer*, default: `None` / CPU cores): Number of worker threads for Tokio async runtime. Setting to `1` activates lightweight single-threaded `current_thread` mode.
- **`server.uds_path`** (*path string*): Path for local IPC listening socket. Supports `AF_UNIX` file paths or Windows Named Pipes (`\\.\pipe\name`).
- **`server.http_bind`** (*string*, optional): TCP bind address and port. Serves REST API and Web Dashboard. TCP listening is disabled if omitted.
- **`server.auth_token`** (*string*, optional): Bearer token for HTTP REST/XMLRPC/SSE (`Authorization: Bearer <token>` or `?token=`). When basic credentials are also configured, **either** may be used (OR). Applied on both TCP and IPC listeners.
- **`server.username` / `server.password`** (*string*, optional): HTTP Basic Auth credentials. Passwords support cleartext or `{SHA}` hashed format. IPC uses `uds_username`/`uds_password` (auto-filled from this pair when omitted). The Web UI signs in via a login modal and keeps an HttpOnly session cookie — secrets are never stored in the browser.
- **`server.identifier`** (*string*, optional): Node identifier, defaults to system hostname.
- **`server.path_translation`** (*boolean*, default: `true`): When true, relative paths in config fields are absolutized against `config_dir` at the parse boundary.
- **`server.allow_unelevated`** (*boolean*, default: `false`): When daemon runs with root or Administrator privileges, permits non-elevated callers to connect via local IPC.
- **`server.uds_chmod`** (*string* / alias `chmod`, optional): Octal IPC endpoint mode (authorization layer, distinct from `allow_unelevated` peer-credential checks). Unix: `set_permissions` after bind. Windows pipe: first-instance `SECURITY_ATTRIBUTES`. Windows file UDS: protected DACL via `SetNamedSecurityInfoW`. Defaults when omitted: `"0700"` if `allow_unelevated=false`, `"0777"` if `true`; an explicit value always wins. Accepts `0700` / `0o700` / `700`, validated fail-fast.

#### 2. `logging` Section (Daemon Logging)
- **`logging.enabled`** (*boolean*, default: `true`): Enable or disable internal daemon logging.
- **`logging.file`** (*path string*, optional): Path to daemon log file. Logs to stderr/console if omitted.
- **`logging.level`** (*string*, default: `"info"`): Log verbosity level (`trace`, `debug`, `info`, `warn`, `error`, `off`).
- **`logging.max_bytes`** (*string*, default: `"20MB"`): Maximum file size before rotation.
- **`logging.backups`** (*integer*, default: `3`): Number of historical backup files to retain.

#### 3. `metrics` Section (Resource Monitoring)
- **`metrics.enabled`** (*boolean*, default: `true`): Enable CPU and RSS memory metrics collection.
- **`metrics.idle_timeout_secs`** (*integer*, default: `30`): Inactivity timeout before pausing metrics sampling. Set to `0` to keep sampling active continuously.
- **`metrics.interval_secs`** (*integer*, default: `2`): Metrics sampling interval during active client connections.

#### 4. `programs.<name>` Section (Managed Program Attributes)
- **Execution & Process Controls**:
  - **`command`** (*string*, required): Executable command line. Supports argument splitting and parse-boundary path translation.
  - **`args`** (*list of strings*, optional): Extra argument tokens passed to the executable.
  - **`directory`** (*path string*, optional): Working directory (CWD) for the child process. Falls back to `config_dir`.
  - **`user`** (*string*, optional, Unix only): Unprivileged system user to execute the child process.
  - **`umask`** (*integer*, optional, Unix only): File mode creation mask (e.g. `022`).
  - **`environment`** (*key-value map*, optional): Environment variables injected into child process, supports `${VAR}` expansion.
  - **`autostart`** (*boolean*, default: `true`): Automatically start program on daemon launch (defaults to `false` when `cron` is specified).
  - **`autorestart`** (*enum*, default: `"unexpected"`): Auto-restart policy:
    - `"unexpected"`: Restarts only if exit code is not present in `exit_codes`;
    - `"always"`: Restarts unconditionally whenever the process exits;
    - `"never"`: Never restarts automatically.
  - **`exit_codes`** (*list of integers*, default: `[0]`): Exit codes considered successful/normal termination.
  - **`start_secs`** (*integer*, default: `1`): Minimum runtime in seconds before a process is considered in `RUNNING` state.
  - **`start_retries`** (*integer*, default: `3`): Maximum consecutive restart attempts before entering `FATAL` state.
  - **`priority`** (*integer 0..99*, default: `50`): Startup/shutdown priority. Lower numbers start earlier and stop later.
  - **`depends_on`** (*list of strings*, optional): Dependent programs. The DAG engine performs topological sorting and layered parallel startup.
  - **`group`** (*string*, optional): Logical group classification.
- **Stop & Signal Controls**:
  - **`stop_signal`** (*string*, default: `"TERM"` on Unix, `"CTRL_BREAK"` on Windows): Signal sent for graceful termination (`TERM`, `INT`, `QUIT`, `KILL`, `HUP`, `CTRL_C`, `CTRL_BREAK`).
  - **`stop_wait_secs`** (*integer*, default: `10`): Grace period before force-killing via OS primitives (Job Object close on Windows, `SIGKILL` on Linux).
- **Log Pipelines (`logs`)**:
  - **`logs.enabled`** (*boolean*, default: `true`): Enable log capturing. Set to `false` to spawn with `Stdio::null()`, bypassing pipe overhead.
  - **`logs.stdout` / `logs.stderr`** (*path string*): Output log destination. Set to `NONE`, `OFF`, `NULL`, or `/dev/null` to disable file writes.
  - **`logs.redirect_stderr`** (*boolean*, default: `false`): Redirect stderr into the stdout stream.
  - **`logs.max_bytes`** (*string*, default: `"20MB"`): Size threshold for log file rotation.
  - **`logs.backups`** (*integer*, default: `3`): Number of historical rotated log files to retain.
- **Active Health Probes (`health_check`)**:
  - **`health_check.type`** (*enum*): Probe type: `"http"`, `"tcp"`, or `"exec"`.
  - **`health_check.url`** (*URL*): HTTP probe URL.
  - **`health_check.expected_status`** (*integer*, default: `200`): Expected HTTP status code.
  - **`health_check.endpoint`** (*string*): TCP probe host and port (e.g. `"127.0.0.1:3306"`).
  - **`health_check.command`** (*string*): Exec probe shell command (exit code 0 indicates healthy).
  - **`health_check.interval_secs`** (*integer*, default: `10`): Probe check interval.
  - **`health_check.timeout_secs`** (*integer*, default: `2`): Single probe execution timeout.
  - **`health_check.failure_threshold`** (*integer*, default: `3`): Consecutive failures before triggering process restart.
  - **`health_check.initial_delay_secs`** (*integer*, default: `0`): Quiet period after process enters `RUNNING` before first probe.
- **Scheduling & Lifecycle Hooks**:
  - **`cron`** (*cron expression*): Cron expression to trigger start (e.g. `"0 2 * * *"`).
  - **`cron_stop`** (*cron expression*): Cron expression to stop process.
  - **`pre_start`** (*string*): Command executed before child process is spawned.
  - **`pre_start_ignore_failure`** (*boolean*, default: `false`): When true, failure of pre-start command does not block process startup.
  - **`pre_stop`** (*string*): Command executed before stopping child process (degrades gracefully on failure to guarantee termination).
  - **`hook_timeout_secs`** (*integer*, default: `15`): Maximum duration allowed for hook execution.
- **Multi-Process Pools**:
  - **`numprocs`** (*integer*, default: `1`): Number of process instances to launch.
  - **`numprocs_start`** (*integer*, default: `0`): Starting index offset for instance numbering.
  - **`process_name`** (*string*): Name formatting expression, e.g. `%(program_name)s_%(process_num)02d`.
- **Dynamic File & Binary Monitoring**:
  - **`restart_when_binary_changed`** (*boolean*, default: `false`): Watch binary executable modification and trigger reload.
  - **`restart_directory_monitor`** (*path string*): Monitored directory for configuration or asset files.
  - **`restart_file_pattern`** (*glob string*, e.g. `"*.json"`): Wildcard pattern for monitored files.
  - **`restart_signal_when_file_changed`** (*signal*, e.g. `SIGHUP`): Signal sent when matching files change.
  - **`restart_cmd_when_file_changed`** (*command*): Custom command executed upon file change.
  - **`restart_debounce_secs`** (*integer*, default: `5`): Settling debounce window in seconds.

---

## Legacy INI Configuration Example

`rsupervisord` natively parses legacy Supervisor INI / Conf syntax without conversion.

### Basic INI Example (`supervisord.conf`)

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
> For complete documentation and syntax specifications of the legacy INI format, please consult the official documentation:
> 📖 **[Supervisor Configuration File Documentation](http://supervisord.org/configuration.html)**

---

## Embedded Web Dashboard

When `server.http_bind` is configured (e.g. `127.0.0.1:9001`), `rsupervisord` serves an embedded Web Dashboard. Access it in your browser at:

```text
http://127.0.0.1:9001/
```

- **Live Status Matrix**: Real-time program states, PID, continuous uptime, CPU percentage, physical memory (RSS), and health check probe results.
- **Interactive Batch Operations**: Checkbox-driven multi-selection for bulk starting, stopping, and restarting.
- **Real-Time SSE Live Log Drawer**: Sliding terminal drawer supporting follow mode, historical line buffering, and autoscroll locking.
- **Visual Configuration Diff Modal**: Review added, modified, and removed programs before confirming configuration reloads.

---

## Quality & Performance Verification

`rsupervisord` enforces a continuous dual-platform automated verification matrix:

| Verification Dimension | Target | Actual Test Result |
| :--- | :--- | :--- |
| **0% Silent Idle CPU** | Zero polling wakeups in silent monitoring | ✅ CPU usage < 0.01% (Deep Reactor sleep) |
| **Windows Process Tree Reclamation** | Win32 Job Objects reclaim multi-tier process trees | ✅ 100% Reclaimed, zero orphan leaks |
| **Automated Test Suite** | Unit tests, integration tests, and platform contracts | ✅ 174/174 Tests Passed (100% Pass) |
| **Code Quality & Lints** | Strict compiler warnings (`-D warnings`) | ✅ 0 Clippy warnings |
| **Code Formatting** | Standard Rust formatting | ✅ `cargo fmt --check` 0 diffs |
| **Cross-Platform Parity** | Windows 11 MSVC & Linux (Ubuntu 22.04 LTS) | ✅ 100% Dual-platform matrix pass |

---

## License

This project is licensed under the **[Mozilla Public License 2.0 (MPL-2.0)](LICENSE)**.
Pull requests and issues are welcome!
