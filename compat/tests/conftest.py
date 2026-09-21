"""Shared pytest fixtures for the rsupervisord compatibility harness.

Two run targets are supported, selected by ``SUPERVISOR_TARGET``:

``rsupervisord`` (default)
    Launches the **compiled Rust binary** (``target/<profile>/rsupervisord``)
    with the YAML fixture ``compat/conf/rsupervisord.yaml``.  The
    ``rsupervisorctl`` binary is used for the native tests; the stock
    supervisorctl/XML-RPC oracle tests are *capability gated* and reported as
    xfail until the corresponding features exist (see ``compat/docs/XMLRPC_COMPAT.md``).

``python``
    Launches stock Python Supervisor 4.2.5 from ``compat/.venv`` with the INI
    fixture ``compat/conf/supervisord.conf``.  This is the golden oracle: all
    tests are expected to pass.

Environment knobs::

    SUPERVISOR_TARGET      rsupervisord | python        (default: rsupervisord)
    SUPERVISOR_RSD_FORMAT  yaml | ini                   (default: yaml)
    SUPERVISOR_STRICT      turn unsupported xfails into hard failures
    RSUPERVISORD_PROFILE   cargo profile dir              (default: debug)
    RSUPERVISORD_BIN       override daemon binary path
    RSUPERVISORCTL_BIN     override ctl binary path

Must run on a POSIX host; on this project that means WSL (see ``compat/run.sh``).
"""

from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path
from xmlrpc.client import ServerProxy

import pytest
from supervisor.xmlrpc import SupervisorTransport

COMPAT_DIR = Path(__file__).resolve().parents[1]
REPO_ROOT = COMPAT_DIR.parent
CONF_SRC = COMPAT_DIR / "conf"
VENV_BIN = COMPAT_DIR / ".venv" / "bin"
SUPERVISORD = VENV_BIN / "supervisord"
SUPERVISORCTL = VENV_BIN / "supervisorctl"

TARGET = os.environ.get("SUPERVISOR_TARGET", "rsupervisord").strip().lower()
STRICT = os.environ.get("SUPERVISOR_STRICT", "").strip().lower() not in (
    "",
    "0",
    "false",
    "no",
)
RSD_FORMAT = os.environ.get("SUPERVISOR_RSD_FORMAT", "yaml").strip().lower() or "yaml"
PROFILE = os.environ.get("RSUPERVISORD_PROFILE", "debug").strip() or "debug"
RSD_BIN = Path(
    os.environ.get("RSUPERVISORD_BIN") or REPO_ROOT / "target" / PROFILE / "rsupervisord"
)
RCTL_BIN = Path(
    os.environ.get("RSUPERVISORCTL_BIN")
    or REPO_ROOT / "target" / PROFILE / "rsupervisorctl"
)

# [unix_http_server]/[inet_http_server] credentials for the python target.
CTL_USER, CTL_PASS = "ctluser", "ctlpass"
RPC_USER, RPC_PASS = "rpcuser", "rpcpass"
INET_PORT = 9011

START_TIMEOUT = 25.0

RPC_UNSUPPORTED = (
    "rsupervisord does not serve XML-RPC yet; stock supervisorctl/XML-RPC "
    "oracle tests are gated on it (compat/docs/XMLRPC_COMPAT.md, feature #5)"
)


