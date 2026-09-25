# rsupervisord: Event Listener Compatibility Requirements (EVENTLISTENER_COMPAT.md)

| Document Version | Status | Target Language | Scope |
| :--- | :--- | :--- | :--- |
| **v1.0.0** | Draft / For Review | Rust (Edition 2024) | `[eventlistener:x]` configuration surface, `READY`/`RESULT` wire protocol, event envelope and event type payloads, pool buffering/dispatch/lifecycle semantics, goal: drop-in compatibility with external listener tools such as superlance; executable baseline in §12 / [`../compat/tests/test_eventlistener.py`](../compat/tests/test_eventlistener.py) |

---

## 1. Purpose & Scope

Question: **What capabilities must be implemented and what protocol behavior achieved so that event listener programs from the stock supervisord ecosystem (e.g. superlance `memmon`/`httpok`) can plug into rsupervisord without modification.**

- **Authoritative baseline**: Python Supervisor **4.2.5** implementation:
  - `supervisor/options.py` (`[eventlistener:x]` section parsing and `EventListenerPoolConfig`);
  - `supervisor/process.py::EventListenerPool` (pool, buffering, envelope, dispatch, serial);
  - `supervisor/dispatchers.py::PEventListenerDispatcher` (handshake state machine and `RESULT` parsing);
  - `supervisor/events.py` (event types and `payload()`);
  - `supervisor/rpcinterface.py::sendRemoteCommEvent`.
- **Current-state baseline**: rsupervisord's internal event bus `src/manager/event.rs` (`EventHub` / `SystemEvent` / `LogEntry`, tokio broadcast, **internal-facing**, not the wire protocol); the INI adapter currently **discards** `[eventlistener:*]` sections (`src/compat/ini/adapter.rs:115-118`); XML-RPC `sendRemoteCommEvent` returns `FAILED` (`src/compat/xmlrpc/supervisor.rs:276`).
- **Related**: feature number #6 is described in [`SUPERVISORD_COMPAT.md`](./SUPERVISORD_COMPAT.md) §7 (conditional enablement); `[eventlistener:x]` fields inherit from `[program:x]` per [`INI_COMPAT.md`](./INI_COMPAT.md); `sendRemoteCommEvent` is described in [`XMLRPC_COMPAT.md`](./XMLRPC_COMPAT.md) §9 P2.

**Core conclusion (preview)**: this is a **bidirectional wire-protocol subsystem** and cannot be replaced by the internal `EventHub` broadcast alone — the daemon must establish **stdout (event stream) / stdin (command stream)** channels with listener child processes, implementing the `READY`/`RESULT` handshake, length-prefixed envelope, per-pool event buffering and serial management. **It is recommended to add a standalone, sidecar "Event Listener subsystem"** that flows `EventHub` → event adapter layer → listener pool dispatch, without rewriting the existing execution core.

---

## 2. Priority Definitions

