# rsupervisord vs go-supervisord Feature Parity (GO_COMPAT.md)

| Document Version | Status | Scope |
| :--- | :--- | :--- |
| **v1.0.0** | Inventory complete; **tracking document** | Every consumed feature in `go-supervisord` (ochinchina, commit `7a73369`) vs rsupervisord's current state |

> **Purpose**: release-readiness cross-check that "go does it → we either do it, or consciously defer it". Python-contract items live in `INI_COMPAT.md` / `XMLRPC_COMPAT.md` / `CLI_COMPAT.md` / `LOG_COMPAT.md`; this file is the **go-specific delta**. Deferrals are explicit — nothing here silently lags.
>
> Sources: go inventory cites `go-supervisord/` paths (untracked reference tree); rs cites `src/…` file:line.

**Legend**: ✅ implemented · 🟡 partial / divergent · ❌ not implemented (with priority) · ⛔ deliberately not chased (go bug, dead code, or we diverge better)

---

## 1. CLI

| Feature | go | rsupervisord | Notes |
| :--- | :--- | :--- | :--- |
| `supervisord` (no subcommand) → run server | ✅ `main.go:166` | ✅ `src/main.rs` | |
| `-c/--configuration`, `-d/--daemon` | ✅ | ✅ / ✅ (`--nodaemon` inverted; `daemon.rs:25`) | |
| `--env-file` global flag | ✅ `main.go:25` | ❌ P2 | program-level `env_files` ✅ (OI-2); daemon-level env-file not wired |
| `init -o/--output` config template | ✅ `config_template.go:157` | ❌ P2 | nice-to-have bootstrap; no stock template in rs |
| `service install/uninstall/start/stop` | ✅ `service.go:85` | ✅ `src/service/mod.rs:14` (systemd + Windows SCM) | rs covers both platforms |
| `version` | ✅ | ✅ | |
| `LOG_FORMAT=json` env | ✅ `main.go:32` | ❌ P3 | rs has structured logging internally; JSON switch is cosmetic |
| `STATIC_DIR`/`STATIC_ZIP` asset override | ✅ `assets.go:23` | ❌ P3 | rs embeds via `rust-embed` (`src/server/web.rs:12`) |
| zombie reaper at startup | ✅ `main.go:163` | ❌ P2 (Unix) | rs relies on Tokio waitpid per-child (`src/platform/unix/mod.rs:98`); reaper only matters for reparented orphans |

### 1.1 `supervisorctl` subcommands

| Command | go | rsupervisord | Notes |
| :--- | :--- | :--- | :--- |
| `status` | ✅ | ✅ `commands.rs:489` region | `CLI_COMPAT.md` matrix |
| `start`/`stop`/`restart` (`all`, `group:*`) | ✅ | ✅ | |
| `add`/`remove` | ✅ | ✅ `commands.rs:845/854` | |
| `clear` | ✅ | ✅ `commands.rs:897` | |
| `reread`/`update`/`reload` | ✅ | ✅ `commands.rs:507/530` + `cli/commands.rs` | |
| `signal` | ✅ | ✅ `commands.rs:769` | |
| `pid` | ✅ | ✅ `commands.rs:742` | |
| `tail` (+`-f`) | ✅ | ✅ `commands.rs:802` | |
| `fg` (foreground attach) | ✅ `ctl.go:935` | ✅ `commands.rs:929` + `stdin` | rs: `fg` + `stdin` cmd (`args.rs:225/233`) |
| `shutdown` | ✅ | ✅ | |
| `maintail` | ❌ (go lacks) | ✅ `commands.rs:897` | rs **ahead** of go (Python parity) |
| `avail` / `events` | ❌ (go lacks) | ✅ `args.rs:169/230` | rs ahead of go |
| `[supervisorctl] prompt`/`history_file` | ⛔ no match | ⛔ Not Supported | aligned |

---

## 2. Config sections

### 2.1 `[unix_http_server]` / `[inet_http_server]`

| Feature | go | rsupervisord | Notes |
| :--- | :--- | :--- | :--- |
| `file`, `username`, `password` (UDS) | ✅ | ✅ | |
| `port`, `username`, `password` (TCP) | ✅ | ✅ | |
| `chmod` | ⛔ no match (go never sets socket mode) | ✅ implemented | rs **ahead** (Python parity, OI era) |
| `chown` | ⛔ no match | 🟡 P2 | `INI_COMPAT` §7 #12 |
| `nodename` | ✅ `supervisor.go:263` | ❌ P3 | multi-node label; meaningless without cluster |
| `remote_N_port/node/user/password` | ✅ `supervisor.go:700` | ❌ deferred | see §8 multi-node |

