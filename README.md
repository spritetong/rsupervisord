# rsupervisord

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](https://www.rust-lang.org)
[![Platform: Linux | Windows | macOS](https://img.shields.io/badge/Platform-Linux%20%7C%20Windows%20%7C%20macOS-lightgrey.svg)]
[![Tests](https://img.shields.io/badge/Tests-49%2F49%20Passing-brightgreen.svg)]

**rsupervisord** is a modern, high-performance, asynchronous process orchestration and monitoring daemon engine written in Rust.

Designed as a modern alternative to legacy tools like Python Supervisor and Go `ochinchina/supervisord`, `rsupervisord` achieves **true zero-process-polling CPU overhead (0% CPU at idle)**, robust descendant process tree lifecycle control via Windows **Job Objects** and Linux **Subreaper/PGID**, DAG-based dependency startup and shutdown, zero-downtime configuration hot-reloads, and an out-of-the-box embedded single-page Web UI with **zero external NPM dependencies**.

---

## Key Highlights

- ⚡ **Zero Process Polling in Minimal Feature Set (0% CPU)**:
  - Event-driven process exit monitoring: Win32 kernel event notifications via `RegisterWaitForSingleObject` and Linux `pidfd` / signal pipelines.
  - Dynamically computes uptime on-demand from `started_at`, eliminating legacy 2-second background timer ticks.
  - Programs without health checks sleep in the Tokio reactor with **0 periodic timers and 0 user-space polling wakeups**.
- 🛡️ **Guaranteed Process Tree Reclamation (No Leaked Orphans)**:
  - **Windows**: Bound to native Win32 `Job Objects` with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Even multi-tier grandchild processes are 100% terminated by the Windows kernel if the daemon terminates or stops a service—without relying on `taskkill.exe`.
  - **Linux / BSD**: Subreaper adoption (`PR_SET_CHILD_SUBREAPER`), isolated process groups (`setpgid`), and group signal dispatching (`killpg`).
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
             | Windows Job Object |     | Linux Subreaper/PG |
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

### 3. Running the Daemon

```bash
# Run daemon with configuration
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

### 5. Accessing the Web Dashboard

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
| **Unit & Integration Tests** | 49 comprehensive tests across all modules | ✅ 49/49 Passed |
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
