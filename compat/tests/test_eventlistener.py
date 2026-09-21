"""Event listener protocol tests against stock Python Supervisor 4.2.5.

These are the executable baseline for ``compat/docs/EVENTLISTENER_COMPAT.md``:
they assert what a *stock* supervisord does with an ``[eventlistener:*]`` pool,
the ``READY``/``RESULT`` line protocol, the event envelope, and the built-in
event taxonomy (process state / process log / tick / remote communication).

Like the other oracle suites these are capability gated: rsupervisord does not
implement event listeners yet, so the whole module is reported as ``xfail`` on
that target until requirement EL-1 lands.  ``SUPERVISOR_STRICT=1`` turns the
gate into a hard failure.
"""

from __future__ import annotations

import json
import time
from pathlib import Path

import pytest

from conftest import STRICT

pytestmark = pytest.mark.oracle

EVENTLISTENER_UNSUPPORTED = (
    "target does not model event listeners; stock event-listener oracle is "
    "gated on it (compat/docs/EVENTLISTENER_COMPAT.md, requirement EL-1)"
)

GOOD_POOL = "listener"
BAD_POOL = "badlistener"
EVENT_LOG_NAME = "events.log"


# --------------------------------------------------------------------------- #
# Gating
# --------------------------------------------------------------------------- #
@pytest.fixture(autouse=True)
def _require_eventlistener(require_rpc, instance):
    """Oracle tests need a target that actually runs ``[eventlistener:*]`` pools."""
    try:
        infos = instance.supervisor.supervisor.getAllProcessInfo()
    except Exception as exc:  # pragma: no cover - require_rpc already gated
        if STRICT:
            pytest.fail(f"{EVENTLISTENER_UNSUPPORTED} ({exc})", pytrace=False)
        pytest.xfail(EVENTLISTENER_UNSUPPORTED)
    if not any(info["group"] == GOOD_POOL for info in infos):
        if STRICT:
            pytest.fail(EVENTLISTENER_UNSUPPORTED, pytrace=False)
        pytest.xfail(EVENTLISTENER_UNSUPPORTED)


# --------------------------------------------------------------------------- #
# Helpers: the listener fixture appends one JSON envelope per line
# --------------------------------------------------------------------------- #
def _event_log(instance) -> Path:
    path = instance.root / "logs" / EVENT_LOG_NAME
    path.parent.mkdir(parents=True, exist_ok=True)
    path.touch(exist_ok=True)
    return path


def _offset(path: Path) -> int:
    return path.stat().st_size


def _events_since(path: Path, offset: int) -> list[dict]:
    if not path.exists():
        return []
    with path.open("rb") as handle:
        handle.seek(offset)
        blob = handle.read().decode("utf-8", "replace")
    events = []
    for line in blob.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            events.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return events


def _wait_events(path: Path, offset: int, predicate, timeout: float = 15.0) -> list[dict]:
    deadline = time.time() + timeout
    while time.time() < deadline:
        matches = [event for event in _events_since(path, offset) if predicate(event)]
        if matches:
            return matches
        time.sleep(0.1)
    return [event for event in _events_since(path, offset) if predicate(event)]


def _wait_log(instance, needles, timeout: float = 15.0) -> str:
    log = instance.root / "logs" / "supervisord.log"
    deadline = time.time() + timeout
    text = ""
    while time.time() < deadline:
        text = log.read_text(errors="replace") if log.exists() else ""
        if all(needle in text for needle in needles):
            return text
        time.sleep(0.2)
    return text


@pytest.fixture
def event_cursor(instance):
    path = _event_log(instance)
    return path, _offset(path)


# --------------------------------------------------------------------------- #
# Pool lifecycle
# --------------------------------------------------------------------------- #
def test_eventlistener_pool_is_running(rpc, instance):
    assert instance.wait_state(GOOD_POOL, "RUNNING", timeout=15)
    info = rpc.supervisor.getProcessInfo(GOOD_POOL)
    assert info["name"] == GOOD_POOL
    assert info["group"] == GOOD_POOL
    assert info["statename"] == "RUNNING"


