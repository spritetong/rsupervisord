"""Functional XML-RPC tests against stock Python Supervisor 4.2.5.

These mirror the method surface documented in ``compat/docs/XMLRPC_COMPAT.md`` and
act as the golden oracle that rsupervisord must later match.
"""

from __future__ import annotations

import time
from xmlrpc.client import Fault

import pytest

from conftest import EXPECTED_SUPERVISOR_VERSION, TARGET

pytestmark = pytest.mark.oracle


@pytest.fixture(autouse=True)
def _require_stock_xmlrpc(require_rpc):
    """Oracle tests need a target that serves XML-RPC (compat/docs/XMLRPC_COMPAT.md)."""


EXPECTED_PROGRAMS = {
    "echo",
    "ticker",
    "catx",
    "worker_00",
    "worker_01",
    "flaky",
    "nosuch",
    "extra",
}

if TARGET == "python":
    # Stock supervisor runs the event listener; rsupervisord does not model
    # event listeners (compat/docs/SUPERVISORD_COMPAT.md #6).
    EXPECTED_PROGRAMS.add("listener")

REQUIRED_PROCESS_INFO_KEYS = {
    "name",
    "group",
    "start",
    "stop",
    "now",
    "state",
    "statename",
    "spawnerr",
    "exitstatus",
    "logfile",
    "stdout_logfile",
    "stderr_logfile",
    "pid",
    "description",
}


# --------------------------------------------------------------------------- #
# API / meta information
# --------------------------------------------------------------------------- #
def test_get_api_version(rpc):
    assert rpc.supervisor.getAPIVersion() == "3.0"


def test_get_version_alias(rpc):
    assert rpc.supervisor.getVersion() == "3.0"


def test_get_supervisor_version(rpc):
    assert rpc.supervisor.getSupervisorVersion() == EXPECTED_SUPERVISOR_VERSION


def test_get_identification(rpc):
    assert rpc.supervisor.getIdentification() == "rsupervisord-compat"


def test_get_state(rpc):
    state = rpc.supervisor.getState()
    assert isinstance(state["statecode"], int)
    assert state["statename"] == "RUNNING"


def test_get_pid(rpc):
    pid = rpc.supervisor.getPID()
    assert isinstance(pid, int)
    assert pid > 0


def test_inet_http_proxy(rpc_inet):
    assert rpc_inet.supervisor.getIdentification() == "rsupervisord-compat"
    assert isinstance(rpc_inet.supervisor.getPID(), int)


# --------------------------------------------------------------------------- #
# system.* introspection
# --------------------------------------------------------------------------- #
def test_system_list_methods(rpc):
    methods = rpc.system.listMethods()
    assert "supervisor.getAPIVersion" in methods
    assert "supervisor.getAllProcessInfo" in methods
    assert "system.multicall" in methods


def test_system_method_help(rpc):
    help_text = rpc.system.methodHelp("supervisor.getAPIVersion")
    assert "version" in help_text.lower()


def test_system_method_signature(rpc):
    sig = rpc.system.methodSignature("supervisor.getAPIVersion")
    assert isinstance(sig, list)
    assert sig and sig[0]


def test_system_method_help_unknown(rpc):
    with pytest.raises(Fault) as exc:
        rpc.system.methodHelp("supervisor.doesNotExist")
    assert exc.value.faultCode == 4


def test_system_multicall(rpc):
    results = rpc.system.multicall(
        [
            {"methodName": "supervisor.getPID", "params": []},
            {"methodName": "supervisor.getState", "params": []},
            {"methodName": "supervisor.doesNotExist", "params": []},
        ]
    )
    assert len(results) == 3
    assert not isinstance(results[0], dict)
    assert results[2]["faultCode"] == 1


def test_system_multicall_recursion_denied(rpc):
    results = rpc.system.multicall(
        [{"methodName": "system.multicall", "params": [[]]}]
    )
    assert results[0]["faultCode"] == 2


def test_unknown_method(rpc):
    with pytest.raises(Fault) as exc:
        rpc.supervisor.doesNotExist()
    assert exc.value.faultCode == 1


