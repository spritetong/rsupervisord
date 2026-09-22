"""rsupervisorctl-native smoke tests against the compiled rsupervisord binary.

These exercise the **contract surface** only — exit codes and stdout shapes
that are machine-consumable (see ``compat/docs/CLI_COMPAT.md`` §2.1/§10).
UX-prose output (``version``/``help`` text, error wording, table rendering)
is deliberately NOT asserted, so that modernization stays free.
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


def test_native_reload_config(instance):
    # Contract: reload-config (hot reload) succeeds; the daemon keeps running
    # and programs stay up.  ``reload`` itself is reserved for the Python
    # "restart daemon" semantics (CLI_COMPAT.md §6.1.5/§6.4).
    res = instance.rctl("reload-config")
    assert res.returncode == 0
    assert instance.wait_state("ticker", "RUNNING")
