# rsupervisord: INI Configuration Compatibility Analysis (INI_COMPAT.md)

| Document Version | Status | Target Language | Scope |
| :--- | :--- | :--- | :--- |
| **v1.0.0** | Draft / For Review | Rust (Edition 2024) | Section/field mapping, differences, examples, and priorities from Python `supervisord.conf`(INI) into the rsupervisord configuration model |

---

## 1. Purpose & Scope

Question answered: **what must be implemented for a standard Python `supervisord.conf`(INI) to be loaded directly by rsupervisord.**

- **Authoritative baseline**: Python Supervisor **4.2.5**(`supervisor/skel/sample.conf` + `options.py`).
- **Reference implementation**: Go `ochinchina/supervisord` `config/config.go`(commit `7a73369`).
- **Current-state baseline**: `src/config/schema.rs`(YAML schema)、`src/config/expand.rs`(macro expansion)、`src/program/config.rs`(runtime configuration).
- **Related**: the client-side semantics of section `[supervisorctl]` are covered in `CLI_COMPAT.md`; `[eventlistener:x]` in `SUPERVISORD_COMPAT.md` §7 #6; `[rpcinterface]` in §7 #5.

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
| 3 | `[supervisord]` | daemon global | `logging` + (runtime surface, §7 #12) | **P0**(logging part) |
| 4 | `[program:x]` | supervised programs | `programs.x` | **P0** |
| 5 | `[group:x]` | heterogeneous process groups | `groups.x` | **P0** |
| 6 | `[supervisorctl]` | **client** connection config | CLI defaults (not daemon) | **P1** |
| 7 | `[include]` | configuration inclusion | file merging | **P1** |
| 8 | `[rpcinterface:supervisor]` | RPC interface registration | tolerated/ignored (§7 #5) | **P1** |
| 9 | `[eventlistener:x]` | event listener programs | conditional (§7 #6) | **Not Supported**(for now) |
| 10 | `[fcgi-program:x]` | FastCGI programs | — | **Not Supported** |

> Additionally: the Go version extends with `[program-default]`(this section does not exist in Python), to which rsupervisord's `program_defaults` corresponds. It can be supported as an extension as well.

---

## 4. Per-Section Field Mapping & Differences

### 4.1 `[unix_http_server]` → `server`

| INI Field | Current YAML | Difference / Action |
| :--- | :--- | :--- |
| `file` | `server.uds_path` | direct mapping |
| `username` | `server.username` | direct; **note**: shares the same field with `[inet_http_server]`, Python allows independent credentials per section → inconsistent |
| `password` | `server.password` | same as above |
| `chmod` | `server.uds_chmod` | implemented: octal mode applied at bind (Unix `set_permissions`, Windows pipe `SECURITY_ATTRIBUTES`, Windows file UDS protected DACL). Quoted string required in YAML. |
| `chown` | — | **P2** |

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
| `username` | `server.username` | shared with UDS → inconsistent |
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
| `logfile_maxbytes` | `logging.max_bytes` | direct; default 50MB (Python) vs 20MB (ours) |
| `logfile_backups` | `logging.backups` | direct; default 10 vs 3 |
| `loglevel` | `logging.level` | direct |
| `pidfile` | — | **P2**(§7 #12) |
| `nodaemon` | — | **P2**(§7 #12) |
| `silent` | — | **P2** |
| `minfds` | — | **P2**(§7 #12 rlimit) |
| `minprocs` | — | **P2**(§7 #12 rlimit) |
| `umask` | — | **P2** |
| `user` | — | not supported/partial (setuid) |
| `identifier` | — | **P2** |
| `directory` | — | **P2** |
| `nocleanup` | — | **Not Supported** |
| `childlogdir` | — | **P2**(AUTO child log directory) |
| `environment` | — | **P2**(daemon-level environment) |
| `strip_ansi` | — | **Not Supported** |

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
| `priority` | `priority` | **range conflict**: Python allows `999`, ours validates `[0,99]` → see §6 |
| `autostart` | `autostart` | boolean parsing difference (see §5) |
| `startsecs` | `start_secs` | naming; default 1 (same as Python) |
| `startretries` | `start_retries` | naming |
| `autorestart` | `autorestart` | **value mapping**: `false→never`,`true→always`,`unexpected→unexpected` |
| `exitcodes` | `exit_codes` | list parsing (`0,2` → `[0,2]`) |
| `stopsignal` | `stop_signal` | alias `SIGTERM` already supported |
| `stopwaitsecs` | `stop_wait_secs` | naming |
| `stopasgroup` | — | **Not Supported/partial**(Unix process-group semantics) |
| `killasgroup` | — | **Not Supported/partial** |
| `user` | `user` | direct |
| `redirect_stderr` | `logs.redirect_stderr` | direct |
| `stdout_logfile` | `logs.stdout` | **`AUTO`/`NONE` semantics**(see §5) |
| `stdout_logfile_maxbytes` | `logs.max_bytes` | naming; **ours uses a single value shared by stdout/stderr → inconsistent** |
| `stdout_logfile_backups` | `logs.backups` | same as above |
| `stderr_logfile` | `logs.stderr` | `AUTO`/`NONE` |
| `stderr_logfile_maxbytes` | `logs.max_bytes` | shared inconsistency |
| `stderr_logfile_backups` | `logs.backups` | shared inconsistency |
| `environment` | `environment` | **format**: `A="1",B="2"` → map (see §5) |
| `stdout_capture_maxbytes` | — | **Not Supported**(capture mode/event) |
| `stdout_events_enabled` | — | **Not Supported**(depends on §7 #6) |
| `stdout_syslog` | — | **Not Supported** |
| `stderr_capture_maxbytes` | — | **Not Supported** |
| `stderr_events_enabled` | — | **Not Supported** |
| `stderr_syslog` | — | **Not Supported** |
| `serverurl` | — | **Not Supported**(childutils) |

> ours/Go extension fields (not in Python): `depends_on`, `cron`, `pre_start`/`pre_stop`, `health_check`, `restart_*`. If they appear in INI, accept them as extensions.

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

### 4.6 `[supervisorctl]` → CLI defaults (P1)

| INI Field | Target | Action |
| :--- | :--- | :--- |
| `serverurl` | `rsupervisorctl -s` default | parsed when the client reads the `-c` file |
| `username` | `-u` default | same as above |
| `password` | `-p` default | same as above |
| `prompt` | — | **Not Supported**(interactive shell) |
| `history_file` | — | **Not Supported** |

> Note: the daemon itself does **not consume** `[supervisorctl]`; it only affects the client. See `CLI_COMPAT.md` §5.1.4.

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

### 4.9 `[eventlistener:x]` → Not Supported (conditional, §7 #6)

The fields are largely the same as `[program:x]`, with the additions of `events`(required), `buffer_size`(default 10), `priority` defaulting to `-1`, and `redirect_stderr` must be false.

**Decision**: **Not supported for now**; if §7 #6 is enabled, map it to a dedicated model later. When the INI parser encounters this section, it should raise a clear error or ignore it with a warning.

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
| `priority` | 999 (no upper bound) | 50, validates `≤99` | **P1**: relax to `0..=999` or explicitly reject out-of-range with an error |
| `startsecs` | 1 | 3 | keep ours (documented difference) |
| `startretries` | 3 | 3 | consistent |
| `stopwaitsecs` | 10 | 10 | consistent |
| `exitcodes` | `[0]` | `[0]` | consistent |
| `autorestart` | unexpected | unexpected | consistent |
| `logfile_maxbytes` | 50MB | 20MB | documented difference |
| `logfile_backups` | 10 | 3 | documented difference |
| `umask` | 022 | None | **P2** |
| `redirect_stderr` | false | false | consistent |
| `path_translation` | N/A (Python always CWD-resolves bare relative paths) | INI frontend forces `false` (YAML native default remains `true`) | **Aligned**: bare relative paths stay relative and resolve against daemon CWD, matching Python/go-supervisord |
| `allow_unelevated` | N/A (Python/go have no elevation gate on IPC) | INI frontend forces `true` (YAML native default remains `false`) | **Aligned**: no app-layer elevation check; access governed by socket file permissions only, matching Python/go |

> Default-value differences do not block loading; recommended: "if a config explicitly writes a value, use the explicit value"; when not written, use the ours default and note it in the docs.
> **Note on `path_translation` / `allow_unelevated`**: these are rsupervisord-only knobs with no Python INI key. The INI frontend forces them to baseline-aligning values (`false` / `true`) before `translate_paths` + `validate`, so a stock Python `supervisord.conf` loads with path and IPC semantics equivalent to Python supervisor / go-supervisord. YAML native configs keep the rsupervisord defaults (`path_translation: true`, `allow_unelevated: false`).

---

## 7. Implementation Checklist (by priority)

### P0 — INI front-end infrastructure

1. **INI parser**(new module `src/config/ini.rs`): sections, `key=value`, comment rules, line continuation, case-insensitive keys, per-value macro expansion.
2. **Suffix detection**: `SupervisorConfig::from_file`(schema.rs:332) dispatches by extension — `.ini`→INI pipeline, `.yaml/.yml`→YAML pipeline; **both produce the same `SupervisorConfig`**.
3. **Value-normalization layer**: lenient booleans, `autorestart` three-state mapping, list parsing (`exitcodes`/`programs`/`files`), dedicated `environment` parsing, `AUTO`/`NONE` log paths.
4. **Field alias table**: full mapping such as `startsecs→start_secs`, `startretries→start_retries`, `stopwaitsecs→stop_wait_secs`, `stopsignal→stop_signal`, `exitcodes→exit_codes`, `stdout_logfile→logs.stdout`.
5. **Core 5-section mapping**: `[unix_http_server]`, `[inet_http_server]`, `[supervisord]`(logging subset), `[program:x]`, `[group:x]`.

### P1

6. `[include]` `files`(glob + merge).
7. `[rpcinterface:*]` tolerance (parse and ignore).
8. `[supervisorctl]` → CLI defaults (in coordination with `CLI_COMPAT.md` `-c`).
9. `priority` range arbitration (`0..=999`).

### P2

10. `[supervisord]` runtime-surface fields (`pidfile`/`nodaemon`/`minfds`/`minprocs`/`umask`/`directory`/`childlogdir`/`identifier`/`environment`/`silent`) → depends on §7 #12.
11. `[unix_http_server]` `chown` (`chmod` implemented, see §4.1).
12. Independent stdout/stderr `maxbytes`/`backups`(currently a single shared `logs` value).

### Not Supported

13. `[eventlistener:x]`(conditional, §7 #6).
14. `[fcgi-program:x]`(complex, not present in the Go version either).
15. `[program:x]` `stdout_capture_maxbytes`/`stderr_capture_maxbytes`/`*_events_enabled`/`*_syslog`/`serverurl`.
16. `[supervisord]` `nocleanup`/`strip_ansi`.
17. `[supervisorctl]` `prompt`/`history_file`.

---

## 8. Acceptance

**P0 acceptance**: a standard production `supervisord.conf`(containing `[unix_http_server]`/`[supervisord]`/`[rpcinterface:supervisor]`/`[program:x]`/`[group:x]`) loads **literally**, macros expand correctly, and `resolve_programs` output matches the equivalent YAML.

```bash
rsupervisord -c /etc/supervisord.conf          # INI load
rsupervisord -c /etc/rsupervisord.yaml         # YAML load (regression)
```

**Equivalence acceptance**: the same config expressed in INI / YAML respectively, with `resolve_programs()` results field-for-field equal.

---

## 9. Relationship to Other Documents

- Client arguments and `[supervisorctl]` consumption: see `CLI_COMPAT.md`.
- `[rpcinterface]` and the XML-RPC method surface: see `SUPERVISORD_COMPAT.md` §7 #5.
- `[eventlistener:x]`(conditional): see `SUPERVISORD_COMPAT.md` §7 #6.
- daemon runtime surface (`pidfile`/rlimit etc.): see `SUPERVISORD_COMPAT.md` §7 #12.