def _build_proxy(serverurl: str, user: str, password: str) -> ServerProxy:
    # ServerProxy refuses non-HTTP schemes, so stock supervisorctl fakes an
    # http:// URL and lets SupervisorTransport carry the real serverurl.
    transport = SupervisorTransport(user, password, serverurl)
    return ServerProxy("http://127.0.0.1", transport=transport)


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Instance:
    """A running target daemon plus helpers to talk to it."""

    def __init__(
        self,
        target: str,
        root: Path,
        proc: subprocess.Popen,
        *,
        uds: Path,
        http_port: int,
        ctl_conf: Path,
        rpc_serverurl: str | None = None,
        rpc_user: str = "",
        rpc_pass: str = "",
    ) -> None:
        self.target = target
        self.root = root
        self.proc = proc
        self.uds = uds
        self.http_port = http_port
        self.ctl_conf = ctl_conf
        self.rpc_serverurl = rpc_serverurl
        self.rpc_user = rpc_user
        self.rpc_pass = rpc_pass
        self._rpc_available: bool | None = None

    # -- XML-RPC -----------------------------------------------------------
    @property
    def supervisor(self) -> ServerProxy:
        if self.rpc_serverurl:
            return _build_proxy(self.rpc_serverurl, self.rpc_user, self.rpc_pass)
        if self.target == "python":
            return _build_proxy(f"unix://{self.uds}", CTL_USER, CTL_PASS)
        return _build_proxy(f"http://127.0.0.1:{self.http_port}", "", "")

    @property
    def inet(self) -> ServerProxy:
        if self.target == "python":
            return _build_proxy(f"http://127.0.0.1:{INET_PORT}", RPC_USER, RPC_PASS)
        return self.supervisor

    @property
    def rpc_available(self) -> bool:
        """True if the target actually speaks XML-RPC at /RPC2."""
        if self._rpc_available is None:
            if self.target == "python":
                self._rpc_available = True
            else:
                try:
                    self.supervisor.supervisor.getAPIVersion()
                    self._rpc_available = True
                except Exception:
                    self._rpc_available = False
        return self._rpc_available

    # -- CLI ---------------------------------------------------------------
    def ctl(self, *args: str, timeout: float = 60.0) -> subprocess.CompletedProcess:
        """Run stock supervisorctl (client side of the compatibility goal)."""
        cmd = [str(SUPERVISORCTL), "-c", str(self.ctl_conf), *args]
        return subprocess.run(
            cmd, cwd=self.root, capture_output=True, text=True, timeout=timeout
        )

    def rctl(self, *args: str, timeout: float = 60.0) -> subprocess.CompletedProcess:
        """Run the native rsupervisorctl (rsupervisord target only)."""
        cmd = [str(RCTL_BIN), "-s", str(self.uds), *args]
        env = {**os.environ, "NO_COLOR": "1"}
        return subprocess.run(
            cmd, cwd=self.root, capture_output=True, text=True, timeout=timeout, env=env
        )

    # -- helpers -----------------------------------------------------------
    def log_dump(self) -> str:
        chunks = []
        for name in ("supervisord.log", "supervisord.log.1", "harness.out"):
            p = self.root / "logs" / name
            if p.exists():
                chunks.append(f"----- {p.name} -----\n{p.read_text(errors='replace')}")
        return "\n".join(chunks)

    def _rest_status(self) -> dict[str, dict]:
        url = f"http://127.0.0.1:{self.http_port}/api/v1/status"
        with urllib.request.urlopen(url, timeout=5) as resp:
            payload = json.loads(resp.read())
        data = payload.get("data") or []
        return {p.get("name", ""): p for p in data}

    def _state_of(self, name: str) -> str | None:
        if self.rpc_available:
            info = self.supervisor.supervisor.getProcessInfo(name)
            return info["statename"]
        programs = self._rest_status()
        if name in programs:
            return programs[name]["state"]
        for p in programs.values():
            if f"{p.get('group', '')}:{p.get('name', '')}" == name:
                return p["state"]
        return None

    def wait_state(self, name: str, state: str, timeout: float = 15.0) -> bool:
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                if self._state_of(name) == state:
                    return True
            except Exception:
                pass
            time.sleep(0.1)
        return False


