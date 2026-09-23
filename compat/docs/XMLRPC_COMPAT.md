# rsupervisord: XML-RPC Compatibility Requirements (XMLRPC_COMPAT.md)

| Document Version | Status | Target Language | Scope |
| :--- | :--- | :--- | :--- |
| **v1.1.0** | Draft / For Review | Rust (Edition 2024) | XML-RPC wire protocol, Fault codes, `supervisor.*` / `system.*` method surface, per-method mapping and priorities; target: stock `supervisorctl` connect directly; executable baseline per §12 / [`../compat/README.md`](../compat/README.md) |

---

## 1. Purpose & Scope

Question answered: **To let a stock `supervisorctl` (Python 4.2.5) connect directly to rsupervisord, which XML-RPC capabilities must be implemented and what on-the-wire behavior must be achieved.**

- **Authoritative baseline**: `supervisor/rpcinterface.py` (method surface) and `supervisor/xmlrpc.py` (wire protocol / Faults / namespaces / `multicall`) of Python Supervisor **4.2.5**.
- **Current state baseline**: `src/server/api.rs` (REST routes + `AppState`), `src/server/auth.rs` (unified authorize + path-tier middleware + `SessionStore`), `src/manager/supervisor.rs` (`ManagerHandle` + `ManagerCommand`).
- **Related**: CLI-side arguments/exit codes see [`CLI_COMPAT.md`](./CLI_COMPAT.md); log byte-offset degradation see [`SUPERVISORD_COMPAT.md`](./SUPERVISORD_COMPAT.md) §7 #8; process groups/events see §7 #4/#6.

**Core conclusion (preview)**: XML-RPC is a **pure adaptation layer** — a new `src/server/xrpc.rs` mounts on the existing Axum route `/RPC2`, reuses `AppState`/`ManagerHandle`/Basic auth, and translates XML-RPC calls into the existing `ManagerCommand`. **No changes to the execution core are required**; the main work is **encode/decode, Fault mapping, name semantics (namespec/group), and log degradation**.

---

## 2. Priority Definitions

| Level | Meaning |
| :--- | :--- |
| **P0** | Make the everyday commands of a stock `supervisorctl` usable via direct connection (status/start/stop/restart/signal/tail/maintail/version/shutdown/reread/update/all). |
| **P1** | Full coverage of the remaining read-only/auxiliary methods (clear, read*, avail/getAllConfigInfo, system.* introspection, group-level signal). |
| **P2 / deferred** | Items with no corresponding underlying capability yet (currently none). |
| **Not supported / degraded** | Complex or conflicting with the architecture; the degraded behavior is explicitly declared. |

---

## 3. Wire Protocol Requirements (wire protocol)

| Item | Python behavior | rsupervisord requirement |
| :--- | :--- | :--- |
| Endpoint | HTTP `POST` to path **`/RPC2`** | Add a `/RPC2` route, integrated into the existing `build_router` (same UDS/TCP, same process) |
| Content-Type | `text/xml` for both request and response | Match; tolerate lenient clients missing `Content-Type` |
| Request body | `<methodCall><methodName>ns.m</methodName><params><param><value>…</value></param></params></methodCall>` | Match; allow zero-argument calls **without `<params>`** |
| Response body | `<methodResponse><params><param><value>…</value></param></params></methodResponse>` | Match |
| Fault | `<methodResponse><fault><value><struct>{faultCode:int,faultString:string}</struct></value></fault></methodResponse>`, HTTP still **200** | Match; Fault codes see §4 |
| Authentication | Unified middleware (`ServerAuthState::authorize`, OR of Basic / Bearer token / session cookie); `WWW-Authenticate: Basic realm="supervisor"` challenge on 401 | Reuse `ServerAuthState` / `http_auth_middleware` from `src/server/auth.rs` (mounted inside `build_router`) |
| Integer | 32-bit `i4`; timestamps saturated via `capped_int` to `MININT/MAXINT` (2038 problem) | Must saturate; `getProcessInfo.start/stop/now` use `i4` |
| Boolean | `<boolean>1</boolean>` / `0` | Match |
| base64 | Used for the binary chars of `sendProcessStdin` | Match |
| dateTime.iso8601 | Parse support (inbound) | Inbound optional; none produced outbound |
| Namespace | `supervisor.*` and `system.*`; **method names must have exactly 2 dot-separated segments** (CVE-2017-11610 mitigation) | Strictly 2 segments; reject `_`-prefixed methods and attribute traversal |
| Unknown method | `Fault(1 UNKNOWN_METHOD)` | Match |
| Argument error | `Fault(2 INCORRECT_PARAMETERS)` | Match |
| Long-running operations | Return a **callback function** (`NOT_DONE_YET`), continued in chunks by the select loop (e.g. `startProcess(wait=True)`) | **Degraded**: `await` synchronously and return once; semantically equivalent result, chunking not simulated |
| `system.multicall` | Executes sequentially, returns a result or fault per item; recursive multicall forbidden | Match (recursion rejected with `INCORRECT_PARAMETERS`) |

