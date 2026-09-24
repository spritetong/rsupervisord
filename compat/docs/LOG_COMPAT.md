# rsupervisord: Logging Compatibility & Design (LOG_COMPAT.md)

| Document Version | Status | Target Language | Scope |
| :--- | :--- | :--- | :--- |
| **v1.0.0** | Draft / For Review | Rust (Edition 2024) | Log destination model (file / syslog / memory / composite), `[supervisord]` + `[program:x]` log keys, go-supervisord full parity + Python config-style parity, OI-7 / OI-10 requirements |

---

## 1. Purpose & Scope

Question answered: **which log destinations and config keys must rsupervisord support so that (a) every go-supervisord log feature works, and (b) stock Python `supervisord.conf` log configuration is accepted with equivalent semantics.**

**Baselines**

| Role | Source |
| :--- | :--- |
| Authoritative config contract (keys + defaults) | Python Supervisor **4.2.5** (`options.py`, `datatypes.py`, `dispatchers.py`, `loggers.py`, `skel/sample.conf`) |
| Reference implementation (destination dispatch, syslog address grammar, multi-file) | Go `ochinchina/supervisord` `logger/log.go` + `logger/log_unix.go` + `process/process.go` (commit `7a73369`) |
| Current-state baseline | `src/config/schema.rs` (`LoggingConfig`, `ProgramLogsConfig*`), `src/logging/{rotator,ring_buffer,pump}.rs`, `src/daemon.rs` (`init_tracing`), `src/compat/ini/adapter.rs` |
| Related | `INI_COMPAT.md` §4.3/§4.4 + §8 OI-7/OI-10; `XMLRPC_COMPAT.md` `getProcessInfo` / `readLog` / `tailLog` / `clearLog` |

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
| absolute / relative path (after macros + `~`) | `FileSink` | ✅ | ✅ | ✅ (exists) |
| `AUTO` / `auto` (program only) | **memory** ring (go) **or** temp file in `childlogdir` (Python) | temp file | memory **1000** | **memory (go parity)** — see §5.1 |
| `NONE` / `none` / `off` / empty | `NullSink` | ✅ | ✅ (empty→Null) | ✅ |
| `/dev/null` / `null` | `NullSink` | ✅ | ✅ | ✅ |
| `/dev/stdout` | `StdIoSink::Stdout` | non-seekable path | ✅ explicit | **new** |
| `/dev/stderr` | `StdIoSink::Stderr` | non-seekable path | ✅ explicit | **new** |
| `syslog` | `SyslogSink::Local` | main only (`logfile=syslog`) | ✅ both | **new** (OI-7) |
| `syslog@[proto:]host[:port]` | `SyslogSink::Remote` | ❌ | ✅ | **new** |
| `memory` (program only) | `MemorySink` (explicit) | ❌ | ✅ | **new** |
| `path1, path2, …` | `CompositeSink` | ❌ | ✅ | **new** |

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
| `logfile` | `logging.file` + destination parse | `$CWD/supervisord.log` (ours: platform default path) | Extend parser: path \| `syslog` \| `syslog@…` \| `/dev/stdout` \| `/dev/null` \| comma-list. Special: `logfile=/dev/stdout` → no file rotator (go early-return). | P0/P1 |
| `logfile_maxbytes` | `logging.max_bytes` | 50MB | exists; `0` = never rotate (required when sharing path / `/dev/stdout`) | ✅ |
| `logfile_backups` | `logging.backups` | 10 | exists | ✅ |
| `logfile_timestamp_suffix` | `logging.timestamp_suffix: bool` | **true** (go) | **new** — see §6.2 | P1 |
| `loglevel` | `logging.level` | info | exists; also gates **debug child mirror** (§6.4) | ✅ (+ P2 mirror) |
| `silent` | `logging.silent` | false | done (OI-4) | ✅ |
| `nodaemon` / `pidfile` / `minfds` / `minprocs` / `environment` / `identifier` | outside pure-log surface | — | done (OI-6/OI-8) | ✅ |
| `childlogdir` | `logging.child_log_dir` | tempdir (Python) | only required if Python `AUTO`→file mode is ever enabled; **deferred** (we ship go `AUTO`→memory) | P2 |
| `nocleanup` / `strip_ansi` / `umask` / `directory` | — | — | Not Supported / separate OI | — |