# --------------------------------------------------------------------------- #
# Process information
# --------------------------------------------------------------------------- #
def test_get_all_process_info(rpc):
    infos = rpc.supervisor.getAllProcessInfo()
    names = {i["name"] for i in infos}
    assert EXPECTED_PROGRAMS <= names
    for info in infos:
        assert REQUIRED_PROCESS_INFO_KEYS <= set(info)


def test_get_process_info_fields(rpc, instance):
    assert instance.wait_state("services:ticker", "RUNNING")
    info = rpc.supervisor.getProcessInfo("services:ticker")
    assert info["name"] == "ticker"
    assert info["group"] == "services"
    assert info["statename"] == "RUNNING"
    assert isinstance(info["pid"], int) and info["pid"] > 0
    assert info["description"].startswith("pid ")


def test_get_process_info_namespec(rpc):
    assert rpc.supervisor.getProcessInfo("services:ticker")["name"] == "ticker"


def test_get_process_info_bad_name(rpc):
    with pytest.raises(Fault) as exc:
        rpc.supervisor.getProcessInfo("no-such-program")
    assert exc.value.faultCode == 10


def test_worker_numprocs_instances(rpc):
    names = {i["name"] for i in rpc.supervisor.getAllProcessInfo()}
    assert {"worker_00", "worker_01"} <= names


# --------------------------------------------------------------------------- #
# Lifecycle control
# --------------------------------------------------------------------------- #
def test_start_oneshot_and_exit(rpc, instance):
    assert rpc.supervisor.startProcess("echo") is True
    assert instance.wait_state("echo", "EXITED") or instance.wait_state("echo", "STOPPED")


def test_start_already_running_fault(rpc):
    with pytest.raises(Fault) as exc:
        rpc.supervisor.startProcess("services:ticker")
    assert exc.value.faultCode == 60


def test_stop_and_restart_process(rpc, instance):
    assert rpc.supervisor.stopProcess("services:ticker") is True
    assert instance.wait_state("services:ticker", "STOPPED")
    assert rpc.supervisor.startProcess("services:ticker") is True
    assert instance.wait_state("services:ticker", "RUNNING")


def test_stop_not_running_fault(rpc):
    with pytest.raises(Fault) as exc:
        rpc.supervisor.stopProcess("echo")
    assert exc.value.faultCode == 70


def test_process_group_lifecycle(rpc, instance):
    stopped = rpc.supervisor.stopProcessGroup("services")
    assert isinstance(stopped, list)
    assert instance.wait_state("services:ticker", "STOPPED")
    assert instance.wait_state("services:catx", "STOPPED")

    started = rpc.supervisor.startProcessGroup("services")
    assert isinstance(started, list)
    assert instance.wait_state("services:ticker", "RUNNING")
    assert instance.wait_state("services:catx", "RUNNING")


def test_start_stop_all(rpc, instance):
    assert isinstance(rpc.supervisor.stopAllProcesses(), list)
    time.sleep(0.5)
    assert isinstance(rpc.supervisor.startAllProcesses(), list)
    assert instance.wait_state("services:ticker", "RUNNING", timeout=20)


def test_add_remove_process_group(rpc):
    # A group never seen in the source config => BAD_NAME for both add and remove.
    with pytest.raises(Fault) as exc:
        rpc.supervisor.addProcessGroup("doesnotexist")
    assert exc.value.faultCode == 10  # BAD_NAME

    with pytest.raises(Fault) as exc:
        rpc.supervisor.removeProcessGroup("doesnotexist")
    assert exc.value.faultCode == 10  # BAD_NAME

    # An already-active group cannot be added again => ALREADY_ADDED.
    with pytest.raises(Fault) as exc:
        rpc.supervisor.addProcessGroup("echo")
    assert exc.value.faultCode == 90  # ALREADY_ADDED

    # Removing a stopped group deactivates it: process lookups then fail.
    assert rpc.supervisor.removeProcessGroup("echo") is True
    with pytest.raises(Fault) as exc:
        assert rpc.supervisor.getProcessInfo("echo")
    assert exc.value.faultCode == 10  # BAD_NAME

    # Re-adding re-activates the group from the source config.
    assert rpc.supervisor.addProcessGroup("echo") is True
    assert rpc.supervisor.getProcessInfo("echo")["name"] == "echo"


