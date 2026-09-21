// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::compat::xmlrpc::fault::Fault;
use crate::compat::xmlrpc::wire::Value;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

/// Static registry of available system methods and docstrings.
pub static ALL_METHODS: &[&str] = &[
    "supervisor.addProcessGroup",
    "supervisor.clearAllProcessLogs",
    "supervisor.clearLog",
    "supervisor.clearProcessLog",
    "supervisor.clearProcessLogs",
    "supervisor.getAPIVersion",
    "supervisor.getAllConfigInfo",
    "supervisor.getAllProcessInfo",
    "supervisor.getIdentification",
    "supervisor.getPID",
    "supervisor.getProcessInfo",
    "supervisor.getState",
    "supervisor.getSupervisorVersion",
    "supervisor.getVersion",
    "supervisor.readLog",
    "supervisor.readMainLog",
    "supervisor.readProcessLog",
    "supervisor.readProcessStderrLog",
    "supervisor.readProcessStdoutLog",
    "supervisor.reloadConfig",
    "supervisor.removeProcessGroup",
    "supervisor.restart",
    "supervisor.sendProcessStdin",
    "supervisor.sendRemoteCommEvent",
    "supervisor.shutdown",
    "supervisor.signalAllProcesses",
    "supervisor.signalProcess",
    "supervisor.signalProcessGroup",
    "supervisor.startAllProcesses",
    "supervisor.startProcess",
    "supervisor.startProcessGroup",
    "supervisor.stopAllProcesses",
    "supervisor.stopProcess",
    "supervisor.stopProcessGroup",
    "supervisor.tailProcessLog",
    "supervisor.tailProcessStderrLog",
    "supervisor.tailProcessStdoutLog",
    "system.listMethods",
    "system.methodHelp",
    "system.methodSignature",
    "system.multicall",
];

pub fn list_methods() -> Value {
    let mut sorted = ALL_METHODS.to_vec();
    sorted.sort();
    Value::Array(
        sorted
            .into_iter()
            .map(|m| Value::String(m.to_string()))
            .collect(),
    )
}

pub fn method_help(name: &str) -> Result<Value, Fault> {
    if !ALL_METHODS.contains(&name) {
        return Err(Fault::signature_unsupported(name));
    }

    let help = match name {
        "supervisor.getAPIVersion" | "supervisor.getVersion" => {
            "Return the version of the RPC API used by supervisord"
        }
        "supervisor.getSupervisorVersion" => {
            "Return the version of the supervisord package in use by the daemon"
        }
        "supervisor.getIdentification" => "Return ident string of the supervisord daemon",
        "supervisor.getState" => {
            "Return current state of supervisord as a struct with statecode and statename"
        }
        "supervisor.getPID" => "Return the PID of supervisord",
        "supervisor.readLog" | "supervisor.readMainLog" => {
            "Read length bytes from the main log starting at offset"
        }
        "supervisor.clearLog" => "Clear the main log",
        "supervisor.shutdown" => "Shut down the supervisor process",
        "supervisor.restart" => "Restart the supervisor process",
        "supervisor.reloadConfig" => {
            "Reload configuration and report added, changed, and removed groups"
        }
        "supervisor.getAllConfigInfo" => "Get info about all available process configurations",
        "supervisor.getAllProcessInfo" => "Get info about all processes from the supervisor",
        "supervisor.getProcessInfo" => "Get info about a process named name",
        "supervisor.startProcess" => "Start a process",
        "supervisor.startProcessGroup" => "Start all processes in the group named name",
        "supervisor.startAllProcesses" => "Start all processes listed in the configuration file",
        "supervisor.stopProcess" => "Stop a process named name",
        "supervisor.stopProcessGroup" => "Stop all processes in the group named name",
        "supervisor.stopAllProcesses" => "Stop all processes listed in the configuration file",
        "supervisor.signalProcess" => "Send an arbitrary UNIX signal to the process named name",
        "supervisor.signalProcessGroup" => "Send a signal to all processes in the group named name",
        "supervisor.signalAllProcesses" => {
            "Send a signal to all processes in the configuration file"
        }
        "supervisor.sendProcessStdin" => {
            "Send a string of characters to the standard input of the process name"
        }
        "supervisor.sendRemoteCommEvent" => {
            "Send an event that will be received by event listener sub-processes"
        }
        "supervisor.tailProcessStdoutLog" | "supervisor.tailProcessLog" => {
            "Provides a more efficient way to tail the stdout log"
        }
        "supervisor.tailProcessStderrLog" => "Provides a more efficient way to tail the stderr log",
        "supervisor.readProcessStdoutLog" | "supervisor.readProcessLog" => {
            "Read length bytes from process name's stdout log starting at offset"
        }
        "supervisor.readProcessStderrLog" => {
            "Read length bytes from process name's stderr log starting at offset"
        }
        "supervisor.clearProcessLogs" | "supervisor.clearProcessLog" => {
            "Clear the stdout and stderr logs for the specified process"
        }
        "supervisor.clearAllProcessLogs" => "Clear all process log files",
        "supervisor.addProcessGroup" => {
            "Update the config for a running process group and add it to the runtime configuration"
        }
        "supervisor.removeProcessGroup" => "Remove a stopped process from the active configuration",
        "system.listMethods" => "Return an array of all available methods",
        "system.methodHelp" => "Return help text for a method",
        "system.methodSignature" => "Return signature for a method",
        "system.multicall" => "Process an array of calls, and return an array of results",
        _ => "Supervisor RPC method",
    };

    Ok(Value::String(help.to_string()))
}

