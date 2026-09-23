// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::compat::xmlrpc::fault::{Fault, FaultCode};
use crate::compat::xmlrpc::types::{make_process_status_struct, program_status_to_process_info};
use crate::compat::xmlrpc::wire::Value;
use crate::config::SupervisorConfig;
use crate::manager::ManagerHandle;
use crate::program::config::StopSignal;
use crate::program::state::ProgramState;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
            read_main_log(ctx, offset, length)
        }
        "supervisor.clearLog" => clear_main_log(ctx),
        "supervisor.clearProcessLogs" | "supervisor.clearProcessLog" => {
            let name = get_str_param(params, 0, "clearProcessLogs requires name parameter")?;
            let cfg = ctx.manager.get_config(name).await.map_err(Fault::from)?;
            if let Some(ref path) = cfg.logs.stdout
                && path.exists()
            {
                let _ = std::fs::write(path, "");
            }
            if let Some(ref path) = cfg.logs.stderr
                && path.exists()
            {
                let _ = std::fs::write(path, "");
            }
            Ok(Value::Boolean(true))
        }
        "supervisor.clearAllProcessLogs" => {
            let configs = ctx.manager.get_all_configs().await.map_err(Fault::from)?;
            for cfg in configs.values() {
                if let Some(ref path) = cfg.logs.stdout
                    && path.exists()
                {
                    let _ = std::fs::write(path, "");
                }
                if let Some(ref path) = cfg.logs.stderr
                    && path.exists()
                {
                    let _ = std::fs::write(path, "");
                }
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
            let new_config = crate::config::SupervisorConfig::from_file(config_path)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogChannel {
    Stdout,
    Stderr,
}

fn read_file_bytes(path: &Path, offset: i32, length: i32) -> Result<String, Fault> {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};

    let mut file = File::open(path).map_err(|_| Fault::no_file(path.to_string_lossy()))?;
    let metadata = file
        .metadata()
        .map_err(|_| Fault::failed("Failed to read metadata"))?;
    let file_len = metadata.len();

    let abs_offset = offset.unsigned_abs() as u64;

    let (pos, to_read) = if offset < 0 {
        if length != 0 {
            return Err(Fault::bad_arguments(
                "length must be 0 when offset is negative",
            ));
        }
        let pos = file_len.saturating_sub(abs_offset);
        let to_read = (file_len - pos).min(abs_offset);
        (pos, to_read)
    } else {
        if length < 0 {
            return Err(Fault::bad_arguments("length cannot be negative"));
        }
        let pos = (offset as u64).min(file_len);
        let remaining = file_len.saturating_sub(pos);
        let to_read = if length == 0 {
            remaining
        } else {
            (length as u64).min(remaining)
        };
        (pos, to_read)
    };

    file.seek(SeekFrom::Start(pos))
        .map_err(|_| Fault::failed("Failed to seek in log file"))?;

    let mut buf = Vec::with_capacity(to_read as usize);
    std::io::Read::take(&mut file, to_read)
        .read_to_end(&mut buf)
        .map_err(|_| Fault::failed("Failed to read log file"))?;

    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn tail_file_bytes(path: &Path, offset: i64, length: i64) -> (String, i64, bool) {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};

    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return (String::new(), offset, false),
    };
    let sz = match file.metadata() {
        Ok(m) => m.len() as i64,
        Err(_) => return (String::new(), offset, false),
    };

    let mut overflow = false;
    let mut off = offset;
    let mut len = length;

    if sz > (off + len) {
        overflow = true;
        off = sz - 1;
    }

    if (off + len) > sz {
        if off > (sz - 1) {
            len = 0;
        }
        off = sz - len;
    }

    if off < 0 {
        off = 0;
    }
    if len < 0 {
        len = 0;
    }

    let data = if len == 0 {
        Vec::new()
    } else {
        if file.seek(SeekFrom::Start(off as u64)).is_err() {
            return (String::new(), offset, false);
        }
        let mut buf = Vec::with_capacity(len as usize);
        match std::io::Read::take(&mut file, len as u64).read_to_end(&mut buf) {
            Ok(_) => buf,
            Err(_) => Vec::new(),
        }
    };

    (String::from_utf8_lossy(&data).to_string(), sz, overflow)
}