def test_signal_process(rpc, instance):
    assert instance.wait_state("services:ticker", "RUNNING")
    assert rpc.supervisor.signalProcess("services:ticker", "HUP") is True


def test_signal_bad_signal(rpc):
    with pytest.raises(Fault) as exc:
        rpc.supervisor.signalProcess("services:ticker", "NOTASIGNAL")
    assert exc.value.faultCode == 11


def test_signal_not_running_fault(rpc):
    with pytest.raises(Fault) as exc:
        rpc.supervisor.signalProcess("echo", "HUP")
    assert exc.value.faultCode == 70


def test_signal_all_processes(rpc):
    # signal 0 is a harmless existence probe; HUP would kill catx/workers.
    assert isinstance(rpc.supervisor.signalAllProcesses("0"), list)


def test_unspawnable_program_fault(rpc):
    with pytest.raises(Fault) as exc:
        rpc.supervisor.startProcess("nosuch")
    assert exc.value.faultCode in (20, 21, 50)  # NO_FILE / NOT_EXECUTABLE / SPAWN_ERROR


# --------------------------------------------------------------------------- #
# stdin + logs
# --------------------------------------------------------------------------- #
def test_send_process_stdin(rpc, instance):
    assert instance.wait_state("services:catx", "RUNNING")
    assert rpc.supervisor.sendProcessStdin("services:catx", "hello-stdin\n") is True
    deadline = time.time() + 10
    text = ""
    while time.time() < deadline:
        text = rpc.supervisor.readProcessStdoutLog("services:catx", 0, 4096)
        if "hello-stdin" in text:
            break
        time.sleep(0.2)
    assert "hello-stdin" in text


def test_send_stdin_not_running_fault(rpc):
    with pytest.raises(Fault) as exc:
        rpc.supervisor.sendProcessStdin("echo", "x\n")
    assert exc.value.faultCode == 70


def test_tail_process_stdout_log(rpc):
    result = rpc.supervisor.tailProcessStdoutLog("services:ticker", 0, 8192)
    assert isinstance(result, list) and len(result) == 3
    text, offset, overflow = result
    assert "tick" in text
    assert isinstance(offset, int)
    assert isinstance(overflow, bool)


def test_tail_process_stderr_log(rpc):
    text, _offset, _overflow = rpc.supervisor.tailProcessStderrLog(
        "services:ticker", 0, 8192
    )
    assert "err" in text


def test_read_process_stdout_log_alias(rpc):
    a = rpc.supervisor.readProcessStdoutLog("services:ticker", 0, 4096)
    b = rpc.supervisor.readProcessLog("services:ticker", 0, 4096)
    assert a == b


def test_tail_process_log_alias(rpc):
    a = rpc.supervisor.tailProcessStdoutLog("services:ticker", 0, 4096)
    b = rpc.supervisor.tailProcessLog("services:ticker", 0, 4096)
    assert a == b


def test_clear_process_logs(rpc):
    assert rpc.supervisor.clearProcessLogs("services:ticker") is True


def test_read_main_log(rpc):
    text = rpc.supervisor.readLog(0, 4096)
    assert isinstance(text, str)


def test_read_main_log_alias(rpc):
    assert rpc.supervisor.readLog(0, 512) == rpc.supervisor.readMainLog(0, 512)


def test_clear_main_log(rpc):
    assert rpc.supervisor.clearLog() is True


# --------------------------------------------------------------------------- #
# Config surface
# --------------------------------------------------------------------------- #
def test_get_all_config_info(rpc):
    infos = rpc.supervisor.getAllConfigInfo()
    by_name = {i["name"]: i for i in infos}
    assert "ticker" in by_name
    entry = by_name["ticker"]
    for key in ("name", "group", "command", "autostart", "startsecs", "startretries"):
        assert key in entry


def test_reload_config(rpc):
    result = rpc.supervisor.reloadConfig()
    assert isinstance(result, list) and len(result) == 1
    added, changed, removed = result[0]
    assert isinstance(added, list)
    assert isinstance(changed, list)
    assert isinstance(removed, list)