pub fn method_signature(name: &str) -> Result<Value, Fault> {
    if !ALL_METHODS.contains(&name) {
        return Err(Fault::signature_unsupported(name));
    }

    // Standard signatures (return type followed by parameter types)
    let sig = match name {
        "supervisor.getAPIVersion"
        | "supervisor.getVersion"
        | "supervisor.getSupervisorVersion"
        | "supervisor.getIdentification" => {
            vec![Value::String("string".to_string())]
        }
        "supervisor.getPID" => vec![Value::String("int".to_string())],
        "supervisor.getState" => vec![Value::String("struct".to_string())],
        "supervisor.startProcess" | "supervisor.stopProcess" | "supervisor.signalProcess" => {
            vec![
                Value::String("boolean".to_string()),
                Value::String("string".to_string()),
            ]
        }
        "supervisor.startProcessGroup"
        | "supervisor.stopProcessGroup"
        | "supervisor.startAllProcesses"
        | "supervisor.stopAllProcesses"
        | "supervisor.signalAllProcesses" => {
            vec![Value::String("array".to_string())]
        }
        _ => vec![Value::String("array".to_string())],
    };

    Ok(Value::Array(sig))
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Executes system.multicall, evaluating each call and capturing faults inside the result array.
pub async fn handle_multicall<'b, F>(params: &'b [Value], dispatch: F) -> Result<Value, Fault>
where
    F: Fn(&'b str, &'b [Value]) -> BoxFuture<'b, Result<Value, Fault>>,
{
    if params.is_empty() {
        return Err(Fault::incorrect_params(
            "system.multicall requires 1 parameter: list of calls",
        ));
    }

    let calls = params[0]
        .as_array()
        .ok_or_else(|| Fault::incorrect_params("system.multicall parameter must be an array"))?;

    let mut results = Vec::with_capacity(calls.len());

    for call in calls {
        let call_struct = match call.as_struct() {
            Some(s) => s,
            None => {
                let mut fault_map = BTreeMap::new();
                fault_map.insert(
                    "faultCode".to_string(),
                    Value::Int(Fault::incorrect_params("").code),
                );
                fault_map.insert(
                    "faultString".to_string(),
                    Value::String("Call specification must be a struct".to_string()),
                );
                results.push(Value::Struct(fault_map));
                continue;
            }
        };

        let method_name = match call_struct.get("methodName").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => {
                let mut fault_map = BTreeMap::new();
                fault_map.insert(
                    "faultCode".to_string(),
                    Value::Int(Fault::incorrect_params("").code),
                );
                fault_map.insert(
                    "faultString".to_string(),
                    Value::String("Missing methodName in multicall item".to_string()),
                );
                results.push(Value::Struct(fault_map));
                continue;
            }
        };

        // Recursive multicall is explicitly forbidden
        if method_name == "system.multicall" {
            let mut fault_map = BTreeMap::new();
            fault_map.insert(
                "faultCode".to_string(),
                Value::Int(crate::compat::xmlrpc::fault::FaultCode::IncorrectParameters.code()),
            );
            fault_map.insert(
                "faultString".to_string(),
                Value::String("Recursive multicall not allowed".to_string()),
            );
            results.push(Value::Struct(fault_map));
            continue;
        }

        static EMPTY_PARAMS: &[Value] = &[];
        let call_params = call_struct
            .get("params")
            .and_then(|v| v.as_array())
            .unwrap_or(EMPTY_PARAMS);

        let res = dispatch(method_name, call_params).await;
        match res {
            Ok(v) => {
                // Standard XML-RPC multicall wraps successful return values in a 1-element array
                results.push(Value::Array(vec![v]));
            }
            Err(f) => {
                let mut fault_map = BTreeMap::new();
                fault_map.insert("faultCode".to_string(), Value::Int(f.code));
                fault_map.insert("faultString".to_string(), Value::String(f.message));
                results.push(Value::Struct(fault_map));
            }
        }
    }

    Ok(Value::Array(results))
}
