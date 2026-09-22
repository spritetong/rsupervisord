# rsupervisord: Full Supervisor Compatibility Feasibility Analysis (SUPERVISORD_COMPAT.md)

| Document Version | Status | Target Language | Scope |
| :--- | :--- | :--- | :--- |
| **v1.3.0** | Draft / For Review | Rust (Edition 2024) | Protocol & Feature Coverage Strategy vs. Python Supervisor / `ochinchina/supervisord`; Feature Upgrade Definition (bone→Skin, incl. three-state adjudication); related: [`CLI_COMPAT.md`](./CLI_COMPAT.md) / [`INI_COMPAT.md`](./INI_COMPAT.md) / [`XMLRPC_COMPAT.md`](./XMLRPC_COMPAT.md); enforceable contract — see [`../compat/README.md`](../compat/README.md) |

---

## 1. Background & Goal

`rsupervisord` is a brand-new, modern process orchestration engine (YAML configuration + UDS/TCP JSON REST API + SSE + Web UI).
It shares **no protocols or configuration formats** with the classic supervisord ecosystem (including Python Supervisor and the Go `ochinchina/supervisord`).

This analysis answers a single core question:

> **To fully cover supervisord's functionality, is translating just the old protocol (XML-RPC) sufficient? Can the existing skeleton be left completely untouched?**

### 1.1 Verified Facts (based on current code baseline `80bfa97`)

| Fact | Location | Impact |
| :--- | :--- | :--- |
| Child process `stdin` is `Stdio::null()` | `src/program/process.rs:680` | No stdin injection channel → `sendProcessStdin` cannot be translated |
| Process logs are an **in-memory line-level ring buffer** (not persisted to disk) | `src/program/*`, `src/server/api.rs:631` | No byte-offset semantics → `tailProcessLog` (offset mode) cannot be translated |
| Schema explicitly rejects `numprocs` (tests assert the error) | `src/config/schema.rs:403-413` | No multi-instance/group domain model |
| `EventHub` only broadcasts internally (consumed by SSE/REST) | `src/manager/*` | No daemon→program event-listener protocol channel |

---

## 2. Methodology: Three-Layer Decomposition

XML-RPC is only the **control/query plane**. Half of supervisord's functionality lives in
configuration semantics and the daemon↔program channel; without a domain model there, that part cannot be translated. So "full coverage" is decomposed into three layers:

- **Layer A — pure protocol translation layer**: XML-RPC codec and mapping onto the existing API/manager; zero skeleton changes.
- **Layer B — configuration-surface layer**: INI parsing, numprocs expansion, signal sequences, etc.; touches the config layer, execution skeleton untouched.
- **Layer C — genuine model gaps**: parts that require new subsystems, unrelated to XML-RPC translation.

**Conclusion preview**: Layer A can be done directly; Layer B needs only incremental config-layer work; the four gaps in Layer C **require new models**, but all
can be implemented as **bolt-on increments** without intruding on the existing actor hot path.

---

## 3. Layer A — Pure Protocol Translation (skeleton fully untouched, lowest cost)

Add a new `server/xrpc` adapter module: XML-RPC codec, `name/params ↔ existing API/manager` mapping, and Basic auth compatibility (`[inet_http_server]` username/password). The coverable method list:

| Method group | Coverable methods | Existing support |
| :--- | :--- | :--- |
| Status query | `getProcessInfo` / `getAllProcessInfo` | Existing status snapshot |
| Control | `startProcess` / `stopProcess` / `restartProcess` / `signalProcess` | Existing manager commands |
| Log reading | `readProcessStdoutLog` / `readProcessStderrLog` / `clearProcessLogs` / `tailProcessLog` (line-level) | Existing line-level read and clear |
| Version/identity | `getVersion` / `getPID` / `getIdentification` | Trivial mapping |
| Group control | `startProcessGroup` / `stopProcessGroup` / `signalProcessGroup` | Available once Layer B's group concept exists |

> Note: the byte-offset mode of `tailProcessLog` (P→O pointer parameter) has no support; see Layer C-4.

---

## 4. Layer B — Configuration-Surface Adaptation (touches the config layer, execution skeleton untouched)