### 2.2 `[supervisord]`

| Feature | go | rsupervisord | Notes |
| :--- | :--- | :--- | :--- |
| `logfile` (+`/dev/stdout` short-circuit) | ✅ | 🟡 | file/stdout ✅; `logfile=syslog` warn-only (`LOG_COMPAT` §6.6) |
| `logfile_maxbytes`/`backups` | ✅ | ✅ | |
| `logfile_timestamp_suffix` (go default **true**) | ✅ | ✅ (default **false**) | deliberate: Python numeric parity (`LOG_COMPAT` §11) |
| `loglevel` | ✅ | ✅ | |
| `pidfile` | ✅ `supervisor.go:760` | ✅ (OI-8) | |
| `identifier` | ✅ | ✅ | |
| `minfds`/`minprocs` setrlimit | ✅ `rlimit.go:12` | ✅ `daemon.rs:178/446` | go's template falsely says "not support" |
| `nodaemon` | ⛔ no match | ✅ (OI-4) | rs ahead (Python parity) |
| `environment` (daemon env) | ⛔ no match | ✅ (OI-6) | rs ahead; go only expands `%(ENV_X)s` |
| `umask`/`user`/`directory`/`childlogdir`/`nocleanup`/`strip_ansi` | ⛔ no match | 🟡/⛔ per `INI_COMPAT` §8.2 | not chased because go also ignores; `childlogdir` deferred |

### 2.3 `[program:x]` — identity & spawn

| Feature | go | rsupervisord | Notes |
| :--- | :--- | :--- | :--- |
| `command`, `process_name`, `numprocs` | ✅ | ✅ | |
| `numprocs_start` (user value) | ⛔ overwritten (`config.go:772`) | ✅ honored (`schema.rs:862`) | rs ahead (Python) |
| `process_num` 1-based | 🟡 go quirk | ✅ Python 0-based default | deliberate divergence |
| `environment` (expand + apply) | ✅ | ✅ | go fails to dedupe overrides (`process.go:1003`); rs replaces properly |
| `envFiles` | ✅ `config.go:575` | ✅ (OI-2, `envfile.rs:50`) | |
| `directory` | ✅ | ✅ | |
| `user` (`user:group`) | ✅ `process.go:1163` | ✅ `unix/mod.rs:503` | |
| `umask` (program) | ⛔ no match | ⛔ Not Supported | aligned |
| `serverurl` | ⛔ no match | ⛔ Not Supported | aligned |

### 2.4 `[program:x]` — start/restart/stop policy

| Feature | go | rsupervisord | Notes |
| :--- | :--- | :--- | :--- |
| `autostart`/`startsecs`/`startretries` | ✅ | ✅ | |
| `autorestart` (incl. `unexpected-exitcode`) | ✅ `process.go:554` | ✅ (`unexpected` three-state) | go's 4th value `unexpected-exitcode` = its own extension; see §9 |
| `exitcodes` | ✅ | ✅ | |
| `priority` (program) | ✅ | ✅ | |
| `depends_on` DAG start order | ✅ `process_sort.go:38` | ✅ `manager/dag.rs` | go: **no cycle detection** (hangs); rs validates cycles → divergence in rs's favor |
| `restartpause` | ✅ `process.go:488` | ✅ `program/config.rs:256` | |
| `stopsignal`/`stopwaitsecs` | ✅ | ✅ | |
| `killwaitsecs` | ✅ `process.go:1276` | ✅ (OI-3, `process.rs:1292`) | |
| `stopasgroup`/`killasgroup` | ✅ | ✅ (OI-9) | |
| `pre_start_hook`/`pre_stop_hook` | ✅ `process.go:505` | ✅ (`schema.rs:172`, `process.rs:1013`) | |
| `cron` | ✅ `process.go:273` | ✅ (`schema.rs:299`, `manager/cron.rs:83`) | go: cron entries accumulate across reloads (bug); rs + rs also has `cron_stop` (ahead) |
| `liveness_check_*` (8 keys) | ✅ `process.go:124-142` | ✅ (OI-5 → `health_check`) | rs additionally supports http/tcp checkers (ahead); go's actions `restart\|stop\|script` — rs maps restart, others warn |
| `restart_when_binary_changed` (+cmd/signal) | ✅ `process.go:641` | ✅ `manager/watch.rs:349` | |
| `restart_directory_monitor`/`restart_file_pattern` (+cmd/signal) | ✅ `process.go:667` | ✅ `manager/watch.rs:382` | |
| `conf_file` (exposed via `/conf/{program}`) | ✅ `confApi.go:15` | ❌ P3 | only consumed by go's `/conf` REST (see §4) |
| start ordering: `depends_on` then `priority` asc | ✅ | ✅ | |
| stop ordering: reverse | ⛔ same sort as start (go bug) | ✅ reverse + desc priority (`dag.rs:136`) | rs ahead (Python semantics) |

