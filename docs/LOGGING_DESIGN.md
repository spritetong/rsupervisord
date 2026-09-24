# Asynchronous Process Logging & Transport Architecture Design

| Document Version | Status | Target System | Scope |
| :--- | :--- | :--- | :--- |
| **v1.1.0** | Approved / Implementation Phase | Rust (Edition 2024) / Windows & Unix | Zero-Thread Process Transport, In-Memory Rotator, Rotating File Backend, RFC 3164 Syslog, and Composite Destinations |

---

## 1. Background & Problem Statement

### 1.1 Root Cause of Windows Thread Escalation
In `rsupervisord`, each supervised subprocess was previously configured with:
```rust
cmd.stdout(std::process::Stdio::piped());
cmd.stderr(std::process::Stdio::piped());
```
When running under `tokio::process` on Windows:
1. `std::process::Stdio::piped()` internally creates synchronous Windows anonymous pipes via `CreatePipe`.
2. Windows anonymous pipes do **not** support overlapped I/O (`FILE_FLAG_OVERLAPPED`) and cannot be bound to Windows I/O Completion Ports (IOCP).
3. Tokio bridges synchronous anonymous pipes by wrapping them in `tokio::io::Blocking<ArcFile>`, delegating every `poll_read` operation to Tokio's blocking thread pool (`tokio::task::spawn_blocking`).
4. Because supervised long-running child processes do not produce continuous output every millisecond, the underlying Win32 `ReadFile` call blocks synchronously inside a thread pool worker.
5. With both `stdout` and `stderr` actively pumped, **exactly 2 OS worker threads are permanently pinned per running subprocess**. Running 50 subprocesses wastes 100 OS threads in blocked `ReadFile` calls.

### 1.2 Functional Requirements (LOG_COMPAT.md)
The logging subsystem must support:
- Extensible log backends (file rotation, syslog RFC 3164, memory ring, `/dev/stdout`, composite).
- Instant memory cache reader for XML-RPC (`readProcessStdoutLog`, `tailProcessStdoutLog`, `clearProcessLogs`), CLI (`supervisorctl tail -f`), and Web UI streaming.
- Independent stream-specific log rotation thresholds (OI-10: `stdout_logfile_maxbytes`, `stderr_logfile_maxbytes`, `backups`).
- Seamless support for `redirect_stderr=true`.

---

## 2. Architecture Overview

```text
[ ProcessProgram / Subprocess ]
       │ 1. platform.create_process_log_transport()
       ▼
┌────────────────────────────────────────────────────────┐
│                   LogController                        │
│                                                        │
│  ┌──────────────────────────────────────────────────┐  │
│  │             Platform LogTransport                │  │
│  │  - Windows: Overlapped Named Pipe (IOCP, 0 thr)  │  │
│  │  - Unix: O_NONBLOCK Pipe (Epoll/Kqueue, 0 thr)   │  │
│  │  - Child Inheritable Stdio (stdout, stderr)      │  │
│  │  - Optional AsyncWrite (Direct Ingestion/Mock)   │  │
│  └──────────────────────────────────────────────────┘  │
│                         │                              │
│       ┌─────────────────┴─────────────────┐            │
│       ▼                                   ▼            │
│  Stdout AsyncRead                     Stderr AsyncRead │
│       │                                   │            │
│       ├───────────────────┬───────────────┴────────┐   │
│       ▼                   ▼                        ▼   │
│ ┌───────────────┐  ┌───────────────┐      ┌────────────┐│
│ │InMemoryRotator│  │Future External│      │  EventHub  ││
│ │(Built-in Sink)│  │  LogBackend   │      │(LogBus Evt)││
│ └───────────────┘  └───────────────┘      └────────────┘│
│       ▲                                                │
│       │ InstantLogReader (read_bytes / tail_bytes)     │
└───────┼────────────────────────────────────────────────┘
        │
[ XML-RPC / supervisorctl / Web UI ]
```

---

## 3. Step 1: Abstract Interfaces

### 3.1 Basic Types (`src/logging/types.rs`)
- `LogChannel`: `Stdout`, `Stderr`.
- `LogChunk`: Immutable, zero-copy byte slice container (`bytes::Bytes`) with metadata (timestamp, channel, process name, PID).

