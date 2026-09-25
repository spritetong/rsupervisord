# rsupervisord: Logging Compatibility & Design (LOG_COMPAT.md)

| Document Version | Status | Target Language | Scope |
| :--- | :--- | :--- | :--- |
| **v1.1.0** | **Implemented (P0 + P1); P2 partial** | Rust (Edition 2024) | Log destination model (file / syslog / memory / composite), `[supervisord]` + `[program:x]` log keys, go-supervisord full parity + Python config-style parity, OI-7 / OI-10 requirements |

> **Implementation status** (commits `e3cc3f6`…`dd45115`): P0 and P1 are implemented and tested (`tests/logging_tests.rs`, `tests/ini_tests.rs`). Open: main-log `logfile=syslog` only warns (daemon tracing layer has no syslog sink — §6.6), Python debug child mirror (§6.3 recipe 2) not implemented, `childlogdir` deferred. §9 checklist reflects actual state.

---

## 1. Purpose & Scope

Question answered: **which log destinations and config keys must rsupervisord support so that (a) every go-supervisord log feature works, and (b) stock Python `supervisord.conf` log configuration is accepted with equivalent semantics.**

**Baselines**

| Role | Source |
| :--- | :--- |
| Authoritative config contract (keys + defaults) | Python Supervisor **4.2.5** (`options.py`, `datatypes.py`, `dispatchers.py`, `loggers.py`, `skel/sample.conf`) |
| Reference implementation (destination dispatch, syslog address grammar, multi-file) | Go `ochinchina/supervisord` `logger/log.go` + `logger/log_unix.go` + `process/process.go` (commit `7a73369`) |
| Current-state baseline | `src/logging/{destination,composite,backend,rotator,in_memory_rotator,pump,reader}.rs`, `src/logging/syslog/{backend,encoder}.rs`, `src/config/schema.rs`, `src/program/{config,process}.rs`, `src/daemon.rs` (`init_tracing`), `src/compat/ini/adapter.rs` |
| Related | `INI_COMPAT.md` §4.3/§4.4 + §8 OI-7/OI-10; `docs/LOGGING_DESIGN.md` (internal design, v1.1.0) |

**Non-goals (explicit)**

- `stdout_capture_maxbytes` / capture-mode FIFO (depends on capture/event bus) — remains Not Supported.
- `strip_ansi` — Not Supported (go also ignores).
- `serverurl` childutils auto-log discovery — Not Supported (go also ignores).
- HTTP/WebSocket live tail redesign — reuse existing XML-RPC / ring-buffer paths; `maintail` is listed under acceptance only as a thin wrapper over main-log read.

---

## 2. Priority Definitions

| Priority | Meaning |
| :--- | :--- |
| **P0** | Destination model + OI-10 independent rotation + Python key acceptance. Without these, stock configs misbehave. |
| **P1** | go full-parity destinations and knobs (`AUTO`→memory, `/dev/stdout`, comma multi-file, `syslog@…`, facility/tag/priority, `logfile_timestamp_suffix`). |
| **P2** | Optional / deferred (debug mirror polish, `childlogdir` if Python `AUTO`→file mode is selected, `maintail`). |
| **Not Supported** | capture, `strip_ansi`, `serverurl`. |

---

## 3. Destination Model (core abstraction)

Introduce a single sink abstraction used by **daemon main log**, **program stdout**, and **program stderr**:

```text
LogSink
├── FileSink { path, max_bytes, backups, timestamp_suffix }   // rotating file
├── SyslogSink { target: Local | Remote{proto,host,port},
│                facility, tag, priority }                     // RFC 3164
├── MemorySink { capacity }                                    // ring buffer
├── StdIoSink { Stdout | Stderr }                              // daemon process fds
├── NullSink                                                  // discard
└── CompositeSink(Vec<LogSink>)                                // comma multi-destination
```

**Rules**

