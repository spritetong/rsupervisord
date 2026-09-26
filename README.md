# rsupervisord

[![License: MPL 2.0](https://img.shields.io/badge/License-MPL_2.0-brightgreen.svg)](LICENSE)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](https://www.rust-lang.org)
[![Platform: Linux | Windows | macOS](https://img.shields.io/badge/Platform-Linux%20%7C%20Windows%20%7C%20macOS-lightgrey.svg)]

**rsupervisord** is a cross-platform process orchestration and monitoring daemon. It runs as a single binary on Linux, Windows, and macOS.

[English](README.md) | [简体中文](README_zh.md)

---

## Table of Contents

- [rsupervisord](#rsupervisord)
  - [Table of Contents](#table-of-contents)
  - [Background \& Motivation](#background--motivation)
  - [Key Features](#key-features)
    - [1. Windows Service Support](#1-windows-service-support)
    - [2. No Polling at Idle](#2-no-polling-at-idle)
    - [3. Linux / Unix Process Control](#3-linux--unix-process-control)
    - [4. Configuration Reload](#4-configuration-reload)
    - [5. Embedded Web Dashboard](#5-embedded-web-dashboard)
    - [6. Path and Command Handling](#6-path-and-command-handling)
    - [7. Local Communication and Runtime Modes](#7-local-communication-and-runtime-modes)
  - [Quick Start](#quick-start)
    - [1. Build from Source](#1-build-from-source)
    - [2. Configuration Search Order \& Default Paths](#2-configuration-search-order--default-paths)
    - [3. Running the Daemon](#3-running-the-daemon)
  - [System Service Management (Windows Service \& Linux Systemd)](#system-service-management-windows-service--linux-systemd)
    - [Windows Service Management (Native SCM)](#windows-service-management-native-scm)
    - [Linux Systemd Service Management](#linux-systemd-service-management)
  - [Command-Line Options \& Usage Examples](#command-line-options--usage-examples)
    - [Supervisord Daemon (`supervisord`)](#supervisord-daemon-supervisord)
      - [Parameter Reference](#parameter-reference)
      - [Usage Examples](#usage-examples)
    - [CLI Controller (`supervisorctl`)](#cli-controller-supervisorctl)
      - [Global Options](#global-options)
      - [Subcommands and Practical Examples](#subcommands-and-practical-examples)
  - [Complete YAML Configuration Reference](#complete-yaml-configuration-reference)
    - [Full Configuration Example (`supervisord.yaml`)](#full-configuration-example-supervisordyaml)
    - [Detailed Parameter Reference](#detailed-parameter-reference)
      - [1. Top-Level \& `server` Section](#1-top-level--server-section)
      - [2. `ctl` Section (supervisorctl Client Connection Defaults)](#2-ctl-section-supervisorctl-client-connection-defaults)
      - [3. `logging` Section (Daemon Logging)](#3-logging-section-daemon-logging)
      - [4. `metrics` Section (Resource Monitoring)](#4-metrics-section-resource-monitoring)
      - [5. `program_defaults` Section (Global Program Defaults)](#5-program_defaults-section-global-program-defaults)
      - [6. `groups` Section (Process Groups)](#6-groups-section-process-groups)
      - [7. `programs.<name>` Section (Managed Program Attributes)](#7-programsname-section-managed-program-attributes)
      - [8. `event_listeners.<name>` Section (Event Listeners)](#8-event_listenersname-section-event-listeners)
  - [Legacy INI Configuration Example](#legacy-ini-configuration-example)
    - [Basic INI Example (`supervisord.conf`)](#basic-ini-example-supervisordconf)
  - [Embedded Web Dashboard](#embedded-web-dashboard)
  - [License](#license)

---

## Background & Motivation

In containerized environments, microservice deployments, edge devices, and Windows hosts, a process management tool is required. The existing solutions have the following limitations:

1. **Python Supervisor**:
   - [Supervisor (Python)](http://supervisord.org/) depends on a full Python runtime, `setuptools`, and several third-party libraries, so packaging it into a self-contained image or a standalone package takes extra work.
   - It does not run on Windows.
2. **Go-based `supervisord`**:
   - The feature set is incomplete.
   - On Windows it stops processes by invoking `taskkill.exe`, so child and grandchild processes are often left behind as orphans and keep ports occupied after a service stops or restarts.

`rsupervisord` is written in Rust and ships as a single binary with no runtime dependency. It manages process start order, health checks, scheduled tasks, log collection, and configuration reloads on Linux, Windows, and macOS.

---

## Key Features

### 1. Windows Service Support

- `supervisord service install` registers `rsupervisord` as an auto-starting Windows Service; `start`, `stop`, `restart`, and `uninstall` manage it from the command line.
- Stopping or restarting a service terminates the whole process tree of the managed program, including descendants spawned by `.bat` / `.cmd` scripts, without calling `taskkill.exe`.
- Compared with single-service wrappers such as `winsw` and `NSSM`, one daemon hosts many programs, with dependency-ordered startup, health checks, cron jobs, file watching, and a dashboard.

### 2. No Polling at Idle

- Process exits are reported by the operating system, so running processes are not polled on a timer.
- A program without an active health probe causes no periodic wakeup while idle.

### 3. Linux / Unix Process Control

- Each program runs in its own process group, and stop signals are delivered to the whole group.
- `supervisord service install` generates and manages a systemd unit for the daemon.

### 4. Configuration Reload

- On reload, programs whose configuration did not change keep running with the same PID and open connections; only added, changed, or removed programs are started, restarted, or stopped.

### 5. Embedded Web Dashboard

- The dashboard is built into the binary; Node.js, NPM, and CDN access are not required.
- Real-time SSE log streaming with autoscroll locking, multi-select batch start/stop/restart, health indicators, CPU/RSS graphs, and a configuration diff view.

### 6. Path and Command Handling

- Relative paths in the configuration are resolved against the directory of the configuration file, so the daemon working directory does not change the result.
- On Windows, `.bat` and `.cmd` scripts run through `cmd.exe`, and long-path prefixes (`\\?\`) are stripped from paths.

### 7. Local Communication and Runtime Modes

- Local IPC over a Unix Domain Socket (file path on Unix, `AF_UNIX` file path or named pipe `\\.\pipe\` on Windows) or over an authenticated HTTP REST API.
- Single-threaded mode (`worker_threads: 1` or `--worker-threads 1`) for edge devices and other resource-constrained environments.

---

## Quick Start

### 1. Build from Source

Requirements: **Rust 1.85+ (Edition 2024)**.

```bash
# Clone repository
git clone https://github.com/spritetong/rsupervisord.git
cd rsupervisord

# Build release binaries
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

- **IPC Endpoint** (when `server.uds_path` is omitted):
  - **Linux / Unix**: `/var/run/<cmd_name>.sock`.
  - **Windows**: named pipe `\\.\pipe\<cmd_name>.rsupervisord.ipc`.
- **Daemon Log** (when `logging.file` is omitted and logging is enabled):
  - **Linux / Unix**: `/var/log/<cmd_name>/<cmd_name>.log`.
  - **Windows**: `<config_dir>/logs/<cmd_name>.log`.
- **Program Log** (when `logs.stdout` is omitted and logging is enabled):
  - **Linux / Unix**: `/var/log/<cmd_name>/<program_name>.log`.
  - **Windows**: `<config_dir>/logs/<program_name>.log`.

---

### 3. Running the Daemon

```bash
# 1. Start daemon with automatic configuration discovery
./target/release/supervisord

# 2. Start daemon with explicit configuration file
./target/release/supervisord -c /etc/supervisord/supervisord.yaml

# 3. Start in single-thread mode (lower memory footprint)
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

# Stop the Windows Service (terminates the process trees of all managed programs)
supervisord.exe service stop

# Restart the Windows Service
supervisord.exe service restart

# Uninstall the Windows Service
supervisord.exe service uninstall
```

> [!NOTE]
> When executing as a registered Windows Service, the Windows Service Control Manager (SCM) automatically invokes the binary with `--service`. Upon receiving stop requests from SCM, `rsupervisord` cancels its work in an orderly way before reporting the stopped state.

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
| `--nodaemon` | `-n` | `false` | Foreground flag kept for compatibility. The current version always runs in the foreground. |
| `--worker-threads <N>` | - | CPU cores | Number of runtime worker threads (`1` enables single-threaded mode) |
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

# 3. Configuration reload and updates
supervisorctl config reload                    # [Recommended] Reload config; unchanged programs keep running
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
supervisorctl signal HUP api-server            # Send a signal (e.g. HUP, TERM, KILL, INT)
supervisorctl stdin api-server "reload\n"      # Send input characters to program stdin
supervisorctl events                           # Stream real-time state and lifecycle events

# 6. Group and session management
supervisorctl avail                            # List configured programs and groups
supervisorctl add web                          # Activate a pending process group
supervisorctl remove web                       # Deactivate a process group
supervisorctl open http://127.0.0.1:9001       # Switch the endpoint for this session
supervisorctl fg api-server                    # Foreground mode: stream logs and forward stdin

# 7. Shut down daemon
supervisorctl shutdown                         # Gracefully shut down the supervisord daemon
```

---

## Complete YAML Configuration Reference

### Full Configuration Example (`supervisord.yaml`)

```yaml
# ==============================================================================
# rsupervisord configuration reference (supervisord.yaml)
# Supports environment variable expansion: ${VAR} or ${VAR:-default_value}
# ==============================================================================

# Number of async runtime worker threads. Defaults to the number of CPU cores.
# Set to 1 for single-threaded mode.
# Resolution order: this key, then --worker-threads, then $TOKIO_WORKER_THREADS.
worker_threads: 2

# Optional: write the daemon PID into this file (removed when the daemon exits)
# pidfile: "/var/run/supervisord.pid"

# Optional: raise the daemon's soft limits on startup (Unix only, best effort)
# minfds: 1024
# minprocs: 512

# Optional: environment variables of the daemon process itself;
# managed programs inherit them.
# environment:
#   APP_HOME: "/opt/app"

# ------------------------------------------------------------------------------
# 1. Daemon Server & HTTP API Settings
# ------------------------------------------------------------------------------
server:
  # Local IPC endpoint (Unix Domain Socket on Unix, named pipe / AF_UNIX on Windows)
  # Linux default: /var/run/supervisord.sock
  # Windows default: \\.\pipe\supervisord.rsupervisord.ipc
  uds_path: "/var/run/supervisord.sock"

  # Optional TCP HTTP listener for REST API and embedded Web dashboard
  # Formats: "127.0.0.1:9001", "0.0.0.0:9001", ":9001", "9001"
  # Omit to disable the TCP listener.
  http_bind: "127.0.0.1:9001"

  # Optional Bearer Token for HTTP REST API authentication
  auth_token: "${SUPERVISORD_TOKEN:-}"

  # Optional HTTP Basic Authentication (Supervisor compatible)
  # Passwords support cleartext or SHA-1 hashes prefixed with {SHA}
  username: "admin"
  password: "{SHA}82ab876d1387bfafe46cc1c8a2ef074eae50cb1d"

  # Optional server identifier reported by the event protocol / XMLRPC
  # (default: rsupervisord-compat)
  identifier: "supervisor-node-01"

  # Path resolution switch (default: true)
  # When true, relative paths in the config are resolved against the directory of
  # the configuration file.
  # When false, they are resolved against the working directory of the daemon.
  # Forced to false for INI (Python-compatible) configs.
  path_translation: true

  # When the daemon runs as root/Administrator, allow non-elevated clients to
  # connect to local IPC (default: false). Forced to true for INI configs.
  allow_unelevated: false

  # Optional IPC endpoint permission mode (alias: chmod). Quoted octal string.
  # Valid values (recommended): "0700", "0770", "0777" (accepts "0700" / "0o700" / "700").
  # Defaults when omitted: "0777" if allow_unelevated=true; otherwise "0700" on Unix or "0770" on Windows.
  # uds_chmod: "0700"

  # When true (default), empty `ctl` fields are filled from this server section at load time.
  # Forced to false for INI (Python-compatible) configs.
  ctl_defaults: true

# ------------------------------------------------------------------------------
# 2. supervisorctl Client Connection Defaults (not consumed by the daemon)
# ------------------------------------------------------------------------------
# YAML key `ctl` (alias `supervisorctl`); INI maps `[supervisorctl]`.
# Fields are independent: -s only overrides serverurl, -k only auth_token;
# -u/-p override as a pair. When this section exists but serverurl is omitted,
# the endpoint defaults to http://localhost:9001 (Python parity).
ctl:
  # serverurl: "http://127.0.0.1:9001"   # or unix:///path/to.sock
  # username: "admin"
  # password: "{SHA}..."
  # auth_token: "${SUPERVISORD_TOKEN:-}"

# ------------------------------------------------------------------------------
# 3. Daemon Logging Settings
# ------------------------------------------------------------------------------
logging:
  # Daemon logging mode: on, off, in_memory_only (also accepts true/false).
  # Default: on
  enabled: true

  # Daemon log file destination. Omitted -> platform default path
  # (Linux: /var/log/supervisord/supervisord.log, Windows: <config_dir>/logs/supervisord.log)
  file: "logs/supervisord.log"

  # Log verbosity level: trace, debug, info, warn, error, off (default: info)
  level: "info"

  # Maximum size before log file rotation (supports B, KB, MB, GB; default: 50MB)
  max_bytes: "50MB"

  # Number of rotated historical log backups to retain (default: 10)
  backups: 10

  # Size of the in-memory log ring buffer used by tail/web streaming (default: 1MB)
  # buffer_size: "1MB"

  # Append a timestamp to rotated log file names (default: false)
  # timestamp_suffix: false

  # Suppress console output; the log file is still written (default: false)
  # silent: false

# ------------------------------------------------------------------------------
# 4. Resource Utilization Metrics (CPU & RSS Memory)
# ------------------------------------------------------------------------------
metrics:
  # Enable resource utilization metrics collection (default: true)
  enabled: true

  # Idle timeout in seconds before pausing metrics sampling when no clients are connected (default: 30)
  # Set to 0 to keep sampling continuously.
  idle_timeout_secs: 30

  # Active sampling interval in seconds when clients are connected (default: 2)
  interval_secs: 2

# ------------------------------------------------------------------------------
# 5. Global Program Defaults Template (inherited by all programs)
# ------------------------------------------------------------------------------
# Only the fields below can be inherited. The following fields are per-program and
# must be set on each program: command, args, directory, user, umask, environment,
# depends_on, exit_codes, group, cron, cron_stop.
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
# 6. Process Groups (batch control via <group>:*)
# ------------------------------------------------------------------------------
groups:
  web-cluster:
    programs:
      - api-server
      - frontend-server
    # Group priority, 0..999 (default: 999)
    priority: 80

# ------------------------------------------------------------------------------
# 7. Managed Programs
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

    # Reload when the executable changes; no signal means a full restart
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
# 8. Event Listeners (Supervisor event protocol compatibility)
# ------------------------------------------------------------------------------
event_listeners:
  memmon:
    command: "python3 -m supervisor.memmon -a 200MB -m admin@example.com"
    # Event subscriptions; at least one is required
    events:
      - "TICK_60"
    # Event queue size (default: 10)
    buffer_size: 10
```

---

### Detailed Parameter Reference

#### 1. Top-Level & `server` Section

- **Top-level fields**:
  - **`worker_threads`** (*integer*, default: number of CPU cores): Number of worker threads of the async runtime. `1` runs in single-threaded mode. Precedence: this key, then `--worker-threads`, then `$TOKIO_WORKER_THREADS`.
  - **`nodaemon`** (*boolean*, default: `false`): Foreground flag kept for compatibility with the original Supervisor. The current version always runs in the foreground.
  - **`environment`** (*key-value map*, default: `{}`): Environment variables of the daemon process; managed programs inherit them.
  - **`pidfile`** (*path string*, optional): File that receives the daemon PID at startup and is removed when the daemon exits.
  - **`minfds`** (*integer*, optional, Unix only): Raise the soft file-descriptor limit to at least this value on startup (best effort).
  - **`minprocs`** (*integer*, optional, Unix only): Raise the soft process limit to at least this value on startup (best effort).
- **`server.uds_path`** (*path string*, default: `/var/run/<cmd_name>.sock` on Unix, `\\.\pipe\<cmd_name>.rsupervisord.ipc` on Windows): Local IPC endpoint. Unix uses a socket file; Windows accepts a named pipe (`\\.\pipe\name`) or an `AF_UNIX` file path.
- **`server.http_bind`** (*string*, optional): TCP bind address and port. Serves REST API and Web Dashboard. TCP listening is disabled if omitted. `":9001"`, `"*:9001"`, and `"9001"` are normalized to `"0.0.0.0:9001"`.
- **`server.auth_token`** (*string*, optional): Bearer token for HTTP REST/XMLRPC/SSE (`Authorization: Bearer <token>` or `?token=`). When basic credentials are also configured, **either** may be used (OR). Applied on both TCP and IPC listeners.
- **`server.username` / `server.password`** (*string*, optional; aliases `http_username` / `http_password`): HTTP Basic Auth credentials. Passwords support cleartext or `{SHA}` hashed format. IPC uses `uds_username`/`uds_password` (auto-filled from this pair when omitted). The Web UI signs in via a login modal and keeps an HttpOnly session cookie — secrets are never stored in the browser.
- **`server.uds_username` / `server.uds_password`** (*string*, optional): Credentials checked on the local IPC endpoint. They default to `username` / `password` when omitted in a YAML config.
- **`server.identifier`** (*string*, default: `rsupervisord-compat`): Server identifier reported by the event protocol and XMLRPC.
- **`server.path_translation`** (*boolean*, default: `true`): When true, relative paths in config fields are resolved against `config_dir` (the directory of the configuration file). When false, they are resolved against the working directory of the daemon. Forced to `false` for INI (Python-compatible) configs.
- **`server.allow_unelevated`** (*boolean*, default: `false`): When daemon runs with root or Administrator privileges, permits non-elevated callers to connect via local IPC. Forced to `true` for INI (Python-compatible) configs.
- **`server.uds_chmod`** (*string* / alias `chmod`, optional): Octal IPC endpoint permission mode. Recommended valid values are `"0700"`, `"0770"`, or `"0777"` (accepts `"0700"`, `"0o700"`, or `"700"` format). Defaults when omitted: `"0777"` if `allow_unelevated=true`; otherwise `"0700"` on Unix and `"0770"` on Windows. An explicit value always wins.
- **`server.ctl_defaults`** (*boolean*, default: `true`): When true, empty fields on an **existing** `ctl` section are filled from the server section at load. For INI (Python-compatible) configs this is forced to `false` (a missing `[supervisorctl]` section is an error, matching Python).

#### 2. `ctl` Section (supervisorctl Client Connection Defaults)

Client-only connection config (alias `supervisorctl`; INI maps `[supervisorctl]`). **Not consumed by the daemon** — it never changes `server.uds_*` / `server.username`.

- **Section present** → the CLI connects to this single endpoint. A missing `serverurl` falls back to `http://localhost:9001`.
- **Section absent + `ctl_defaults: true`** → endpoints are derived from the `server` section: local IPC first, then TCP when `http_bind` is set (credentials and token copied accordingly).
- **Section absent + `ctl_defaults: false`** (INI) → error, because the original Supervisor requires `[supervisorctl]`.
- **No configuration file** → local IPC endpoint first, then `http://localhost:9001`.

Fields:

- **`ctl.serverurl`** (*string*, optional): Endpoint used when `-s` is absent. Supports `http://…`, `tcp://…`, `unix://…`, named pipes, and bare IPC paths.
- **`ctl.username` / `ctl.password`** (*string*, optional): HTTP Basic Auth for this endpoint. CLI `-u`/`-p` override them as a pair (missing side becomes `""`).
- **`ctl.auth_token`** (*string*, optional): Bearer token; CLI `-k` overrides this field only.

An explicit `-c` that fails to load is always an error.

#### 3. `logging` Section (Daemon Logging)

- **`logging.enabled`** (*mode*, default: `on`): `on`, `off`, or `in_memory_only` (booleans `true`/`false` are also accepted). `off` disables logging; `in_memory_only` keeps logs in memory only, for `tail`/`maintail` and the Web UI.
- **`logging.file`** (*path string*, optional): Path to daemon log file. When omitted and logging is `on`, the platform default path is used (see [Default Runtime Paths](#2-configuration-search-order--default-paths)). Console output is written as well unless `silent` is set.
- **`logging.level`** (*string*, default: `"info"`): Log verbosity level (`trace`, `debug`, `info`, `warn`, `error`, `off`). `RUST_LOG` takes precedence when set.
- **`logging.max_bytes`** (*string*, default: `"50MB"`): Maximum file size before rotation (supports `B`, `KB`, `MB`, `GB`).
- **`logging.backups`** (*integer*, default: `10`): Number of historical backup files to retain.
- **`logging.buffer_size`** (*string*, default: `"1MB"`): Size of the in-memory log ring buffer used by `maintail` and the Web UI.
- **`logging.timestamp_suffix`** (*boolean*, default: `false`): Append a timestamp to rotated log file names.
- **`logging.silent`** (*boolean*, default: `false`): Suppress console output; the log file is still written.

#### 4. `metrics` Section (Resource Monitoring)

- **`metrics.enabled`** (*boolean*, default: `true`): Enable CPU and RSS memory metrics collection.
- **`metrics.idle_timeout_secs`** (*integer*, default: `30`): Inactivity timeout before pausing metrics sampling. Set to `0` to keep sampling active continuously.
- **`metrics.interval_secs`** (*integer*, default: `2`): Metrics sampling interval during active client connections.

#### 5. `program_defaults` Section (Global Program Defaults)

Every field is optional and only fills the matching field of a program that does not set it. Supported fields:

`autostart`, `autorestart`, `start_secs`, `start_retries`, `restart_pause_secs`, `stop_signal`, `stop_wait_secs`, `priority`, `kill_wait_secs`, `stop_as_group`, `kill_as_group`, `env_files`, `logs`, `health_check`, `pre_start`, `pre_stop`, `pre_start_ignore_failure`, `hook_timeout_secs`, `numprocs`, `numprocs_start`, `process_name`, `restart_when_binary_changed`, `restart_signal_when_binary_changed`, `restart_cmd_when_binary_changed`, `restart_directory_monitor`, `restart_file_pattern`, `restart_signal_when_file_changed`, `restart_cmd_when_file_changed`, `restart_debounce_secs`.

`command`, `args`, `directory`, `user`, `umask`, `environment`, `depends_on`, `exit_codes`, `group`, `cron`, and `cron_stop` are **not** inheritable and must be set on each program.

#### 6. `groups` Section (Process Groups)

- **`groups.<name>.programs`** (*list of strings*, default: `[]`): Program names belonging to this group. Every name must exist under `programs`.
- **`groups.<name>.priority`** (*integer 0..999*, default: `999`): Group priority; lower values start earlier and stop later.

#### 7. `programs.<name>` Section (Managed Program Attributes)

- **Execution & Process Controls**:
  - **`command`** (*string*, required): Executable command line. Supports argument splitting and path resolution.
  - **`args`** (*list of strings*, optional): Extra argument tokens passed to the executable. When empty, `command` is split into program and arguments.
  - **`directory`** (*path string*, optional): Working directory (CWD) for the child process. Defaults to the working directory of the daemon.
  - **`user`** (*string*, optional, Unix only): Unprivileged system user to execute the child process.
  - **`umask`** (*integer*, optional, Unix only): File mode creation mask (e.g. `022`).
  - **`environment`** (*key-value map*, optional): Environment variables injected into child process, supports `${VAR}` expansion.
  - **`env_files`** (*list of paths*, optional): Files loaded into the child environment before `environment`; values in `environment` win.
  - **`autostart`** (*boolean*, default: `true`): Automatically start program on daemon launch (defaults to `false` when `cron` is specified).
  - **`autorestart`** (*enum*, default: `"unexpected"`): Auto-restart policy:
    - `"unexpected"`: Restarts only if exit code is not present in `exit_codes`;
    - `"always"`: Restarts unconditionally whenever the process exits;
    - `"never"`: Never restarts automatically.
  - **`exit_codes`** (*list of integers*, default: `[0]`): Exit codes considered successful/normal termination.
  - **`start_secs`** (*integer*, default: `1`): Minimum runtime in seconds before a process is considered in `RUNNING` state.
  - **`start_retries`** (*integer*, default: `3`): Number of retries after a startup failure. The counter starts at 0 and resets once the process reaches `RUNNING`, so the default allows 1 start + 3 retries before entering `FATAL`.
  - **`restart_pause_secs`** (*integer*, default: `0`): Flat wait in seconds before startup-failure retries when `> 0`; otherwise the backoff is `2^n` seconds (capped at `32`). Does not delay autorestart after `RUNNING`.
  - **`priority`** (*integer 0..999*, default: `50`): Startup/shutdown priority. Lower numbers start earlier and stop later.
  - **`depends_on`** (*list of strings*, optional): Dependent programs. Startup is ordered topologically, and dependent programs are started in layers.
  - **`group`** (*string*, optional): Logical group classification. Defaults to the first `groups` entry that lists this program, otherwise the program name.
- **Stop & Signal Controls**:
  - **`stop_signal`** (*string*, default: `"TERM"` on Unix, `"CTRL_BREAK"` on Windows): Signal sent for graceful termination (`TERM`, `INT`, `QUIT`, `KILL`, `HUP`, `CTRL_C`, `CTRL_BREAK`).
  - **`stop_wait_secs`** (*integer*, default: `10`): Grace period in seconds before the process is force-terminated.
  - **`kill_wait_secs`** (*integer*, default: `2`): Seconds to wait for the process to disappear after the force-termination signal.
  - **`stop_as_group`** (*boolean*, default: `false`): Send `stop_signal` to the whole process group.
  - **`kill_as_group`** (*boolean*, default: same as `stop_as_group`): Force-terminate the whole process group. `stop_as_group: true` together with `kill_as_group: false` is a configuration error.
- **Log Pipelines (`logs`)**:
  - **`logs.enabled`** (*mode*, default: `on`): `on`, `off`, or `in_memory_only` (booleans accepted). `off` runs the process with stdout/stderr connected to the null device (nothing is captured).
  - **`logs.stdout`** (*path string*): Output log destination. Defaults to the platform program log path (see [Default Runtime Paths](#2-configuration-search-order--default-paths)) when `logs.enabled` is `on`. Set to `NONE`, `OFF`, `NULL`, or `/dev/null` to disable file writes.
  - **`logs.stderr`** (*path string*): stderr destination. No file is written unless this is set.
  - **`logs.redirect_stderr`** (*boolean*, default: `false`): Redirect stderr into the stdout stream.
  - **`logs.max_bytes`** (*string*, default: `"50MB"`): Size threshold for log file rotation.
  - **`logs.backups`** (*integer*, default: `10`): Number of historical rotated log files to retain.
  - **`logs.stdout_max_bytes` / `logs.stderr_max_bytes`** (*string*, default: value of `logs.max_bytes`): Per-stream rotation size.
  - **`logs.stdout_backups` / `logs.stderr_backups`** (*integer*, default: value of `logs.backups`): Per-stream retention count.
  - **`logs.buffer_size`** (*string*, default: `"1MB"`): Size of the in-memory ring buffer used by `tail` and the Web UI.
  - **`logs.stdout_timestamp_suffix` / `logs.stderr_timestamp_suffix`** (*boolean*, default: `false`): Append a timestamp to rotated file names.
  - **`logs.stdout_syslog` / `logs.stderr_syslog`** (*boolean*, default: `false`, Unix only): Also send the stream to syslog.
  - **`logs.syslog_facility` / `logs.syslog_tag`** (*string*, optional): Syslog facility and tag.
  - **`logs.syslog_stdout_priority` / `logs.syslog_stderr_priority`** (*string*, optional): Syslog priorities of the two streams.
  - **`logs.stdout_events_enabled` / `logs.stderr_events_enabled`** (*boolean*, default: `false`): Emit `PROCESS_LOG_STDOUT` / `PROCESS_LOG_STDERR` events. The program-level keys are used when `logs.*` does not set them.
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
  - **`cron_stop`** (*cron expression*, alias `stop_cron`): Cron expression to stop process.
  - **`pre_start`** (*string*, alias `pre_start_hook`): Command executed before child process is spawned.
  - **`pre_start_ignore_failure`** (*boolean*, default: `false`): When true, failure of pre-start command does not block process startup.
  - **`pre_stop`** (*string*, alias `pre_stop_hook`): Command executed before stopping child process (a failure does not prevent the stop).
  - **`hook_timeout_secs`** (*integer*, default: `15`): Maximum duration allowed for hook execution.
- **Multi-Process Pools**:
  - **`numprocs`** (*integer*, default: `1`): Number of process instances to launch.
  - **`numprocs_start`** (*integer*, default: `0`): Starting index offset for instance numbering.
  - **`process_name`** (*string*): Name formatting expression, e.g. `%(program_name)s_%(process_num)02d`.
- **Dynamic File & Binary Monitoring**:
  - **`restart_when_binary_changed`** (*boolean*, default: `false`): Watch the executable and reload when it changes.
  - **`restart_signal_when_binary_changed`** (*signal*): Signal sent on executable change. Unset means a full restart.
  - **`restart_cmd_when_binary_changed`** (*command*): Command executed on executable change. Takes precedence over the signal when set.
  - **`restart_directory_monitor`** (*path string*): Monitored directory for configuration or asset files. Unset disables directory monitoring.
  - **`restart_file_pattern`** (*glob string*, e.g. `"*.json"`): Wildcard pattern for monitored files. Unset matches every file.
  - **`restart_signal_when_file_changed`** (*signal*, e.g. `SIGHUP`): Signal sent when matching files change. Unset means a full restart.
  - **`restart_cmd_when_file_changed`** (*command*): Command executed upon file change. Takes precedence over the signal when set.
  - **`restart_debounce_secs`** (*integer*, default: `5`): Settling debounce window in seconds.

#### 8. `event_listeners.<name>` Section (Event Listeners)

- **`command`** (*string*, required): Executable command line of the listener.
- **`args`** (*list of strings*, optional): Extra argument tokens.
- **`events`** (*list of strings*, required): Event subscriptions (e.g. `TICK_60`, `PROCESS_STATE`, `PROCESS_LOG`). At least one must be declared.
- **`buffer_size`** (*integer*, default: `10`): Size of the event queue per listener.
- **`result_handler`** (*string*, default: `supervisor.dispatchers:default_handler`): Handler name sent on the event wire.
- **`priority`** (*integer*, default: `0`; negative values are treated as `0`): Start/stop priority of the listener.
- **`numprocs`** (*integer*, default: `1`): Number of listener instances.
- **`numprocs_start`** (*integer*, default: `0`): Starting index offset for instance numbering.
- **`process_name`** (*string*): Name formatting expression when `numprocs > 1`.
- **`autostart`** (*boolean*, default: `true`): Start the listener with the daemon.
- **`autorestart`** (*enum*, default: `"unexpected"`): Same values as programs.
- **`start_secs`** (*integer*, default: `1`): Minimum runtime before the listener counts as started.
- **`start_retries`** (*integer*, default: `3`): Retries after a startup failure.
- **`stop_signal`** (*string*, default: `"TERM"` on Unix, `"CTRL_BREAK"` on Windows): Signal for graceful termination.
- **`stop_wait_secs`** (*integer*, default: `10`): Grace period before force-termination.
- **`directory`** (*path string*, optional): Working directory. Defaults to the daemon working directory.
- **`user`** (*string*, optional, Unix only): Execution user.
- **`environment`** (*key-value map*, optional): Environment variables of the listener process.
- **`umask`** (*integer*, optional, Unix only): File mode creation mask.
- **`env_files`** (*list of paths*, optional): Files loaded into the listener environment.
- **`stderr_logfile`** (*path string*, optional): stderr destination of the listener.
- **`stop_as_group` / `kill_as_group`** (*boolean*, default: `false`): Group semantics as in programs.

> [!NOTE]
> The stdout channel of an event listener carries the event protocol, so `stdout_logfile` is ignored and `redirect_stderr` must not be `true`.

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

## License

This project is licensed under the **[Mozilla Public License 2.0 (MPL-2.0)](LICENSE)**.
Pull requests and issues are welcome!