### 3.2 Transport Trait (`src/logging/transport.rs`)
Encapsulates OS handles passed to the child process and exposes async read streams to the logging engine:
```rust
pub struct ProcessStdioHandles {
    pub stdout: Option<std::process::Stdio>,
    pub stderr: Option<std::process::Stdio>,
}

pub struct TransportStreams {
    pub stdout: Option<Box<dyn tokio::io::AsyncRead + Send + Unpin>>,
    pub stderr: Option<Box<dyn tokio::io::AsyncRead + Send + Unpin>>,
}

pub trait LogTransport: Send + Sync + 'static {
    fn take_child_stdio(&mut self) -> Result<ProcessStdioHandles, ProgramError>;
    fn into_streams(self: Box<Self>) -> Result<TransportStreams, ProgramError>;
    fn direct_writer(&self, channel: LogChannel) -> Option<Box<dyn tokio::io::AsyncWrite + Send + Unpin>> {
        None
    }
}
```

### 3.3 Extensible LogBackend Trait (`src/logging/backend.rs`)
Provides an open interface for log destinations without coupling the core runtime to any specific external backend:
```rust
#[async_trait]
pub trait LogBackend: Send + Sync + 'static {
    async fn write_chunk(&self, chunk: &LogChunk) -> Result<(), ProgramError>;
    async fn flush(&self) -> Result<(), ProgramError>;
    async fn close(&self) -> Result<(), ProgramError> {
        self.flush().await
    }
}
```

### 3.4 InstantLogReader Trait (`src/logging/reader.rs`)
Exposes real-time in-memory reading, subscription, and byte-offset seeking for RPC/Web APIs:
```rust
pub trait InstantLogReader: Send + Sync + 'static {
    fn read_bytes(&self, channel: LogChannel, offset: i64, length: i64) -> (String, i64, bool);
    fn tail_bytes(&self, channel: LogChannel, offset: i64, length: i64) -> (String, i64, bool);
    fn read_lines(&self, channel: LogChannel, max_lines: Option<usize>) -> Vec<String>;
    fn subscribe(&self, channel: LogChannel) -> tokio::sync::broadcast::Receiver<String>;
    fn clear(&self, channel: Option<LogChannel>);
}
```

---

## 4. Step 2: Platform Integration (Zero-Thread Subprocess I/O)

### 4.1 Windows Overlapped Named Pipes
To eliminate the 2 blocked threads per child process on Windows:
1. Generate an isolated named pipe per stream: `\\.\pipe\rsupervisord-{pid}-{program_name}-{stdout|stderr}-{seq}` (monotonic per-process counter, not a UUID).
2. Server end is created using `tokio::net::windows::named_pipe::ServerOptions::new().first_pipe_instance(true).create(&pipe_name)`. This handle is opened with `FILE_FLAG_OVERLAPPED` and registered directly with Tokio's IOCP reactor.
3. Client end is opened synchronously via `OpenOptions` with `FILE_FLAG_WRITE_THROUGH` and passed to `std::process::Command` via `Stdio::from(client_file)`. **Do not** set `HANDLE_FLAG_INHERIT` on the long-lived client handle: Rust std already duplicates `Stdio::Handle` with `bInheritHandle = TRUE` under `CREATE_PROCESS_LOCK` during spawn; leaving the original inheritable would leak the write end into every concurrent `CreateProcess` and can prevent pipe EOF after the child exits.
4. When `redirect_stderr=true`, the client file is `try_clone()`d so both child stdout and stderr write to the same pipe; no separate inherit flag is needed on the clone either.
5. The child process writes to its standard descriptor synchronously; the parent wakes up on native IOCP packet arrival.
6. **Thread count cost: 0 extra OS threads**.

