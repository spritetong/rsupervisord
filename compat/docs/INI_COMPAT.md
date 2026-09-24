# rsupervisord: INI Configuration Compatibility Analysis (INI_COMPAT.md)

| Document Version | Status | Target Language | Scope |
| :--- | :--- | :--- | :--- |
| **v1.1.0** | Draft / For Review | Rust (Edition 2024) | Section/field mapping, differences, examples, priorities, and **open issues** for Python `supervisord.conf`(INI) → rsupervisord; includes go-supervisord field audit results |

---

## 1. Purpose & Scope

Question answered: **what must be implemented for a standard Python `supervisord.conf`(INI) to be loaded directly by rsupervisord.**

- **Authoritative baseline**: Python Supervisor **4.2.5**(`supervisor/skel/sample.conf` + `options.py`).
- **Reference implementation**: Go `ochinchina/supervisord` `config/config.go`(commit `7a73369`).
- **Current-state baseline**: `src/config/schema.rs`(YAML schema)、`src/config/expand.rs`(macro expansion)、`src/program/config.rs`(runtime configuration).
- **Related**: the client-side semantics of section `[supervisorctl]` are covered in `CLI_COMPAT.md`; `[eventlistener:x]` in `SUPERVISORD_COMPAT.md` §7 #6 / `EVENTLISTENER_COMPAT.md`; `[rpcinterface]` in §7 #5.
- **Open issues / field audit**: §8 records unresolved gaps found by auditing go-supervisord's INI key consumption against our adapter (`src/compat/ini/adapter.rs`), with technical approach and priority.

**Core conclusion (preview)**: the execution skeleton needs no changes. INI is only a **front-end parser** that translates sections/fields into the existing `SupervisorConfig`(raw)→ reuse the existing `validate()` and `resolve_programs()`. **The macro expander, numprocs expansion, group parsing, and program_defaults already exist.**

---

## 2. Priority Definitions

| Level | Meaning |
| :--- | :--- |
| **P0** | Infrastructure for INI usability (parser + suffix detection + value normalization + core section mapping). Mandatory. |
| **P1** | Common and low cost (`[include]`, `[rpcinterface]` tolerance, `[supervisorctl]`, priority range arbitration). |
| **P2** | Depends on deferred items or higher cost (daemon runtime fields, socket permissions, independent stdout/stderr rotation). |
| **Not Supported** | Cost too high and not implemented in the Go version either, or the semantics are already covered by other mechanisms. |

---

## 3. Python INI Section Taxonomy (10 categories in total)