1. **INI parser**: `[program:x]` sections → internal `ProgramConfig`. Standard sections such as
   `[supervisord]`, `[inet_http_server]`, `[supervisorctl]` must be implemented, along with
   `%(ENV_xxx)s` / `%(program_name)s` expansion. `deny_unknown_fields` must take the lenient path for INI.
2. **numprocs expansion**: `numprocs: N` expands at `resolve_programs` time into
   `name:0 .. name:N-1` — N independent actors — with `%(process_num)02d` substitution.
   This fits the existing "DAG registers N actors" model naturally; **Manager needs no structural change**.
3. **stopsignal sequence / stopasgroup / killasgroup**:
   process-group reaping is already a default capability; only two config switches need to be added to the stop state machine and the signal array
   (the Go version sends multiple stopsignals in sequence; classic supervisord uses a single signal).
4. **exitcodes / startretries semantics alignment**: verified that `exit_codes` and `start_retries`
   already exist; alignment cost is low.

> Once this layer is done, `supervisorctl` day-to-day operations (status/start-stop/restart/signal/line-level logs) are essentially all open.

---

## 5. Layer C — Genuine Model Gaps (new subsystems required)

| # | Gap | Current status | Required additions | Recommendation |
| :--- | :--- | :--- | :--- | :--- |
| C-1 | Runtime `addProcessGroup` / `removeProcessGroup` | **implemented (lightweight approach)**: group is a "name prefix + metadata"; add/remove = batch register/unregister actors based on pending configuration (depends on Layer B's expansion output); `ALREADY_ADDED`/`BAD_NAME`/`STILL_RUNNING` match Python | | Lightweight approach already landed |
| C-2 | **Event Listener protocol** | `EventHub` only broadcasts internally | A brand-new daemon→program channel: `README`/`RESULT` three-step handshake, serialization of custom `STATE_CHANGED` / `PROCESS_LOG` events, `{FD_2}`/`FD_NUM` expansion; unrelated to XML-RPC | **Largest effort of phase two**; independent subsystem |
| C-3 | `sendProcessStdin` (incl. F_EVENT) | `Stdio::null()` (`process.rs:680`) | Change stdin to piped + add a manager→actor write channel + pipe lifecycle management | Independent subsystem |
| C-4 | `tailProcessLog` byte offset / daemon `getLog` | In-memory line-level ring buffer, no persistence | Persist process logs to disk or a "byte cursor" cache; otherwise the P→O offset semantics cannot be provided | Choose one of two; touches the logging pipeline |

---

## 6. Conclusion

- **Not "translation only"**: Layers A + B cover roughly 90% of `supervisorctl` production day-to-day scenarios, and the existing
  actor hot path can be preserved as-is;
- **Must be newly added**: Layer C's four gaps — the event-listener protocol (C-2) and stdin injection (C-3) are the only two requiring
  newly written user-space subsystems; the group model (C-1) gets lightweight support from numprocs expansion + batch registration;
  the byte-offset tail (C-4) depends on log-persistence rework.
- Horizon: **don't tear down the existing skeleton; bolt on the four capability channels incrementally** is the current optimal evolution path.

---

## 7. Feature Upgrade Definitions & Landing Order (bone first, then Skin)

> This section is the **complete definition of feature upgrades** (numbers #1-#12), including the **final three-state ruling**:
> **immediate / implemented / deferred / conditional / Skin**. It corresponds to the discussions' conclusions: the **configuration layer of numprocs and Group has landed**,
> **runtime add/remove has landed per Python semantics**, the Event Listener has landed,
> and the log "absolute byte cursor" was rejected because it contradicts rotating logs.

### 7.1 Three-State Summary

| Status | Numbers | Ruling rationale |
| :--- | :--- | :--- |
| **immediate** | #1 vehicle, #2 INI+macros, #5 XML-RPC subset + Basic auth, #7 sendProcessStdin | Supports "supervisorctl connects day-to-day" with zero intrusion into the existing execution core |
| **implemented (config layer)** | #3 numprocs, #4 Group | See §7.2: `numprocs` expansion, `group` membership and validation, and `program_defaults` are all in place in `schema.rs`; runtime add/remove has landed per Python semantics (see the next row) |
| **implemented (runtime surface)** | #4 runtime `addProcessGroup`/`removeProcessGroup`, `group:*` batch operations | Pending configuration (`pending_configs`, mirroring `process_group_configs`) activation/removal + `ALREADY_ADDED`/`BAD_NAME`/`STILL_RUNNING`; removal only affects the active set, the source configuration is preserved, and it is reproducible after `reloadConfig` |
| **implemented** | #6 Event Listener | READY/RESULT broadcast protocol, event-class mapping, event pool backpressure/UNKNOWN violation, `sendRemoteCommEvent` — all landed; see [`EVENTLISTENER_COMPAT.md`](./EVENTLISTENER_COMPAT.md) |
| **Optional/As-we-go** | #8 log byte offset | A pure protocol-compatibility requirement; the "absolute cursor" has been rejected; it only rides along if the original log feature is hard-built, with a block-based byte chain as the underlying fallback |
| **Skin** | #9-#12 | Laid out after the bone is complete |

### 7.2 Feature Numbering Definitions

| # | Status | Feature | Scope & key decision points | Acceptance criteria |
| :--- | :--- | :--- | :--- | :--- |
| #1 | immediate | **Service Install/Uninstall/Start/Stop/Restart** (Windows + Systemd) | Windows: SCM service, single-binary self-hosting, `--install`/`--uninstall`, `--username/password` login account, AutoStart delay, `SC_ACTION_RESTART` crash recovery; Systemd: generate `rsupervisord.service` (Restart=always, LimitNOFILE, User/Group, Environment, KillMode aligned), with `--enable/--disable/--start/--stop/--restart` support; install path and arguments written back into the unit/registry (ExecStart carries an absolute `-c`); uninstall must stop first, then remove, returning non-zero on failure; recommended entry: the `rsupervisorctl service ...` subcommand (local UDS auth) | install→start→restart→stop→uninstall passes on both Windows and Linux; no leftover process tree; uninstalling without stopping first returns non-zero + an explicit message |
| #2 | immediate | **INI configuration parsing + `%()` macro expansion** | Consume `[program:x]` / `[supervisord]` / `[inet_http_server]` / `[unix_http_server]` / `[eventlistener]` / `[supervisorctl]` sections; macro expansion for `%(ENV_x)s` / `%(program_name)s` / `%(process_num)02d`; **coexists with YAML** (`-c x.ini` auto-detected by suffix), feeding the same resolve pipeline; `deny_unknown_fields` takes the lenient path for INI. **Per-section field mapping / value formats / precedence: see [`INI_COMPAT.md`](./INI_COMPAT.md)**; the macro expander (`ENV_`/`here`/`program_name`/`process_num`/`numprocs`/`group_name`) already exists | Canonical supervisord production configs load verbatim; macros expand correctly; YAML/INI dual-path regression tests pass |
| #3 | implemented (config layer) | **numprocs instance expansion** | **Already landed in `src/config/schema.rs`**: `numprocs` / `numprocs_start` / `process_name` fields exist; `resolve_programs` expands into `name:0..N-1` — N actors total (via `%(process_num)02d` working with `MacroExpander`), sharing the same `ProgramConfig` template. **Remaining**: the XML-RPC per-instance method surface (`supervisor.process.*` per instance) is laid out per #5 | All N instances start/stop independently; status/logs isolated per instance |
| #4 | implemented (config layer + runtime) | **Group metadata model (lightweight) + runtime add/remove** | **Config layer landed in `schema.rs`**: `groups:` (programs + priority), `ProgramConfig.group`, and `resolve_programs` membership resolution and validation are all complete; **no heavyweight GroupActor model**. **Runtime landed per Python semantics**: `ManagerCommand::AddProcessGroup`/`RemoveProcessGroup` activate/remove based on pending configuration (mirroring `process_group_configs`), with `ALREADY_ADDED`/`BAD_NAME`/`STILL_RUNNING` matching stock; removal only touches the active set, the source configuration is preserved | Runtime group register/unregister succeeds atomically; a non-running group can be removed, a running group returns `STILL_RUNNING`, a missing group returns `BAD_NAME`; no orphan processes |
| #5 | immediate | **XML-RPC protocol adaptation layer** (`server/xrpc`) | Standard `supervisord.*` / `supervisor.process.*` method surface + Basic auth (`[inet_http_server]` username/password); mounted on a separate Axum router (same process, reusing the UDS/port); long-term goal is stock `supervisorctl` direct connection; **the group method surface and runtime add/remove have landed**; log methods use the #8 degraded mapping | All of stock `supervisorctl`'s status/start/stop/restart/signal/tail pass 100%; auth checks are correct; `addProcessGroup`/`removeProcessGroup` return Python-consistent fault codes |
| #6 | conditional | **Event Listener protocol** | Enabling precondition: drop-in compatibility with monitoring/alerting tools (e.g. superlance) becomes a hard requirement. Priority is the **EventHub→listener bridge** (read `[eventlistener:]` sections from the #2 INI; bridge an adapter that feeds EventHub events to the listener program); the native `READY`/`RESULT` handshake is only built if the bridge proves insufficient | **(when enabled)** superlance `memmon` and similar tools attach and receive state/log events |
| #7 | immediate | **sendProcessStdin (data plane)** | Change stdin from `Stdio::null()` (`process.rs:680`) to piped + a manager→actor **bounded write channel** (backpressure: Bounded + overflow dropped + a warning when the child isn't reading) + write timing (no blocking write_all inside select!; a dedicated writer task or non-blocking drain) + **pipe lifecycle management** (drop the write end after wait_exit; writes to an already-exited process return an error; Restart swaps in a new write end); F_EVENT expansion optional | Injected stdin is readable by the child process; the write channel closes safely after stop/exit with no leaks |
| #8 | Optional/As-we-go | **Original log feature (byte offset / getLog)** | **The "absolute byte cursor" is rejected**: log-file rotation makes a monotonic cursor contradict rotating files, introducing more problems. **Parameter surface matches the Python version field-for-field** (`tailProcessStdoutLog(name, offset, length)` → `(bytes, offset, total)`), mapping semantics onto the existing line-level ring buffer: offset = a line index within the retention window, length = line count, the returned offset advances to a line cursor, total = total lines in the window; **an extreme offset (`0x7fffffffffffffff`, the supervisorctl tail -f convention) saturates to "tail from the end of the window"**; degraded behavior is explicit (deep history unreachable, reset to zero on restart) | Real `supervisorctl tail -f` works (shallow tail-follow); boundary behavior matches the degradation statement |
| #9 | Skin | **Cron scheduling + hooks (pre_start / pre_stop)** | `cron:` expression-driven start/stop scheduling; pre_start / pre_stop script hooks; failure semantics tied to events | Cron programs start on schedule; hooks fire at lifecycle points and can fail with graceful degradation |
| #10 | Skin | **File/binary change-triggered restart** | filechangemonitor semantics: watch file/directory patterns, restart on a change, with support for a custom restart command/signal | File change → program restarts per configuration, converging within N attempts |
| #11 | Skin | **Prometheus metrics endpoint** | Export `process_*` metrics based on the existing activity-aware sampler; `/metrics` can be toggled | `/metrics` output follows the naming convention; the idle auto-pause of sampling still works |
| #12 | Skin | **daemon runtime-surface completion** | pidfile, minfds / minprocs rlimit, and clear separation between `reload` (daemon restart) and hot-reload (`reload-config`) semantics | Each option takes effect with argument validation; `reload` = stop all → re-read → restart (Python semantics), consistent with XML-RPC `supervisor.restart`; `reload-config`/`config reload` = zero-downtime hot reload (`reloadConfig`) |

### 7.3 Key Decision Records (conclusions from this discussion)

- **numprocs (#3) / Group (#4)**: **config layer implemented** (fields, expansion, membership, and validation in `schema.rs`); **runtime add/remove landed per Python semantics** (pending-config activation/removal, `ALREADY_ADDED`/`BAD_NAME`/`STILL_RUNNING`); the #5 group method surface and runtime add/remove are both wired up.
- **INI (#2)**: per-section field mapping, value formats, and precedence — see [`INI_COMPAT.md`](./INI_COMPAT.md); the conclusion is a **front-end parser + reuse of the existing resolve pipeline**, with zero changes to the execution skeleton.
- **Event Listener (#6)**: implemented (READY/RESULT, event classification, event pool backpressure, UNKNOWN violations, `sendRemoteCommEvent`) — see [`EVENTLISTENER_COMPAT.md`](./EVENTLISTENER_COMPAT.md); the native handshake is the protocol itself and has landed directly.
- **Log byte offset (#8)**: the absolute cursor was rejected (rotating logs contradict it); changed to **parameter-surface parity + line-level semantic degradation**,
   with saturation mapping for extreme tail offsets — guaranteeing real `supervisorctl tail -f` works.
- **Log "block-based Bytes chain + absolute cursor" optimization**: mentioned in passing; implement only if necessary; it only rides along when the original log feature is hard-built.
- **sendProcessStdin (#7)**: a one-way channel is only the transport leg; the hard parts are backpressure, write timing, and pipe lifecycle.

### 7.4 Milestones

| Milestone | Contents | Acceptance, in one sentence |
| :--- | :--- | :--- |
| Phase A (vehicle) | #1 | The system service can be installed, started, and uninstalled cleanly |
| Phase B (config/protocol entry) | #2 #5 (subset + auth, incl. tail degradation) | `-c x.ini` loads; `supervisorctl` status/start-stop/restart/signal/tail all pass over direct connection |
| Phase C (data plane) | #7 | stdin injection works and has no leaks |
| Phase D (surface/conditional) | #9 #10 #11 #12; #6 on demand | Ops comfort is complete; monitoring tools attach on demand |

---

## 8. Compatibility Test Baseline (enforceable contract)

Compatibility is no longer judged by prose alone; it is driven by the pytest suite under [`../compat/`](../compat) as the **enforceable contract**. How to run, prerequisites, and details: see [`../compat/README.md`](../compat/README.md).

**Two targets** (`SUPERVISOR_TARGET`):

| Target | Server | Client | Purpose |
| :--- | :--- | :--- | :--- |
| `python` (golden baseline) | Stock Python supervisord 4.2.5 (INI fixtures) | Stock `supervisorctl` + XML-RPC | Defines expected behavior; must be all green |
| `rsupervisord` (**default**) | **the compiled `target/<profile>/rsupervisord`** (YAML fixtures) | `rsupervisorctl` (native) + stock client (capability-gated) | Measures the current gap |

**Running** (inside WSL):

```bash
bash compat/run.sh                              # default target: compiled bin
SUPERVISOR_TARGET=python bash compat/run.sh     # golden baseline (4.2.5)
SUPERVISOR_STRICT=1 bash compat/run.sh          # unimplemented items => hard failure
```

**Gating semantics**: oracle cases ([`test_cli.py`](../compat/tests/test_cli.py), [`test_xmlrpc.py`](../compat/tests/test_xmlrpc.py), `test_zz_daemon.py`) first probe the compiled bin for `/RPC2` capability; when missing, they are recorded via `pytest.xfail` as `xfail` (reason pointing to §7 #5), turning into a hard failure under `SUPERVISOR_STRICT=1`. Native cases ([`test_native_cli.py`](../compat/tests/test_native_cli.py)) run only under the `rsupervisord` target.

**Current baseline** (`target/debug` local build):

| Run | Result |
| :--- | :--- |
| `SUPERVISOR_TARGET=python` | **59 passed, 5 skipped** (defines the golden behavior) |
| Default (compiled bin) | **5 passed (native), 59 xfailed** (XML-RPC unimplemented, i.e. #5) |
| `SUPERVISOR_STRICT=1` (compiled bin) | **5 passed, 59 errors** (= #5's outstanding surface) |

> **Build fix**: to make the Linux build compile, the `nix` dependency in `Cargo.toml` had the `hostname` feature added (`src/platform/unix/mod.rs::hostname` relies on `nix::unistd::gethostname`).