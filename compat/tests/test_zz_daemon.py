"""Daemon-level CLI test; named ``zz`` so it runs last and may kill the server."""

from __future__ import annotations

import time

import pytest

pytestmark = pytest.mark.oracle


@pytest.fixture(autouse=True)
def _require_stock_xmlrpc(require_rpc):
    """The ``shutdown`` RPC needs a target that serves XML-RPC (#5)."""


def test_cli_shutdown(instance):
    res = instance.ctl("shutdown")
    assert res.returncode == 0

    deadline = time.time() + 15
    while time.time() < deadline and instance.proc.poll() is None:
        time.sleep(0.1)
    assert instance.proc.poll() is not None, "supervisord did not exit after shutdown"