# --------------------------------------------------------------------------- #
# Launch / teardown per target
# --------------------------------------------------------------------------- #
def _launch_python(root: Path) -> Instance:
    (root / "run").mkdir(parents=True, exist_ok=True)
    (root / "logs").mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    env["COMPAT_TAG"] = "alpha"
    env["COMPAT_MODE"] = "oracle"

    out = open(root / "logs" / "harness.out", "w")
    err = open(root / "logs" / "harness.err", "w")
    proc = subprocess.Popen(
        [str(SUPERVISORD), "-n", "-c", str(root / "supervisord.conf")],
        cwd=root,
        env=env,
        stdout=out,
        stderr=err,
    )
    inst = Instance(
        "python",
        root,
        proc,
        uds=root / "run" / "supervisor.sock",
        http_port=INET_PORT,
        ctl_conf=root / "supervisord.conf",
    )

    deadline = time.time() + START_TIMEOUT
    last_err: Exception | None = None
    while time.time() < deadline:
        if proc.poll() is not None:
            break
        if inst.uds.exists():
            try:
                inst.supervisor.supervisor.getState()
                return inst
            except Exception as exc:  # not ready yet
                last_err = exc
        time.sleep(0.1)

    _teardown(inst)
    raise RuntimeError(
        f"stock supervisord failed to start (exit={proc.poll()}, last_err={last_err})\n"
        + inst.log_dump()
    )


_SCRIPTS = {
    "ticker.sh": "#!/bin/sh\ntrap '' HUP\nwhile true; do echo tick; echo err 1>&2; sleep 0.2; done\n",
    "worker.sh": "#!/bin/sh\nwhile true; do echo worker; sleep 0.5; done\n",
    "flaky.sh": "#!/bin/sh\necho boom; exit 3\n",
}


def _launch_rsupervisord() -> Instance:
    if not RSD_BIN.exists():
        pytest.fail(
            f"{RSD_BIN} missing; run `cargo build` (or compat/run.sh) first"
        )

    # Short base dir: AF_UNIX sockaddr_un limits the socket path to ~108 bytes,
    # and pytest's tmp_path_factory paths are too long.
    root = Path(tempfile.mkdtemp(prefix="rsd-", dir="/tmp"))
    logs = root / "logs"
    logs.mkdir()
    for name, body in _SCRIPTS.items():
        script = root / name
        script.write_text(body)
        script.chmod(0o755)

    uds = root / "rsupervisord.sock"
    port = _free_port()
    cfg = (root / "rsupervisord.yaml")
    template = (CONF_SRC / "rsupervisord.yaml").read_text()
    cfg.write_text(
        template.replace("__CFGDIR__", str(root))
        .replace("__LOGDIR__", str(logs))
        .replace("__UDS__", str(uds))
        .replace("__HTTP__", str(port))
    )

    # Stock supervisorctl client config for the gated oracle tests.
    ctl_conf = root / "supervisorctl.conf"
    ctl_conf.write_text(f"[supervisorctl]\nserverurl=unix://{uds}\n")

    log = open(root / "logs" / "harness.out", "w")
    proc = subprocess.Popen(
        [str(RSD_BIN), "-c", str(cfg), "-n", "-l", "info"],
        cwd=root,
        stdout=log,
        stderr=subprocess.STDOUT,
    )
    inst = Instance(
        "rsupervisord", root, proc, uds=uds, http_port=port, ctl_conf=ctl_conf
    )

    deadline = time.time() + START_TIMEOUT
    last_err: Exception | None = None
    while time.time() < deadline:
        if proc.poll() is not None:
            break
        if uds.exists():
            try:
                inst._rest_status()
                return inst
            except Exception as exc:  # not ready yet
                last_err = exc
        time.sleep(0.1)

    _teardown(inst)
    raise RuntimeError(
        f"rsupervisord failed to start (exit={proc.poll()}, last_err={last_err})\n"
        + inst.log_dump()
    )