**YAML native shape** (additive; existing keys unchanged):

```yaml
logging:
  enabled: true
  file: /var/log/supervisord.log   # or "syslog" / "syslog@udp:logs:514"
  max_bytes: 50MB
  backups: 10
  timestamp_suffix: true           # new, default true
  level: info
  silent: false
```

### 5.2 `[program:x]` — per-stream logs

| INI Key | Target | Default | Status / Action | Priority |
| :--- | :--- | :--- | :--- | :--- |
| `stdout_logfile` | `logs.stdout` | `AUTO` | exists (AUTO→default path today) → **redefine AUTO→memory** per §5.1/go | P0 |
| `stderr_logfile` | `logs.stderr` | `AUTO` | same | P0 |
| `stdout_logfile_maxbytes` | `logs.stdout_max_bytes` | 50MB | **OI-10**: independent; fallback to shared `logs.max_bytes` | P0 |
| `stderr_logfile_maxbytes` | `logs.stderr_max_bytes` | 50MB | **OI-10** | P0 |
| `stdout_logfile_backups` | `logs.stdout_backups` | 10 | **OI-10** | P0 |
| `stderr_logfile_backups` | `logs.stderr_backups` | 10 | **OI-10** | P0 |
| `stdout_logfile_timestamp_suffix` | `logs.stdout_timestamp_suffix` | true | **new** (go) | P1 |
| `stderr_logfile_timestamp_suffix` | `logs.stderr_timestamp_suffix` | true | **new** (go) | P1 |
| `redirect_stderr` | `logs.redirect_stderr` | false | exists; stderr sink := stdout sink | ✅ |
| `stdout_syslog` | `logs.stdout_syslog: bool` | false | **Python way**: also mirror this stream to syslog (tag = program name). Coexists with file sink → implement as implicit `CompositeSink(file, syslog)` when true. | P0 (Python) |
| `stderr_syslog` | `logs.stderr_syslog: bool` | false | same | P0 (Python) |
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
  stdout_max_bytes: 10MB       # optional override
  stderr_max_bytes: 50MB
  stdout_backups: 5
  stderr_backups: 10
  stdout_timestamp_suffix: true
  stderr_timestamp_suffix: true
  redirect_stderr: false
  stdout_syslog: false         # Python-style fan-out flag
  stderr_syslog: false
  stdout_events_enabled: false
  stderr_events_enabled: false