1. **One logical stream → one `LogSink` tree** (program stdout may fan out; stderr may fan out; `redirect_stderr=true` reuses the stdout sink tree).
2. **Dispatch happens by parsing the destination string**, not by ad-hoc branching in pumps — same grammar for main `logfile` and program `stdout_logfile`/`stderr_logfile` where the sources agree.
3. **Syslog is Unix-only.** On Windows: construct fails at config validation with a clear error, or degrades to `NullSink` + `warn!` (decision in §7.4). Prefer **hard config error on Windows** to match “fail loud” CLI policy; go silently no-ops — document the divergence.
4. **Rotation only applies to `FileSink`.** syslog / memory / stdio never rotate.
5. Pumps (`LogPump`) write to: ring buffer (always, for `supervisorctl tail` / events) → optional `LogSink`. EventHub emission stays on the pump path independent of sink type (syslog still emits `PROCESS_LOG_*` when `*_events_enabled`).

---

## 4. Destination Grammar (shared)

Accepted values for **`[supervisord] logfile`** and **`[program:x] stdout_logfile` / `stderr_logfile`**:

| Value | Sink | Python | go | Ours |
| :--- | :--- | :--- | :--- | :--- |
| absolute / relative path (after macros + `~`) | `FileSink` | ✅ | ✅ | ✅ |
| `AUTO` / `auto` (program only) | INI `AUTO`→default file path; literal `auto`→ring-only | temp file | memory **1000** | ✅ — see §6.1 |
| `NONE` / `none` / `off` / empty | `NullSink` | ✅ | ✅ (empty→Null) | ✅ |
| `/dev/null` / `null` | `NullSink` | ✅ | ✅ | ✅ |
| `/dev/stdout` | `StdIoSink::Stdout` | non-seekable path | ✅ explicit | ✅ |
| `/dev/stderr` | `StdIoSink::Stderr` | non-seekable path | ✅ explicit | ✅ |
| `syslog` | `SyslogSink::Local` | main only (`logfile=syslog`) | ✅ both | ✅ program; main warn-only (§6.6) |
| `syslog@[proto:]host[:port]` | `SyslogSink::Remote` | ❌ | ✅ | ✅ |
| `memory` (program only) | `MemorySink` (explicit) | ❌ | ✅ | ✅ |
| `path1, path2, …` | `CompositeSink` | ❌ | ✅ | ✅ |

**Address grammar (`syslog@…`)** — go `parseSysLogConfig` (`log_unix.go`):

| Form | Proto | Default port |
| :--- | :--- | :--- |
| `syslog@host` | udp | 514 |
| `syslog@host:port` | udp | given |
| `syslog@udp:host` | udp | 514 |
| `syslog@tcp:host` | tcp | 6514 |
| `syslog@udp:host:port` / `syslog@tcp:host:port` | explicit | given |

- Local `syslog` = OS default unix socket (`/dev/log` / `/var/run/syslog`).
- Wire format: **RFC 3164** (BSD) only. No RFC 5424.
- Only `udp` / `tcp` (case-insensitive) for remote; other proto → config error.

**Comma multi-file**: trim tokens; first token owns lock + event emitter (go `CompositeLogger`); remaining sinks get writes only. Empty token list → `NullSink`.

---

## 5. Per-Section Key Requirements

### 5.1 `[supervisord]` — main activity log

| INI Key | Target | Default | Status / Action | Priority |
| :--- | :--- | :--- | :--- | :--- |
| `logfile` | `logging.file` + destination parse | `$CWD/supervisord.log` (ours: platform default path) | Implemented: path \| `syslog` \| `syslog@…` \| `/dev/stdout` \| `/dev/null` \| comma-list (§6.5: main syslog is warn-only today). | ✅ |
| `logfile_maxbytes` | `logging.max_bytes` | 50MB | exists; `0` = never rotate (required when sharing path / `/dev/stdout`) | ✅ |
| `logfile_backups` | `logging.backups` | 10 | exists | ✅ |
| `logfile_timestamp_suffix` | `logging.timestamp_suffix: bool` | **false** (Python numeric parity; go defaults true) | implemented — see §6.2 | ✅ |
| `loglevel` | `logging.level` | info | exists; also gates **debug child mirror** (§6.3, **not implemented**) | ✅ (+ P2 mirror) |
| `silent` | `logging.silent` | false | done (OI-4) | ✅ |
| `nodaemon` / `pidfile` / `minfds` / `minprocs` / `environment` / `identifier` | outside pure-log surface | — | done (OI-6/OI-8) | ✅ |
| `childlogdir` | `logging.child_log_dir` | tempdir (Python) | only required if Python `AUTO`→file mode is ever enabled; **deferred** (accepted-ignored by INI allowlist) | P2 |
| `nocleanup` / `strip_ansi` / `umask` / `directory` | — | — | Not Supported / separate OI | — |

