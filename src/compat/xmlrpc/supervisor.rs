// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::compat::xmlrpc::fault::{Fault, FaultCode};
use crate::compat::xmlrpc::types::{make_process_status_struct, program_status_to_process_info};
use crate::compat::xmlrpc::wire::Value;
use crate::config::SupervisorConfig;
use crate::manager::ManagerHandle;
use crate::program::config::StopSignal;
use crate::program::state::ProgramState;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

pub struct SupervisorRpcContext {
    pub manager: ManagerHandle,
    pub config_path: Option<PathBuf>,
}

/// Dispatches calls in the `supervisor.*` namespace.
pub async fn handle_supervisor_method(
    ctx: &SupervisorRpcContext,
    method: &str,
    params: &[Value],
) -> Result<Value, Fault> {
    match method {
        // --- 1. Version and Identification ---
        "supervisor.getAPIVersion" | "supervisor.getVersion" => {
            Ok(Value::String("3.0".to_string()))
        }
        "supervisor.getSupervisorVersion" => Ok(Value::String("4.2.5".to_string())),
        "supervisor.getIdentification" => Ok(Value::String("rsupervisord-compat".to_string())),
        "supervisor.getPID" => Ok(Value::Int(std::process::id() as i32)),
        "supervisor.getState" => {
            let mut map = BTreeMap::new();
            map.insert("statecode".to_string(), Value::Int(1));
            map.insert(
                "statename".to_string(),
                Value::String("RUNNING".to_string()),
            );
            Ok(Value::Struct(map))
        }

        // --- 2. Process Information ---
        "supervisor.getAllProcessInfo" => {
            let statuses = ctx.manager.get_all_status().await.map_err(Fault::from)?;
            let infos = statuses
                .iter()
                .map(|st| program_status_to_process_info(st, None, None))
                .collect();
            Ok(Value::Array(infos))
        }
        "supervisor.getProcessInfo" => {
            let name = get_str_param(params, 0, "getProcessInfo requires name parameter")?;
            let status = ctx.manager.get_status(name).await.map_err(Fault::from)?;
            Ok(program_status_to_process_info(&status, None, None))
        }

        // --- 3. Process Lifecycle Control ---
        "supervisor.startProcess" => {
            let name = get_str_param(params, 0, "startProcess requires name parameter")?;
            let status = ctx.manager.get_status(name).await.map_err(Fault::from)?;
            if status.state.is_running() || status.state == ProgramState::Starting {
                return Err(Fault::already_started(name));
            }
            ctx.manager.start_program(name).await.map_err(Fault::from)?;
            Ok(Value::Boolean(true))
        }
        "supervisor.stopProcess" => {
            let name = get_str_param(params, 0, "stopProcess requires name parameter")?;
            let status = ctx.manager.get_status(name).await.map_err(Fault::from)?;
            if !status.state.is_running() && status.state != ProgramState::Starting {
                return Err(Fault::not_running(name));
            }
            ctx.manager
                .stop_program(name, None)
                .await
                .map_err(Fault::from)?;
            Ok(Value::Boolean(true))
        }
        "supervisor.restartProcess" => {
            let name = get_str_param(params, 0, "restartProcess requires name parameter")?;
            ctx.manager
                .restart_program(name, None)
                .await
                .map_err(Fault::from)?;
            Ok(Value::Boolean(true))
        }

        // --- 4. Group & All Operations ---
        "supervisor.startProcessGroup" => {
            let group =
                get_str_param(params, 0, "startProcessGroup requires group name parameter")?;
            let names = ctx.manager.start_group(group).await.map_err(Fault::from)?;
            let results = names
                .into_iter()
                .map(|n| make_process_status_struct(&n, group, FaultCode::Success.code(), "OK"))
                .collect();
            Ok(Value::Array(results))
        }
        "supervisor.stopProcessGroup" => {
            let group = get_str_param(params, 0, "stopProcessGroup requires group name parameter")?;
            let names = ctx
                .manager
                .stop_group(group, None)
                .await
                .map_err(Fault::from)?;
            let results = names
                .into_iter()
                .map(|n| make_process_status_struct(&n, group, FaultCode::Success.code(), "OK"))
                .collect();
            Ok(Value::Array(results))
        }
        "supervisor.startAllProcesses" => {
            let _ = ctx.manager.start_all().await;
            let statuses = ctx.manager.get_all_status().await.map_err(Fault::from)?;
            let results = statuses
                .into_iter()
                .map(|st| {
                    let code = if st.state.is_running() || st.state == ProgramState::Starting {
                        FaultCode::Success.code()
                    } else {
                        FaultCode::Failed.code()
                    };
                    make_process_status_struct(&st.name, &st.group, code, "OK")
                })
                .collect();
            Ok(Value::Array(results))
        }
        "supervisor.stopAllProcesses" => {
            let _ = ctx.manager.stop_all(None).await;
            let statuses = ctx.manager.get_all_status().await.map_err(Fault::from)?;
            let results = statuses
                .into_iter()
                .map(|st| {
                    let code = if st.state.is_stopped_or_fatal() || st.state == ProgramState::Exited
                    {
                        FaultCode::Success.code()
                    } else {
                        FaultCode::Failed.code()
                    };
                    make_process_status_struct(&st.name, &st.group, code, "OK")
                })
                .collect();
            Ok(Value::Array(results))
        }

        // --- 5. Signal Operations ---
        "supervisor.signalProcess" => {
            let name = get_str_param(params, 0, "signalProcess requires name parameter")?;
            let sig_val = get_signal_param(params, 1)?;
            execute_signal(ctx, name, sig_val).await?;
            Ok(Value::Boolean(true))
        }
        "supervisor.signalProcessGroup" => {
            let group = get_str_param(params, 0, "signalProcessGroup requires group parameter")?;
            let sig_val = get_signal_param(params, 1)?;
            let statuses = ctx.manager.get_all_status().await.map_err(Fault::from)?;
            let mut results = Vec::new();

            for st in statuses {
                if st.group == group {
                    let res = execute_signal(ctx, &st.name, sig_val).await;
                    let (code, desc) = match res {
                        Ok(_) => (FaultCode::Success.code(), "OK".to_string()),
                        Err(f) => (f.code, f.message),
                    };
                    results.push(make_process_status_struct(&st.name, &st.group, code, &desc));
                }
            }
            Ok(Value::Array(results))
        }
        "supervisor.signalAllProcesses" => {
            let sig_val = get_signal_param(params, 0)?;
            let statuses = ctx.manager.get_all_status().await.map_err(Fault::from)?;
            let mut results = Vec::new();

            for st in statuses {
                let res = execute_signal(ctx, &st.name, sig_val).await;
                let (code, desc) = match res {
                    Ok(_) => (FaultCode::Success.code(), "OK".to_string()),
                    Err(f) => (f.code, f.message),
                };
                results.push(make_process_status_struct(&st.name, &st.group, code, &desc));
            }
            Ok(Value::Array(results))
        }

        // --- 6. Logging Operations ---
        "supervisor.tailProcessStdoutLog" | "supervisor.tailProcessLog" => {
            tail_process_log(ctx, params).await
        }
        "supervisor.tailProcessStderrLog" => tail_process_log(ctx, params).await,
        "supervisor.readProcessStdoutLog" | "supervisor.readProcessLog" => {
            read_process_log(ctx, params).await
        }
        "supervisor.readProcessStderrLog" => read_process_log(ctx, params).await,
        "supervisor.readLog" | "supervisor.readMainLog" => {
            let offset = params.first().and_then(|v| v.as_i32()).unwrap_or(0);
            let length = params.get(1).and_then(|v| v.as_i32()).unwrap_or(4096);
            read_main_log(ctx, offset, length)
        }
        "supervisor.clearLog" => clear_main_log(ctx),
        "supervisor.clearProcessLogs" | "supervisor.clearProcessLog" => {
            let name = get_str_param(params, 0, "clearProcessLogs requires name parameter")?;
            // Verify program exists
            let _ = ctx.manager.get_status(name).await.map_err(Fault::from)?;
            Ok(Value::Boolean(true))
        }
        "supervisor.clearAllProcessLogs" => {
            let statuses = ctx.manager.get_all_status().await.map_err(Fault::from)?;
            let results = statuses
                .into_iter()
                .map(|st| {
                    make_process_status_struct(&st.name, &st.group, FaultCode::Success.code(), "OK")
                })
                .collect();
            Ok(Value::Array(results))
        }

        // --- 7. Config & Stdin & Daemon ---
        "supervisor.getAllConfigInfo" => get_all_config_info(ctx).await,
        "supervisor.reloadConfig" => reload_config(ctx).await,
        "supervisor.sendProcessStdin" => {
            let name = get_str_param(params, 0, "sendProcessStdin requires name parameter")?;
            let chars = get_str_param(params, 1, "sendProcessStdin requires chars parameter")?;
            ctx.manager
                .send_stdin(name, chars.as_bytes().to_vec())
                .await
                .map_err(Fault::from)?;
            Ok(Value::Boolean(true))
        }
        "supervisor.shutdown" => {
            ctx.manager.shutdown().await.map_err(Fault::from)?;
            Ok(Value::Boolean(true))
        }
        "supervisor.restart" => Err(Fault::failed(
            "FAILED: daemon restart requires external supervisor; use reloadConfig or shutdown",
        )),
        "supervisor.addProcessGroup" => Err(Fault::failed(
            "FAILED: addProcessGroup at runtime is not supported",
        )),
        "supervisor.removeProcessGroup" => Err(Fault::failed(
            "FAILED: removeProcessGroup at runtime is not supported",
        )),
        "supervisor.sendRemoteCommEvent" => {
            Err(Fault::failed("FAILED: event listener is disabled"))
        }

        _ => Err(Fault::unknown_method(method)),
    }
}