| Level | Meaning |
| :--- | :--- |
| **P0** | The minimal closed loop that makes the protocol correct: `[eventlistener:x]` parsing, pool process management, `READY`/`RESULT` handshake, `PROCESS_STATE_*` event delivery, buffering and backpressure. |
| **P1** | Full event surface: `PROCESS_LOG_*`, `PROCESS_COMMUNICATION_*`, `REMOTE_COMMUNICATION`, `TICK_*`, `PROCESS_GROUP_*`, `SUPERVISOR_STATE_CHANGE_*`; `result_handler`; protocol violation → `UNKNOWN`. |
| **P2 / deferred** | `PROCESS_COMMUNICATION_*` depends on stdin injection (#7) and capture tokens; `sendRemoteCommEvent` can be unlocked together with this subsystem. |
| **Unsupported / degraded** | `result_handler` import spec coupled to the Python runtime (see §4, §13). |

---

## 3. Terminology & Architecture Placement

| Term | Definition |
| :--- | :--- |
| **listener pool (pool)** | One `[eventlistener:x]` section is a **homogeneous process group**; `numprocs` listener processes share the same `events=` subscription set. Pool name = section name `x`. |
| **listener process (listener)** | A child process within the pool; stdout is the **protocol channel**, stdin is the **command channel**, stderr goes to logs. |
| **event** | An object produced by daemon-internal `notify(event)` that carries a `payload()` and a `serial`. |
| **envelope** | A one-line header plus variable-length payload sent to a listener process (§5.1). |
| **`READY`/`RESULT`** | Two control lines on the listener-process side: announcing readiness to receive and reporting the processing result. |

**Architecture placement**: add daemon→listener channels, alongside the existing `ManagerActor`/`ProcessActor`. `EventListenerPool` is not a simple reuse of `ProgramConfig`: it needs a **dedicated listener state machine** and an **event buffer queue**. Recommended design:

```
EventHub (internal broadcast)
   └─> EventAdapter  (SystemEvent/LogEntry -> stock event payload)
          └─> EventListenerPool(s)  (per-pool buffer + serial + dispatch)
                 └─> ListenerProcess.stdin/stdout  (READY/RESULT wire protocol)
```

---

## 4. Configuration Surface `[eventlistener:x]`

Fields are **fully compatible** with `[program:x]` (inherits `EventListenerConfig(ProcessConfig)`), with the additions:

| Field | Default | Constraint | Description |
| :--- | :--- | :--- | :--- |
| `command` | — | required (inherited) | The listener program command line. |
| `events` | — | **required**; comma/whitespace separated; uppercased; unknown event name → configuration error | Subscription set, e.g. `PROCESS_STATE,PROCESS_LOG,TICK_5`. |
| `buffer_size` | `10` | integer `>= 1`, otherwise configuration error | Per-pool upper bound for the event buffer (§7). |
| `result_handler` | `supervisor.dispatchers:default_handler` | import spec, parse failure → configuration error | See §5.3. |
| `priority` | `-1` (high) | integer | Listeners are **started first and stopped last**. |
| `redirect_stderr` | `false` | **must be false**; setting true → configuration error | Mixing into stdout would corrupt the protocol. |
| `autostart` | `true` (inherited) | boolean | The pool starts together with the daemon. |
| `numprocs` | `1` | integer | Number of listener processes in the pool. |

> The other `[program:x]` fields (`environment`, `user`, `startsecs`, `stopsignal`, `stdout_logfile`, etc.) keep the same semantics; `use_stderr` is **forced to true** (the listener process's stderr is separate from stdout).

---

## 5. Wire Protocol

### 5.1 Event Envelope

The daemon writes to the listener process's stdout (UTF-8 bytes):

```
ver:3.0 server:<identifier> serial:<global_serial> pool:<pool_name> poolserial:<pool_serial> eventname:<NAME> len:<payload_len>\n<payload>
```

| Field | Semantics |
| :--- | :--- |
| `ver` | Fixed at `3.0`. |
| `server` | `[supervisord] identifier`. |
| `serial` | **Global** monotonically increasing sequence (across all pools), wrapping at `maxint`. |
| `pool` | Pool name (`x` of `[eventlistener:x]`). |
| `poolserial` | **Per-pool** monotonically increasing sequence. |
| `eventname` | An event name from §6 (abstract types never appear). |
| `len` | **Character count** of the payload (Python `len(payload)`). Equal to the byte count under ASCII; **counted by character when multibyte UTF-8 is present, matching the original**, and the receiver reads under this semantics. |
| `<payload>` | The fixed-length body following the newline (§6); no trailing newline required. |

### 5.2 Handshake State Machine (listener-process side)

The listener process's `listener_state` starts as `ACKNOWLEDGED` (busy); the state machine:

| Current state | Received | Transition | daemon action |
| :--- | :--- | :--- | :--- |
| `ACKNOWLEDGED` | buffer begins with `READY\n` | → `READY` | may deliver events |
| `ACKNOWLEDGED` | data present but not `READY\n` | → **`UNKNOWN`** | log warning, stop delivery |
| `READY` | any speculative data | → **`UNKNOWN`** | log warning, stop delivery |
| `BUSY` | `RESULT <n>\n<n bytes>` | → `ACKNOWLEDGED` | invoke `result_handler`, continue on success |
| `BUSY` | invalid `RESULT` header | → **`UNKNOWN`** + `EventRejectedEvent` | event is dropped and warned |
| `BUSY` | insufficient `<n>` | stay `BUSY` (continue reading) | wait for the remaining data |
| `UNKNOWN` | anything | stay `UNKNOWN` | the listener process is **permanently** out of receive |

> The `READY` token is **exactly** `READY\n` (newline included); the `RESULT` prefix is `RESULT`.

### 5.3 Result Handling (`result_handler`)

- Default `supervisor.dispatchers:default_handler`: the body must be `OK`, otherwise `RejectEvent` is raised.
- `RejectEvent` → state returns to `ACKNOWLEDGED` and `notify(EventRejectedEvent)` is called; the pool **reinserts the rejected event at the head of the buffer** (redelivery).
- Handler raising any exception → → `UNKNOWN` + `EventRejectedEvent`.

---

## 6. Event Types & Payload Specification

`payload` is a **single line of space-separated `k:v` pairs** (`PROCESS_LOG_*`/`PROCESS_COMMUNICATION_*`/`REMOTE_COMMUNICATION` add one newline followed by a data body):

| eventname | payload format |
| :--- | :--- |
| `PROCESS_STATE_STARTING` / `PROCESS_STATE_BACKOFF` | `processname:<n> groupname:<g> from_state:<STATE> tries:<n>` |
| `PROCESS_STATE_RUNNING` / `PROCESS_STATE_STOPPING` / `PROCESS_STATE_STOPPED` | `processname:<n> groupname:<g> from_state:<STATE> pid:<pid>` |
| `PROCESS_STATE_EXITED` | `processname:<n> groupname:<g> from_state:<STATE> expected:<0\|1> pid:<pid>` |
| `PROCESS_STATE_FATAL` / `PROCESS_STATE_UNKNOWN` | `processname:<n> groupname:<g> from_state:<STATE>` |
| `PROCESS_LOG_STDOUT` | `processname:<n> groupname:<g> pid:<pid> channel:stdout\n<data>` |
| `PROCESS_LOG_STDERR` | same as above, `channel:stderr` |
| `PROCESS_COMMUNICATION_STDOUT` / `_STDERR` | `processname:<n> groupname:<g> pid:<pid>\n<data>` |
| `REMOTE_COMMUNICATION` | `type:<type>\n<data>` |
| `TICK_5` / `TICK_60` / `TICK_3600` | `when:<unix seconds integer>` |
| `SUPERVISOR_STATE_CHANGE_RUNNING` / `_STOPPING` | empty string |
| `PROCESS_GROUP_ADDED` / `PROCESS_GROUP_REMOVED` | `groupname:<g>\n` |

`<STATE>` is a `getProcessStateDescription` name (`STOPPED`/`STARTING`/`RUNNING`/`BACKOFF`/`STOPPING`/`EXITED`/`FATAL`/`UNKNOWN`); `expected` is an `0/1` integer.

**Trigger points**:

- `PROCESS_STATE_*`: state transitions of processes in `[program:x]`/groups.
- `PROCESS_LOG_*`: the process config sets `stdout_events_enabled` / `stderr_events_enabled=true`, and the process produces output.
- `PROCESS_COMMUNICATION_*`: `stdout_capture_maxbytes` / `stderr_capture_maxbytes` are configured and the output contains `<!--XSUPERVISOR:BEGIN-->…<!--XSUPERVISOR:END-->` capture tokens.
- `REMOTE_COMMUNICATION`: XML-RPC `sendRemoteCommEvent(type, data)`.
- `TICK_*`: daemon timers every 5s / 60s / 3600s.
- `PROCESS_GROUP_ADDED/REMOVED`: runtime `addProcessGroup`/`removeProcessGroup` (rsupervisord's compat shim, see [`XMLRPC_COMPAT.md`](./XMLRPC_COMPAT.md) §9).
- `SUPERVISOR_STATE_CHANGE_*`: daemon enters RUNNING / begins STOPPING.

---

## 7. Pool, Buffering & Dispatch Semantics

1. **Per-pool independent buffer**: events first go through `_acceptEvent` into the pool queue, then are dispatched in order during `transition()`.
2. **Serial assigned at enqueue time**: `serial` (global) and `poolserial` (per-pool) are assigned on first enqueue; redelivery reuses the same `serial`/`poolserial`.
3. **Backpressure**: events are delivered only to a `RUNNING` listener process with `listener_state == READY`; the first available one receives the event and that process transitions to `BUSY`.
4. **Redelivery**: on dispatch failure (including `RejectEvent`) the event is **reinserted at the head of the queue** and the current round of further dispatch stops.
5. **Overflow**: when the queue length is `>= buffer_size`, the **oldest** event is discarded and an error logged (`pool <name> event buffer overflowed, discarding event <serial>`).
6. **Multiple listener processes**: when several processes in the same pool are concurrent, each event is delivered to only one `READY` process.

---

## 8. Lifecycle & Process Semantics

- Pools use the default `priority` of `-1` and are **started first and stopped last**; when the daemon stops, ordinary programs should be stopped first and listener pools last.
- Listener stdout carries only the protocol; `redirect_stderr=true` is rejected.
- A pool can be operated like an ordinary group via `startProcess`/`stopProcess`/`signalProcess`, etc. (the pool name is the group name and the process name equals the pool name, so the namespecs are `listener`, `listener:*`).

---

## 9. XML-RPC / CLI Interaction Surface

| Method/command | Behavior |
| :--- | :--- |
| `supervisor.getProcessInfo("listener")` / `getAllProcessInfo` | The listener pool appears as an ordinary group (`group == pool name`, process name == pool name, `statename` normal). |
| `supervisor.sendRemoteCommEvent(type, data)` | Triggers the `REMOTE_COMMUNICATION` event, returns `True`. |
| `supervisor.getAllConfigInfo` | Listener pool configuration appears **in** `process_group_configs` (alongside program groups). |
| `supervisorctl status` | Displays the listener pool, consistent with program groups. |

---

## 10. rsupervisord Current State & Gap Mapping

| Capability | Current state | Gap |
| :--- | :--- | :--- |
| `[eventlistener:x]` parsing | INI adapter **discards** and logs a warning (`src/compat/ini/adapter.rs:115-118`) | must build pool configuration (§4) |
| Event source | `EventHub` (`src/manager/event.rs`: `SystemEvent`/`LogEntry`, internal) | must map to §6's stock payload + serial |
| Wire protocol | none | brand-new `READY`/`RESULT` channel + state machine (§5) |
| Buffering/dispatch | none | per-pool queue + `buffer_size` + redelivery/overflow (§7) |
| `sendRemoteCommEvent` | `FAILED` (`src/compat/xmlrpc/supervisor.rs:276`) | returns `True` once wired into this subsystem |
| `PROCESS_COMMUNICATION_*` | depends on stdin injection (#7) and capture tokens | P2, depends on #7 |

**Minimal mapping of EventHub → stock events** (P0):

| internal `SystemEvent` | stock event |
| :--- | :--- |
| `StateChanged`(→ Starting/Running/Exited/...) | `PROCESS_STATE_*` |
| `LogEntry`(stdout/stderr) | `PROCESS_LOG_STDOUT` / `PROCESS_LOG_STDERR`(gated by `*_events_enabled`) |
| `ConfigReloaded` + group add/remove | `PROCESS_GROUP_ADDED` / `PROCESS_GROUP_REMOVED` |
| `DaemonLifecycle` | `SUPERVISOR_STATE_CHANGE_RUNNING` / `_STOPPING` |
| timers | `TICK_5` / `TICK_60` / `TICK_3600` |
| XML-RPC `sendRemoteCommEvent` | `REMOTE_COMMUNICATION` |

---

## 11. Detailed Requirements

Convention: **MUST** must be implemented; **SHOULD** strongly recommended; **MAY** optional.

- **EL-1 (MUST)** Parse `[eventlistener:x]` into listener pool configuration: `events` required and validated, `buffer_size>=1`, `redirect_stderr` forbidden, `priority` defaults to `-1`; the listener pool appears in `getAllProcessInfo` as group `<x>`.
- **EL-2 (MUST)** Listener stdin/stdout within the pool are created as pipes; stdout carries only the protocol, stderr is separate.
- **EL-3 (MUST)** Implement the `READY\n` handshake and the `ACKNOWLEDGED→READY→BUSY→ACKNOWLEDGED` state machine; `BUSY` advances only after a complete `RESULT <n>\n` header and `<n>` byte body.
- **EL-4 (MUST)** Generate the envelope per §5.1; `serial` (global) and `poolserial` (per-pool) increase monotonically; `len` is the payload character count.
- **EL-5 (MUST)** Deliver only events in the subscription set (§6); types not listed in `events=` must not be sent down.
- **EL-6 (MUST)** P0 covers at least `PROCESS_STATE_*`, `TICK_*`, `REMOTE_COMMUNICATION`; P1 covers `PROCESS_LOG_*`, `PROCESS_GROUP_*`, `SUPERVISOR_STATE_CHANGE_*`; payload fields match §6 field by field.
- **EL-7 (MUST)** Protocol violations (non-`READY` data, speculative data after `READY`, invalid `RESULT` header) → the listener process transitions to `UNKNOWN`, a warning is logged, and delivery to it **stops**.
- **EL-8 (MUST)** Per-pool buffering and backpressure: on overflow drop the oldest and log an error; on dispatch failure redeliver from the head of the queue; redeliver when `result_handler` rejects.
- **EL-9 (SHOULD)** `result_handler` semantics align (default body==`OK`); rsupervisord may initially fix the default handler.
- **EL-10 (MUST)** `sendRemoteCommEvent(type, data)` returns `True` and triggers `REMOTE_COMMUNICATION`.
- **EL-11 (SHOULD)** Pools get the highest default priority: started first, stopped last.
- **EL-12 (MAY)** `PROCESS_COMMUNICATION_*`(depends on #7 stdin injection and capture tokens).

---

## 12. Acceptance & Baseline Test Mapping

Executable baseline: [`../compat/tests/test_eventlistener.py`](../compat/tests/test_eventlistener.py); first get all green on Python **4.2.5**, then gate release against rsupervisord.

| Requirement | Baseline case |
| :--- | :--- |
| EL-1 | `test_eventlistener_pool_is_running`, `test_process_state_groupname` |
| EL-3/EL-4 | `test_event_envelope_fields`, `test_protocol_violation_marks_listener_unknown` |
| EL-5/EL-6 | `test_process_state_events`, `test_process_log_stdout_events`, `test_tick_event`, `test_remote_communication_event` |
| EL-7 | `test_protocol_violation_marks_listener_unknown` |
| EL-8 | `test_event_buffer_overflow_discards_oldest` |
| EL-10 | `test_remote_communication_event` |

Running:

```bash
SUPERVISOR_TARGET=python bash compat/run.sh -k eventlistener   # baseline (all green)
bash compat/run.sh -k eventlistener                           # rsupervisord (currently xfail; unblock once implemented)
```

> Until implemented, this module is marked uniformly as **`xfail`** on the rsupervisord target; `SUPERVISOR_STRICT=1` escalates it to a hard failure, i.e. the full §11 backlog.

---

## 13. Non-Goals & Degradation

- **`result_handler` Python import spec**: rsupervisord does not embed a Python runtime. **Degradation**: only the built-in default handler (`body == OK`) is supported; custom `result_handler` is declared unsupported, or an equivalent Rust-side registration point may be provided (MAY).
- **`PROCESS_COMMUNICATION_*`**: depends on #7 stdin injection and capture tokens, listed as P2.
- **No rewrite of the existing execution core**: the listener pool is a sidecar subsystem; `EventHub` remains the internal source of truth, and the wire protocol is a product of the adapter layer.

---

## 14. References

- Python Supervisor **4.2.5**: `options.py`(`EventListenerConfig` / `EventListenerPoolConfig`), `process.py::EventListenerPool`, `dispatchers.py::PEventListenerDispatcher`, `events.py`, `rpcinterface.py::sendRemoteCommEvent`.
- Protocol documentation: the official Supervisor *Event Listeners* / *Events* sections (event types and `READY`/`RESULT` examples).
- Go reference implementation `ochinchina/supervisord` `events` package (`EventSysVersion = "3.0"`, `EventListener`, `BaseEvent`, `ProcessStateEvent`, `RemoteCommunicationEvent`) — a comparable implementation for event naming/envelopes.
- Related docs:[`SUPERVISORD_COMPAT.md`](./SUPERVISORD_COMPAT.md) §7 #6, [`XMLRPC_COMPAT.md`](./XMLRPC_COMPAT.md) §9, [`INI_COMPAT.md`](./INI_COMPAT.md), [`../compat/README.md`](../compat/README.md).
