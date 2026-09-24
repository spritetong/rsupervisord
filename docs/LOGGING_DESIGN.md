# Asynchronous Process Logging & Transport Architecture Design

| Document Version | Status | Target System | Scope |
| :--- | :--- | :--- | :--- |
| **v1.0.0** | Approved / Implementation Phase | Rust (Edition 2024) / Windows & Unix | Zero-Thread OS Process Transport, Extensible Log Backend Abstractions, and In-Memory Rotator |

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