**YAML native shape** (additive; existing keys unchanged):

```yaml
logging:
  enabled: true
  file: /var/log/supervisord.log   # or "syslog" / "syslog@udp:logs:514"
  max_bytes: 50MB
  backups: 10
  timestamp_suffix: false          # default false (numeric .1/.2)
  level: info
  silent: false
```

### 5.2 `[program:x]` — per-stream logs

| INI Key | Target | Default | Status / Action | Priority |
| :--- | :--- | :--- | :--- | :--- |
| `stdout_logfile` | `logs.stdout` | `AUTO` | implemented — see §6.1 for actual AUTO resolution | ✅ |
| `stderr_logfile` | `logs.stderr` | `AUTO` | same | ✅ |
| `stdout_logfile_maxbytes` | `logs.stdout_max_bytes` | 50MB | **OI-10 done**: independent; fallback to shared `logs.max_bytes` | ✅ |
| `stderr_logfile_maxbytes` | `logs.stderr_max_bytes` | 50MB | **OI-10 done** | ✅ |
| `stdout_logfile_backups` | `logs.stdout_backups` | 10 | **OI-10 done** | ✅ |
| `stderr_logfile_backups` | `logs.stderr_backups` | 10 | **OI-10 done** | ✅ |
| `stdout_logfile_timestamp_suffix` | `logs.stdout_timestamp_suffix` | false | implemented (go key; default false for Python parity) | ✅ |
| `stderr_logfile_timestamp_suffix` | `logs.stderr_timestamp_suffix` | false | same | ✅ |
| `redirect_stderr` | `logs.redirect_stderr` | false | exists; stderr sink := stdout sink | ✅ |
| `stdout_syslog` | `logs.stdout_syslog: bool` | false | **Python way implemented**: implicit `Composite(file-or-null, syslog)`; deduped when destination already contains syslog; Windows = config error | ✅ |
| `stderr_syslog` | `logs.stderr_syslog: bool` | false | same (incl. shared-backend `redirect_stderr` case) | ✅ |
| `stdout_events_enabled` / `stderr_events_enabled` | existing | false | exists | ✅ |
| `stdout_capture_maxbytes` / `stderr_capture_maxbytes` | — | 0 | Not Supported (XML-RPC still reports 0) | — |
| `serverurl` | — | — | Not Supported | — |

**YAML native shape (OI-10)**:

```yaml
logs:
  enabled: true
  stdout: auto                 # or path / none / syslog / syslog@…
  stderr: none
  max_bytes: 50MB              # shared fallback (backward compat)
  backups: 10                  # shared fallback
  stdout_max_bytes: 10MB       # stream-specific override
  stderr_max_bytes: 50MB
  stdout_backups: 5
  stderr_backups: 10
  stdout_timestamp_suffix: false
  stderr_timestamp_suffix: false
  redirect_stderr: false
  stdout_syslog: false         # Python-style fan-out flag
  stderr_syslog: false
  stdout_events_enabled: false
  stderr_events_enabled: false
  # go syslog extensions:
  syslog_facility: local0
  syslog_tag: myapp
  syslog_stdout_priority: notice
  syslog_stderr_priority: err
```

Resolution order (per stream): stream-specific field → shared `max_bytes`/`backups` → `DEFAULT_LOG_MAX_BYTES` / `DEFAULT_LOG_BACKUPS`. Effective values reported over XML-RPC (`effective_stdout_max_bytes` etc.).