### 2.5 `[program:x]` — logging keys

Covered fully by `LOG_COMPAT.md`. Summary: destination grammar, OI-10 four-field rotation, `*_syslog` composites, go `syslog_*` keys, `timestamp_suffix` — all ✅. Main `logfile=syslog` 🟡 (`LOG_COMPAT` §6.6).

### 2.6 `[eventlistener:x]`

| Feature | go | rsupervisord | Notes |
| :--- | :--- | :--- | :--- |
| `events` list (incl. abstract types) | ✅ `process.go:1066` | ✅ `eventlistener/config.rs` | |
| `buffer_size` | ✅ `process.go:1105` | ✅ | |
| generic program keys apply | ✅ | ✅ | |
| separate map (invisible to `getAllProcessInfo`) | 🟡 go quirk | ✅ rs: EL visible via dedicated APIs (Python parity) | divergence — rs follows Python |
| always started (ignores `autostart`) | 🟡 go quirk | ✅ Python semantics | rs follows Python |
| `result_handler` | ⛔ no match | ⛔ Not Supported | aligned |

### 2.7 `[group:x]`, `[include]`, `[program-default]`, `[fcgi]`, `[rpcinterface]`

| Feature | go | rsupervisord | Notes |
| :--- | :--- | :--- | :--- |
| `programs` list + membership diff | ✅ | ✅ | |
| group `priority` propagation | ⛔ no match (template lies) | ✅ propagated (`schema.rs:963`) — but ordering only uses program prio (`dag.rs:120`) 🟡 | partial: field exists + reported, not used as sort tiebreak |
| `[include] files` glob | ✅ single-level (`config.go:168`) | ✅ | rs supports nested includes? → verified in INI_COMPAT #6 |
| `[program-default]` | ✅ (programs only) | ✅ | |
| `[fcgi-program:x]` | ⛔ | ⛔ | aligned |
| `[rpcinterface:x]` consumed | ⛔ (hard-coded codec) | 🟡 tolerated/ignored | aligned with Python accept |

---

## 3. Process lifecycle & platform

| Feature | go | rsupervisord | Notes |
| :--- | :--- | :--- | :--- |
| 8 states + `statename` | ✅ | ✅ | |
| `startsecs` gate, backoff, exit monitor | ✅ | ✅ | |
| `setuid`/`setgid` run-as | ✅ `set_user_id.go` | ✅ `unix/mod.rs:221` | |
| `pdeathsig=SIGKILL` (Linux) | ✅ `pdeathsig_linux.go` | ❌ P2 (Unix) | rs daemon reaps its own children on exit paths; only kills orphaned children if daemon dies — gap worth noting |
| `Setpgid` for group signals | ✅ | ✅ | rs spawn sets pgid (OI-9) |
| Windows: `taskkill /T` tree kill | ✅ `signal_windows.go:47` | ✅ Windows job/tree kill path | verify parity detail in platform code |
| rlimit check on reload (`os.Exit(1)` on fail) | ✅ (harsh) | 🟡 rs re-validates config without killing daemon | divergence — rs gentler |
| daemonize (`-d`) | ✅ go-daemon | ✅ | |
| stdin to process (`sendProcessStdin`) | ✅ | ✅ `supervisor.rs:256` + REST `/stdin` + `ctl stdin` | |

---

## 4. XML-RPC & REST

### 4.1 XML-RPC methods

All 31 `supervisor.*` methods go registers are implemented in rs (verified via `XMLRPC_COMPAT.md`): `getVersion/getAPIVersion/getIdentification/getState/getPID/readLog/clearLog/shutdown/restart/getProcessInfo/getSupervisorVersion/getAllProcessInfo/start*/stop*/signal*/sendProcessStdin/sendRemoteCommEvent/reloadConfig/add/removeProcessGroup/read/tailProcess*Log/clearProcess*Logs`.

| Divergence | go | rs |
| :--- | :--- | :--- |
| `getVersion` value | `"3.0"` | Python `4.2.5` (`conftest.EXPECTED_SUPERVISOR_VERSION`) — rs follows Python |
| `getAPIVersion` | aliases `getVersion` | ✅ distinct + correct |
| `system.*` (listMethods/help/multicall) | ⛔ | ✅ present (rs ahead — Python parity) |
| `spawnerr` in ProcessInfo | field exists, never written (go bug) | ✅ populated |
| `getState` SHUTDOWN | ⛔ | ✅ |