### 4.2 Unix Asynchronous Captured Pipes
1. **Atomic CLOEXEC creation**: Pipes are created with `O_CLOEXEC` on both ends via `create_cloexec_pipe()`, leveraging atomic `pipe2(O_CLOEXEC)` on modern kernels (Linux/BSD) with fallback to `pipe()` + `F_SETFD(FD_CLOEXEC)` on legacy platforms.
2. **Read/Write semantic decoupling (Critical)**:
   - **Read end (supervisor side)**: Marked with `O_NONBLOCK` via `fcntl(&read_fd, F_SETFL(OFlag::O_NONBLOCK))` and integrated directly with Tokio's Epoll/Kqueue reactor using `tokio::io::unix::AsyncFd`.
   - **Write end (child process side)**: **Must remain standard blocking**. Standard runtimes and programs (Python, Node.js, Java, C/C++) expect stdout/stderr to be standard blocking streams. If `O_NONBLOCK` were set on the write end, any burst output exceeding the 64KB kernel buffer would immediately return `EAGAIN` / `EWOULDBLOCK`, crashing Python with `BlockingIOError: [Errno 11] Resource temporarily unavailable` or terminating C/C++ runtimes. A blocking write end guarantees binary compatibility and provides natural OS-level backpressure.
3. **Atomic redirection**: When `redirect_stderr=true`, the write file descriptor is duplicated using `fcntl(&write_file, F_DUPFD_CLOEXEC(0))`, atomically duplicating and setting `FD_CLOEXEC` to prevent handle leaks across concurrent `spawn()` windows.
4. **Thread count cost: 0 extra OS threads**.

### 4.3 Platform Backend Factory
`PlatformBackend` in `src/platform/traits.rs` exposes:
```rust
fn create_process_log_transport(
    &self,
    config: &TransportConfig,
) -> Result<Box<dyn LogTransport>, ProgramError>;
```

---

## 5. Step 3: In-Memory LogRotator (Built-in Logger Backend)

`InMemoryLogRotator` acts as the primary in-memory log warehouse and default backend:
1. **Generational Memory Segments**:
   - Organized into an `active_segment` and a bounded queue of `backup_segments` (`VecDeque<MemorySegment>`).
   - When the active segment exceeds `max_bytes`, it rotates into the backup queue.
   - If `backup_segments.len() > backups`, the oldest segment is dropped.
2. **Byte & Line Seeking**:
   - Calculates global offsets across backup and active segments.
   - Implements XML-RPC compliant `read_bytes` and `tail_bytes` with accurate overflow detection.
3. **Dual Role**:
   - Implements `LogBackend`: Receives stream chunks from the transport pump.
   - Implements `InstantLogReader`: Directly consumed by `supervisorctl`, XML-RPC server, and Web UI.
4. **Zero Disk Dependency**:
   - Retains 100% of supervisor logging capabilities entirely in RAM without requiring disk access.

---

## 6. Step 4: Destination Grammar & Composite Sinks

### 6.1 Unified Grammar
Both daemon main log (`[supervisord] logfile`) and program stream logs (`[program:x] stdout_logfile`, `stderr_logfile`) share a unified destination grammar:

| Value Syntax | Resolved Backend | Semantics & Compatibility |
| :--- | :--- | :--- |
| `/path/to/app.log`, `relative/app.log` | `FileLogBackend` | Standard rotating file on disk with macro expansion and `~` support |
| `AUTO` / `auto` (program only) | `InMemoryLogRotator` | In-memory generational ring buffer (Go parity: 1000 lines default; 0 disk I/O) |
| `NONE` / `none` / `off` / `/dev/null` / `null` / `""` | `NullLogBackend` | Discard child output completely (maps child stdio to `Stdio::null()`) |
| `/dev/stdout` | `StdIoLogBackend::Stdout` | Writes output directly to daemon process `stdout` (Go parity) |
| `/dev/stderr` | `StdIoLogBackend::Stderr` | Writes output directly to daemon process `stderr` (Go parity) |
| `syslog` | `SyslogLogBackend::Local` | Unix local syslog socket (`/dev/log`, `/var/run/syslog`, `/var/run/log`) |
| `syslog@[proto:]host[:port]` | `SyslogLogBackend::Remote` | Remote syslog via UDP (default port 514) or TCP (default port 6514) |
| `dest1, dest2, ...` | `CompositeLogBackend` | Multi-destination fan-out (e.g. `stdout_logfile = test.log, /dev/stdout`) |