def _launch_rsupervisord_ini(root: Path) -> Instance:
    """Launch the Rust binary against the stock INI fixture (translation test)."""
    if not RSD_BIN.exists():
        pytest.fail(
            f"{RSD_BIN} missing; run `cargo build` (or compat/run.sh) first"
        )

    (root / "run").mkdir(parents=True, exist_ok=True)
    (root / "logs").mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    env["COMPAT_TAG"] = "alpha"
    env["COMPAT_MODE"] = "oracle"

    log = open(root / "logs" / "harness.out", "w")
    proc = subprocess.Popen(
        [str(RSD_BIN), "-n", "-c", str(root / "supervisord.conf")],
        cwd=root,
        env=env,
        stdout=log,
        stderr=subprocess.STDOUT,
    )

    inst = Instance(
        "rsupervisord",
        root,
        proc,
        uds=root / "run" / "supervisor.sock",
        http_port=INET_PORT,
        ctl_conf=root / "supervisord.conf",
        rpc_serverurl=f"unix://{root / 'run' / 'supervisor.sock'}",
        rpc_user=CTL_USER,
        rpc_pass=CTL_PASS,
    )

    deadline = time.time() + START_TIMEOUT
    last_err: Exception | None = None
    while time.time() < deadline:
        if proc.poll() is not None:
            break
        try:
            inst.supervisor.supervisor.getState()
            return inst
        except Exception as exc:  # not ready yet
            last_err = exc
        time.sleep(0.1)

    _teardown(inst)
    raise RuntimeError(
        f"rsupervisord (INI) failed to start (exit={proc.poll()}, last_err={last_err})\n"
        + inst.log_dump()
    )


def _teardown(inst: Instance) -> None:
    if inst.proc.poll() is None:
        if inst.target == "python":
            try:
                inst.supervisor.supervisor.shutdown()
            except Exception:
                pass
        inst.proc.terminate()
        try:
            inst.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            inst.proc.kill()
            inst.proc.wait(timeout=5)
    if inst.target == "rsupervisord":
        shutil.rmtree(inst.root, ignore_errors=True)


# --------------------------------------------------------------------------- #
# Fixtures
# --------------------------------------------------------------------------- #
@pytest.fixture(scope="session")
def instance() -> Instance:
    if sys.platform == "win32":  # pragma: no cover
        pytest.skip("the harness requires a POSIX host; run inside WSL")
    if not SUPERVISORCTL.exists():
        pytest.fail(f"{SUPERVISORCTL} missing; run compat/bootstrap.sh first")

    if TARGET == "python":
        root = Path(tempfile.mkdtemp(prefix="sup-", dir="/tmp"))
        shutil.copytree(CONF_SRC, root, dirs_exist_ok=True)
        inst = _launch_python(root)
    elif TARGET == "rsupervisord":
        if RSD_FORMAT == "ini":
            root = Path(tempfile.mkdtemp(prefix="rsdini-", dir="/tmp"))
            shutil.copytree(CONF_SRC, root, dirs_exist_ok=True)
            inst = _launch_rsupervisord_ini(root)
        else:
            inst = _launch_rsupervisord()
    else:
        pytest.fail(f"unknown SUPERVISOR_TARGET={TARGET!r} (use python|rsupervisord)")

    try:
        yield inst
    finally:
        _teardown(inst)


@pytest.fixture(scope="session")
def rpc(instance: Instance) -> ServerProxy:
    return instance.supervisor


@pytest.fixture(scope="session")
def rpc_inet(instance: Instance) -> ServerProxy:
    return instance.inet


@pytest.fixture
def require_rpc(instance: Instance) -> None:
    """Gate oracle tests on the target actually serving XML-RPC (#5)."""
    if not instance.rpc_available:
        if STRICT:
            pytest.fail(RPC_UNSUPPORTED, pytrace=False)
        pytest.xfail(RPC_UNSUPPORTED)


def pytest_configure(config: pytest.Config) -> None:
    config.addinivalue_line(
        "markers", "oracle: stock supervisorctl / XML-RPC oracle tests"
    )
    config.addinivalue_line(
        "markers", "native: rsupervisorctl-native tests (rsupervisord target only)"
    )