### 4.2 HTTP/REST endpoints

| Endpoint | go | rsupervisord | Notes |
| :--- | :--- | :--- | :--- |
| `/RPC2` + basic auth | ✅ | ✅ (IPC + TCP, optional auth) | |
| `/program/list`, `/program/info/{name}`, `/program/start|stop|restart/{name}` (incl. `/{node}`) | ✅ `rest-rpc.go:117` | 🟡 different surface: `/api/v1/status`, `/api/v1/programs/{name}/start` (`api.rs:91-95`) | **not byte-compatible** with go clients; rs chose a cleaner REST dialect — acceptable divergence, documented |
| `/program/log_stdout|stderr/{name}` | ✅ | 🟡 `/api/v1/programs/{name}/logs` | same story |
| `/logtail/{program}/stdout` (chunked follow) | ✅ (marked buggy/deprecated in-source) | 🟡 `/logs/stream` SSE instead | rs ahead (SSE); go's own comment says deprecated |
| `/conf/{program}` | ✅ | ❌ P3 | depends on `conf_file` key; low value — configs are YAML/INI the user already has |
| `/confFile`, `/log` (serve missing files → 404) | 🟡 go dead endpoints | ⛔ | not chased (go bug) |
| `/supervisor/listNodes`, `/{node}/ping`, node proxying | ✅ | ❌ deferred | §8 |
| `/metrics` Prometheus | ✅ `xmlrpc.go:198` | ❌ **P1 candidate** | tracked as "Skin" item `SUPERVISORD_COMPAT.md:131`; unauthenticated in go — if implemented, gate behind auth |
| `/log/{program}/` directory browsing (basic auth) | ✅ | 🟡 via `/api/v1/programs/{name}/logs` (read/tail/stream) | equivalent capability, different shape |
| `/` static web GUI (basic auth) | ✅ | ✅ `web/index.html` SPA + `/api/v1/*` | rs GUI is Vue SPA — richer (group ops, SSE events, auth login UI) |

---

## 5. Web GUI

| Feature | go | rsupervisord |
| :--- | :--- | :--- |
| Embedded single-page UI | ✅ Alpine+Tailwind `webgui/index.html` (27 KB) | ✅ Vue SPA `web/index.html` (75 KB, `server/web.rs:12`) |
| Start/stop/restart from UI | ✅ | ✅ (`/api/v1/programs/{name}/…`) |
| Live log tail | ✅ via `/logtail` | ✅ `/logs/stream` SSE |
| Event viewer page | ⛔ | ✅ `/events` SSE |
| Auth UI (login) | ⛔ (browser native basic prompt) | ✅ `/auth/login` (`api.rs:113`) |
| Group operations | 🟡 | ✅ `/api/v1/groups/…` |
| `conf.html`/`log.html` | ⛔ referenced but absent (404) | ⛔ not chased |

---

## 6. Logging

Full detail in `LOG_COMPAT.md`. go-specific delta:

| Feature | go | rsupervisord |
| :--- | :--- | :--- |
| destination kinds (file/syslog/memory/AUTO/composite/dev/null) | ✅ | ✅ |
| remote syslog `[proto:]host[:port]` | ✅ | ✅ |
| Windows syslog | ⛔ silent no-op | ✅ hard config error (deliberate divergence) |
| main `logfile=syslog` | ✅ | 🟡 warn + console only (`LOG_COMPAT` §6.6) — **the one open P1** |
| `timestamp_suffix` default | true | false (Python parity) |
| debug child mirror into main log | ⛔ | ❌ P2 (Python-only feature; `LOG_COMPAT` §6.3) |
| `PROCESS_LOG_*` per-line events | ✅ | ✅ |

---

## 7. Event system