**`redirect_stderr=true`**: Python forces `stderr_logfile=None`; we share the stdout backend (go assignment) — equivalent visible behavior.

**Deprecated form**: `stdout_logfile=syslog` (value) is accepted (go path) as syslog-only sink; `stdout_syslog=true` adds syslog to an existing file sink (Python). No double-log when both express syslog.

### 5.3 go syslog extension keys (program section)

| INI Key | Default | Values | Target | Priority |
| :--- | :--- | :--- | :--- | :--- |
| `syslog_facility` | `LOCAL0` | KERN, USER, MAIL, DAEMON, AUTH, SYSLOG, LPR, NEWS, UUCP, CRON, AUTHPRIV, FTP, LOCAL0–LOCAL7 (±`LOG_`, ci) | `SyslogSink.facility` | P1 |
| `syslog_tag` | program name | free-form | `SyslogSink.tag` | P1 |
| `syslog_stdout_priority` | `NOTICE` | EMERG…DEBUG (±`LOG_`, ci) | stdout syslog priority | P1 |
| `syslog_stderr_priority` | `NOTICE` | same | stderr syslog priority | P1 |

- Effective priority = `severity | facility` (go `log_unix.go`).
- Keys apply only when that stream has a syslog component (value `syslog*` or `*_syslog=true`).
- **Not Python keys** — document as go extension; still parse under `[program:x]` (INI allowlist already lists `stdout_syslog`/`stderr_syslog`; add the four go keys).
- Main-log syslog uses defaults (facility `LOCAL0`, tag from `identifier` or `supervisord`, severity `NOTICE`) unless we later add `[supervisord] syslog_*` (go does not; **out of scope**).

### 5.4 `[eventlistener:x]`

- No stdout data sink (protocol channel) — stdout `None` as today.
- stderr follows program stderr rules (minus `redirect_stderr`, which is a hard error).
- Syslog keys on EL: accept + apply to stderr only if we already allow stderr logs; otherwise warn-ignore (align current “stdout reserved” behavior).

### 5.5 `[fcgi-program:x]`

Not Supported (same as INI_COMPAT §4.10).

---

## 6. Behavioral Requirements

### 6.1 `AUTO` semantics (as implemented)

| Path | Behavior |
| :--- | :--- |
| INI `stdout_logfile=AUTO` / empty (`parse_log_path`) | → `None` → schema resolves **default per-program file path** when `logs.enabled` (rsupervisord default path, e.g. `/var/log/<cmd>/<prog>.log`). File backend with normal rotation. |
| Literal `auto` / `memory` reaching runtime (e.g. YAML `stdout: auto`) | → `LogDestination::Auto` → **no disk backend**; lines flow only to the per-program `RingBuffer` (tail/`-f`/web). Go-parity intent: 0 disk writes. |
| `InMemoryLogRotator` (generational memory ring) | Implemented + unit-tested (`src/logging/in_memory_rotator.rs`), **not yet wired** as the `Auto` backend; `docs/LOGGING_DESIGN.md` §286 describes the intended wiring. |
| Python temp-file mode (`childlogdir` + `nocleanup`) | Not Supported (deferred, §5.1). |

**Divergence note**: go maps `AUTO` → `MemoryLogger(1000)` always; we split INI (default file path — backward compatible with rsupervisord’s historic behavior) vs literal `auto`/`memory` (ring only). Full go parity for INI `AUTO` = wire `InMemoryLogRotator` into `LogDestination::Auto` + make INI `AUTO` parse to it — **open**, low priority because ring buffer already covers read/tail.

### 6.2 Rotation naming

| Mode | Trigger key | Filenames |
| :--- | :--- | :--- |
| Numeric (Python / classic) | `*_timestamp_suffix=false` — **our default** | `app.log` → `app.log.1` … `.N` (N = backups) |
| Timestamp (go default) | `*_timestamp_suffix=true` | `app.log.2006-01-02T15-04-05`; prune oldest beyond backups |