fn read_memory_log(logs: &[String], offset: i32, length: i32) -> Result<String, Fault> {
    let mut full_text = logs.join("\n");
    if !full_text.is_empty() {
        full_text.push('\n');
    }
    let bytes = full_text.as_bytes();
    let total_len = bytes.len();
    let abs_offset = offset.unsigned_abs() as usize;

    let (pos, to_read) = if offset < 0 {
        if length != 0 {
            return Err(Fault::bad_arguments(
                "length must be 0 when offset is negative",
            ));
        }
        let pos = total_len.saturating_sub(abs_offset);
        let to_read = (total_len - pos).min(abs_offset);
        (pos, to_read)
    } else {
        if length < 0 {
            return Err(Fault::bad_arguments("length cannot be negative"));
        }
        let pos = (offset as usize).min(total_len);
        let remaining = total_len.saturating_sub(pos);
        let to_read = if length == 0 {
            remaining
        } else {
            (length as usize).min(remaining)
        };
        (pos, to_read)
    };

    Ok(String::from_utf8_lossy(&bytes[pos..pos + to_read]).to_string())
}

fn tail_memory_log(logs: &[String], offset: i64, length: i64) -> (String, i64, bool) {
    let mut full_text = logs.join("\n");
    if !full_text.is_empty() {
        full_text.push('\n');
    }
    let bytes = full_text.as_bytes();
    let sz = bytes.len() as i64;

    let mut overflow = false;
    let mut off = offset;
    let mut len = length;

    if sz > (off + len) {
        overflow = true;
        off = sz - 1;
    }

    if (off + len) > sz {
        if off > (sz - 1) {
            len = 0;
        }
        off = sz - len;
    }

    if off < 0 {
        off = 0;
    }
    if len < 0 {
        len = 0;
    }

    let slice = if len == 0 || off >= sz {
        &[]
    } else {
        let end = (off + len).min(sz) as usize;
        &bytes[off as usize..end]
    };

    (String::from_utf8_lossy(slice).to_string(), sz, overflow)
}

async fn tail_process_log(
    ctx: &SupervisorRpcContext,
    params: &[Value],
    channel: LogChannel,
) -> Result<Value, Fault> {
    let name = get_str_param(params, 0, "tailProcessLog requires name parameter")?;
    let offset = params.get(1).and_then(|v| v.as_i64()).unwrap_or(0);
    let length = params.get(2).and_then(|v| v.as_i64()).unwrap_or(4096);

    let cfg = ctx.manager.get_config(name).await.map_err(Fault::from)?;

    match channel {
        LogChannel::Stderr => {
            if cfg.logs.redirect_stderr {
                return Ok(Value::Array(vec![
                    Value::String(String::new()),
                    Value::Int(offset as i32),
                    Value::Boolean(false),
                ]));
            }
            if let Some(ref path) = cfg.logs.stderr
                && path.exists()
            {
                let (data, new_off, overflow) = tail_file_bytes(path, offset, length);
                return Ok(Value::Array(vec![
                    Value::String(data),
                    Value::Int(new_off as i32),
                    Value::Boolean(overflow),
                ]));
            }
            Ok(Value::Array(vec![
                Value::String(String::new()),
                Value::Int(offset as i32),
                Value::Boolean(false),
            ]))
        }
        LogChannel::Stdout => {
            if let Some(ref path) = cfg.logs.stdout
                && path.exists()
            {
                let (data, new_off, overflow) = tail_file_bytes(path, offset, length);
                return Ok(Value::Array(vec![
                    Value::String(data),
                    Value::Int(new_off as i32),
                    Value::Boolean(overflow),
                ]));
            }
            let logs = ctx
                .manager
                .read_logs(name, None)
                .await
                .map_err(Fault::from)?;
            let (data, new_off, overflow) = tail_memory_log(&logs, offset, length);
            Ok(Value::Array(vec![
                Value::String(data),
                Value::Int(new_off as i32),
                Value::Boolean(overflow),
            ]))
        }
    }
}