| Event type | go | rsupervisord |
| :--- | :--- | :--- |
| `PROCESS_STATE_*` (8) | ✅ | ✅ |
| `PROCESS_LOG_STDOUT/STDERR` | ✅ | ✅ |
| `PROCESS_COMMUNICATION_STDOUT/STDERR` | ✅ | ❌ P2 (capture mode deferred; `EVENTLISTENER_COMPAT.md:192`) |
| `REMOTE_COMMUNICATION` (`sendRemoteCommEvent`) | ✅ | ✅ `supervisor.rs:316` |
| `TICK_5/60/3600` | ✅ | ✅ `supervisor.rs:799` |
| `SUPERVISOR_STATE_CHANGE_STOPPING` | ⛔ defined, never emitted | ✅ `pool.rs:218` |
| `SUPERVISOR_STATE_CHANGE_RUNNING` | ⛔ defined, never emitted | 🟡 branch exists, no publisher (minor dead code — candidate cleanup) |
| `PROCESS_GROUP_ADDED/REMOVED` | ⛔ defined, never emitted | ✅ `pool.rs:226` |
| `supervisorctl events` / HTTP `/events` stream | ⛔ | ✅ (`args.rs:230`, SSE `/events`) |
| listener lifecycle bugs (EL ignored on reload, autostart ignored) | 🟡 | ✅ correct |

---

## 8. Multi-node / cluster (go extension)

go: `nodename`, `remote_N_*`, `NodeLoginManager`, per-node proxy on REST (`listNodes`, `pingNode`, `/{node}/…`), `node` field in `ProcessInfo`.

rsupervisord: ❌ **deferred (P3)** — no partial pieces exist. Rationale: single-node supervisor is the 95% use case; multi-node needs a discovery/auth/proxy design, and go's implementation is Unix-credential-bound (`remote_N_*` only read when `[inet_http_server]` exists). Track under a future enhancement, not a parity blocker. If `nodename`/`node` appears in configs it is currently unknown-key-warned (OI-11 behavior).

---

## 9. Deliberate divergences (rs behavior ≠ go behavior, on purpose)

| Topic | go | rsupervisord | Why |
| :--- | :--- | :--- | :--- |
| Version reported | `3.0` | `4.2.5` | rs targets Python contract |
| `timestamp_suffix` default | true | false | Python numeric `.1` files |
| `AUTO` | memory(1000) | INI→default file path; literal `auto`→ring | backward compat (see `LOG_COMPAT` §6.1) |
| depends_on cycles | infinite hang | config error | safety |
| stop ordering | same as start (bug) | reverse | Python semantics |
| `numprocs_start` | ignored | honored | Python semantics |
| key case | exact/case-sensitive (template camelCase keys silently dead) | keys lowercased in INI parser | robustness |
| Windows syslog | silent stub | config error | fail-loud policy |
| rlimit failure on reload | `os.Exit(1)` | warn + continue | availability |
| Event listeners in process list | hidden | visible (Python parity) | Python semantics |
| `/metrics` auth | none | n/a (not implemented) | if added later: behind auth |

---

## 10. go bugs NOT to replicate

Verified during inventory; rs either already correct or unimplemented:

1. `stopProcessGroup` inverted condition (only stops non-running) — `supervisor.go:403`
2. `startAllProcesses`/`stopAllProcesses` always/garbled failure replies — `supervisor.go:335,437`
3. `depends_on` cycle hang — `process_sort.go:38`
4. `environment` override dedupe missing — `process.go:1003`
5. camelCase template keys ignored (case-sensitive lookup) — `config.go:659`
6. unauthenticated `/metrics`, `/log`, `/confFile` — `xmlrpc.go:184,196,198`
7. removed event listeners never stopped on reload; EL `autostart` ignored — `supervisor.go:569,630`
8. cron entries accumulate across reloads — `process.go:264`
9. `SUPERVISOR_STATE_CHANGE_*` / `PROCESS_GROUP_*` defined but never emitted — `events.go:733,805`
10. `getAPIVersion` = `getVersion` — `xmlrpc.go:239`
11. reload rlimit failure kills daemon — `supervisor.go:559`
12. systemd unit `ExecStop=shutdown`/`ExecReload=reload` reference non-existent top-level commands — `supervisord.service:11`
13. 1-based `process_num` vs Python 0-based — `config.go:741`

---

## 11. Action items from this audit

| # | Item | Priority | State |
| :--- | :--- | :--- | :--- |
| 1 | Main `logfile=syslog` sink (close `LOG_COMPAT` §6.6) | P1 | open |
| 2 | Decide `/metrics` (Prometheus) — tracked as Skin item | P2 | open (design decision) |
| 3 | `--env-file` daemon flag / `init` template / `LOG_FORMAT` env | P3 | deferred |
| 4 | pdeathsig + zombie reaper review (Unix hardening) | P2 | deferred |
| 5 | Multi-node (`remote_N_*`, `nodename`) | P3 | deferred (§8) |
| 6 | `SUPERVISOR_STATE_CHANGE_RUNNING` publisher dead-branch cleanup | P3 | open (code hygiene) |
| 7 | go REST path aliases (`/program/start/{name}`) for go client compat | P3 | deferred — only if third-party go clients matter |