**Inbound parsing note**: Python's `loads` uses `iterparse` with special deserialization for `array/struct/value`; implementation may parse per standard XML-RPC, but must tolerate Python's lenient points (missing `<params>`, empty `<value>`).

---

## 4. Fault Code Table (`supervisor/xmlrpc.py::Faults`)

All business errors must map to the `faultCode`/`faultString` in the table below (the Chinese/English strings follow Python for easy client recognition):

| Code | Name | Triggering scenario | rsupervisord source |
| :--- | :--- | :--- | :--- |
| 1 | `UNKNOWN_METHOD` | Method not found / invalid namespace | Returned directly by the adaptation layer |
| 2 | `INCORRECT_PARAMETERS` | Wrong argument count/type (incl. TypeError) | Adaptation layer |
| 3 | `BAD_ARGUMENTS` | Invalid argument value | Adaptation layer (where applicable) |
| 4 | `SIGNATURE_UNSUPPORTED` | `system.methodHelp/Signature` not found | `system.*` |
| 6 | `SHUTDOWN_STATE` | Daemon in SHUTDOWN/RESTARTING state, call refused | New mood/state bits |
| 10 | `BAD_NAME` | Name does not exist | `ProgramError::NotFound` / group does not exist |
| 11 | `BAD_SIGNAL` | Invalid signal name/number | Signal parsing failure |
| 20 | `NO_FILE` | Log file does not exist / stdin EPIPE | `readFile`/missing log path; stdin write failure |
| 21 | `NOT_EXECUTABLE` | command not executable / insufficient permission | `StartFailed` (executability) |
| 30 | `FAILED` | Generic failure | Fallback for most `ProgramError`s |
| 40 | `ABNORMAL_TERMINATION` | Not STARTING/RUNNING during start wait | Start wait timeout/abnormal |
| 50 | `SPAWN_ERROR` | Spawn failed (spawnerr) | `StartFailed` |
| 60 | `ALREADY_STARTED` | Already running | `AlreadyRunning` |
| 70 | `NOT_RUNNING` | Not running | `NotRunning` |
| 80 | `SUCCESS` | (Only used for the `status` field of batch results) | Batch results |
| 90 | `ALREADY_ADDED` | Group already exists | `addProcessGroup` |
| 91 | `STILL_RUNNING` | Processes still running when removing a group | `removeProcessGroup` |
| 92 | `CANT_REREAD` | Configuration reload failed | `reloadConfig` parse/validation failure |

> Recommended `ProgramError` (`src/error.rs`) → Fault mapping: `AlreadyRunning→60`, `NotRunning→70`, `NotFound→10`, `StartFailed→50/21`, `Timeout→40`, `InvalidState→40/70`, `ShuttingDown→6`, everything else→`30`.

---

## 5. Method Overview

### 5.1 `supervisor` namespace (29 entries total, incl. aliases)