```

Resolution order (per stream): stream-specific field → shared `max_bytes`/`backups` → `DEFAULT_LOG_MAX_BYTES` / `DEFAULT_LOG_BACKUPS`.

**`redirect_stderr=true`**: ignore explicit `stderr_logfile` (Python warns; we warn + ignore), force stderr → stdout sink (go assignment).

**Deprecated form**: `stdout_logfile=syslog` (value) is accepted (go path). Python rewrites it to `NULL` + `stdout_syslog=true` — **equivalent outcome under Composite model**: treat value `syslog` as syslog-only sink (Null file + syslog). Do **not** double-log.

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

### 6.1 `AUTO` semantics (decision)

| Mode | Behavior |
| :--- | :--- |
| **Chosen: go parity** | `stdout_logfile=AUTO` / unset → `MemorySink(1000)` (or configurable `DEFAULT_LOG_MEMORY_LINES`). Readable via `readProcessStdoutLog` / `tailProcessStdoutLog` / ring. No file on disk; no `childlogdir`; no `nocleanup`. |
| Rejected for now: Python | Temp file `{name}-{channel}---{identifier}-XXXX.log` in `childlogdir`, deleted on restart unless `nocleanup`. |

**Migration note**: current rsupervisord treats AUTO/unset as “default per-program path”. Changing to memory is a **behavior break** for YAML configs that relied on the default path. Mitigation options (pick at implementation):

1. INI frontend only: AUTO→memory; YAML `stdout: null`/`auto` string still file (asymmetric — reject).
2. Global: AUTO/unset → memory; YAML users must set explicit path (clean, go-aligned).
3. Global: keep file default; only literal `AUTO` (case-sensitive) → memory (partial go).

**Recommendation: (2)** — one rule, matches go, document in INI_COMPAT §5. Explicit path always means file.

### 6.2 Rotation naming

| Mode | Trigger key | Filenames |
| :--- | :--- | :--- |
| Numeric (Python / classic) | `*_timestamp_suffix=false` | `app.log` → `app.log.1` … `.N` (N = backups) |
| Timestamp (go default) | `*_timestamp_suffix=true` | `app.log.2006-01-02T15-04-05`; prune oldest beyond backups |

- `backups <= 0` with timestamp mode → no file (go).
- `max_bytes = 0` → never rotate (Python contract for shared paths).
- Daemon main log and each program stream honor the flag independently.

Implementation: extend `LogRotator` / wrap `file_rotate` suffix strategy, or adopt go’s open-rotate-prune loop. Numeric mode must remain byte-compatible with Python (`supervisord.log.1`).

### 6.3 Shared main log (“all programs + main in one file”)

No dedicated mode (matches all three upstreams). Support the two documented recipes:

1. **Same path, rotation off**: every program `stdout_logfile=<shared>` + `*_logfile_maxbytes=0`, main `logfile=<shared>` + `logfile_maxbytes=0`. Appends interleave; **warn at startup if >1 stream targets same path with rotation enabled** (Python warns conceptually; we enforce warn not error).
2. **Python debug mirror**: when effective `loglevel ≤ debug`, each child stdout/stderr line is **also** written to the **main log sink** at debug level with Python format:
   ```text
   '{process_name}' {stdout|stderr} output:\n{line}
   ```
   - Applies to file **and** syslog main sinks.
   - Controlled solely by `loglevel` (no extra key), matching Python `dispatchers.py`.
   - go does **not** implement this — still required for Python config parity.

`redirect_stderr` remains “within one program only”, never “into main log”.

### 6.4 `logfile=/dev/stdout` (main)

- No file layer; tracing console layer remains (unless `silent`).
- Matches go early-return + Python non-seekable requirement (`logfile_maxbytes` should be 0; warn if non-zero).

### 6.5 Program → daemon stdout

`stdout_logfile=/dev/stdout` writes child bytes to the **daemon process stdout** (StdioSink). In daemonized mode the OS fd may already point at `logfile` (platform daemonize concern, not log-sink concern).

---

## 7. Syslog Design (OI-7)

### 7.1 Components

```text
src/logging/syslog.rs          // pure encoder + connection (Unix)
src/logging/sink.rs            // LogSink enum + dispatch
// Windows: cfg(unix) real; cfg(windows) returns Err(ConfigError)
```

- Local: connect unix datagram/stream to `/dev/log`, `/var/run/syslog`, `/var/run/log` (probe order); fallback: ignore + warn (Python `syslog.syslog()` uses libc; go `log/syslog.New` similar).
- Remote: `UdpSocket` / `TcpStream` to parsed address; **async write from pump** must not block forever — use try-write with timeout or dedicated writer task + bounded channel (go uses background goroutine + re-dial).
- Facility/severity enums: mirror go tables; accept optional `LOG_` prefix; case-insensitive.

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

## 8. Schema / Adapter / Runtime Touchpoints

| Layer | Change |
| :--- | :--- |
| `LoggingConfig` | + `timestamp_suffix: bool` (default true); `file` may hold non-path sentinel — prefer parallel `destination: Option<LogDestination>` parsed once, keep `file: Option<PathBuf>` for pure paths (XML-RPC / path probing). |
| `ProgramLogsConfigRaw` / resolved | + `stdout_max_bytes`, `stderr_max_bytes`, `stdout_backups`, `stderr_backups`, `stdout_timestamp_suffix`, `stderr_timestamp_suffix`, `stdout_syslog`, `stderr_syslog`, `syslog_facility`, `syslog_tag`, `syslog_stdout_priority`, `syslog_stderr_priority`; `stdout`/`stderr` become destination strings pre-resolve → `LogDestination`. |
| `ProgramLogsConfig` (resolved) | + same stream-specific fields; helpers `stdout_sink()` / `stderr_sink()` build `LogSink` tree. |
| INI adapter | Parse OI-10 keys (stop first-wins); parse syslog keys; remove `stdout_syslog`/`stderr_syslog` from “ignored allowlist” into real consumers; parse `logfile_timestamp_suffix`. |
| INI allowlist | Add go keys: `stdout_logfile_timestamp_suffix`, `stderr_logfile_timestamp_suffix`, `logfile_timestamp_suffix`, `syslog_facility`, `syslog_tag`, `syslog_stdout_priority`, `syslog_stderr_priority`. |
| `init_tracing` | Main `LogSink`: file \| syslog \| stdout fan-out; console layer rules unchanged (`silent`). |
| `LogPump` | Optional secondary sinks (syslog/stdio/composite) after rotator write; memory sink replaces file rotator when AUTO. |
| `LogRotator` | Timestamp suffix mode + per-stream max/backups (already parameterized — wire distinct instances). |
| XML-RPC `getProcessInfo` | Report real `stdout_syslog`/`stderr_syslog`/`stdout_logfile_maxbytes`/`…backups` (stop hardcoding `false`/`0`). |
| XML-RPC `readLog`/`tailLog`/`clearLog` | Memory sink: read from `RingBuffer` (capacity ≥ go 1000 when AUTO); syslog sink: `NO_FILE` fault. |
| Docs | This file is source of truth; update `INI_COMPAT.md` §4.3/§4.4 cells + §8 OI-7/OI-10 → done when shipped. |

---

## 9. Implementation Checklist

### P0 — destination model + Python keys + OI-10

1. [ ] `LogDestination` parse (path / NONE / AUTO / memory / syslog / syslog@ / /dev/stdout|stderr|null / comma) shared by main + program.
2. [ ] OI-10: four stream-specific fields + resolution fallback; adapter reads `stdout_*` / `stderr_*` separately (no first-wins).
3. [ ] `stdout_syslog` / `stderr_syslog` bool → Composite(file-or-null, syslog).
4. [ ] Local syslog sink (Unix) + RFC 3164 encoder; Windows hard-error.
5. [ ] AUTO → `MemorySink`; wire XML-RPC read/tail/clear to ring for AUTO programs.
6. [ ] Startup warn: multiple writers, same file, rotation enabled.

### P1 — go full parity

7. [ ] Remote `syslog@udp|tcp:host[:port]` + async writer.
8. [ ] `syslog_facility` / `syslog_tag` / `syslog_{stdout,stderr}_priority`.
9. [ ] `logfile_timestamp_suffix` + per-stream timestamp suffix (default true).
10. [ ] `/dev/stdout` / `/dev/stderr` program destinations.
11. [ ] Comma `CompositeSink` (first-token lock/events).
12. [ ] Main `logfile=syslog` / `syslog@…` / `/dev/stdout`.
13. [ ] XML-RPC snapshot fields for syslog/backups/maxbytes truth.

### P2 — Python polish

14. [ ] Debug-level child mirror into main sink (`loglevel≤debug`).
15. [ ] Optional: `childlogdir` + Python AUTO-tempfile mode behind explicit value (only if users demand; not default).
16. [ ] `supervisorctl maintail` → main-log tail (thin over `readLog`).

### Not Supported

17. [ ] capture_maxbytes, strip_ansi, serverurl, fcgi logs — unchanged.

### Suggested landing order

| Step | Contents | Rationale |
| :--- | :--- | :--- |
| 1 | §9 P0 items 1–2 (model + OI-10) | Unblocks everything else; pure schema/adapter/rotator |
| 2 | P0 3–5 (syslog local + AUTO memory + XML-RPC) | User-visible feature complete for “file + syslog + mem” |
| 3 | P1 7–13 | go parity |
| 4 | P2 14–16 | Python polish |

---

## 10. Test Matrix (acceptance)

| # | Case | Expect |
| :--- | :--- | :--- |
| T1 | INI program `stdout_logfile_maxbytes=1MB` + `stderr_logfile_maxbytes=2MB` | two rotators, independent thresholds |
| T2 | `stdout_logfile=syslog` (Unix CI) | no file; local syslog write; XML-RPC `stdout_syslog` report consistent |
| T3 | `stdout_syslog=true` + `stdout_logfile=/tmp/a.log` | file **and** syslog both receive lines |
| T4 | `syslog@udp:127.0.0.1:1514` | datagram received by test listener; bad proto → config error |
| T5 | `stdout_logfile=AUTO` | no file; `readProcessStdoutLog` returns lines; capacity ≥ 1000 |
| T6 | `stdout_logfile=/dev/stdout` | child bytes on daemon stdout |
| T7 | `stdout_logfile=a.log,b.log` | both files grow; clear/read use first |
| T8 | `logfile_timestamp_suffix=false` | rotated names `.1` `.2` (Python shape) |
| T9 | `logfile_timestamp_suffix=true` | timestamp names; prune at backups |
| T10 | same path, `maxbytes=0` ×2 streams | appends OK; no rotate |
| T11 | same path, `maxbytes=50MB` ×2 | startup **warn** |
| T12 | `loglevel=debug` + child output | main log contains `'{name}' stdout output:\n…` |
| T13 | Windows + any syslog | config error (or agreed degrade) |
| T14 | `redirect_stderr=true` + explicit `stderr_logfile` | stderr → stdout sink; stderr path warn-ignored |
| T15 | YAML regression: existing `logs.max_bytes` only | still applies to both streams (fallback) |

Unit: `tests/log_tests.rs` (new) + extend `tests/ini_tests.rs` for key parsing. Integration: local UDP syslog listener on ephemeral port.

---

## 11. Explicit Divergences (document, do not “fix” silently)

| Topic | Python | go | **Ours** |
| :--- | :--- | :--- | :--- |
| `AUTO` | temp file in `childlogdir` | memory ring 1000 | **memory (go)** |
| `logfile_timestamp_suffix` | always numeric | default **true** | default **true**; `false` → Python numeric |
| `stdout_syslog` bool | yes | no (value-only) | **yes (both)** |
| `syslog@remote` | no | yes | yes |
| `syslog_facility/tag/priority` | no | yes | yes (program only) |
| debug child mirror | yes | no | **yes (Python)** |
| Windows syslog | works (libc) | stub no-op | **config error** |
| shared file + rotation on | corrupt (docs warn) | not special-cased | **startup warn** |
| `maintail` | yes | no | P2 thin wrapper |
| comma multi-destination | no | yes | yes |

---

## 12. Relationship to Other Documents

- INI key mapping cells & OI status: `INI_COMPAT.md` §4.3, §4.4, §8 (OI-7, OI-10) — update when this doc’s checklist lands.
- `getProcessInfo` log fields: `XMLRPC_COMPAT.md` §process info snapshot.
- Ring buffer / tail RPC semantics: `XMLRPC_COMPAT.md` `readProcessStdoutLog` / `tailProcessStdoutLog`.
- go reference tree (`go-supervisord/`, untracked): research only — not a shipped dependency.
