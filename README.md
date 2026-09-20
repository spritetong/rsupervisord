# rsupervisord

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](https://www.rust-lang.org)
[![Platform: Linux | Windows | macOS](https://img.shields.io/badge/Platform-Linux%20%7C%20Windows%20%7C%20macOS-lightgrey.svg)]
[![Tests](https://img.shields.io/badge/Tests-73%2F73%20Passing-brightgreen.svg)]

**rsupervisord** is a modern, high-performance, asynchronous process orchestration and monitoring daemon engine written in Rust.

Designed as a modern alternative to legacy tools like Python Supervisor and Go `ochinchina/supervisord`, `rsupervisord` achieves **true zero-process-polling CPU overhead (0% CPU at idle)**, robust descendant process tree lifecycle control via Windows **Job Objects** and Linux **Process Groups (PGID)**, DAG-based dependency startup and shutdown, zero-downtime configuration hot-reloads, and an out-of-the-box embedded single-page Web UI with **zero external NPM dependencies**.

---

## Key Highlights

- ⚡ **Zero Process Polling in Minimal Feature Set (0% CPU)**:
  - Event-driven process exit monitoring: Win32 kernel event notifications via `RegisterWaitForSingleObject` and Linux `pidfd` / signal pipelines.
  - Dynamically computes uptime on-demand from `started_at`, eliminating legacy 2-second background timer ticks.
  - Programs without health checks sleep in the Tokio reactor with **0 periodic timers and 0 user-space polling wakeups**.
- 🛡️ **Guaranteed Process Tree Reclamation (No Leaked Orphans)**:
  - **Windows**: Bound to native Win32 `Job Objects` with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Even multi-tier grandchild processes are 100% terminated by the Windows kernel if the daemon terminates or stops a service—without relying on `taskkill.exe`.
  - **Linux / BSD**: Isolated process groups (`setpgid`), group signal dispatching (`killpg`), and atomic child lifecycle monitoring via `pidfd`.
  - **Scope-Guarded Pre-Attachment**: Early startup errors or cancellations automatically terminate newly spawned child processes via `scopeguard`, eliminating orphan processes before OS tree attachment.
- 🧰 **Resilient RAII Lifecycles & Zero-Polling API**:
  - Replaces boilerplate `impl Drop` with `tokio_util::sync::DropGuard` and `scopeguard::ScopeGuard` for OS handles and socket files.
  - Fully reactive synchronous REST API (`sync=true`) unblocks instantaneously via `EventHub` with zero busy-polling delay.
- 🔄 **DAG Dependency Orchestration & Zero-Downtime Hot Reload**:
  - Directed Acyclic Graph topology with `priority: 0..99` and automatic cycle detection.
  - Layered parallel startup for non-dependent tasks.
  - 3-Way configuration diffing on `reload`: **unchanged processes keep running with the same PID and zero connection drops**.
- 📉 **Activity-Aware Adaptive Metrics**:
  - Automatically pauses CPU and memory RSS sampling after `idle_timeout_secs` (default 30s) of inactivity when no CLI or Web clients are connected.
  - Instantly awakens on demand upon incoming requests.
- 🔕 **Fully Disableable Logging**:
  - Process level: `logs.enabled: false` (or paths to `/dev/null`, `none`, `off`) spawns directly with `Stdio::null()`, omitting pipes, buffers, and background pump tasks.
  - Daemon level: `logging.enabled: false` or `level: "off"` completely mutes tracing.
- 🧵 **Flexible Threading & Single-Thread CurrentThread Mode**:
  - Configurable worker threads (`worker_threads`). Set to `1` to run a lightweight `current_thread` single-core event loop (2~4MB memory baseline), ideal for edge devices and resource-constrained nodes.
  - CLI client (`rsupervisorctl`) defaults to `current_thread` for instantaneous sub-millisecond execution.
- 🌐 **Modern IPC & Embedded Web Dashboard**:
  - Local cross-platform Unix Domain Sockets (native `AF_UNIX` on both Linux and Windows 10/11) for zero-port Caddy/Nginx reverse proxy integration.
  - Single-binary embedded Web UI powered by production single-file Vue 3 (`rust-embed`, zero NPM dependencies) with real-time SSE live logs, batch controls, and diff modals.
  - Strict caller privilege checks: Unix UID/GID compatibility and Windows `TokenElevation` (`is_admin`) verification.

---

## Architecture

```text
               +-------------------------------------------+
               |  CLI: rsupervisorctl  |  Web UI: Browser  |
               +-------------------------------------------+
                                     |
                          [UDS / AF_UNIX / HTTP JSON]
                                     |
               +-------------------------------------------+
               |       Axum REST API & Embedded SPA        |
               +-------------------------------------------+
                                     | (MPSC Commands)
                                     v
               +-------------------------------------------+
               |    SupervisorManager (Actor & DAG Engine)  |
               +-------------------------------------------+
                        |                          |
            (PlatformProcessGuard)         (PlatformProcessGuard)
                        v                          v
             [ProcessProgram: MySQL]    [ProcessProgram: Core-API]
                        |                          |
             +--------------------+     +--------------------+
             | Windows Job Object |     | Linux Process Group|
             +--------------------+     +--------------------+
```

---

## Quick Start

### 1. Build from Source

Requirements: Rust 1.85+ (Edition 2024).

```bash
# Clone repository
git clone https://github.com/spritetong/rsupervisord.git
cd rsupervisord

# Build release binaries (LTO optimized, stripped)
cargo build --release

# Binaries located at:
#   target/release/rsupervisord      (Daemon engine)
#   target/release/rsupervisorctl    (CLI client)
```

### 2. Configuration Example (`rsupervisord.yaml`)

```yaml
# Tokio worker threads (1 activates current_thread single-thread mode)
worker_threads: 2

server:
  uds_path: "/var/run/rsupervisord.sock"
  http_bind: "127.0.0.1:9001"
  auth_token: ""

metrics:
  enabled: true
  idle_timeout_secs: 30 # Auto-pauses sampling when clients idle

program_defaults:
  autostart: true
  autorestart: unexpected
  start_secs: 3
  start_retries: 3
  stop_signal: "SIGTERM"
  stop_wait_secs: 10
  priority: 50
  logs:
    enabled: true
    max_bytes: "20MB"
    backups: 3

programs:
  database:
    command: "mysqld --console"
    priority: 10
    logs:
      stdout: "/var/log/rsupervisord/db.log"

  api-server:
    command: "./server --port 8080"
    priority: 20
    depends_on: ["database"]
    health_check:
      type: "http"
      url: "http://127.0.0.1:8080/health"
      interval_secs: 10
      timeout_secs: 2
```

See [config-example.yaml](config-example.yaml) for full configuration parameters.

### 3. Running the Daemon & Dynamic Path Conventions

`rsupervisord` dynamically derives `cmd_name` from `argv[0]` by taking the basename without extension and replacing trailing `ctl` with `d`. If invoked via a symlink ending in `ctl` (e.g. `ln -s rsupervisord myctl`), it automatically executes in CLI mode with `cmd_name = "myd"`.

#### Configuration File Search Order
When `-c / --config` is not explicitly specified on the command line, the daemon searches for the first existing configuration file in this order:
1. Environment variable `<UPPERCASE_CMD_NAME>_CONFIG` (e.g. `RSUPERVISORD_CONFIG`, `MYD_CONFIG`)
2. `<executable path>/<cmd_name>.yaml` (and symlink parent directory)
3. `<executable path>/<cmd_name>/config.yaml`
4. OS-specific system path:
   - **Unix**: `/etc/<cmd_name>/config.yaml`
   - **Windows**: None

#### Default Log Paths
- **Daemon Log** (when `logging.file` is omitted):
  - **Windows**: `<config dir>/logs/<cmd_name>.log`
  - **Unix**: `/var/log/<cmd_name>/<cmd_name>.log`
- **Supervised Programs** (when `logs.stdout` is omitted):
  - **Windows**: `<config dir>/logs/<program_name>.log`
  - **Unix**: `/var/log/<cmd_name>/<program_name>.log`

#### Default UDS (`.sock`) Path
- **Windows**: `<config dir>/<cmd_name>.sock` (unprivileged, works without Administrator elevation)
- **Unix**: `/var/run/<cmd_name>.sock`

```bash
# Run daemon (auto-detects configuration file)
./target/release/rsupervisord

# Run daemon with explicit configuration
./target/release/rsupervisord -c config.yaml

# Run with single-threaded event loop (ultra-lightweight)
./target/release/rsupervisord -c config.yaml --worker-threads 1
```

### 4. CLI Control (`rsupervisorctl`)

```bash
# Check status (formatted table)
./target/release/rsupervisorctl status

# Start, stop, or restart programs (Synchronous mode with status confirmation)
./target/release/rsupervisorctl start api-server
./target/release/rsupervisorctl stop api-server
./target/release/rsupervisorctl restart api-server

# Asynchronous command (fire-and-return)
./target/release/rsupervisorctl start api-server --async

# Tail real-time logs with automatic history replay
./target/release/rsupervisorctl tail -f api-server --lines 50

# Hot-reload configuration with zero downtime
./target/release/rsupervisorctl reload
```

### 5. System Service Management (Windows Service & Linux Systemd)

`rsupervisord` provides built-in, cross-platform system service lifecycle management without requiring external wrappers.

#### Command-Line Service Flags
- `--install`: Installs `rsupervisord` as an auto-starting system service (Windows Service via SCM, or Linux systemd unit). An optional `-c / --config <path>` embeds an explicit configuration path.
- `--uninstall`: Stops the running service (if active) and removes it from the service database.
- `--start`: Starts the registered system service.
- `--stop`: Gracefully stops the registered system service and drains all supervised child processes.
- `--restart`: Restarts the registered system service.
- `--service` (*Windows only*): Invoked automatically by the Windows Service Control Manager (SCM) to execute the daemon within the SCM background worker thread.

#### Windows Service (SCM)
Open **PowerShell** or **Command Prompt** as Administrator:

```powershell
# Install Windows Service with auto-start (uses default config path if -c is omitted)
rsupervisord.exe --install

# Install with explicit configuration file
rsupervisord.exe --install -c C:\rsupervisord\config.yaml

# Manage service lifecycle
rsupervisord.exe --start
rsupervisord.exe --stop
rsupervisord.exe --restart
rsupervisord.exe --uninstall
```

When running as a Windows Service, SCM control requests (`Stop`, `Shutdown`) signal cooperative cancellation via `tokio_util::sync::CancellationToken`, cleanly terminating all supervised processes within native Win32 Job Objects before reporting `ServiceState::Stopped`.

#### Linux Systemd Service
On Linux systems, run with root privileges:

```bash
# Install and enable systemd service (/etc/systemd/system/<cmd_name>.service)
sudo rsupervisord --install

# Install with explicit configuration path
sudo rsupervisord --install -c /etc/rsupervisord/config.yaml

# Manage service lifecycle via rsupervisord CLI
sudo rsupervisord --start
sudo rsupervisord --stop
sudo rsupervisord --restart
sudo rsupervisord --uninstall

# Or manage directly via native systemctl
sudo systemctl status rsupervisord
sudo systemctl restart rsupervisord
```

### 6. Accessing the Web Dashboard

Open your browser and navigate to:

```text
http://127.0.0.1:9001/
```

- Real-time aggregated statistics (CPU, RSS memory, health status).
- Interactive multi-select checkboxes for batch operations.
- SSE-driven terminal log drawer with autoscroll locking.
- Hot reload visual configuration diff breakdown.

---

## Performance & Quality Verification

`rsupervisord` enforces a continuous dual-platform verification matrix:

| Verification Suite | Target | Status |
| :--- | :--- | :--- |
| **Silent 0% CPU** | Zero polling wakeups without health check | ✅ Verified (< 0.01% CPU) |
| **Windows Job Objects** | Multi-tier child/grandchild process reclamation | ✅ 100% Reclaimed |
| **Unit & Integration Tests** | 73 comprehensive tests across all modules | ✅ 73/73 Passed |
| **Clippy Strict Lints** | Strict `-D warnings` enforcement | ✅ 0 Warnings |
| **Code Formatting** | Standard Rust formatting (`cargo fmt --check`) | ✅ 0 Diffs |
| **Cross-Platform Matrix** | Windows 11 Native MSVC & Ubuntu 22.04 LTS (WSL2) | ✅ 100% Dual-Platform Pass |

---

## Documentation

- **Product Requirements Document (PRD)**: [PRD.md](docs/PRD.md)
- **Technical Design Specification (DESIGN)**: [DESIGN.md](docs/DESIGN.md)

---

## License

This project is licensed under the [MIT License](LICENSE).
