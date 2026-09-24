# rsupervisord: CLI Compatibility Analysis (rsupervisorctl vs. supervisorctl) (CLI_COMPAT.md)

| Document Version | Status | Target Language | Scope |
| :--- | :--- | :--- | :--- |
| **v1.1.0** | Draft / For Review | Rust (Edition 2024) | Compatibility analysis, per-item requirements, examples and priority classification for `rsupervisorctl` client command format vs. Python `supervisorctl`; executable baseline in §9 / [`../compat/README.md`](../compat/README.md) |

---

## 1. Purpose & Scope

This document answers exactly one question: **how `rsupervisorctl`'s command-line format aligns with Python `supervisorctl`**.

**Baseline & References**

- **Authoritative baseline**: Python Supervisor **4.2.5**'s `supervisor/supervisorctl.py` (the command surface is governed by it).
- **Reference implementation**: Go `ochinchina/supervisord`'s `ctl.go` (commit `7a73369`).
- **Current-state baseline**: `rsupervisorctl` (`src/cli/args.rs`, `src/cli/commands.rs`, `src/cli/client.rs`).

**Two compatibility goals (keep them distinct to avoid duplicated work)**

| Goal | Meaning | Contract placement | Covered by this document |
| :--- | :--- | :--- | :--- |
| **A — stock `supervisorctl` direct connection** | Users point the Python `supervisorctl` binary straight at our daemon | **XML-RPC method surface + `[supervisorctl]` config section** (see `SUPERVISORD_COMPAT.md` #5) | No (independent of CLI syntax) |
| **B — `rsupervisorctl` syntax alignment** | Scripts/muscle memory written for `supervisorctl` work as-is with `rsupervisorctl` | This document's commands, options, output, exit codes | **Yes** |

> A is the real drop-in contract. B is low-cost and worth doing, but do **not** use B to re-do what A already provides.

---

## 2. Priority Definitions

| Priority | Meaning | Criterion |
| :--- | :--- | :--- |
| **P0** | Compatibility-breaking / script contract | Flunks unless changed — conflicts with Python semantics, or breaks existing scripts/automation. Must-do. |
| **P1** | Important but low-risk | Commonly used Python commands, low implementation cost, no conflicts. |
| **P2** | Minor / dependent-deferred item | Depends on deferred items in #3/#4, or high cost with acceptable degraded/fallback option. |
| **Not Supported** | Explicitly out of scope | **Cost too high and the Go version also doesn't implement it**; or there's a better alternative. |

---

## 2.1 Compatibility positioning: contract vs. UX (master summary)

One decision rule:

> **Will that output be pipe-consumed by old scripts?** Yes → **contract surface**, must match Python character-for-character / byte-for-byte / exit-code-for-exit-code;
> No → **UX surface**, free design, and deliberately made more modern than Python.

Position: **compatibility exists to accommodate old applications that refuse to migrate, covering only the interfaces they actually step on; we are not a doormat for Python's UX.**
Every item in §3–§7 below is labeled along these two layers.

### [Contract Surface] (relied on by old scripts, locked down by oracle tests)

| Surface | Item | Why it's a contract |
| :--- | :--- | :--- |
| Command syntax + namespec | `stop mygroup:*`, `start all` | Scripts invoke verbatim |
| LSB exit codes | `status >/dev/null; [ $? -eq 3 ]` | The highest-value compatibility point of this CLI |
| `status` non-TTY plain text | `status \| grep RUNNING` | Template `%(namespec)-33s%(state)-10s%(desc)s` (see §4.2) |
| `pid` pure-numeric output | `pid=$(supervisorctl pid web)` | Numeric value taken directly |
| `tail` / `maintail` byte upper bound | `tail -100 web \| wc -c` | Byte semantics |
| `reread` / `update` lines | `reread \| grep available` | `name: available\|changed\|disappeared` |
| `start` / `stop` / `restart` result lines | `\| grep -q started` | `namespec: started\|stopped` and `ERROR (...)`, on stdout |
| `avail` non-TTY columns | `avail \| awk '{print $1}'` | Fixed column positions |
| `-u/-p/-s/-c` (short + long names) | `supervisorctl -u a -p b status` | Fixed invocation from scripts |

### [UX Surface] (free → deliberately modern)

| Surface | Python current state | rsupervisord's approach |
| :--- | :--- | :--- |
| `version` | Prints daemon version `4.2.5` | Prints its own identity `rsupervisorctl <ver> (protocol supervisor 4.2.5)`; keeps a compatibility marker, but the subject is "itself" |
| `help` / `--help` | Flat command list | clap rendering: grouping, examples, alias annotations |
| Error detail | Mixed into the stdout result lines | stdout carries only contract lines; detail goes to **stderr**, structured and contextual |
| `shutdown` / `reload` wording | `Shut down` / `Restarted supervisord` | Its own voice (`Daemon restarted successfully` …); exit codes remain the contract |
| Tables / colors under TTY | Verbatim-fixed | Table + state coloring + highlighting (already present, kept) |

### Enforcement mechanisms

1. **Tests pin only the contract surface**: `test_cli.py` (stock `supervisorctl` direct connection) and the §10 scripted acceptance assert per item —
   exit code / stdout text shape / byte upper bound; **the UX surface never enters tests** — no assertions on `version`/`help` wording,
   TTY table rendering, or error prose, so modernization has full freedom and is not frozen by tests.
2. **Contract surface is handed to the oracle**: `test_cli.py` (stock `supervisorctl` direct connection) and §9 acceptance assert per item,
   gated for auto-release by XML-RPC/#5.

---

## 3. Overview

### 3.1 Command surface

| Command | Python 4.2.5 | Go reference | rsupervisorctl current state | Priority | Classification |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `status` | ✅ supports namespec/`all` | ✅ | ⚠️ present, but table output, no exit code | **P0** | Contract (TTY table = UX) |
| `help` | ✅ | ❌ | ❌ | **P0** | UX |
| `version` | ✅ | ⚠️ top-level `version` | ❌ | **P0** | UX (syntax must exist) |
| `pid` | ✅ | ✅ | ❌ | **P0** | Contract |
| `shutdown` | ✅ | ✅ | ❌ | **P0** | Contract (exit code) / wording UX |
| `reload` | ✅ (restart the daemon) | ✅ (same as Python) | ✅ restarts the daemon (implemented) | **P0** | Contract (semantics: restart the daemon) |
| `reload-config` | ❌ (extension) | ❌ | ✅ hot reload (implemented) | Extension kept | Extension (hot reload) |
| `reread` | ✅ | ✅ | ❌ | **P0** | Contract |
| `update` | ✅ | ✅ | ⚠️ capability inside `reload` | **P0** | Contract |
| `start` / `stop` / `restart` | ✅ namespec/`all` | ✅ | ⚠️ present, namespec semantics to be aligned | **P0** | Contract |
| `tail` | ✅ `[-f\|-N] <name> [stdout\|stderr]` | ⚠️ no `-N` | ⚠️ different shape (`-n` lines, no channel) | **P0** | Contract |
| `signal` | ✅ | ✅ | ❌ | **P1** | Contract |
| `avail` | ✅ | ❌ | ❌ | **P1** | Contract (non-TTY columns) |
| `open` | ✅ | ❌ | ❌ | **P1** | UX |
| `maintail` | ✅ | ❌ | ❌ | **P1** | Contract |
| `clear` | ✅ | ✅ | ❌ | **P2** | Contract |
| `add` / `remove` | ✅ | ✅ | ❌ | **P2** (depends on #3/#4) | Contract |
| `fg` | ✅ | ✅ (simplified implementation) | ❌ | **P2** (reference Go) | Extension (not a real PTY, Go-simplified shape) |
| `quit` / `exit` / `^D` | ✅ | ❌ | ❌ | **Not Supported** (goes with the interactive shell) | Not Supported |
| (interactive shell) | ✅ | ❌ | ❌ | **Not Supported** | Not Supported |
| `events` | ❌ (extension) | ✅ | ✅ | Extension kept | Extension |
| `stdin` | ❌ (extension) | ❌ (XML-RPC only) | ✅ | Extension kept | Extension |

### 3.2 Client options

| Option | Python 4.2.5 | Go reference | rsupervisorctl current state | Priority |
| :--- | :--- | :--- | :--- | :--- |
| `-c/--configuration` | ✅ | ❌ (auto-detect) | ⚠️ `-c/--config` | **P0** (add alias) |
| `-s/--serverurl` | ✅ | ✅ | ⚠️ `-s/--server` | **P0** (add alias) |
| `-u/--username` | ✅ | ⚠️ `-u/--user` | ⚠️ `-u/--user` | **P0** (add alias) |
| `-p/--password` | ✅ | ⚠️ `-P/--password` | ❌ `-P/--password` | **P0** (change to `-p`) |
| `-k/--key` | ❌ | ❌ | ✅ | Extension kept |
| `-i/--interactive` | ✅ | ❌ | ❌ | **Not Supported** |
| `-r/--history-file` | ✅ | ❌ | ❌ | **Not Supported** |

---

## 4. Global Behavior (cross-command)

### 4.1 Exit codes (P0)

**Requirement**: adopt Python's LSB exit codes so shell scripts can decide via `$?`.

| Code | Meaning | Trigger example |
| :--- | :--- | :--- |
| 0 | SUCCESS | command succeeded |
| 1 | GENERIC | generic error, missing argument (stop/restart/signal/clear/tail) |
| 2 | INVALID_ARGS | `start` missing process name; unknown command |
| 3 | NOT_RUNNING (at the init layer: UNIMPLEMENTED_FEATURE) | `status` finds any process in STOPPED state |
| 4 | UNKNOWN (at the init layer: INSUFFICIENT_PRIVILEGES) | `status` names a nonexistent process; upcheck failed |
| 5 | NOT_INSTALLED | API version mismatch |
| 7 | NOT_RUNNING | start/stop/pid on an already-stopped/dead process |

**Example**

```bash
rsupervisorctl status web >/dev/null; echo $?   # web not running -> 3
rsupervisorctl status nosuch; echo $?           # nonexistent     -> 4
rsupervisorctl start nosuch; echo $?            # nonexistent     -> 7 (dead-process class)
rsupervisorctl start; echo $?                   # missing arg     -> 2
```

> Current state: `rsupervisorctl` only returns 0/1 (anyhow errors). Exit codes must be uniformly mapped in `src/cli/commands.rs`.

### 4.2 Output format (P0)

**Requirement**: `status` outputs Python-compatible plain text on **non-TTY** (pipe/redirect) for script parsing; the existing colored table may be kept on TTY.

Python template: `'%(namespec)-33s%(state)-10s%(desc)s'`

**Example**

```
web                          RUNNING   pid 1234, uptime 0:00:10
db                           STOPPED   Not started
```

> Current state: `src/cli/commands.rs:60` uses the `tabled` rounded-corner table. Suggestion: when `std::io::stdout().is_terminal()` is false, take the plain-text branch.

### 4.3 namespec and `all` (P0)

**Requirement**: uniformly support Python's three name forms:

| Form | Semantics | Example |
| :--- | :--- | :--- |
| `name` | single process (group == name when no colon) | `start web` |
| `group:process` | specific process within a group | `start mygroup:worker` |
| `group:*` | all processes within a group | `stop mygroup:*` |
| `all` | all processes | `restart all` |

**Example**

```bash
rsupervisorctl status mygroup:*
rsupervisorctl stop mygroup:*
rsupervisorctl start all
```

> Current state: `client.rs` already handles `all` and `group:*` (client.rs:120/148/150), but the **bare `name`-as-group semantics, `status`'s `group:*` filtering, and the error text/exit code for nonexistent names** are not aligned.

### 4.4 Interactive shell (Not Supported)

**Requirement**: Python enters a REPL with no arguments or with `-i` (`supervisor>` prompt, auto-runs `status` at startup, tab completion, `quit`/`exit`/`^D`, interactive mode always returns 0).

**Decision**: **Not Supported**. Reasons:

- The Go version (`ctl.go`) is subcommand-style and **also does not implement** an interactive shell.
- High cost (need to bring in `rustyline`, completion, history, command dispatch), while one-shot subcommands already cover all automation/script scenarios.

**Alternative**: everything is provided as one-shot subcommands; `quit`/`exit`/`-i`/`-r` are likewise folded into this Not-Supported item.

---

## 5. Client Option Requirements [Contract Surface]

> Options are fixed invocation points for old scripts and all belong to the **contract surface**; except the rsupervisord-only extensions in `§5.2`.

### 5.1 P0

#### 5.1.1 `-p/--password` (short-option correction)

**Requirement**: change the password short option to `-p` (Python convention); keep `-P` as a compatibility alias. The current `-P` conflicts with Python.

```bash
# Python-style (must work)
rsupervisorctl -u admin -p secret status
# legacy style still works (alias)
rsupervisorctl -u admin -P secret status
```

#### 5.1.2 `-s/--serverurl`

**Requirement**: add the long name `--serverurl` (keep `-s`/`--server`). The value supports `http://` and `unix://`; default `http://localhost:9001`.

```bash
rsupervisorctl --serverurl http://127.0.0.1:9001 status
rsupervisorctl --serverurl unix:///run/rsupervisord.sock status
```

#### 5.1.3 `-u/--username`

**Requirement**: add the long name `--username` (keep `-u`/`--user`).

```bash
rsupervisorctl -u admin -p secret status
rsupervisorctl --username admin --password secret status
```

#### 5.1.4 `-c/--configuration`

**Requirement**: add the long name `--configuration` (keep `-c`/`--config`). Once #2 (INI) lands, `serverurl`/`username`/`password` should be readable from the `[supervisorctl]` section as defaults.

**Implemented** (Python parity + multi-candidate connectivity): config is always loaded when available; the effective form is an ordered **`Vec<CtlConfig>`** chain. Resolution rules in `resolve_ctl_chain` / `resolve_endpoint_candidates`:

1. **No config file found** (no `-c` and default path missing) → dual chain: `[default local UDS/pipe, http://localhost:9001]` (IPC first). Explicit `-c` pointing at an unloadable file → hard error (no silent fallback).
2. **Config loaded, section present** (`ctl` / `[supervisorctl]`) → **single** candidate (strict Python: no server fallbacks). Partial fields filled from `server` at load when `server.ctl_defaults`. Missing `serverurl` with a present section → `http://localhost:9001`.
3. **Config loaded, no section, `ctl_defaults=true`** (YAML) → full server backfill via `CtlConfig::vec_from_server`: IPC entry first (uds_path + uds credentials + token), then TCP entry when `http_bind` is set (http URL + username/password + token).
4. **Config loaded, no section, `ctl_defaults=false`** (INI) → **hard error** even with `-s` (Python `options.py` requires the section).

CLI flags (after chain build): `-s` **replaces the whole chain** with one endpoint (credential seed chosen by endpoint type: TCP vs IPC); `-u`/`-p` and `-k` apply field-independently to **every** remaining `CtlConfig` via `CliArgs::apply`.

```bash
rsupervisorctl -c /etc/supervisord.conf status
rsupervisorctl --configuration /etc/supervisord.conf status
```

**Note**: `-s` does **not** bypass a missing `[supervisorctl]` / `ctl` section when a config file was loaded (Python parity).

### 5.2 Extensions kept

`-k/--key` (Bearer token) is rsupervisord-only and is kept; it does not conflict with Python. (Caller elevation verification is enforced daemon-side via `server.allow_unelevated`).

---

## 6. Command Requirements (per-command)

> Convention: each item gives **syntax / semantic requirements / example / current-state gap**.

### 6.1 P0

#### 6.1.1 `help` [UX Surface]

**Syntax**: `help [action]`
**Semantics**: with no arguments, list all actions; with an argument, print that action's help.
**Example**

```bash
rsupervisorctl help
rsupervisorctl help start
```

**Classification**: UX surface (see §2.1). **Not verbatim-aligned** with Python's flat list; use `clap`'s modern rendering: grouping, examples, alias annotations.
**Current state**: none. Can be implemented by wiring through clap's help text.

#### 6.1.2 `version` [UX Surface]

**Syntax**: `version`
**Semantics** (**not aligned with Python output**): print rsupervisord's own identity, plus one line of compatibility protocol marker. No longer parrots `4.2.5`.
**Example**

```bash
rsupervisorctl version
# rsupervisorctl 0.6.0 (protocol supervisor 4.2.5)
```

**Classification**: UX surface (the syntax just needs to exist; format is free; see §2.1).
**Current state**: none (there is `--version`, but that is the client's own version with different semantics — this time the semantics of `version` are corrected to "print its own identity", and `--version` is merged/kept).

#### 6.1.3 `pid` [Contract Surface]

**Syntax**: `pid [name…]` / `pid all`
**Semantics**: no argument = daemon PID; `all` = one line per child process; named = that process's PID; exit code 7 when PID == 0. Output is **numbers/lines only**, consumable by `pid=$(...)`.
**Classification**: contract surface.
**Example**

```bash
rsupervisorctl pid          # -> 4321
rsupervisorctl pid web      # -> 4567
rsupervisorctl pid all
```

**Current state**: none.

#### 6.1.4 `shutdown` [Contract Surface · wording UX]

**Syntax**: `shutdown`
**Semantics**: shut down the remote daemon. Errors when given arguments (exit code 1). Confirmation required in interactive mode; non-interactive executes directly.
**Classification**: exit code is the contract; the output wording `Shut down` is UX surface and may be in its own voice (consistent within C is enough).
**Example**

```bash
rsupervisorctl shutdown
```

**Current state**: none.

#### 6.1.5 `reload` (semantic ruling) [Contract Surface]

**Syntax**: `reload`
**Semantics (aligned with Python)**: restart the remote daemon (stop all → re-read config → restart); errors when given arguments.
**Classification**: contract surface — `reload`'s **semantics** must match Python's (restart the daemon); the output wording is UX surface (currently `Daemon restarted successfully`; no contract assertion, only exit code 0 + daemon alive).

> **Python reference**:
>
> - `reread` = re-read config only, **no add/remove**
> - `update` = re-read + add/remove + restart affected groups
> - `reload` = **restart the daemon**
> - `reload-config` (rsupervisord extension) = zero-downtime **hot reload**, strictly distinct from `reload`

**Example**

```bash
rsupervisorctl reload        # restart the daemon
```

**Implementation (already landed)**:

- Client: `rsupervisorctl reload` → daemon restart (`handle_daemon_reload`; daemon stops all → re-reads config → restarts; see `manager/supervisor.rs::execute_restart_daemon`); the output prose is UX surface, no contract assertion.
- Protocol: HTTP `POST /api/v1/reload`; XML-RPC `supervisor.restart` (both implemented).
- **hot-reload and reload are two separate paths on the daemon side**: hot reload goes via `POST /api/v1/config/reload` and `supervisor.reloadConfig`; `reload` goes via `/api/v1/reload` and `supervisor.restart`.

#### 6.1.6 `reread` [Contract Surface]

**Syntax**: `reread`
**Semantics**: re-read the config, **no process add/remove**. Outputs the change list: each line `name: available|changed|disappeared`; outputs `No config updates to processes` when nothing changed.
**Classification**: contract surface (lines consumable by `reread \| grep available`).
**Example**

```
$ rsupervisorctl reread
web: changed
api: available
```

**Current state**: none (its "read config and produce diff" capability can reuse the parsing path of `reloadConfig`/`config reload`).

#### 6.1.7 `update` [Contract Surface]

**Syntax**: `update [gname…]` / `update all`
**Semantics**: re-read config + add/remove + restart affected groups. Output: `gname: stopped` / `gname: removed process group` / `gname: updated process group` / `gname: added process group`.
**Classification**: contract surface (lines consumable by scripts).
**Example**

```bash
rsupervisorctl update
rsupervisorctl update mygroup
rsupervisorctl update all
```

**Current state**: none (`reload-config`/`config reload` already have a "read + apply" hot-reload path that can be reused here; but Python's output line format is missing).

#### 6.1.8 `status` (rework) [Contract Surface]

**Syntax**: `status [name…|gname:*|all]`
**Semantics**: no argument or `all` = everything; supports `group:*` and multiple names; a nonexistent name outputs `X: ERROR (no such group|process)` and sets exit code 4; any process STOPPED sets exit code 3.
**Classification**: contract surface — on **non-TTY** it must output the Python template `%(namespec)-33s %(state)-10s %(desc)s` as plain text consumable by `grep`/`awk`; the **TTY** table + coloring is UX surface, free.
**Example**

```bash
rsupervisorctl status
rsupervisorctl status web api
rsupervisorctl status mygroup:*
```

**Current-state gap**: output is a table (see §4.2); exit codes missing; `group:*` filtering to be verified.

#### 6.1.9 `start` / `stop` / `restart` (align) [Contract Surface]

**Syntax**

- `start <name…>` / `start all` / `start gname:*`
- `stop <name…>` / `stop all` / `stop gname:*`
- `restart <name…>` / `restart all` / `restart gname:*`

**Classification**: contract surface — result lines `namespec: started|stopped` and error lines `namespec: ERROR (…)` **on stdout**, consumable by `\| grep -q started`.
**Semantics**: support namespec and `all`; `restart` = stop+start, **does not re-read config**; `start` with missing arguments exits 2, other missing-argument cases exit 1; dead-process-class errors exit 7; outputs `namespec: started|stopped`, errors `namespec: ERROR (…)`.

**Example**

```bash
rsupervisorctl start web api
rsupervisorctl stop mygroup:*
rsupervisorctl restart all
```

**Current-state gap**: already has `-a/--async`, `-t/--timeout` (**Python has no such options; kept as extensions**); namespec/`all`/exit codes/output text need aligning.

#### 6.1.10 `tail` (rework) [Contract Surface]

**Syntax**: `tail [-f|-N] <name> [stdout|stderr]`
**Semantics**: defaults to `stdout`; defaults to the last **1600 bytes**; `-f` follows continuously; `-N` takes the last N bytes.
**Classification**: contract surface — the byte upper bound can be verified by `tail -100 web \| wc -c`.

**Example**

```bash
rsupervisorctl tail web            # last 1600 bytes of stdout
rsupervisorctl tail web stderr
rsupervisorctl tail -100 web       # last 100 bytes
rsupervisorctl tail -f web         # follow continuously
```

**Current-state gap**: currently `tail <name> [-f] [-n lines]` (line count, no channel). Needed:

- add the `stdout|stderr` positional argument;
- support the `-N` byte modifier;
- byte semantics via the **degraded/fallback implementation** of `SUPERVISORD_COMPAT.md` #8 (line-level ring-buffer approximation);
- keep `-n` as an extension alias.

### 6.2 P1

#### 6.2.1 `signal` [Contract Surface]

**Syntax**: `signal <sig> <name…>` / `signal <sig> all` / `signal <sig> gname:*`
**Semantics**: send a signal; requires ≥2 arguments; outputs `namespec: signalled`.
**Classification**: contract surface (result lines follow the start/stop/restart rules).
**Example**

```bash
rsupervisorctl signal HUP nginx
rsupervisorctl signal TERM all
```

**Current state**: none. Depends on daemon signal capability.

#### 6.2.2 `avail` [Contract Surface]

**Syntax**: `avail`
**Semantics**: list all configured processes; template `'%(name)-32s %(inuse)-9s %(autostart)-9s %(priority)s'`, `inuse` = in use/avail, `autostart` = auto/manual, `priority` = `group_prio:process_prio`.
**Classification**: contract surface — column positions fixed on **non-TTY**, consumable by `awk '{print $1}'`; TTY highlighting is UX surface.
**Example**

```
web                              in use    auto      999:999
api                              avail     manual    999:999
```

**Current state**: none (`/api/v1/status` exists and can be reused).

#### 6.2.3 `open` [UX Surface]

**Syntax**: `open <url>`
**Semantics**: switch the current session's serverurl; accepts only `http://` or `unix://`.
**Classification**: UX surface (session-level operation, no script consumption scenario; syntax alignment is enough).
**Example**

```bash
rsupervisorctl open unix:///run/rsupervisord.sock
```

**Current state**: none (cheap).

#### 6.2.4 `maintail` [Contract Surface]

**Syntax**: `maintail [-f|-N]`
**Semantics**: tail the **daemon's own** log; default 1600 bytes.
**Classification**: contract surface — byte upper bound follows the same rule as `tail`.
**Example**

```bash
rsupervisorctl maintail
rsupervisorctl maintail -f
```

**Current state**: none. Depends on daemon main log readability (filed under `SUPERVISORD_COMPAT.md` #8/#12).

### 6.3 P2

#### 6.3.1 `clear` [Contract Surface]

**Syntax**: `clear <name…>` / `clear all`
**Semantics**: clear the process log; outputs `namespec: cleared`.
**Classification**: contract surface (result lines follow the start/stop/restart rules).
**Example**

```bash
rsupervisorctl clear web
rsupervisorctl clear all
```

**Current state**: none. Needs log-clearing capability (truncate file + flush ring buffer).

#### 6.3.2 `add` / `remove` [Contract Surface]

**Syntax**: `add <name…>` / `remove <name…>`
**Semantics**: activate/remove config groups at runtime; `remove` errors for groups still running.
**Classification**: contract surface (command syntax/exit codes; result lines follow the generic rules).
**Example**

```bash
rsupervisorctl add newgroup
rsupervisorctl remove oldgroup
```

**Current state**: none. **Depends on #3/#4 (already landed)**, now N/A — the daemon-side `addProcessGroup`/`removeProcessGroup` and XML-RPC are already implemented (see `XMLRPC_COMPAT.md`); only the CLI wiring remains.

#### 6.3.3 `fg` [Extension]

**Syntax**: `fg <name>`
**Semantics**: foreground adopts: follows stdout+stderr and forwards terminal input to the process's stdin.
**Example**

```bash
rsupervisorctl fg web
```

**Current state**: none. The Go version uses a "double logtail + stdin loop" simplified implementation (not a real PTY). Suggestion: align with Go's simplified version, no terminal raw mode. Depends on `sendProcessStdin` (#7).

### 6.4 Extension commands (non-Python, kept)

| Command | Description |
| :--- | :--- |
| `events` | SSE real-time system event stream (rsupervisord-only) |
| `stdin <name> <chars>` | inject into a process's stdin (Python only exposes it via XML-RPC, no CLI) |
| `reload-config` (alias `config reload`) | zero-downtime hot reload (implemented); `reload` is restored to Python's "restart the daemon" semantics |

### 6.5 Not Supported

| Item | Reason |
| :--- | :--- |
| Interactive shell (REPL) | High cost and the Go version doesn't implement it; one-shot subcommands already cover script scenarios |
| `-i/--interactive` | Same as above |
| `-r/--history-file` | Same as above (readline history, only meaningful for the interactive shell) |
| `quit` / `exit` / `^D` | Same as above (only meaningful for the interactive shell) |

---

## 7. Acceptance

**Scripted acceptance (exit codes + plain text)**

```bash
set -e
rsupervisorctl status >/dev/null || [ $? -eq 3 ]     # stopped process present
rsupervisorctl start all
rsupervisorctl status | grep -q RUNNING
rsupervisorctl tail -100 web | wc -c                  # no more than 100 bytes
rsupervisorctl signal HUP web
rsupervisorctl pid web
rsupervisorctl update
rsupervisorctl reread
rsupervisorctl reload                                  # restart the daemon
rsupervisorctl shutdown
```

**Option-compatibility acceptance**

```bash
rsupervisorctl -u admin -p secret status              # -p works
rsupervisorctl --serverurl http://127.0.0.1:9001 status
rsupervisorctl --configuration /etc/supervisord.conf status
```

---

## 8. Relationship to Other Documents

- Server-side XML-RPC method surface (the real contract for Goal A): see `SUPERVISORD_COMPAT.md` §7 #5.
- Degraded/fallback implementation for `tail` byte offsets: see `SUPERVISORD_COMPAT.md` §7 #8.
- `sendProcessStdin` (data plane, dependency of `fg`/`stdin`): see `SUPERVISORD_COMPAT.md` §7 #7.

---

## 9. Compatibility Test Baseline

The two compatibility goals map to two executable baselines respectively (see [`../compat/README.md`](../compat/README.md)):

- **Goal B (native syntax alignment)**: [`../compat/tests/test_native_cli.py`](../compat/tests/test_native_cli.py) runs a **contract-surface** smoke check against the compiled `rsupervisorctl` (`status` / `start` / `stop` / `restart` / `tail` / `stdin` / `reload-config`; exit-code + state assertions, **no UX-wording assertions**), **5 passed** on the default target.
- **Goal A (unmodified stock `supervisorctl` direct connection)**: the 27 cases in [`../compat/tests/test_cli.py`](../compat/tests/test_cli.py) are the oracle; first **27 passed** on Python 4.2.5; they uniformly `xfail` against the compiled bin because `/RPC2` is not implemented (same as `XMLRPC_COMPAT.md` §12).

That is: once the server side has XML-RPC (§7 #5), the P0/P1 requirements listed in §6/§7 will **automatically** turn the above oracle from `xfail` into per-item assertions.

---

## 10. Contract vs. UX — Test Division of Labor

The contract surface and the UX surface receive **completely different** treatment in tests:

| Layer | Test strategy | Assertion points |
| :--- | :--- | :--- |
| **Contract surface** | **Strict hardening** (oracle/§7 scripted acceptance) | Exit code, stdout text shape, byte upper bound, command syntax |
| **UX surface** | **No assertions, and guarded against "being tested"** | `version`/`help` wording, TTY table, error prose, stderr detail — **strictly forbidden from entering validation scripts** |

Key points:

- `test_cli.py` (stock `supervisorctl` direct connection) is the authoritative oracle for the contract surface; `test_native_cli.py` only runs
  the **contract-surface** native smoke test (`status`/`start`/`stop`/`restart`/`tail`/`stdin`/`reload-config`…),
  and each case asserts only the exit code and a machine-consumable stdout shape, **never any UX wording**.
- Modernization is a free choice: **tests must not freeze "modern"** (e.g. don't assert "`version` must not contain `4.2.5`"),
  and must not freeze "Python verbatim" either. Contracts don't regress, modernization isn't frozen.
- Every contract-surface assertion traces to §2.1 / §6 / §7; the UX surface has **no** corresponding entries in the validation scripts.