# --------------------------------------------------------------------------- #
# PROCESS_STATE events
# --------------------------------------------------------------------------- #
def test_process_state_events(rpc, instance, event_cursor):
    path, offset = event_cursor

    rpc.supervisor.startProcess("echo", False)
    assert instance.wait_state("echo", "EXITED", timeout=10)

    events = _wait_events(
        path,
        offset,
        lambda e: e.get("eventname", "").startswith("PROCESS_STATE")
        and "processname:echo" in e.get("payload", ""),
    )
    names = {event["eventname"] for event in events}
    assert "PROCESS_STATE_EXITED" in names
    assert {"PROCESS_STATE_STARTING", "PROCESS_STATE_RUNNING"} & names

    exited = next(e for e in events if e["eventname"] == "PROCESS_STATE_EXITED")
    payload = exited["payload"]
    assert "processname:echo" in payload
    assert "groupname:echo" in payload
    assert "from_state:RUNNING" in payload
    assert "expected:1" in payload
    assert "pid:" in payload


def test_process_state_groupname(rpc, instance, event_cursor):
    path, offset = event_cursor

    rpc.supervisor.stopProcess("services:catx")
    assert instance.wait_state("services:catx", "STOPPED", timeout=10)
    rpc.supervisor.startProcess("services:catx")
    assert instance.wait_state("services:catx", "RUNNING", timeout=10)

    events = _wait_events(
        path,
        offset,
        lambda e: e.get("eventname", "").startswith("PROCESS_STATE")
        and "processname:catx" in e.get("payload", ""),
    )
    assert events
    assert all("groupname:services" in event["payload"] for event in events)


# --------------------------------------------------------------------------- #
# PROCESS_LOG events
# --------------------------------------------------------------------------- #
def test_process_log_stdout_events(rpc, event_cursor):
    path, offset = event_cursor

    events = _wait_events(
        path,
        offset,
        lambda e: e.get("eventname") == "PROCESS_LOG_STDOUT"
        and "processname:ticker" in e.get("payload", ""),
        timeout=10,
    )
    assert events
    payload = events[0]["payload"]
    assert "groupname:services" in payload
    assert "pid:" in payload
    assert "channel:stdout" in payload
    assert payload.rstrip().endswith("tick") or "tick" in payload


# --------------------------------------------------------------------------- #
# TICK events
# --------------------------------------------------------------------------- #
def test_tick_event(event_cursor):
    path, offset = event_cursor

    events = _wait_events(
        path, offset, lambda e: e.get("eventname") == "TICK_5", timeout=12
    )
    assert events
    payload = events[0]["payload"]
    assert payload.startswith("when:")
    assert int(payload.split("when:", 1)[1]) > 0


# --------------------------------------------------------------------------- #
# REMOTE_COMMUNICATION events (XML-RPC sendRemoteCommEvent)
# --------------------------------------------------------------------------- #
def test_remote_communication_event(rpc, event_cursor):
    path, offset = event_cursor

    assert rpc.supervisor.sendRemoteCommEvent("deploy", "v1.2.3") is True

    events = _wait_events(
        path, offset, lambda e: e.get("eventname") == "REMOTE_COMMUNICATION"
    )
    assert events
    payload = events[0]["payload"]
    assert "deploy" in payload
    assert "v1.2.3" in payload


# --------------------------------------------------------------------------- #
# Envelope shape
# --------------------------------------------------------------------------- #
def test_event_envelope_fields(rpc, instance, event_cursor):
    path, offset = event_cursor

    events = _wait_events(
        path, offset, lambda e: e.get("eventname") == "TICK_5", timeout=12
    )
    assert events
    event = events[0]

    assert event["ver"] == "3.0"
    assert event["server"] == rpc.supervisor.getIdentification()
    assert event["pool"] == GOOD_POOL
    assert event["eventname"] == "TICK_5"
    assert int(event["serial"]) >= 0
    assert int(event["poolserial"]) >= 0
    assert int(event["len"]) == len(event["payload"])
    assert event["header"].startswith(
        f"ver:3.0 server:{event['server']} serial:"
    )
    assert " pool:listener poolserial:" in event["header"]
    assert " eventname:TICK_5 len:" in event["header"]


# --------------------------------------------------------------------------- #
# Protocol violations: UNKNOWN state and buffer overflow
# --------------------------------------------------------------------------- #
def test_protocol_violation_marks_listener_unknown(instance):
    text = _wait_log(instance, [BAD_POOL, "UNKNOWN"], timeout=15)
    assert f"{BAD_POOL}: has entered the UNKNOWN state" in text


def test_event_buffer_overflow_discards_oldest(instance):
    text = _wait_log(instance, ["event buffer overflowed"], timeout=30)
    assert "event buffer overflowed" in text
