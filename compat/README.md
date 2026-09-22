# rsupervisord compatibility harness

Executable compatibility contract for rsupervisord. The suite drives **stock
Python Supervisor 4.2.5** (the golden oracle) and the **compiled rsupervisord
binary** with the same test corpus, so compatibility is measured, not asserted
by prose. See `docs/XMLRPC_COMPAT.md`, `docs/CLI_COMPAT.md`,
`docs/SUPERVISORD_COMPAT.md` (§8) and `docs/EVENTLISTENER_COMPAT.md`.

> Stock Python Supervisor has **no native Windows support**. Everything here
> runs on the Linux side, i.e. inside WSL on this project (`ubuntu22`).

## Targets

Selected with `SUPERVISOR_TARGET`:

| Target | Server under test | Client | Purpose |
| :--- | :--- | :--- | :--- |
| `rsupervisord` (**default**) | compiled `target/<profile>/rsupervisord` (YAML fixture) | `rsupervisorctl` (native) + stock client (gated) | measure current parity |
| `python` | stock `supervisord` 4.2.5 (INI fixture) | stock `supervisorctl` + XML-RPC | define expected behavior (must be green) |

## Quick start

```sh
# from the repo root, inside WSL:
bash compat/run.sh                              # default: build + test compiled bin
SUPERVISOR_TARGET=python bash compat/run.sh     # golden oracle (4.2.5)
SUPERVISOR_STRICT=1 bash compat/run.sh          # unsupported features => hard failures
bash compat/run.sh -m native                    # only native rsupervisorctl tests
bash compat/run.sh -k xmlrpc                    # subset (any pytest args pass through)
```

From Windows without entering WSL interactively:

```powershell
wsl -d ubuntu22 -- bash -lc "cd /mnt/d/githome/spritetong/rsupervisord && bash compat/run.sh"
```

`run.sh` bootstraps the local venv on first use, and (for the default target)
runs `cargo build` before pytest. Extra knobs: `SUPERVISOR_STRICT`,
`RSUPERVISORD_PROFILE`, `RSUPERVISORD_BIN`, `RSUPERVISORCTL_BIN`.

Ubuntu prerequisite for `venv` (once): `apt-get install -y python3-venv`.

## Result semantics

The suite selects its target from the same corpus:

- **native** tests (`-m native`) run only against the compiled bin.
- **oracle** tests (`-m oracle`) need a server that speaks XML-RPC. Against the
  compiled bin they are **capability-probed** at `/RPC2`; XML-RPC is implemented,
  so they run, including `addProcessGroup`/`removeProcessGroup`. The **event-listener**
  module (feature #6) is implemented and its oracle tests run against the compiled
  bin as well. `SUPERVISOR_STRICT=1` turns gated tests into hard failures.

| Run | Result |
| :--- | :--- |
| `SUPERVISOR_TARGET=python` | 69 passed, 5 skipped |
| default compiled bin (YAML) | 74 passed |
| compiled bin with `SUPERVISOR_RSD_FORMAT=ini` | 74 passed |
| `SUPERVISOR_STRICT=1` (compiled bin) | 74 passed |

## Layout

```
compat/
  bootstrap.sh            # creates .venv/ and installs the pinned toolchain locally
  requirements.txt        # supervisor==4.2.5, pytest
  run.sh                  # target-aware runner (builds cargo for the default target)
  conf/
    supervisord.conf      # python target: comprehensive INI (all section classes)
    conf.d/extra.ini      # python target: [include] files = conf.d/*.ini demo
    rsupervisord.yaml     # rsupervisord target: YAML fixture (placeholders filled by conftest)
    listener.py           # python target: protocol-correct [eventlistener] program (logs envelopes)
    badlistener.py        # python target: protocol-violating [eventlistener] (UNKNOWN state)
  tests/
    conftest.py           # target dispatch: launch/teardown, XML-RPC + REST probes, CLI runners
    test_cli.py           # oracle: stock supervisorctl grammar + exit codes
    test_xmlrpc.py        # oracle: supervisor.* / system.* methods + faults
    test_eventlistener.py # oracle: READY/RESULT protocol, event taxonomy, buffering (gated #6)
    test_zz_daemon.py     # oracle: shutdown (runs last)
    test_native_cli.py    # native: rsupervisorctl against the compiled bin
```

The local venv (`.venv/`) is a **disposable, project-local** environment so the
toolchain never collides with any system/global `supervisord` on `PATH`
(`.venv/` is git-ignored).

## What the fixtures exercise

`conf/supervisord.conf` (python) uses every standard INI section class;
`conf/rsupervisord.yaml` (rsupervisord) mirrors the same program set in YAML:
one-shot exit, long-running stdout+stderr, stdin (`sendProcessStdin`), a
`numprocs` fan-out, start-failure → `FATAL`, and an unspawnable binary.

## Coverage

- **XML-RPC oracle** (`test_xmlrpc.py`): API/state/pid, `system.listMethods` /
  `methodHelp` / `methodSignature` / `multicall`, `getAllProcessInfo` /
  `getProcessInfo` field set, start/stop/restart for process/group/all,
  `signal*`, `sendProcessStdin`, `tail*Log` / `read*Log` / `clear*Log`,
  `getAllConfigInfo`, `reloadConfig`, aliases, and Fault codes (`BAD_NAME` 10,
  `BAD_SIGNAL` 11, `NO_FILE` 20, `ALREADY_STARTED` 60, `NOT_RUNNING` 70,
  `SIGNATURE_UNSUPPORTED` 4, `UNKNOWN_METHOD` 1).
- **supervisorctl oracle** (`test_cli.py`): `version`, `pid`, `status`
  (+ LSB exit 3/4), `start`/`stop`/`restart`, `all`, `group:*`,
  `signal <sig> <name>`, `tail`, `maintail`, `clear`, `avail`, `reread`,
  `update`.
- **Native** (`test_native_cli.py`): `rsupervisorctl status/start/stop/restart/
  tail/stdin/reload` over the UDS.
- **Event listener oracle** (`test_eventlistener.py`, gated #6): pool as a
  process group, `READY`/`RESULT` envelope fields, `PROCESS_STATE_*` /
  `PROCESS_LOG_*` / `TICK_5` / `REMOTE_COMMUNICATION` payloads, protocol
  violation → `UNKNOWN`, and buffer overflow (`buffer_size`).

## Notes

- Grouped programs' namespec is `group:name` (e.g. `services:ticker`), not the
  bare name; the python fixture relies on this.
- The client transport is stock `supervisor.xmlrpc.SupervisorTransport`
  (`unix://` socket + Basic auth, and `http://` for `[inet_http_server]`).
- `status` exit codes follow LSB: `0` all running, `3` some stopped, `4` unknown.