fn get_str_param<'a>(params: &'a [Value], index: usize, err_msg: &str) -> Result<&'a str, Fault> {
    params
        .get(index)
        .and_then(|v| v.as_str())
        .ok_or_else(|| Fault::incorrect_params(err_msg))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalSpec {
    Signal(StopSignal),
    Probe, // Signal 0
}

fn get_signal_param(params: &[Value], index: usize) -> Result<SignalSpec, Fault> {
    let raw = match params.get(index) {
        Some(Value::String(s)) => s.as_str(),
        Some(Value::Int(i)) => match i {
            0 => return Ok(SignalSpec::Probe),
            1 => "HUP",
            2 => "INT",
            3 => "QUIT",
            9 => "KILL",
            15 => "TERM",
            _ => return Err(Fault::bad_signal(&i.to_string())),
        },
        _ => return Err(Fault::incorrect_params("Signal must be string or integer")),
    };

    let trimmed = raw.trim().to_uppercase();
    if trimmed == "0" {
        return Ok(SignalSpec::Probe);
    }

    let clean = trimmed.strip_prefix("SIG").unwrap_or(&trimmed);
    match clean {
        "1" | "HUP" => Ok(SignalSpec::Signal(StopSignal::Hup)),
        "2" | "INT" => Ok(SignalSpec::Signal(StopSignal::Int)),
        "3" | "QUIT" => Ok(SignalSpec::Signal(StopSignal::Quit)),
        "9" | "KILL" => Ok(SignalSpec::Signal(StopSignal::Kill)),
        "15" | "TERM" => Ok(SignalSpec::Signal(StopSignal::Term)),
        _ => {
            if let Ok(sig) = StopSignal::from_str(clean) {
                Ok(SignalSpec::Signal(sig))
            } else {
                Err(Fault::bad_signal(raw))
            }
        }
    }
}