| Method | Priority | Python signature | Current mapping / implementation path | Differences / degradation |
| :--- | :--- | :--- | :--- | :--- |
| `getAPIVersion` | **P0** | `() → str` | Constant `"3.0"` (the `getVersion` alias returns the same) | Return directly |
| `getSupervisorVersion` | **P0** | `() → str` | rsupervisord version number | Returns our own version (not "4.2.5") |
| `getIdentification` | **P0** | `() → str` | `server`/`[supervisord] identifier`, or default `supervisor` | Configurable, default `supervisor` |
| `getState` | **P0** | `() → {statecode,statename}` | New mood states (RUNNING/SHUTDOWN/RESTARTING/FATAL) | Requires introducing the supervisor mood concept |
| `getPID` | **P0** | `() → int` | `std::process::id()` | Direct |
| `getAllProcessInfo` | **P0** | `() → [struct]` | `ManagerHandle::get_all_status` → **getProcessInfo field table** (§6) | Field adaptation |
| `getProcessInfo` | **P0** | `(name) → struct` | `get_status(name)` + namespec parsing | Field/description adaptation |
| `startProcess` | **P0** | `(name, wait=True) → bool` | `ManagerCommand::StartProgram` (namespec→process/group/`*`) | Synchronous wait |
| `startProcessGroup` | **P0** | `(name, wait=True) → [struct]` | `StartGroup` | Result struct adaptation |
| `startAllProcesses` | **P0** | `(wait=True) → [struct]` | `StartAll` | Result struct adaptation |
| `stopProcess` | **P0** | `(name, wait=True) → bool` | `StopProgram` | Synchronous wait |
| `stopProcessGroup` | **P0** | `(name, wait=True) → [struct]` | `StopGroup` | Result struct adaptation |
| `stopAllProcesses` | **P0** | `(wait=True) → [struct]` | `StopAll` | Result struct adaptation |
| `signalProcess` | **P0** | `(name, signal) → bool` | `SignalProgram` | Signal name↔`StopSignal` |
| `signalProcessGroup` | **P1** | `(name, signal) → [struct]` | Signal per process within the group | Result struct adaptation |
| `signalAllProcesses` | **P1** | `(signal) → [struct]` | Signal across all processes | Result struct adaptation |
| `tailProcessStdoutLog` | **P0** | `(name, offset, length) → [str, int, bool]` | `subscribe/read_logs` + **#8 degradation** | **Line-level degradation**: see §7.1 |
| `tailProcessStderrLog` | **P0** | `(name, offset, length) → [str, int, bool]` | Same as above (stderr merged semantics) | Line-level degradation |
| `readLog` | **P0** | `(offset, length) → str` | Read main log (`logging.file`) | maintail; see §7.2 |
| `readProcessStdoutLog` | **P1** | `(name, offset, length) → str` | Read program log | Line-level degradation |
| `readProcessStderrLog` | **P1** | `(name, offset, length) → str` | Program stderr log | Line-level degradation |
| `clearLog` | **P1** | `() → bool` | Clear/reopen main log | see §7.2 |
| `clearProcessLogs` | **P1** | `(name) → bool` | Clear program log | Rotation reset |
| `clearAllProcessLogs` | **P1** | `() → [struct]` | Clear all | Result struct adaptation |
| `reloadConfig` | **P0** | `() → [[added,changed,removed]]` | `ReloadConfig` → `ReloadSummary` | see §7.3 |
| `addProcessGroup` | **P1** | `(name) → bool` | `ManagerCommand::AddProcessGroup` (activate pending config) | Python semantics: not in source config → `BAD_NAME`; already active → `ALREADY_ADDED` |
| `removeProcessGroup` | **P1** | `(name) → bool` | `ManagerCommand::RemoveProcessGroup` (removes only the active group; source config preserved) | Python semantics: not active → `BAD_NAME`; running → `STILL_RUNNING` |
| `getAllConfigInfo` | **P1** | `() → [struct]` | Config (raw) + `inuse` computation | Field subset, see §7.4 |
| `sendProcessStdin` | **P1** | `(name, chars) → bool` | `SendStdin` (depends on **#7**) | Return `FAILED`/`NO_FILE` until #7 lands |
| `sendRemoteCommEvent` | **P0** | `(type, data) → bool` | `send_remote_comm_event` → `REMOTE_COMMUNICATION` | Straight to the event listener pool |
| `shutdown` | **P0** | `() → bool` | `ManagerCommand::Shutdown` | Direct |
| `restart` | **P0** | `() → bool` | `ManagerCommand::RestartDaemon` (stop all→re-read config→start again) | **Semantics difference**: see §7.5 (kept separate from hot reload) |
| Aliases `getVersion`/`readMainLog`/`readProcessLog`/`tailProcessLog`/`clearProcessLog` | **P0/P1** | Same as the corresponding method | Forward directly | Zero cost, required |

### 5.2 `system` namespace (introspection)

| Method | Priority | Signature | Description |
| :--- | :--- | :--- | :--- |
| `system.listMethods` | **P1** | `() → [str]` | Returns all available method names (with namespace prefix), sorted lexicographically |
| `system.methodHelp` | **P1** | `(name) → str` | Returns the method docstring; not found → `SIGNATURE_UNSUPPORTED` |
| `system.methodSignature` | **P1** | `(name) → [rtype, ptype...]` | Parsed from docstring `@param/@return` |
| `system.multicall` | **P1** | `(calls) → [result]` | Per item `{methodName, params}`; failed items return `{faultCode,faultString}`; recursion forbidden |

> Introspection methods can be implemented with a **static registry** (method name→(doc, signature)); no need to reflect over Rust types.

---

## 6. `getProcessInfo` Return Fields (must align field by field)

| Field | Type | Python source | rsupervisord source / description |
| :--- | :--- | :--- | :--- |
| `name` | string | `config.name` | `ProgramStatus.name` |
| `group` | string | `group.config.name` | `ProgramStatus.group` |
| `start` | int | `laststart` (capped) | Need to record last start UNIX seconds |
| `stop` | int | `laststop` (capped) | Need to record last stop |
| `now` | int | `time.time()` (capped) | Current time |
| `state` | int | `ProcessStates` | **Needs 0..7 code mapping**, see below |
| `statename` | string | `getProcessStateDescription` | `STARTING/RUNNING/BACKOFF/STOPPING/STOPPED/EXITED/FATAL/UNKNOWN` |
| `spawnerr` | string | `spawnerr or ''` | Startup error text (empty string by default) |
| `exitstatus` | int | `exitstatus or 0` | `exit_code` defaults to 0 |
| `logfile` | string | stdout path (compat alias) | Same as `stdout_logfile` |
| `stdout_logfile` | string | stdout log path | `''` if empty |
| `stderr_logfile` | string | stderr log path | `''` if empty |
| `pid` | int | `process.pid` | `pid` defaults to 0 (not `null`) |
| `description` | string | `_interpretProcessInfo` | See below |

**`state` code mapping** (Python `ProcessStates`): `STOPPED=0, STARTING=10, RUNNING=20, BACKOFF=30, STOPPING=40, EXITED=100, FATAL=200, UNKNOWN=1000`. rsupervisord's `ProgramState` (`Stopped/Starting/Running/Backoff/Stopping/Exited/Fatal`) must map to the codes above.

**`description` rules** (`_interpretProcessInfo`):
- RUNNING → `"pid {pid}, uptime {H:MM:SS}"` (uptime = now - start, negative values clamped to zero)
- FATAL/BACKOFF → `spawnerr` (or `unknown error (try "tail {name}")` if empty)
- STOPPED/EXITED → local time `"%b %d %I:%M %p"` if a start exists; otherwise `"Not started"`
- Others → `""`

---

## 7. Semantics & Degradation of Key Methods

### 7.1 `tail*Log` (P0, depends on #8 degradation)

- **Python semantics**: `tailProcessStdoutLog(name, offset, length) → [bytes, offset, overflow]`. Reads up to `length` bytes from `offset`; if total length > `offset+length`, set `overflow=True` and align offset to the end of the log; the returned offset is always "last read position + 1".
- **rsupervisord degradation** (consistent with [`SUPERVISORD_COMPAT.md`](./SUPERVISORD_COMPAT.md) §7 #8):
  - `offset` = **line index** in the line-level ring buffer; `length` = **number of lines**; returns `[text, new line cursor, overflow]`.
  - **A max offset (`0x7fffffffffffffff`, the `supervisorctl tail -f` convention) is saturated to "follow from the end of the window"**.
  - Deep history unreachable (window only), window resets on daemon restart — **must be declared in the documentation**.
- **Acceptance**: a real `supervisorctl tail -f <name>` works (shallow tail follow).

### 7.2 `readLog` / `clearLog` (maintail)

- `readLog(offset,length)`: reads the main log file (`logging.file`). No file → `NO_FILE`.
- `clearLog`: relied upon by `supervisorctl maintail`; implemented as truncating/reopening the main log file.
- Follows the same **line-level degradation** as program logs; `maintail -f` handled like `tail -f`.

### 7.3 `reloadConfig` (P0)

- **Python return**: `[[added, changed, removed]]` (three arrays of names; note the **outer extra wrapping array**).
- **rsupervisord mapping**: `ManagerHandle::reload_config` returns `ReloadSummary` → split out the added/changed/removed name lists.
- **Parse/validation failure** → `Fault(92 CANT_REREAD, <details>)`.
- **Note**: the relationship to `supervisorctl reread` (detect only) / `update` (apply) see [`CLI_COMPAT.md`](./CLI_COMPAT.md) §5.1.2.

### 7.4 `getAllConfigInfo` (P1, `supervisorctl avail`)

Python returns a **configuration snapshot** per program (groups flattened), keys include: `autostart, directory, uid, command, exitcodes, group, group_prio, inuse, killasgroup, name, process_prio, redirect_stderr, startretries, startsecs, stdout_capture_maxbytes, stdout_events_enabled, stdout_logfile, stdout_logfile_backups, stdout_logfile_maxbytes, stdout_syslog, stopsignal(int), stopwaitsecs, stderr_* , serverurl`, with `Automatic→'auto'`, `None→'none'`.

- **Requirement**: the returned fields may be read by clients **regardless of superset/subset**; at minimum provide the fields in the table above that have a mapping; fill missing fields with `'none'`/default values to avoid client KeyErrors.
- `inuse` = whether the group is currently in the running registry.

### 7.5 `restart` vs hot reload (semantics difference)

- Python `restart`: sets the daemon mood to `RESTARTING`; after the process exits, it is **brought back up externally** (init/systemd/supervisor itself), and configuration takes effect accordingly.
- rsupervisord's "hot reload" is the application path of `reloadConfig`, **not** `restart`.
- **Ruling**: `restart` is implemented as `ManagerCommand::RestartDaemon` (**stop all → re-read config → start again**, see `manager/supervisor.rs::execute_restart_daemon`), aligned with Python `reload`/`restart` semantics. The boundary with hot reload:
  - `supervisor.restart` / CLI `reload` → **restart the daemon** (stop all, rebuild instances, autostart restarts).
  - `supervisor.reloadConfig` / CLI `reload-config` / `config reload` → **zero-downtime hot reload** (incremental diff, unchanged programs keep PIDs online).
- **Do not graft hot reload onto `restart`** (consistent with the semantics ruling for `reload` at P0 in `CLI_COMPAT.md`).

### 7.6 `sendProcessStdin` / `sendRemoteCommEvent`

- `sendProcessStdin`: maps to `ManagerCommand::SendStdin`, **fully depends on #7** (stdin piped + backpressure channel). Before #7, return `NOT_RUNNING`/`FAILED`; non-string argument → `INCORRECT_PARAMETERS`; EPIPE → `NO_FILE`.
- `sendRemoteCommEvent`: depends on **#6** EventHub→listener; until it lands, `Fault(UNKNOWN_METHOD)` or `FAILED`.

---

## 8. Architecture Placement

- **New module**: `src/server/xrpc.rs`, exporting `pub fn xrpc_router() -> Router`, registered in `src/server/mod.rs`, and `merge`d in `build_router(state)` (path `/RPC2`, coexisting with `/api/v1/*`).
- **Reuse**: `AppState { manager: ManagerHandle, basic_auth, auth_token, sessions, .. }`; auth enforced by `http_auth_middleware` (not in the handler).
- **Encode/decode**: introduce a pure-Rust XML-RPC codec (self-implemented or a lightweight crate), **no Python dependency**; Faults as enum constants (§4).
- **State bits**: add a supervisor mood (`RUNNING/SHUTDOWN/RESTARTING/FATAL`) to support `getState`/`restart`/`SHUTDOWN_STATE`.
- **Name semantics**: implement `namespec` parsing (`group:name`, `group:*`, bare name), aligned with [`CLI_COMPAT.md`](./CLI_COMPAT.md) §4.3.

---

## 9. Priority Checklist (Summary)

**P0 (make stock supervisorctl usable)**
1. `/RPC2` route + XML-RPC codec + unified middleware auth (Basic OR token OR session) + Faults mapping + 2-segment method name validation.
2. `getAPIVersion`/`getVersion`, `getSupervisorVersion`, `getIdentification`, `getState`, `getPID`.
3. `getAllProcessInfo`, `getProcessInfo` (all fields per §6).
4. `startProcess`/`stopProcess` + group + all; `signalProcess`.
5. `tailProcessStdoutLog`/`tailProcessStderrLog` + aliases; `readLog` (maintail).
6. `reloadConfig`; `shutdown`; **`restart`** (daemon restart semantics, §7.5).

**P1**
7. `readProcessStdoutLog`/`readProcessStderrLog`, `clearLog`/`clearProcessLogs`/`clearAllProcessLogs`.
8. `signalProcessGroup`/`signalAllProcesses`, `getAllConfigInfo`.
9. `sendProcessStdin` (after #7), `system.listMethods`/`methodHelp`/`methodSignature`/`multicall`.
10. `addProcessGroup`/`removeProcessGroup` (activate/remove pending config, Python semantics).

**P2 / deferred**
11. (Already folded into P0: `restart` daemon restart is implemented)

**Not supported / degraded**
12. Long-running-operation callback chunking (`NOT_DONE_YET`) → **synchronous-return degradation**.
13. `tail`/`read` deep history → **line-level ring buffer degradation** (declarative).

---

## 10. Acceptance

- **End-to-end**: an unmodified Python `supervisorctl` connects to rsupervisord directly via `-s unix://…` (or `http://…` + `-u/-p`); the following commands **pass 100%**:
  `status`, `status <name>`, `start/stop/restart <name>`, `start/stop/restart all`, `signal <name> <sig>`, `tail -f <name>`, `maintail`, `version`, `pid`, `reread`, `update`, `shutdown`.
- **Fault correctness**: error names → expected `faultCode` (§4) asserted item by item.
- **Protocol robustness**: missing `<params>`, invalid 2-segment names, `_`-prefixed methods, and recursive `multicall` are all correctly rejected.
- **Interop regression**: REST (`/api/v1/*`) and XML-RPC (`/RPC2`) coexist; the existing 67 tests and clippy `-D warnings` keep passing.

```bash
supervisorctl -c /etc/supervisord.conf status
supervisorctl -c /etc/supervisord.conf tail -f web
supervisorctl -c /etc/supervisord.conf shutdown
```

---

## 11. Relationship to Other Documents

- CLI arguments, exit codes, `reload`/`reread`/`update` semantics: see [`CLI_COMPAT.md`](./CLI_COMPAT.md).
- Loading of INI sections (especially `[rpcinterface:supervisor]`, `[inet_http_server]`): see [`INI_COMPAT.md`](./INI_COMPAT.md).
- #4 Group, #6 Event Listener, #7 stdin, #8 log byte offsets: see [`SUPERVISORD_COMPAT.md`](./SUPERVISORD_COMPAT.md) §7.

---

## 12. Compatibility Test Baseline

The contracts in this document are carried by [`../compat/tests/test_xmlrpc.py`](../compat/tests/test_xmlrpc.py) (31 cases, asserting `supervisor.*` / `system.*` / Fault directly) and [`test_cli.py`](../compat/tests/test_cli.py) (27 cases, running through an unmodified stock `supervisorctl`) as the executable baseline, all green on Python **4.2.5** first.

For the compiled rsupervisord (default target), first do `/RPC2` capability probing: currently **not yet implemented**, so the relevant cases are uniformly marked **`xfail` (59 cases)**; with `SUPERVISOR_STRICT=1` they turn into hard failures, i.e. the full picture of the §9 priority checklist's pending work. Once `/RPC2` is implemented, **no test changes are needed**; the gating automatically passes and starts asserting item by item (for details, see [`SUPERVISORD_COMPAT.md`](./SUPERVISORD_COMPAT.md) §8).