### 6.2 Comma-Separated Multi-Destination Compatibility
Go supervisord supports comma-separated destinations (e.g. `stdout_logfile = test.log, /dev/stdout`). `rsupervisord` provides **100% full compatibility** via `CompositeLogBackend`:
1. **Token Parsing & Trimming**: Destination strings are split by comma `,` and each token is trimmed. For example, `test.log, /dev/stdout` resolves to `CompositeLogBackend([FileLogBackend("test.log"), StdIoLogBackend::Stdout])`.
2. **Fan-Out on Write**: Incoming `LogChunk`s pumped from the child process are dispatched to all child backends in the composite tree.
3. **Primary-Reader Semantics**:
   - For read/inspect operations (e.g., XML-RPC `readProcessStdoutLog`, `tailProcessStdoutLog`, or `clearProcessLogs`), **only the primary (first) destination in the list is queried or cleared**, exactly matching Go supervisord's `CompositeLogger` behavior.
   - Secondary destinations are write-only sinks.
4. **Python `stdout_syslog` Interop**:
   - In Python `supervisord`, setting `stdout_syslog = true` alongside `stdout_logfile = /path/app.log` causes logs to be written to both file and syslog.
   - In `rsupervisord`, this is implemented cleanly as an implicit `CompositeLogBackend([FileLogBackend, SyslogLogBackend])`.

---

## 7. Step 5: Rotating File Backend (`FileLogBackend`)

`FileLogBackend` wraps file rotation with full Python and Go behavioral fidelity:

### 7.1 Rotation Naming Modes (`timestamp_suffix`)
- **Timestamp Mode (`timestamp_suffix = true`, Default)**:
  - Matches Go supervisord default.
  - Rotated files are named `app.log.YYYY-MM-DDTHH-MM-SS` (using `file_rotate::suffix::AppendTimestamp`).
  - Automatically prunes oldest backup files exceeding the `backups` count.
- **Numeric Mode (`timestamp_suffix = false`)**:
  - Matches Python Supervisor classic behavior.
  - Rotated files are named `app.log.1`, `app.log.2`, ..., `app.log.N` (using `file_rotate::suffix::AppendCount`).

### 7.2 Append-Only Mode (`max_bytes = 0`)
- When `max_bytes == 0` (or `logfile_maxbytes = 0`), rotation is completely disabled.
- The file is opened in standard append mode (`std::fs::OpenOptions::new().append(true)`).
- Required for shared log files between multiple programs or when logging to special character devices.

### 7.3 Multi-Writer Collision Detection
- At startup, configuration validation checks whether multiple programs (or stdout + stderr streams) point to the same physical file path while `max_bytes > 0`.
- If detected, emits a `tracing::warn!` alerting the operator that concurrent rotation on a shared file may cause log truncation or corruption, recommending `max_bytes = 0` for shared destinations.

---

## 8. Step 6: RFC 3164 Syslog Backend (`SyslogLogBackend`)

### 8.1 Wire Format (RFC 3164 BSD Syslog)
Log chunks are formatted according to RFC 3164:
```text
<PRI>TIMESTAMP HOST TAG: MESSAGE
```
- `PRI = (facility * 8) + severity`.
- `TIMESTAMP`: `Mon dd hh:mm:ss` in local time (e.g. `Sep 24 23:30:00`).
- `HOST`: Local machine hostname.
- `TAG`: Configured `syslog_tag` (defaults to program name, or `supervisord` for daemon main log).
- Total message length is safely truncated to 1024 bytes per RFC 3164 recommendation.

### 8.2 Supported Targets & Network Transports
1. **Local Domain Socket (`syslog`)**:
   - Probed sequentially: `/dev/log`, `/var/run/syslog`, `/var/run/log`.
   - Available on Unix (`#[cfg(unix)]`).
2. **Remote UDP (`syslog@udp:host[:port]`)**:
   - Standard UDP datagram transmission via `tokio::net::UdpSocket`. Default port: `514`.
3. **Remote TCP (`syslog@tcp:host[:port]`)**:
   - Async TCP transmission via `tokio::net::TcpStream`. Default port: `6514`.
   - To prevent network latency or remote server disconnection from blocking subprocess log pumps, TCP writes use an internal bounded mpsc channel and a dedicated background writer task with automatic reconnection.

### 8.3 Platform Matrix & Fail-Loud Policy
- **Unix (Linux/macOS)**: Fully supported for both local socket and remote UDP/TCP.
- **Windows**: Syslog is not natively available. In alignment with `LOG_COMPAT.md` §7.4, attempting to configure a syslog destination on Windows triggers a **hard configuration error** (`ProgramError::ConfigError`) during validation, following the "fail loud" CLI policy rather than silently discarding output.

