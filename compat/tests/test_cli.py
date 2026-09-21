"""Functional supervisorctl (CLI) tests against stock Python Supervisor 4.2.5.

Exercises the client-side command grammar, output shapes and exit codes that
``compat/docs/CLI_COMPAT.md`` describes.  Program ``ticker`` is in group ``services``
so its namespec is ``services:ticker``.
"""

from __future__ import annotations

import time

import pytest

pytestmark = pytest.mark.oracle


@pytest.fixture(autouse=True)
def _require_stock_xmlrpc(require_rpc):
    """Stock supervisorctl needs a target that serves XML-RPC (#5)."""


def _out(result) -> str:
    return (result.stdout or "") + (result.stderr or "")


def test_cli_version(instance):
    res = instance.ctl("version")
    assert res.returncode == 0
    assert "4.2.5" in _out(res)


def test_cli_pid(instance):
    res = instance.ctl("pid")
    assert res.returncode == 0
    assert int(res.stdout.strip()) > 0


def test_cli_status_all(instance):
    res = instance.ctl("status")
    # LSB status exit 3 == NOT_RUNNING: some configured programs are stopped.
    assert res.returncode == 3
    out = _out(res)
    assert "echo" in out
    assert "services:ticker" in out
    assert "RUNNING" in out


def test_cli_status_named(instance):
    res = instance.ctl("status", "services:ticker")
    assert res.returncode == 0
    assert "RUNNING" in _out(res)


def test_cli_status_unknown_exit_code(instance):
    res = instance.ctl("status", "no-such-program")
    # LSB status exit 4 == UNKNOWN.
    assert res.returncode == 4
    assert "no such process" in _out(res).lower()


def test_cli_start_stop_restart(instance):
    assert instance.ctl("stop", "services:ticker").returncode == 0
    assert instance.wait_state("services:ticker", "STOPPED")

    res = instance.ctl("start", "services:ticker")
    assert res.returncode == 0
    assert instance.wait_state("services:ticker", "RUNNING")

    res = instance.ctl("restart", "services:ticker")
    assert res.returncode == 0
    assert instance.wait_state("services:ticker", "RUNNING")


def test_cli_start_all_stop_all(instance):
    assert instance.ctl("stop", "all").returncode == 0
    time.sleep(0.5)
    # ``start all`` will report failures for the unspawnable / flaky programs,
    # so only assert that the healthy group came back up.
    instance.ctl("start", "all")
    assert instance.wait_state("services:ticker", "RUNNING", timeout=20)


def test_cli_group_wildcard(instance):
    assert instance.ctl("stop", "services:*").returncode == 0
    assert instance.wait_state("services:ticker", "STOPPED")
    assert instance.ctl("start", "services:*").returncode == 0
    assert instance.wait_state("services:ticker", "RUNNING")


def test_cli_signal(instance):
    res = instance.ctl("signal", "HUP", "services:ticker")
    assert res.returncode == 0
    assert "signalled" in _out(res)


def test_cli_signal_bad_name(instance):
    res = instance.ctl("signal", "HUP", "no-such-program")
    assert res.returncode != 0


def test_cli_tail(instance):
    res = instance.ctl("tail", "services:ticker")
    assert "tick" in _out(res)


def test_cli_maintail(instance):
    res = instance.ctl("maintail")
    assert res.returncode == 0


def test_cli_clear(instance):
    res = instance.ctl("clear", "services:ticker")
    assert res.returncode == 0
    assert "cleared" in _out(res)


def test_cli_avail(instance):
    res = instance.ctl("avail")
    assert res.returncode == 0
    out = _out(res)
    assert "ticker" in out
    assert "echo" in out


def test_cli_reread_unchanged(instance):
    res = instance.ctl("reread")
    assert res.returncode == 0
    assert "No config updates" in _out(res)


def test_cli_update_unchanged(instance):
    res = instance.ctl("update")
    # stock supervisorctl prints nothing when there are no config changes
    assert res.returncode == 0