| # | Section | Scope | rsupervisord Target | Priority |
| :--- | :--- | :--- | :--- | :--- |
| 1 | `[unix_http_server]` | daemon UDS listener | `server` | **P0** |
| 2 | `[inet_http_server]` | daemon TCP listener | `server` | **P0** |
| 3 | `[supervisord]` | daemon global | `logging` + (runtime surface, SUPERVISORD #12 / §8 OI-8) | **P0**(logging part) |
| 4 | `[program:x]` | supervised programs | `programs.x` | **P0** |
| 5 | `[group:x]` | heterogeneous process groups | `groups.x` | **P0** |
| 6 | `[supervisorctl]` | **client** connection config | CLI defaults (not daemon); stored as `CliDefaults` (§8 OI-1 **done**) | **P1** → done |
| 7 | `[include]` | configuration inclusion | file merging | **P1** |
| 8 | `[rpcinterface:supervisor]` | RPC interface registration | tolerated/ignored (§7 #5) | **P1** |
| 9 | `[eventlistener:x]` | event listener programs | `event_listeners` (implemented: `parse_event_listener_config`) | **Implemented** |
| 10 | `[fcgi-program:x]` | FastCGI programs | — | **Not Supported** |

> Additionally: the Go version extends with `[program-default]`(this section does not exist in Python), to which rsupervisord's `program_defaults` corresponds. It can be supported as an extension as well.

---

## 4. Per-Section Field Mapping & Differences

### 4.1 `[unix_http_server]` → `server`

| INI Field | Current YAML | Difference / Action |
| :--- | :--- | :--- |
| `file` | `server.uds_path` | direct mapping |
| `username` | `server.uds_username` | direct; **INI maps UDS credentials independently** of `[inet_http_server]` (adapter does not share fields) |
| `password` | `server.uds_password` | same as above |
| `chmod` | `server.uds_chmod` | implemented: octal mode applied at bind (Unix `set_permissions`, Windows pipe `SECURITY_ATTRIBUTES`, Windows file UDS protected DACL). Quoted string required in YAML. |
| `chown` | — | **P2** (§7 #11; go also ignores this key) |

**Example**

```ini
[unix_http_server]
file=/tmp/supervisor.sock
username=admin
password=secret
```

```yaml
server:
  uds_path: /tmp/supervisor.sock
  username: admin
  password: secret
```

### 4.2 `[inet_http_server]` → `server`

| INI Field | Current YAML | Difference / Action |
| :--- | :--- | :--- |
| `port` | `server.http_bind` | direct; `normalize_http_bind` already supports `:9001` / `*:9001` / `9001` |
| `username` | `server.username` | direct; independent of UDS credentials |
| `password` | `server.password` | same as above |

**Example**

```ini
[inet_http_server]
port=127.0.0.1:9001
username=admin
password=secret
```

```yaml
server:
  http_bind: 127.0.0.1:9001
  username: admin
  password: secret
```

### 4.3 `[supervisord]` → `logging` + runtime surface

| INI Field | Current YAML | Difference / Action |
| :--- | :--- | :--- |
| `logfile` | `logging.file` | direct |
| `logfile_maxbytes` | `logging.max_bytes` | direct; default 50MB (Python & ours) |
| `logfile_backups` | `logging.backups` | direct; default 10 vs ours 10 |
| `loglevel` | `logging.level` | direct |
| `pidfile` | — | `pidfile` written/removed at runtime (§8 OI-8 **done**) |
| `nodaemon` | `nodaemon` | `string_to_bool` (§8 OI-4 **done**) |
| `silent` | — | **P2** (go also ignores) |
| `minfds` | — | `minfds` → Unix `setrlimit` best-effort (§8 OI-8 **done**) |
| `minprocs` | — | `minprocs` → Unix `setrlimit` best-effort (§8 OI-8 **done**) |
| `umask` | — | **P2** (daemon-level; go also ignores at `[supervisord]`) |
| `user` | — | not supported/partial (setuid); go also ignores at `[supervisord]` |
| `identifier` | `server.identifier` | implemented (adapter) |
| `directory` | — | **P2**; go also ignores at `[supervisord]` |
| `nocleanup` | — | **Not Supported** (go also ignores) |
| `childlogdir` | — | **P2**(AUTO child log directory; go also ignores) |
| `environment` | — | `config.environment`; applied at daemon startup, not parse time (§8 OI-6 **done**); program-level still via YAML/INI program sections |
| `strip_ansi` | — | **Not Supported** (go also ignores) |

**Example**

```ini
[supervisord]
logfile=/var/log/supervisord.log
logfile_maxbytes=50MB
logfile_backups=10
loglevel=info
pidfile=/run/supervisord.pid
```

```yaml
logging:
  enabled: true
  file: /var/log/supervisord.log
  max_bytes: 50MB
  backups: 10
  level: info
```

### 4.4 `[program:x]` → `programs.x`(core)

| INI Field | Current YAML | Difference / Action |
| :--- | :--- | :--- |
| `command` | `command`(+`args`) | direct; ours auto shell-splits when `args` is empty |
| `process_name` | `process_name` | direct |
| `numprocs` | `numprocs` | direct(**expansion implemented**) |
| `numprocs_start` | `numprocs_start` | direct |
| `directory` | `directory` | direct |
| `umask` | `umask` | direct |
| `priority` | `priority` | range `0..=999` (`MAX_PRIORITY`); Python has no upper bound → §6 |
| `autostart` | `autostart` | boolean parsing difference (see §5) |
| `startsecs` | `start_secs` | naming; default 1 (same as Python) |
| `startretries` | `start_retries` | naming |
| `autorestart` | `autorestart` | **value mapping**: `false→never`,`true→always`,`unexpected→unexpected` |
| `exitcodes` | `exit_codes` | list parsing (`0,2` → `[0,2]`) |
| `stopsignal` | `stop_signal` | alias `SIGTERM` already supported |
| `stopwaitsecs` | `stop_wait_secs` | naming |
| `stopasgroup` | — | `stop_as_group` group signal (§8 OI-9 **done**) |
| `killasgroup` | — | `kill_as_group` group signal (§8 OI-9 **done**) |
| `user` | `user` | direct |
| `redirect_stderr` | `logs.redirect_stderr` | direct |
| `stdout_logfile` | `logs.stdout` | **`AUTO`/`NONE` semantics**(see §5) |
| `stdout_logfile_maxbytes` | `logs.max_bytes` | naming; **ours uses a single value shared by stdout/stderr → SUPERVISORD #12 / §8 OI-10** |
| `stdout_logfile_backups` | `logs.backups` | same as above |
| `stderr_logfile` | `logs.stderr` | `AUTO`/`NONE` |
| `stderr_logfile_maxbytes` | `logs.max_bytes` | shared inconsistency (OI-10) |
| `stderr_logfile_backups` | `logs.backups` | shared inconsistency (OI-10) |
| `environment` | `environment` | **format**: `A="1",B="2"` → map (see §5) |
| `stdout_capture_maxbytes` | — | **Not Supported**(capture mode/event) |
| `stdout_events_enabled` | `logs.stdout_events_enabled` | **implemented** (parsed; feeds EventHub when listeners registered) |
| `stdout_syslog` | — | **Not Supported** (go: ignored unless `syslog_facility` set → §8 OI-7) |
| `stderr_capture_maxbytes` | — | **Not Supported** |
| `stderr_events_enabled` | `logs.stderr_events_enabled` | **implemented** (same as stdout) |
| `stderr_syslog` | — | **Not Supported** (OI-7) |
| `serverurl` | — | **Not Supported**(childutils; go also ignores) |
| `envFiles` | — | `env_files` loaded at spawn (§8 OI-2 **done**) |
| `killwaitsecs` | — | `kill_wait_secs` (default 2s) (§8 OI-3 **done**) |
| `liveness_check_*` | `health_check` | mapped to exec `HealthCheckConfig` (§8 OI-5 **done**) |

> ours/Go extension fields (not in Python): `depends_on`, `cron`, `pre_start`/`pre_stop`, `health_check`, `restart_*`, `envFiles`, `killwaitsecs`, `liveness_check_*`. If they appear in INI, accept them as extensions where mapped (§8).

**Example**

```ini
[program:web]
command=/usr/bin/gunicorn app:app
process_name=%(program_name)s_%(process_num)02d
numprocs=2
directory=/srv/app
priority=10
autostart=true
startsecs=5
startretries=3
autorestart=unexpected
exitcodes=0,2
stopsignal=QUIT
stopwaitsecs=15
redirect_stderr=true
stdout_logfile=/var/log/web.log
stdout_logfile_maxbytes=10MB
stdout_logfile_backups=5
environment=PORT="8080",DEBUG="false"
```

```yaml
programs:
  web:
    command: /usr/bin/gunicorn app:app
    process_name: "%(program_name)s_%(process_num)02d"
    numprocs: 2
    directory: /srv/app
    priority: 10
    autostart: true
    start_secs: 5
    start_retries: 3
    autorestart: unexpected
    exit_codes: [0, 2]
    stop_signal: QUIT
    stop_wait_secs: 15
    logs:
      redirect_stderr: true
      stdout: /var/log/web.log
      max_bytes: 10MB
      backups: 5
    environment:
      PORT: "8080"
      DEBUG: "false"
```

### 4.5 `[group:x]` → `groups.x`

| INI Field | Current YAML | Difference / Action |
| :--- | :--- | :--- |
| `programs` | `groups.x.programs` | direct (comma/whitespace-delimited list) |
| `priority` | `groups.x.priority` | direct |

**Example**

```ini
[group:web]
programs=frontend,backend
priority=80
```

```yaml
groups:
  web:
    programs: [frontend, backend]
    priority: 80
```

### 4.6 `[supervisorctl]` → CLI defaults (P1, **done** §8 OI-1)

| INI Field | Target | Status / Action |
| :--- | :--- | :--- |
| `serverurl` | `rsupervisorctl -s` default | **consumed**: stored as `CliDefaults.serverurl`; `resolve_endpoint_candidates` seeds a single endpoint when `-s` is absent |
| `username` | `-u` default | **consumed**: seeds basic auth when `-u`/`-p` are absent (CLI wins) |
| `password` | `-p` default | same as `username` |
| `prompt` | — | **Not Supported**(interactive shell; go also ignores) |
| `history_file` | — | **Not Supported** (go also ignores) |

> Note: the daemon itself does **not consume** `[supervisorctl]`; it only affects the client. Contract: `CLI_COMPAT.md` §5.1.4. Implementation gap and approach: §8 OI-1.

### 4.7 `[include]` → file merging (P1)

| INI Field | Action |
| :--- | :--- |
| `files` | resolved relative to this file; whitespace/newline-delimited; globs supported; an included file cannot include again |

**Example**

```ini
[include]
files = conf.d/*.ini
```

### 4.8 `[rpcinterface:supervisor]` → tolerated (P1)

| INI Field | Action |
| :--- | :--- |
| `supervisor.rpcinterface_factory` | **ignored**(parsed but discarded); once §7 #5 lands, XML-RPC can be enabled based on it |

> Production configs **always include** this section, so the parser must accept rather than error on it.

### 4.9 `[eventlistener:x]` → Implemented (go does not parse this section)

Fields largely mirror `[program:x]`, plus `events` (required), `buffer_size` (default 10), `priority` defaulting to `-1`, and `redirect_stderr` must be false.

**Status**: **implemented** — `parse_event_listener_config` in `adapter.rs` maps into `EventListenerConfigRaw` (protocol/handshake: `EVENTLISTENER_COMPAT.md` / SUPERVISORD #6). **go-supervisord does not recognize `[eventlistener:*]`** (falls through its lexer as an unknown section); we are intentionally ahead of go here.

**INI-only gap**: `result_handler` / log rotation keys are accepted or defaulted as in `EVENTLISTENER_COMPAT.md`; unknown keys under known sections now warn (§8 OI-11 **done**).

### 4.10 `[fcgi-program:x]` → Not Supported

FastCGI programs: extra `socket` / `socket_owner` / `socket_mode`, and reuse the program fields.

**Decision**: **Not supported**(requires a FastCGI socket forwarding subsystem — complex, and not implemented in the Go version either).

---

## 5. Syntax & Value-Format Differences (parser must handle)

| Item | Python INI | Current YAML | Requirement |
| :--- | :--- | :--- | :--- |
| Comments | leading whitespace + `;` or `#`; `a=b ;comment` valid, `a=b;comment` invalid | YAML `#` | INI comment rules must be implemented precisely |
| Quotes | **no quotes supported** except in `environment=` | YAML quotes | INI values taken literally |
| Booleans | `true/false/yes/no/1/0/on/off`(case-insensitive) | `true/false` | add lenient boolean parsing |
| `autorestart` | `false` / `unexpected` / `true` | `never/unexpected/always` | value mapping |
| `exitcodes` | `0,2`(comma-delimited) | `[0,2]` | list parsing |
| `environment` | `A="1",B="2"`(comma-delimited, quoted values) | map | dedicated parser |
| `stdout_logfile` | `AUTO` / `NONE` / path | `None`/path | `NONE→disabled`, `AUTO→default path` |
| Macros | `%(ENV_X)s`/`%(here)s`/`%(program_name)s`/`%(process_num)02d`/`%(numprocs)d`/`%(group_name)s`/`%(host_node_name)s` | same (expand.rs implemented) | reuse existing `MacroExpander` |
| Key names | case-insensitive, no underscore style (`startsecs`) | snake_case (`start_secs`) | alias table |
| Multi-value lists | `programs`/`files`/`exitcodes` comma or whitespace | YAML sequences | pick delimiter rules per field |

---

## 6. Default-Value Differences (Python 4.2.5 vs Current)

| Field | Python Default | Current Default | Verdict |
| :--- | :--- | :--- | :--- |
| `priority` | 999 (no upper bound) | 50, validates `0..=999` (`MAX_PRIORITY`) | aligned with Python sample; arbitrary values >999 rejected with a clear error |
| `startsecs` | 1 | 1 | consistent |
| `startretries` | 3 | 3 | consistent |
| `stopwaitsecs` | 10 | 10 | consistent |
| `exitcodes` | `[0]` | `[0]` | consistent |
| `autorestart` | unexpected | unexpected | consistent |
| `logfile_maxbytes` | 50MB | 50MB | consistent |
| `logfile_backups` | 10 | 10 | consistent |
| `umask` | 022 | None | **P2** (daemon-level) |
| `redirect_stderr` | false | false | consistent |
| `path_translation` | N/A (Python always CWD-resolves bare relative paths) | INI frontend forces `false` (YAML native default remains `true`) | **Aligned**: bare relative paths stay relative and resolve against daemon CWD, matching Python/go-supervisord |
| `allow_unelevated` | N/A (Python/go have no elevation gate on IPC) | INI frontend forces `true` (YAML native default remains `false`) | **Aligned**: no app-layer elevation check; access governed by socket file permissions only, matching Python/go |

> Default-value differences do not block loading; recommended: "if a config explicitly writes a value, use the explicit value"; when not written, use the ours default and note it in the docs.
> **Note on `path_translation` / `allow_unelevated`**: these are rsupervisord-only knobs with no Python INI key. The INI frontend forces them to baseline-aligning values (`false` / `true`) before `translate_paths` + `validate`, so a stock Python `supervisord.conf` loads with path and IPC semantics equivalent to Python supervisor / go-supervisord. YAML native configs keep the rsupervisord defaults (`path_translation: true`, `allow_unelevated: false`).

---

## 7. Implementation Checklist (by priority)

### P0 — INI front-end infrastructure

1. **INI parser** (module `src/compat/ini/{parser,values,adapter}.rs`): sections, `key=value`, comment rules, line continuation, case-insensitive keys, per-value macro expansion.
2. **Suffix detection**: `SupervisorConfig::from_file` (`schema.rs`) — `.yaml`/`.yml` → YAML pipeline; everything else (`.conf`/`.ini`/extensionless) → INI pipeline; **both produce the same `SupervisorConfig`**.
3. **Value-normalization layer**: lenient booleans, `autorestart` three-state mapping, list parsing (`exitcodes`/`programs`/`files`), dedicated `environment` parsing, `AUTO`/`NONE` log paths.
4. **Field alias table**: full mapping such as `startsecs→start_secs`, `startretries→start_retries`, `stopwaitsecs→stop_wait_secs`, `stopsignal→stop_signal`, `exitcodes→exit_codes`, `stdout_logfile→logs.stdout`.
5. **Core 5-section mapping**: `[unix_http_server]`, `[inet_http_server]`, `[supervisord]`(logging subset), `[program:x]`, `[group:x]`.

### P1

6. `[include]` `files`(glob + merge).
7. `[rpcinterface:*]` tolerance (parse and ignore).
8. `[supervisorctl]` → CLI defaults (in coordination with `CLI_COMPAT.md` `-c`) — **done** §8 OI-1.
9. ~~`priority` range arbitration (`0..=999`)~~ **done** (`MAX_PRIORITY`).
10. `nodaemon` / `logfile_backups` / `identifier` value handling polish — **done** §8 OI-4.

### P2

11. `[supervisord]` runtime-surface fields (`pidfile`/`nodaemon`/`minfds`/`minprocs`/`umask`/`directory`/`childlogdir`/`silent`) → depends on SUPERVISORD §7 #12 — **done** §8 OI-8 (pidfile/minfds/minprocs/nodaemon/silent).
12. `[unix_http_server]` `chown` (`chmod` implemented, see §4.1).
13. Independent stdout/stderr `maxbytes`/`backups` (currently a single shared `logs` value) — **open** §8 OI-10.
14. Daemon `[supervisord] environment` without `set_var` side effect — **done** §8 OI-6.

### Not Supported

15. `[fcgi-program:x]` (complex, not present in the Go version either).
16. `[program:x]` `stdout_capture_maxbytes`/`stderr_capture_maxbytes`/`*_syslog`/`serverurl` childutils.
17. `[supervisord]` `nocleanup`/`strip_ansi`.
18. `[supervisorctl]` `prompt`/`history_file`.

---

## 8. Open Issues (pending) — go field audit → tech approach & priority

> Audit basis: go-supervisord `config/config.go` (lex stores all keys; gaps are **no consumers**, not parse drops) + our `adapter.rs`. Priority here reuses §2; “go status” distinguishes “we lag go” vs “both lag Python” vs “ours-only bug”.

| ID | Issue | Go status | Ours | Technical approach | Priority |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **OI-1** | `[supervisorctl]` `serverurl`/`username`/`password` never reach the client | consumed (client) | **implemented** (`CliDefaults` + `resolve_endpoint_candidates` seeds; CLI flags still win) | Store a `SupervisorConfig`-adjacent `CliDefaults { serverurl, username, password }` **outside** the daemon `server` auth fields (or a skipped field on config). In `cli::resolve_endpoint_candidates`, when `-s`/`-u`/`-p` absent, seed candidates/auth from `CliDefaults`. Wire via `CLI_COMPAT` §5.1.4. **Do not** map into `server.uds_*`/`server.username` (those are daemon listener credentials). | **P1** → done |
| **OI-2** | go extension `envFiles` (comma list of `.env` paths) on `[program:x]` / `[eventlistener:x]` / `[program-default]` | parsed & loaded at spawn | **implemented** (parse + absolutize + load before `environment`; missing file → warn skip) | Add `env_files: Vec<PathBuf>` to `ProgramConfigRaw`/`ProgramDefaults`/EL raw; INI `envFiles` alias; resolve relative to config dir (`abs_path`); load `KEY=VALUE` lines at spawn **before** `environment` (environment wins). Skip missing file with warn (align go) or hard error (Python has no equivalent — document choice). | **P1** → done |
| **OI-3** | go `killwaitsecs` (post-SIGKILL reap wait, default 2) | honored (`process.go`) | **implemented** (`kill_wait_secs`, default `DEFAULT_KILL_WAIT`=2s) | Promote to config: `kill_wait_secs` on program (default 2s = current `DRAIN_TIMEOUT`). INI: parse `killwaitsecs` → `kill_wait_secs`. Use in post-`force_kill` wait instead of raw `DRAIN_TIMEOUT`. YAML: optional field with serde default. | **P2** → done |
| **OI-4** | `nodaemon` only accepts Rust `bool` (`true`/`false`); `logfile_backups` parse failure is **silently dropped**; `silent` ignored | nodaemon/`logfile*` consumed properly | **implemented** (`string_to_bool` for nodaemon/silent; backups parse-or-Err; silent → `LoggingConfig.silent` + console layer skip) | Route `nodaemon` through `string_to_bool` (yes/no/1/0/on/off). Route backups through parse-or-`Err` like program backups. Accept `silent` → `logging.level=error` (or dedicated flag if schema grows) or keep P2 with warn. | **P1** → done |
| **OI-5** | go `liveness_check_*` (8 keys: script/period/timeout/initial_delay/thresholds/actions) | fully consumed | **implemented** (mapped to `HealthCheckConfig` exec; only `restart` failure action; others warn) | Map subset onto existing `HealthCheckConfig` (exec/http/tcp): `liveness_check_script` → `exec`, period/timeout/thresholds → matching fields; `success_action`/`failure_action` — only map actions we support (`restart`); otherwise warn. Reject unknown action values. | **P2** → done |
| **OI-6** | `[supervisord] environment` applied via `std::env::set_var` (process-global side effect at **parse** time) | only program-level env honored | **implemented** (store on config; `set_var` at daemon startup, never at parse) | Stop mutating process env during parse. Prefer: (a) keep a `daemon_environment` map on config and apply in daemon startup **before** children spawn, or (b) document that INI `[supervisord] environment` seeds process env at load (matches go’s implicit “parent env”) and gate behind explicit load path with a comment. Prefer (a) if field lands with OI-8; else (b) + test. | **P2** → done |
| **OI-7** | go `syslog_facility`/`syslog_tag`/`syslog_{stdout,stderr}_priority` (program) | consumed (Unix logger) | **not parsed**; program `*_syslog` Not Supported | Keep **Not Supported** until a log-transport abstraction exists (file-only today). If later: parse into optional `LoggingTransport::Syslog { .. }` Unix-only; ignore with warn on Windows. | **Not Supported** (revisit with #12) |
| **OI-8** | go honors `pidfile` / `minfds` / `minprocs`; listed P2 under SUPERVISORD #12 but **do not parse** | implemented | **implemented** (schema + adapter + write/remove pidfile + Unix `setrlimit` best-effort) | **Bundle under SUPERVISORD #12**: add `pidfile: Option<PathBuf>`, `minfds`/`minprocs: Option<u32>` to daemon config; adapter maps INI keys; runtime: write/remove pidfile on start/shutdown; `setrlimit(RLIMIT_NOFILE/RLIMIT_NPROC)` on Unix at startup (best-effort warn on failure). Windows: pidfile only; rlimits N/A. | **P2** → done |
| **OI-9** | go `stopasgroup`/`killasgroup` | consumed (group signals) | **implemented** (Unix group vs pid; Windows ignore; XML-RPC snapshot from config; `stop && !kill` rejected) | Add `stop_as_group`/`kill_as_group: Option<bool>` to program raw; Unix: send stop/kill to process **group** (`kill(-pgid)`) when true (spawn already `setpgid`); default false for Python parity of single-PID signal unless Python config says true. Windows: ignore + warn. Update XML-RPC `getProcessInfo` snapshot from config. | **P2** → done |
| **OI-10** | Independent stdout/stderr `maxbytes`/`backups` (Python has separate keys; we share one `logs.max_bytes`/`logs.backups`) | separate keys conceptually | single shared field; adapter `find_map` first of stdout/stderr keys | Schema: add optional `stdout_max_bytes`/`stderr_max_bytes`/`stdout_backups`/`stderr_backups` **or** nested `logs.stdout.*`. Adapter: prefer stdout key for stdout pump, stderr for stderr (stop first-wins). Backward compat: shared `max_bytes` remains fallback. | **P2** (checklist #13) |
| **OI-11** | Unknown INI keys under known sections are silently ignored (no warn) | lex stores; getters ignore | **implemented** (`warn_unknown_keys` allowlists + program known-key list) | After section adapt, diff section key set vs known-key allowlist per section; `tracing::warn!` for leftovers (not error — go/Python both tolerate). Enable behind `log` level ≥ debug or always warn once per key. | **P2** → done |

### 8.1 Suggested landing order

1. **P1 batch (OI-1, OI-4, OI-2)** — unblocks `supervisorctl -c` defaults, correct value parsing, and go-parity env files without schema surgery on daemon runtime. **Status: implemented + tested.**
2. **P2 batch A (OI-3, OI-9, OI-10)** — process lifecycle / log rotation knobs; each is a small schema + adapter + runtime touchpoint. **OI-3/OI-9: implemented; OI-10 remains open.**
3. **P2 batch B (OI-5, OI-6, OI-8, OI-11)** — health mapping polish, daemon env ownership, #12 runtime surface, observability of ignored keys. **Status: implemented + tested.**
4. **Not Supported** (OI-7) — revisit only with a syslog transport design.

### 8.2 Explicitly out of scope (go dead keys — do not chase)

Keys that **go parses but never consumes** are **not** automatic requirements for us; we implement them only when Python baseline demands or our YAML already has them:

| Key | Go | Decision for us |
| :--- | :--- | :--- |
| `[supervisord]` `umask`/`user`/`directory`/`childlogdir`/`nocleanup`/`strip_ansi` | ignored | `umask`/`directory`/`childlogdir` stay P2 where useful; `nocleanup`/`strip_ansi` Not Supported |
| `[unix_http_server]` `chmod`/`chown` | ignored (no consumer) | we **implement chmod**; `chown` P2 |
| `[supervisorctl]` `prompt`/`history_file` | ignored | Not Supported |
| program `numprocs_start` | overwritten (`i-1`) | we honor explicit value |
| `priority` on group | not propagated | we map group priority |
| `stdout_capture_maxbytes` / event capture | not wired | Not Supported until capture/event bus |
| `[rpcinterface]` factory string | ignored | still tolerated (OI N/A) |

### 8.3 go documentation traps (reference only)

Do not treat go README as contract: `restartpause` case sensitivity, `remote_1_passwrod` typo sample, `loglevel=warning` only accepting `warn`, “minfds/minprocs = not support” while code implements them. Our §4/§6 tables remain the contract.

---

## 9. Acceptance

**P0 acceptance**: a standard production `supervisord.conf`(containing `[unix_http_server]`/`[supervisord]`/`[rpcinterface:supervisor]`/`[program:x]`/`[group:x]`) loads **literally**, macros expand correctly, and `resolve_programs` output matches the equivalent YAML.

```bash
rsupervisord -c /etc/supervisord.conf          # INI load
rsupervisord -c /etc/rsupervisord.yaml         # YAML load (regression)
```

**Equivalence acceptance**: the same config expressed in INI / YAML respectively, with `resolve_programs()` results field-for-field equal.

**Open-issue acceptance** (per §8 row): unit test under `tests/ini_tests.rs` covering the mapped keys; OI-1 also gets a CLI resolution test (`resolve_endpoint_candidates` with fixture INI `[supervisorctl]`).

---

## 10. Relationship to Other Documents

- Client arguments and `[supervisorctl]` consumption: see `CLI_COMPAT.md` (esp. §5.1.4 ↔ §8 OI-1).
- `[rpcinterface]` and the XML-RPC method surface: see `SUPERVISORD_COMPAT.md` §7 #5.
- `[eventlistener:x]`: see `EVENTLISTENER_COMPAT.md` (runtime) and `SUPERVISORD_COMPAT.md` §7 #6 (history).
- Daemon runtime surface (`pidfile`/rlimit etc.): see `SUPERVISORD_COMPAT.md` §7 #12 ↔ §8 OI-8.
- go-supervisord reference tree (`go-supervisord/`, untracked): research only — not a shipped dependency.