- `backups <= 0` with timestamp mode → no file (go).
- `max_bytes = 0` → never rotate (Python contract for shared paths).
- Daemon main log and each program stream honor the flag independently.

Implementation: extend `LogRotator` / wrap `file_rotate` suffix strategy, or adopt go’s open-rotate-prune loop. Numeric mode must remain byte-compatible with Python (`supervisord.log.1`).

### 6.3 Shared main log (“all programs + main in one file”)

No dedicated mode (matches all three upstreams). Two documented recipes:

1. **Same path, rotation off** ✅: every program `stdout_logfile=<shared>` + `*_logfile_maxbytes=0`, main `logfile=<shared>` + `logfile_maxbytes=0`. Appends interleave; **startup warn when >1 stream targets the same rotating file with `max_bytes > 0`** (schema Phase 4 validation — implemented).
2. **Python debug mirror** ❌ **open (P2)**: when effective `loglevel ≤ debug`, each child stdout/stderr line is **also** written to the **main log sink** at debug level with Python format:

   ```text
   '{process_name}' {stdout|stderr} output:\n{line}
   ```

   - Would apply to file **and** syslog main sinks.
   - Controlled solely by `loglevel` (no extra key), matching Python `dispatchers.py`.
   - go does **not** implement this — required only for Python config parity.

`redirect_stderr` remains “within one program only”, never “into main log”.

### 6.4 `logfile=/dev/stdout` (main)

- Implemented: no file layer; tracing console layer remains (unless `silent`); destination resolves to the daemon stdout writer.
- Matches go early-return + Python non-seekable requirement (`logfile_maxbytes` should be 0; warn if non-zero).

### 6.5 Program → daemon stdout

`stdout_logfile=/dev/stdout` writes child bytes to the **daemon process stdout** (StdIoLogBackend). In daemonized mode the OS fd may already point at `logfile` (platform daemonize concern, not log-sink concern).

### 6.6 Main log syslog (open gap)

`logfile=syslog` / `syslog@…` **passes config validation** (non-Windows) but `init_tracing` currently warns `"…not supported for the daemon file layer; console logging only"` and falls back to console — the `tracing` subscriber has no syslog writer yet. Program-level syslog is fully implemented; **daemon activity log → syslog is the one remaining P1 item**. `supervisorctl maintail`/`readMainLog` read the file path, so they also see nothing when `logfile=syslog`.

---

## 7. Syslog Design (OI-7)

### 7.1 Components (as implemented)

```text
src/logging/destination.rs          // LogDestination grammar + build_backend()
src/logging/syslog/backend.rs       // SyslogLogBackend (UDS/UDP/TCP, async)
src/logging/syslog/encoder.rs       // RFC 3164 encoder
src/logging/composite.rs            // CompositeLogBackend / Null / StdIo
// Windows: SyslogLogBackend::new → Err(ConfigError); schema validates fail-loud
```

- Local: unix datagram to `/dev/log`-style sockets (probe); unreachable → warn once, drop (never blocks pumps).
- Remote: UDP socket / TCP with dedicated writer task + bounded channel (auto-reconnect; full channel → warn + drop).
- Facility/severity enums mirror go tables; optional `LOG_` prefix, case-insensitive; invalid values are config errors (schema validation).

### 7.2 Message shape

```text
<PRI>TIMESTAMP HOST TAG: MESSAGE
```

- `PRI = facility * 8 + severity` (RFC 3164).
- Timestamp: `Mon dd hh:mm:ss` local time.
- Tag: `syslog_tag` or program name; main: `supervisord` / `identifier`.
- Truncate to 1024 bytes total (RFC advice).

### 7.3 Keys consumed

| Where | Keys |
| :--- | :--- |
| Main | `logfile=syslog\|syslog@…` only |
| Program | `stdout_logfile`/`stderr_logfile` value **or** `stdout_syslog`/`stderr_syslog=true` **or** go `syslog_*` modifiers |

### 7.4 Platform matrix