async fn read_process_log(
    ctx: &SupervisorRpcContext,
    params: &[Value],
    channel: LogChannel,
) -> Result<Value, Fault> {
    let name = get_str_param(params, 0, "readProcessLog requires name parameter")?;
    let offset = params.get(1).and_then(|v| v.as_i32()).unwrap_or(0);
    let length = params.get(2).and_then(|v| v.as_i32()).unwrap_or(0);

    let cfg = ctx.manager.get_config(name).await.map_err(Fault::from)?;

    match channel {
        LogChannel::Stderr => {
            if cfg.logs.redirect_stderr {
                return Err(Fault::no_file("no log file"));
            }
            if let Some(ref path) = cfg.logs.stderr
                && path.exists()
            {
                return read_file_bytes(path, offset, length).map(Value::String);
            }
            Err(Fault::no_file("no log file"))
        }
        LogChannel::Stdout => {
            if let Some(ref path) = cfg.logs.stdout
                && path.exists()
            {
                return read_file_bytes(path, offset, length).map(Value::String);
            }
            let logs = ctx
                .manager
                .read_logs(name, None)
                .await
                .map_err(Fault::from)?;
            if logs.is_empty() && cfg.logs.stdout.is_some() {
                return Err(Fault::no_file("no log file"));
            }
            let res = read_memory_log(&logs, offset, length)?;
            Ok(Value::String(res))
        }
    }
}

fn read_main_log(_ctx: &SupervisorRpcContext, offset: i32, length: i32) -> Result<Value, Fault> {
    let candidate_paths = [
        PathBuf::from("logs/supervisord.log"),
        PathBuf::from("supervisord.log"),
        PathBuf::from("/tmp/supervisord.log"),
    ];

    for p in &candidate_paths {
        if p.exists() {
            return read_file_bytes(p, offset, length).map(Value::String);
        }
    }

    Ok(Value::String(String::new()))
}

fn clear_main_log(_ctx: &SupervisorRpcContext) -> Result<Value, Fault> {
    let candidate_paths = [
        PathBuf::from("logs/supervisord.log"),
        PathBuf::from("supervisord.log"),
        PathBuf::from("/tmp/supervisord.log"),
    ];
    for p in &candidate_paths {
        if p.exists() {
            let _ = std::fs::write(p, "");
        }
    }
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
        map.insert("startsecs".to_string(), Value::Int(cfg.start_secs as i32));
        map.insert(
            "startretries".to_string(),
            Value::Int(cfg.start_retries as i32),
        );
        map.insert("inuse".to_string(), Value::Boolean(true));
        map.insert("killasgroup".to_string(), Value::Boolean(false));
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
            Value::Int(cfg.stop_wait_secs as i32),
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

        let stdout_maxbytes = cfg
            .logs
            .max_bytes
            .as_deref()
            .and_then(|s| crate::logging::parse_byte_size(s).ok())
            .unwrap_or(crate::consts::DEFAULT_LOG_MAX_BYTES);
        map.insert(
            "stdout_logfile_maxbytes".to_string(),
            Value::Int(stdout_maxbytes as i32),
        );
        map.insert(
            "stderr_logfile_maxbytes".to_string(),
            Value::Int(stdout_maxbytes as i32),
        );

        let backups = cfg
            .logs
            .backups
            .unwrap_or(crate::consts::DEFAULT_LOG_BACKUPS) as i32;
        map.insert("stdout_logfile_backups".to_string(), Value::Int(backups));
        map.insert("stderr_logfile_backups".to_string(), Value::Int(backups));

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
        map.insert("stdout_syslog".to_string(), Value::Boolean(false));
        map.insert("stderr_syslog".to_string(), Value::Boolean(false));
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
