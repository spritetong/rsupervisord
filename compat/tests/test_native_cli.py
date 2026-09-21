"""rsupervisorctl-native smoke tests against the compiled rsupervisord binary.

These exercise the control surface that exists today (see ``compat/docs/CLI_COMPAT.md``
for how it differs from stock supervisorctl).  They run only when
``SUPERVISOR_TARGET=rsupervisord`` (the default).
"""

from __future__ import annotations

import time

import pytest

pytestmark = pytest.mark.native


@pytest.fixture(autouse=True)
def _require_rsupervisord(instance):
    if instance.target != "rsupervisord":
        pytest.skip("native tests require SUPERVISOR_TARGET=rsupervisord")


def _out(result) -> str:
    return (result.stdout or "") + (result.stderr or "")


def test_native_status_lists_programs(instance):
    res = instance.rctl("status")
    assert res.returncode == 0
    out = _out(res)
    assert "ticker" in out
    assert "catx" in out


def test_native_tail(instance):
    res = instance.rctl("tail", "ticker")
    assert "tick" in _out(res)


def test_native_start_stop_restart(instance):
    assert instance.rctl("stop", "ticker").returncode == 0
    assert instance.wait_state("ticker", "STOPPED")

    assert instance.rctl("start", "ticker").returncode == 0
    assert instance.wait_state("ticker", "RUNNING")

    assert instance.rctl("restart", "ticker").returncode == 0
    assert instance.wait_state("ticker", "RUNNING")


def test_native_stdin(instance):
    assert instance.wait_state("catx", "RUNNING", timeout=20)
    res = instance.rctl("stdin", "catx", "hello-from-stdin\n")
    assert res.returncode == 0

    deadline = time.time() + 10
    while time.time() < deadline:
        if "hello-from-stdin" in _out(instance.rctl("tail", "catx")):
            return
        time.sleep(0.2)
    pytest.fail("stdin payload never showed up in catx output")


def test_native_reload(instance):
    res = instance.rctl("reload")
    assert res.returncode == 0
    assert "reload" in _out(res).lower()