| OS | Local | Remote | Notes |
| :--- | :--- | :--- | :--- |
| Linux/macOS | ✅ | ✅ udp/tcp | |
| Windows | config error (preferred) or Null+warn | same | go stubs silently — we diverge on purpose |

### 7.5 Config validation

- Syslog sink present + Windows → `ProgramError::ConfigError` with key path (message mentions “syslog is not supported on Windows”).
- Unknown proto in `syslog@` → ConfigError.
- `syslog_facility` etc. present but no syslog sink on that stream → `warn` (key unused), do not fail.

---

## 8. Schema / Adapter / Runtime Touchpoints (shipped reference)

| Layer | Shipped change |
| :--- | :--- |
| `LoggingConfig` | + `timestamp_suffix: bool` (**default false**); main `file` may hold destination sentinels (parsed in `init_tracing` via `LogDestination::parse`). |
| `ProgramLogsConfigRaw` / resolved | + `stdout_max_bytes`, `stderr_max_bytes`, `stdout_backups`, `stderr_backups`, `stdout_timestamp_suffix`, `stderr_timestamp_suffix`, `stdout_syslog`, `stderr_syslog`, `syslog_facility`, `syslog_tag`, `syslog_stdout_priority`, `syslog_stderr_priority`. |
| `ProgramLogsConfig` (resolved) | helpers `effective_stdout_max_bytes()` / `effective_stderr_*` (stream-specific → shared fallback → default). |
| INI adapter | Table-driven `map_field!` for OI-10 + syslog + timestamp keys (no first-wins); `stdout_syslog`/`stderr_syslog` removed from ignored allowlist. |
| `init_tracing` | Resolves `logfile` through `LogDestination::parse`; file/stdio/composite-primary wired; syslog → warn + console-only (§6.6); `silent` unchanged. |
| `LogPump` | Generic `LogBackend` (File/Syslog/StdIo/Composite) after ring push; syslog flag attach via `attach_syslog_backend` (dedupe). |
| `LogRotator` | `with_options(path, max_bytes, backups, timestamp_suffix)`; per-stream instances. |
| XML-RPC `getProcessInfo` | Reports `effective_*` + real `stdout_syslog`/`stderr_syslog`. |
| Config validation | Fail-loud: invalid destination, invalid facility/priority, syslog-on-Windows, shared rotating file multi-writer warn. |
| Docs | This file + `docs/LOGGING_DESIGN.md` v1.1.0; `INI_COMPAT.md` §4.3/§4.4 + §8 OI-7/OI-10 → done. |

---

## 9. Implementation Checklist (actual state)

### P0 — destination model + Python keys + OI-10

1. [x] `LogDestination` parse (path / NONE / AUTO / memory / syslog / syslog@ / /dev/stdout|stderr|null / comma) shared by main + program.
2. [x] OI-10: four stream-specific fields + resolution fallback; adapter reads `stdout_*` / `stderr_*` separately (no first-wins).
3. [x] `stdout_syslog` / `stderr_syslog` bool → Composite(file-or-null, syslog); no double-log.
4. [x] Local syslog sink (Unix) + RFC 3164 encoder; Windows hard-error (schema + backend).
5. [x] AUTO destination → ring-only path (no disk); `InMemoryLogRotator` built + unit-tested — **wiring as Auto backend still open** (§6.1).
6. [x] Startup warn: multiple writers, same rotating file (schema Phase 4).

### P1 — go full parity

1. [x] Remote `syslog@udp|tcp:host[:port]` + async writer (UDP socket; TCP writer task + re-dial).
2. [x] `syslog_facility` / `syslog_tag` / `syslog_{stdout,stderr}_priority`.
3. [x] `logfile_timestamp_suffix` + per-stream timestamp suffix (**default false** — Python parity; go defaults true).
4. [x] `/dev/stdout` / `/dev/stderr` program destinations (+ daemon `logfile=/dev/stdout`).
5. [x] Comma `CompositeSink` (first = primary reader).
6. [ ] **Main `logfile=syslog` / `syslog@…`** — validation ✅ but tracing layer has no syslog writer (§6.6, **open**).
7. [x] XML-RPC snapshot fields for syslog/backups/maxbytes truth.