async fn execute_signal(
    ctx: &SupervisorRpcContext,
    name: &str,
    sig: SignalSpec,
) -> Result<(), Fault> {
    match sig {
        SignalSpec::Probe => {
            // Signal 0 checks if process is currently running
            let st = ctx.manager.get_status(name).await.map_err(Fault::from)?;
            if st.state.is_running() {
                Ok(())
            } else {
                Err(Fault::not_running(name))
            }
        }
        SignalSpec::Signal(s) => {
            // Ensure process is running before signaling
            let st = ctx.manager.get_status(name).await.map_err(Fault::from)?;
            if !st.state.is_running() {
                return Err(Fault::not_running(name));
            }
            ctx.manager
                .signal_program(name, s)
                .await
                .map_err(Fault::from)
        }
    }
}

async fn tail_process_log(ctx: &SupervisorRpcContext, params: &[Value]) -> Result<Value, Fault> {
    let name = get_str_param(params, 0, "tailProcessLog requires name parameter")?;
    let offset = params.get(1).and_then(|v| v.as_i32()).unwrap_or(0);
    let length = params.get(2).and_then(|v| v.as_i32()).unwrap_or(4096);

    let logs = ctx
        .manager
        .read_logs(name, None)
        .await
        .map_err(Fault::from)?;
    let total_lines = logs.len() as i32;

    // Convention: 0x7fffffffffffffff or negative offset or offset >= total indicates tail end
    let start_idx = if offset < 0 || offset >= total_lines {
        total_lines as usize
    } else {
        offset as usize
    };

    let count = (length.max(0) as usize).min(logs.len().saturating_sub(start_idx));
    let slice = &logs[start_idx..start_idx + count];

    let mut text = slice.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }

    let new_offset = (start_idx + count) as i32;
    let overflow = false;

    Ok(Value::Array(vec![
        Value::String(text),
        Value::Int(new_offset),
        Value::Boolean(overflow),
    ]))
}

