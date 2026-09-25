// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::compat::xmlrpc::fault::{Fault, FaultCode};
use crate::compat::xmlrpc::types::{make_process_status_struct, program_status_to_process_info};
use crate::compat::xmlrpc::wire::Value;
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
        "supervisor.getSupervisorVersion" => {
            Ok(Value::String(env!("CARGO_PKG_VERSION").to_string()))
        }
        "supervisor.getIdentification" => {
            Ok(Value::String(ctx.manager.server_identifier().to_string()))
        }
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
            tail_process_log(ctx, params, LogChannel::Stdout).await
        }
        "supervisor.tailProcessStderrLog" => {
            tail_process_log(ctx, params, LogChannel::Stderr).await
        }
        "supervisor.readProcessStdoutLog" | "supervisor.readProcessLog" => {
            read_process_log(ctx, params, LogChannel::Stdout).await
        }
        "supervisor.readProcessStderrLog" => {
            read_process_log(ctx, params, LogChannel::Stderr).await
        }
        "supervisor.readLog" | "supervisor.readMainLog" => {
            let offset = params.first().and_then(|v| v.as_i32()).unwrap_or(0);
            let length = params.get(1).and_then(|v| v.as_i32()).unwrap_or(0);
            read_main_log(ctx, offset, length).await
        }
        "supervisor.tailMainLog" => {
            let offset = params.first().and_then(|v| v.as_i64()).unwrap_or(0);
            let length = params.get(1).and_then(|v| v.as_i64()).unwrap_or(4096);
            let (data, new_off, overflow) = ctx
                .manager
                .tail_main_log(offset, length)
                .await
                .map_err(Fault::from)?;
            Ok(Value::Array(vec![
                Value::String(data),
                Value::Int(new_off as i32),
                Value::Boolean(overflow),
            ]))
        }
        "supervisor.clearLog" => clear_main_log(ctx).await,
        "supervisor.clearProcessLogs" | "supervisor.clearProcessLog" => {
            let name = get_str_param(params, 0, "clearProcessLogs requires name parameter")?;
            ctx.manager
                .clear_process_logs(name)
                .await
                .map_err(Fault::from)?;
            Ok(Value::Boolean(true))
        }
        "supervisor.clearAllProcessLogs" => {
            let configs = ctx.manager.get_all_configs().await.map_err(Fault::from)?;
            for cfg in configs.values() {
                let _ = ctx.manager.clear_process_logs(&cfg.name).await;
            }
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
            // Stock supervisord sleeps briefly before quitting so the enclosing
            // XML-RPC connection can flush its response; mirror that so the
            // engine's teardown never races an in-flight shutdown response.
            let manager = ctx.manager.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let _ = manager.shutdown().await;
            });
            Ok(Value::Boolean(true))
        }
        "supervisor.restart" => {
            let config_path = ctx.config_path.as_ref().ok_or_else(|| {
                Fault::cant_reread("No configuration file path specified for reload")
            })?;
            let new_config = crate::daemon::load_config(config_path, crate::daemon::daemon_args())
                .map_err(|e| Fault::cant_reread(format!("Invalid configuration: {}", e)))?;
            ctx.manager
                .restart_daemon(new_config)
                .await
                .map_err(Fault::from)?;
            Ok(Value::Boolean(true))
        }
        "supervisor.addProcessGroup" => {
            let name = get_str_param(params, 0, "addProcessGroup requires group name parameter")?;
            let ok = ctx
                .manager
                .add_process_group(name)
                .await
                .map_err(Fault::from)?;
            if !ok {
                return Err(Fault::already_added(name));
            }
            Ok(Value::Boolean(true))
        }
        "supervisor.removeProcessGroup" => {
            let name = get_str_param(
                params,
                0,
                "removeProcessGroup requires group name parameter",
            )?;
            let ok = ctx
                .manager
                .remove_process_group(name)
                .await
                .map_err(Fault::from)?;
            if !ok {
                return Err(Fault::still_running(name));
            }
            Ok(Value::Boolean(true))
        }
        "supervisor.sendRemoteCommEvent" => {
            let type_str = get_str_param(params, 0, "type string required")?;
            let data = get_str_param(params, 1, "data string required")?;
            ctx.manager.send_remote_comm_event(type_str, data);
            Ok(Value::Boolean(true))
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

use crate::logging::LogChannel;

async fn tail_process_log(
    ctx: &SupervisorRpcContext,
    params: &[Value],
    channel: LogChannel,
) -> Result<Value, Fault> {
    let name = get_str_param(params, 0, "tailProcessLog requires name parameter")?;
    let offset = params.get(1).and_then(|v| v.as_i64()).unwrap_or(0);
    let length = params.get(2).and_then(|v| v.as_i64()).unwrap_or(4096);

    let (data, new_off, overflow) = ctx
        .manager
        .tail_log(name, channel, offset, length)
        .await
        .map_err(Fault::from)?;

    Ok(Value::Array(vec![
        Value::String(data),
        Value::Int(new_off as i32),
        Value::Boolean(overflow),
    ]))
}

async fn read_process_log(
    ctx: &SupervisorRpcContext,
    params: &[Value],
    channel: LogChannel,
) -> Result<Value, Fault> {
    let name = get_str_param(params, 0, "readProcessLog requires name parameter")?;
    let offset = params.get(1).and_then(|v| v.as_i64()).unwrap_or(0);
    let length = params.get(2).and_then(|v| v.as_i64()).unwrap_or(0);

    let (data, _new_off, _overflow) = ctx
        .manager
        .read_log(name, channel, offset, length)
        .await
        .map_err(Fault::from)?;

    Ok(Value::String(data))
}

async fn read_main_log(
    ctx: &SupervisorRpcContext,
    offset: i32,
    length: i32,
) -> Result<Value, Fault> {
    let data = ctx
        .manager
        .read_main_log(offset as i64, length as i64)
        .await
        .map_err(Fault::from)?;
    Ok(Value::String(data))
}

async fn clear_main_log(ctx: &SupervisorRpcContext) -> Result<Value, Fault> {
    ctx.manager.clear_main_log().await.map_err(Fault::from)?;
    Ok(Value::Boolean(true))
}

fn stop_signal_to_int(sig: StopSignal) -> i32 {
    match sig {
        StopSignal::Hup => 1,
        StopSignal::Int | StopSignal::CtrlC => 2,
        StopSignal::Quit | StopSignal::CtrlBreak => 3,
        StopSignal::Kill => 9,
        StopSignal::Term => 15,
    }
}

async fn get_all_config_info(ctx: &SupervisorRpcContext) -> Result<Value, Fault> {
    let configs = ctx.manager.get_all_configs().await.map_err(Fault::from)?;
    let mut infos = Vec::new();

    for cfg in configs.values() {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), Value::String(cfg.name.clone()));
        map.insert("group".to_string(), Value::String(cfg.group.clone()));
        map.insert(
            "group_prio".to_string(),
            Value::Int(cfg.group_priority as i32),
        );
        map.insert("process_prio".to_string(), Value::Int(cfg.priority as i32));
        map.insert("command".to_string(), Value::String(cfg.full_command()));
        map.insert("autostart".to_string(), Value::Boolean(cfg.autostart));
        map.insert(
            "startsecs".to_string(),
            Value::Int(cfg.start_secs.as_secs() as i32),
        );
        map.insert(
            "startretries".to_string(),
            Value::Int(cfg.start_retries as i32),
        );
        map.insert("inuse".to_string(), Value::Boolean(true));
        map.insert("killasgroup".to_string(), Value::Boolean(cfg.kill_as_group));
        map.insert("stopasgroup".to_string(), Value::Boolean(cfg.stop_as_group));
        map.insert(
            "redirect_stderr".to_string(),
            Value::Boolean(cfg.logs.redirect_stderr),
        );
        map.insert(
            "stopsignal".to_string(),
            Value::Int(stop_signal_to_int(cfg.stop_signal)),
        );
        map.insert(
            "stopwaitsecs".to_string(),
            Value::Int(cfg.stop_wait_secs.as_secs() as i32),
        );
        map.insert(
            "directory".to_string(),
            Value::String(
                cfg.directory
                    .as_ref()
                    .map(|d| d.to_string_lossy().to_string())
                    .unwrap_or_else(|| "none".to_string()),
            ),
        );
        map.insert(
            "uid".to_string(),
            Value::String(cfg.user.clone().unwrap_or_else(|| "none".to_string())),
        );
        map.insert(
            "exitcodes".to_string(),
            Value::Array(cfg.exit_codes.iter().map(|&c| Value::Int(c)).collect()),
        );

        let stdout_logfile = if cfg.logs.is_stdout_disabled() {
            "none".to_string()
        } else {
            cfg.logs
                .stdout
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "none".to_string())
        };
        map.insert("stdout_logfile".to_string(), Value::String(stdout_logfile));

        let stderr_logfile = if cfg.logs.is_stderr_disabled() || cfg.logs.redirect_stderr {
            "none".to_string()
        } else {
            cfg.logs
                .stderr
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "none".to_string())
        };
        map.insert("stderr_logfile".to_string(), Value::String(stderr_logfile));

        map.insert(
            "stdout_logfile_maxbytes".to_string(),
            Value::Int(cfg.logs.effective_stdout_max_bytes() as i32),
        );
        map.insert(
            "stderr_logfile_maxbytes".to_string(),
            Value::Int(cfg.logs.effective_stderr_max_bytes() as i32),
        );

        map.insert(
            "stdout_logfile_backups".to_string(),
            Value::Int(cfg.logs.effective_stdout_backups() as i32),
        );
        map.insert(
            "stderr_logfile_backups".to_string(),
            Value::Int(cfg.logs.effective_stderr_backups() as i32),
        );

        map.insert("stdout_capture_maxbytes".to_string(), Value::Int(0));
        map.insert("stderr_capture_maxbytes".to_string(), Value::Int(0));
        map.insert(
            "stdout_events_enabled".to_string(),
            Value::Boolean(cfg.logs.stdout_events_enabled),
        );
        map.insert(
            "stderr_events_enabled".to_string(),
            Value::Boolean(cfg.logs.stderr_events_enabled),
        );
        map.insert(
            "stdout_syslog".to_string(),
            Value::Boolean(cfg.logs.stdout_syslog),
        );
        map.insert(
            "stderr_syslog".to_string(),
            Value::Boolean(cfg.logs.stderr_syslog),
        );
        map.insert("serverurl".to_string(), Value::String("none".to_string()));

        infos.push(Value::Struct(map));
    }

    infos.sort_by(|a, b| {
        let name_a = match a {
            Value::Struct(m) => m.get("name").and_then(|v| v.as_str()).unwrap_or(""),
            _ => "",
        };
        let name_b = match b {
            Value::Struct(m) => m.get("name").and_then(|v| v.as_str()).unwrap_or(""),
            _ => "",
        };
        name_a.cmp(name_b)
    });

    Ok(Value::Array(infos))
}

async fn reload_config(ctx: &SupervisorRpcContext) -> Result<Value, Fault> {
    let config_path = ctx
        .config_path
        .as_ref()
        .ok_or_else(|| Fault::cant_reread("No configuration file path specified for reload"))?;

    let new_cfg = crate::daemon::load_config(config_path, crate::daemon::daemon_args())
        .map_err(|e| Fault::cant_reread(e.to_string()))?;

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