### P2 — Python polish

 1. [ ] Debug-level child mirror into main sink (`loglevel≤debug`) — **open** (§6.3 recipe 2).
 2. [ ] Optional: `childlogdir` + Python AUTO-tempfile mode — **deferred** (only if users demand).
 3. [x] `supervisorctl maintail` → main-log tail (CLI `handle_maintail`).

### Not Supported (confirmed unchanged)

 1. [x] capture_maxbytes, strip_ansi, serverurl, fcgi logs — unchanged.

---

## 10. Test Matrix (acceptance)

| # | Case | Expect |
| :--- | :--- | :--- |
| T1 | INI program `stdout_logfile_maxbytes=1MB` + `stderr_logfile_maxbytes=2MB` | two rotators, independent thresholds |
| T2 | `stdout_logfile=syslog` (Unix CI) | no file; local syslog write; XML-RPC `stdout_syslog` report consistent |
| T3 | `stdout_syslog=true` + `stdout_logfile=/tmp/a.log` | file **and** syslog both receive lines |
| T4 | `syslog@udp:127.0.0.1:1514` | datagram received by test listener; bad proto → config error |
| T5 | literal `stdout: auto` / `memory` | no file; lines readable via ring/tail |
| T6 | `stdout_logfile=/dev/stdout` | child bytes on daemon stdout |
| T7 | `stdout_logfile=a.log,b.log` | both files grow; clear/read use first |
| T8 | `logfile_timestamp_suffix=false` (default) | rotated names `.1` `.2` (Python shape) |
| T9 | `logfile_timestamp_suffix=true` | timestamp names; prune at backups |
| T10 | same path, `maxbytes=0` ×2 streams | appends OK; no rotate |
| T11 | same path, `maxbytes>0` ×2 | startup **warn** |
| T12 | `loglevel=debug` + child output | main log contains `'{name}' stdout output:\n…` — **spec only, not implemented (§6.3)** |
| T13 | Windows + any syslog | config error (implemented) |
| T14 | `redirect_stderr=true` + explicit `stderr_logfile` | stderr → stdout sink (shared backend) |
| T15 | YAML regression: existing `logs.max_bytes` only | still applies to both streams (fallback) |

Unit: `tests/log_tests.rs` (new) + extend `tests/ini_tests.rs` for key parsing. Integration: local UDP syslog listener on ephemeral port.

---

## 11. Explicit Divergences (document, do not “fix” silently)

| Topic | Python | go | **Ours** |
| :--- | :--- | :--- | :--- |
| `AUTO` | temp file in `childlogdir` | memory ring 1000 | **INI → default file path; literal `auto`/`memory` → ring-only (§6.1)** |
| `logfile_timestamp_suffix` | always numeric | default **true** | **default false** (Python numeric); `true` → go timestamp mode |
| `stdout_syslog` bool | yes | no (value-only) | **yes (both)** |
| `syslog@remote` | no | yes | yes |
| `syslog_facility/tag/priority` | no | yes | yes (program only) |
| debug child mirror | yes | no | **spec only — not implemented (§6.3)** |
| main `logfile=syslog` | yes | yes | **warn + console only (§6.6, open)** |
| Windows syslog | works (libc) | stub no-op | **config error** |
| shared file + rotation on | corrupt (docs warn) | not special-cased | **startup warn** |
| `maintail` | yes | no | yes (CLI) |
| comma multi-destination | no | yes | yes |

---

## 12. Relationship to Other Documents

- INI key mapping cells & OI status: `INI_COMPAT.md` §4.3, §4.4, §8 (OI-7, OI-10) — update when this doc’s checklist lands.
- `getProcessInfo` log fields: `XMLRPC_COMPAT.md` §process info snapshot.
- Ring buffer / tail RPC semantics: `XMLRPC_COMPAT.md` `readProcessStdoutLog` / `tailProcessStdoutLog`.
- go reference tree (`go-supervisord/`, untracked): research only — not a shipped dependency.