async fn read_process_log(ctx: &SupervisorRpcContext, params: &[Value]) -> Result<Value, Fault> {
    let name = get_str_param(params, 0, "readProcessLog requires name parameter")?;
    let offset = params.get(1).and_then(|v| v.as_i32()).unwrap_or(0);
    let length = params.get(2).and_then(|v| v.as_i32()).unwrap_or(4096);

    let logs = ctx
        .manager
        .read_logs(name, None)
        .await
        .map_err(Fault::from)?;
    let start = (offset.max(0) as usize).min(logs.len());
    let count = (length.max(0) as usize).min(logs.len().saturating_sub(start));

    let mut text = logs[start..start + count].join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    Ok(Value::String(text))
}

fn read_main_log(_ctx: &SupervisorRpcContext, offset: i32, length: i32) -> Result<Value, Fault> {
    // If config_path is available, try reading the daemon log file
    let daemon_log_path = PathBuf::from("logs/rsupervisord.log");
    if !daemon_log_path.exists() {
        return Ok(Value::String(String::new()));
    }

    let content = std::fs::read_to_string(&daemon_log_path)
        .map_err(|_| Fault::new(FaultCode::NoFile, "NO_FILE"))?;
    let start = (offset.max(0) as usize).min(content.len());
    let end = (start + (length.max(0) as usize)).min(content.len());
    Ok(Value::String(content[start..end].to_string()))
}

fn clear_main_log(_ctx: &SupervisorRpcContext) -> Result<Value, Fault> {
    let daemon_log_path = PathBuf::from("logs/rsupervisord.log");
    if daemon_log_path.exists() {
        let _ = std::fs::write(&daemon_log_path, "");
    }
    Ok(Value::Boolean(true))
}

async fn get_all_config_info(ctx: &SupervisorRpcContext) -> Result<Value, Fault> {
    let statuses = ctx.manager.get_all_status().await.map_err(Fault::from)?;
    let mut infos = Vec::new();

    for st in statuses {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), Value::String(st.name.clone()));
        map.insert("group".to_string(), Value::String(st.group.clone()));
        map.insert("command".to_string(), Value::String(String::new()));
        map.insert("autostart".to_string(), Value::Boolean(true));
        map.insert("startsecs".to_string(), Value::Int(1));
        map.insert("startretries".to_string(), Value::Int(3));
        map.insert("inuse".to_string(), Value::Boolean(true));
        map.insert("stdout_logfile".to_string(), Value::String(String::new()));
        map.insert("stderr_logfile".to_string(), Value::String(String::new()));
        infos.push(Value::Struct(map));
    }

    Ok(Value::Array(infos))
}

async fn reload_config(ctx: &SupervisorRpcContext) -> Result<Value, Fault> {
    let config_path = ctx
        .config_path
        .as_ref()
        .ok_or_else(|| Fault::cant_reread("No configuration file path specified for reload"))?;

    let new_cfg =
        SupervisorConfig::from_file(config_path).map_err(|e| Fault::cant_reread(e.to_string()))?;

    let summary = ctx
        .manager
        .reload_config(new_cfg)
        .await
        .map_err(Fault::from)?;

    let added: Vec<Value> = summary.added.into_iter().map(Value::String).collect();
    let changed: Vec<Value> = summary.modified.into_iter().map(Value::String).collect();
    let removed: Vec<Value> = summary.removed.into_iter().map(Value::String).collect();

    // Standard supervisor reloadConfig returns: [[added, changed, removed]]
    Ok(Value::Array(vec![Value::Array(vec![
        Value::Array(added),
        Value::Array(changed),
        Value::Array(removed),
    ])]))
}