### 8.4 Facility & Severity Parsing
- **Facilities**: `KERN(0)`, `USER(1)`, `MAIL(2)`, `DAEMON(3)`, `AUTH(4)`, `SYSLOG(5)`, `LPR(6)`, `NEWS(7)`, `UUCP(8)`, `CRON(9)`, `AUTHPRIV(10)`, `FTP(11)`, `LOCAL0(16)`..`LOCAL7(23)`. Default: `LOCAL0`.
- **Severities**: `EMERG(0)`, `ALERT(1)`, `CRIT(2)`, `ERR(3)`, `WARNING(4)`, `NOTICE(5)`, `INFO(6)`, `DEBUG(7)`. Default: `NOTICE`.
- Parsing is case-insensitive and tolerates optional `LOG_` prefix (e.g. `LOG_LOCAL0`, `local0`).

---

## 9. Step 7: Configuration Schema & INI Compatibility (OI-10)

### 9.1 Independent Rotation Thresholds (OI-10)
Previously, `stdout_logfile_maxbytes` and `stderr_logfile_maxbytes` were bound together via a first-wins rule in the INI adapter. Under this design:
- `stdout_max_bytes` and `stderr_max_bytes` are decoupled into independent configuration fields in `ProgramLogsConfig`.
- `stdout_backups` and `stderr_backups` are likewise independent.
- Fallback chain per stream:
  `stream_max_bytes` -> shared `logs.max_bytes` -> `DEFAULT_LOG_MAX_BYTES` (50MB).
  `stream_backups` -> shared `logs.backups` -> `DEFAULT_LOG_BACKUPS` (10).

### 9.2 INI Keys Mapped
- `stdout_logfile`, `stderr_logfile`
- `stdout_logfile_maxbytes`, `stderr_logfile_maxbytes`
- `stdout_logfile_backups`, `stderr_logfile_backups`
- `stdout_logfile_timestamp_suffix`, `stderr_logfile_timestamp_suffix`
- `stdout_syslog`, `stderr_syslog`
- `syslog_facility`, `syslog_tag`, `syslog_stdout_priority`, `syslog_stderr_priority`
- `[supervisord] logfile_timestamp_suffix`

---

## 10. Step 8: Runtime & Subprocess Wiring

1. **`LogPump` Generic Backend Binding**:
   - `LogPumpBuilder` is upgraded from `with_rotator(Option<LogRotator>)` to `with_backend(Option<Arc<dyn LogBackend>>)`.
   - During each line iteration, chunks are dispatched to the configured backend.
   - On EOF, cancel, or drop, `backend.flush()` is invoked asynchronously.
2. **`AUTO` Mode Integration**:
   - When `stdout_logfile = AUTO`, `ProcessProgram` assigns an `InMemoryLogRotator` as the stream backend.
   - XML-RPC methods (`readProcessStdoutLog`, `tailProcessStdoutLog`) directly read from `InstantLogReader`, guaranteeing full API availability without creating temporary files on disk.
3. **Redirection (`redirect_stderr = true`)**:
   - Stderr completely bypasses separate backend initialization and shares the stdout backend tree and pipe.

---

## 11. Implementation Roadmap & Verification Plan

| Phase | Scope | Acceptance Criteria |
| :--- | :--- | :--- |
| **Phase 1** | Grammar & Destination Parsing | `LogDestination` parses all targets, comma-separated tokens, and `syslog@` addresses |
| **Phase 2** | `FileLogBackend` & Rotation | Tests verify timestamp suffix, numeric suffix, `max_bytes = 0`, and collision warnings |
| **Phase 3** | `SyslogLogBackend` (RFC 3164) | Tests verify RFC 3164 format, UDP sender against mock socket, and Windows fail-loud error |
| **Phase 4** | Composite Sinks & Stdio | Tests verify `test.log, /dev/stdout` dual writes and primary-reader semantics |
| **Phase 5** | OI-10 Config & INI Adapter | Tests verify independent stream maxbytes/backups and Python `stdout_syslog` fan-out |
| **Phase 6** | E2E Integration & Clippy | All 100+ tests pass; `cargo clippy` produces 0 warnings |
