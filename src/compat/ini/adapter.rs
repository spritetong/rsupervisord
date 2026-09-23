// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::compat::ini::parser::ParsedIni;
use crate::compat::ini::values::{
    parse_autorestart, parse_environment, parse_exitcodes, parse_list, parse_log_path,
    parse_loose_bool, parse_stop_signal, parse_umask,
};
use crate::config::schema::{
    GroupConfigRaw, ProgramConfigRaw, ProgramDefaults, ProgramLogsConfigRaw, SupervisorConfig,
    normalize_http_bind,
};
use crate::consts::{
    DEFAULT_EVENT_BUFFER_SIZE, DEFAULT_EVENTLISTENER_PRIORITY, default_result_handler,
};
use crate::error::ProgramError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Adapts a `ParsedIni` into a standard `SupervisorConfig`.
pub fn adapt_ini_to_config(
    ini: &ParsedIni,
    config_dir: Option<&Path>,
) -> Result<SupervisorConfig, ProgramError> {
    let mut config = SupervisorConfig {
        config_dir: config_dir.map(|p| p.to_path_buf()),
        ..Default::default()
    };

    // 1. Process [unix_http_server]
    if let Some(sec) = ini.sections.get("unix_http_server") {
        if let Some(file) = sec.get("file") {
            config.server.uds_path = PathBuf::from(file);
        }
        if let Some(chmod) = sec.get("chmod") {
            config.server.uds_chmod = Some(chmod.clone());
        }
        if let Some(username) = sec.get("username") {
            config.server.uds_username = Some(username.clone());
        }
        if let Some(password) = sec.get("password") {
            config.server.uds_password = Some(password.clone());
        }
    }

    // 2. Process [inet_http_server]
    if let Some(sec) = ini.sections.get("inet_http_server") {
        if let Some(port) = sec.get("port") {
            config.server.http_bind = Some(normalize_http_bind(port));
        }
        if let Some(username) = sec.get("username") {
            config.server.username = Some(username.clone());
        }
        if let Some(password) = sec.get("password") {
            config.server.password = Some(password.clone());
        }
    }

    // 3. Process [supervisord]
    if let Some(sec) = ini.sections.get("supervisord") {
        if let Some(logfile) = sec.get("logfile") {
            if logfile.eq_ignore_ascii_case("NONE") {
                config.logging.enabled = false;
                config.logging.file = None;
            } else {
                config.logging.enabled = true;
                config.logging.file = Some(PathBuf::from(logfile));
            }
        }
        if let Some(maxbytes) = sec.get("logfile_maxbytes") {
            config.logging.max_bytes = Some(maxbytes.clone());
        }
        if let Some(backups_str) = sec.get("logfile_backups")
            && let Ok(backups) = backups_str.parse::<usize>()
        {
            config.logging.backups = backups;
        }
        if let Some(level) = sec.get("loglevel") {
            config.logging.level = level.to_ascii_lowercase();
        }
        if let Some(ident) = sec.get("identifier") {
            config.server.identifier = Some(ident.clone());
        }
        if let Some(env_str) = sec.get("environment")
            && let Ok(env_map) = parse_environment(env_str)
        {
            for (k, v) in env_map {
                // Export daemon-level environment variables
                unsafe {
                    std::env::set_var(k, v);
                }
            }
        }
    }

    // 4. Process [program-default]
    if let Some(sec) = ini.sections.get("program-default") {
        config.program_defaults = parse_program_defaults(sec)?;
    }

    // 5. Process sections
    for section_name in &ini.section_order {
        if let Some(prog_name) = section_name.strip_prefix("program:") {
            if let Some(sec) = ini.sections.get(section_name) {
                let prog = parse_program_config(prog_name, sec)?;
                config.programs.insert(prog_name.to_string(), prog);
            }
        } else if let Some(group_name) = section_name.strip_prefix("group:") {
            if let Some(sec) = ini.sections.get(section_name) {
                let group = parse_group_config(sec)?;
                config.groups.insert(group_name.to_string(), group);
            }
        } else if section_name.starts_with("rpcinterface:")
            || section_name == "supervisorctl"
            || section_name == "include"
            || section_name == "unix_http_server"
            || section_name == "inet_http_server"
            || section_name == "supervisord"
            || section_name == "program-default"
        {
            // Already handled or standard ignored section
        } else if let Some(pool_name) = section_name.strip_prefix("eventlistener:") {
            if let Some(sec) = ini.sections.get(section_name) {
                let el_cfg = parse_event_listener_config(pool_name, sec)?;
                config.event_listeners.insert(pool_name.to_string(), el_cfg);
            }
        } else if section_name.starts_with("fcgi-program:") {
            tracing::warn!(
                "Section '[{}]' encountered: fcgi-program is not supported, ignoring",
                section_name
            );
        } else {
            tracing::debug!(
                "Section '[{}]' encountered: unrecognized section, ignoring",
                section_name
            );
        }
    }

    config.apply_default_paths();
    config = config.translate_paths()?;
    config.validate()?;

    Ok(config)
}

fn parse_program_config(
    prog_name: &str,
    sec: &HashMap<String, String>,
) -> Result<ProgramConfigRaw, ProgramError> {
    let command = sec.get("command").cloned().ok_or_else(|| {
        ProgramError::ConfigError(format!(
            "Program '{}' missing required 'command' field",
            prog_name
        ))
    })?;

    let process_name = sec.get("process_name").cloned();
    let numprocs = sec
        .get("numprocs")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!("Program '{}' invalid numprocs: {}", prog_name, e))
        })?;
    let numprocs_start = sec
        .get("numprocs_start")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!(
                "Program '{}' invalid numprocs_start: {}",
                prog_name, e
            ))
        })?;
    let priority = sec
        .get("priority")
        .map(|s| s.parse::<u32>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!("Program '{}' invalid priority: {}", prog_name, e))
        })?;

    let autostart = sec
        .get("autostart")
        .map(|s| parse_loose_bool(s))
        .transpose()?;
    let autorestart = sec
        .get("autorestart")
        .map(|s| parse_autorestart(s))
        .transpose()?;

    let start_secs = sec
        .get("startsecs")
        .or_else(|| sec.get("start_secs"))
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!("Program '{}' invalid startsecs: {}", prog_name, e))
        })?;

    let start_retries = sec
        .get("startretries")
        .or_else(|| sec.get("start_retries"))
        .map(|s| s.parse::<u32>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!(
                "Program '{}' invalid startretries: {}",
                prog_name, e
            ))
        })?;

    let exit_codes = sec
        .get("exitcodes")
        .or_else(|| sec.get("exit_codes"))
        .map(|s| parse_exitcodes(s))
        .transpose()?;

    let stop_signal = sec
        .get("stopsignal")
        .or_else(|| sec.get("stop_signal"))
        .map(|s| parse_stop_signal(s))
        .transpose()?;

    let stop_wait_secs = sec
        .get("stopwaitsecs")
        .or_else(|| sec.get("stop_wait_secs"))
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!(
                "Program '{}' invalid stopwaitsecs: {}",
                prog_name, e
            ))
        })?;

    let directory = sec.get("directory").map(PathBuf::from);
    let user = sec.get("user").cloned();
    let umask = sec.get("umask").map(|s| parse_umask(s)).transpose()?;

    let environment = if let Some(env_str) = sec.get("environment") {
        parse_environment(env_str)?
    } else {
        HashMap::new()
    };

    let redirect_stderr = sec
        .get("redirect_stderr")
        .map(|s| parse_loose_bool(s))
        .transpose()?;

    // Build log configuration
    let stdout_path = sec.get("stdout_logfile").and_then(|s| parse_log_path(s));
    let stderr_path = sec.get("stderr_logfile").and_then(|s| parse_log_path(s));

    let max_bytes = sec
        .get("stdout_logfile_maxbytes")
        .or_else(|| sec.get("stderr_logfile_maxbytes"))
        .cloned();

    let backups = sec
        .get("stdout_logfile_backups")
        .or_else(|| sec.get("stderr_logfile_backups"))
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!(
                "Program '{}' invalid logfile_backups: {}",
                prog_name, e
            ))
        })?;

    let stdout_events_enabled = sec
        .get("stdout_events_enabled")
        .map(|s| parse_loose_bool(s))
        .transpose()?;
    let stderr_events_enabled = sec
        .get("stderr_events_enabled")
        .map(|s| parse_loose_bool(s))
        .transpose()?;

    let logs = if stdout_path.is_some()
        || stderr_path.is_some()
        || max_bytes.is_some()
        || backups.is_some()
        || redirect_stderr.is_some()
        || stdout_events_enabled.is_some()
        || stderr_events_enabled.is_some()
    {
        Some(ProgramLogsConfigRaw {
            enabled: Some(true),
            stdout: stdout_path,
            stderr: stderr_path,
            max_bytes,
            backups,
            redirect_stderr,
            stdout_events_enabled,
            stderr_events_enabled,
        })
    } else {
        None
    };

    // Extended fields
    let depends_on = sec
        .get("depends_on")
        .map(|s| parse_list(s))
        .unwrap_or_default();
    let cron = sec.get("cron").cloned();
    let cron_stop = sec
        .get("cron_stop")
        .or_else(|| sec.get("stop_cron"))
        .cloned();
    let pre_start = sec
        .get("pre_start")
        .or_else(|| sec.get("pre_start_hook"))
        .cloned();
    let pre_stop = sec
        .get("pre_stop")
        .or_else(|| sec.get("pre_stop_hook"))
        .cloned();
    let pre_start_ignore_failure = sec
        .get("pre_start_ignore_failure")
        .map(|s| parse_loose_bool(s))
        .transpose()?;
    let hook_timeout_secs = sec
        .get("hook_timeout_secs")
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!(
                "Program '{}' invalid hook_timeout_secs: {}",
                prog_name, e
            ))
        })?;

    let restart_when_binary_changed = sec
        .get("restart_when_binary_changed")
        .map(|s| parse_loose_bool(s))
        .transpose()?;
    let restart_signal_when_binary_changed = sec
        .get("restart_signal_when_binary_changed")
        .map(|s| parse_stop_signal(s))
        .transpose()?;
    let restart_cmd_when_binary_changed = sec.get("restart_cmd_when_binary_changed").cloned();
    let restart_directory_monitor = sec.get("restart_directory_monitor").map(PathBuf::from);
    let restart_file_pattern = sec.get("restart_file_pattern").cloned();
    let restart_signal_when_file_changed = sec
        .get("restart_signal_when_file_changed")
        .map(|s| parse_stop_signal(s))
        .transpose()?;
    let restart_cmd_when_file_changed = sec.get("restart_cmd_when_file_changed").cloned();
    let restart_debounce_secs = sec
        .get("restart_debounce_secs")
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!(
                "Program '{}' invalid restart_debounce_secs: {}",
                prog_name, e
            ))
        })?;

    Ok(ProgramConfigRaw {
        command,
        args: Vec::new(),
        directory,
        user,
        environment,
        priority,
        depends_on,
        autostart,
        autorestart,
        start_secs,
        start_retries,
        stop_signal,
        stop_wait_secs,
        exit_codes,
        umask,
        logs,
        health_check: None,
        group: None,
        cron,
        cron_stop,
        pre_start,
        pre_stop,
        pre_start_ignore_failure,
        hook_timeout_secs,
        numprocs,
        numprocs_start,
        process_name,
        restart_when_binary_changed,
        restart_signal_when_binary_changed,
        restart_cmd_when_binary_changed,
        restart_directory_monitor,
        restart_file_pattern,
        restart_signal_when_file_changed,
        restart_cmd_when_file_changed,
        restart_debounce_secs,
        stdout_events_enabled,
        stderr_events_enabled,
    })
}

fn parse_event_listener_config(
    pool_name: &str,
    sec: &HashMap<String, String>,
) -> Result<crate::eventlistener::EventListenerConfigRaw, ProgramError> {
    let command = sec.get("command").cloned().ok_or_else(|| {
        ProgramError::ConfigError(format!(
            "EventListener '{}' missing required 'command' field",
            pool_name
        ))
    })?;

    let events_str = sec.get("events").ok_or_else(|| {
        ProgramError::ConfigError(format!(
            "EventListener '{}' missing required 'events' field",
            pool_name
        ))
    })?;

    let events: Vec<String> = events_str
        .split(&[',', ' ', '\t'][..])
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    crate::eventlistener::validate_event_list(&events)?;

    let buffer_size = sec
        .get("buffer_size")
        .or_else(|| sec.get("buffersize"))
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!(
                "EventListener '{}' invalid buffer_size: {}",
                pool_name, e
            ))
        })?
        .unwrap_or(DEFAULT_EVENT_BUFFER_SIZE);
    if buffer_size < 1 {
        return Err(ProgramError::ConfigError(format!(
            "EventListener '{}' buffer_size must be >= 1",
            pool_name
        )));
    }

    let redirect_stderr = sec
        .get("redirect_stderr")
        .map(|s| parse_loose_bool(s))
        .transpose()?;
    if redirect_stderr == Some(true) {
        return Err(ProgramError::ConfigError(format!(
            "EventListener '{}' redirect_stderr cannot be true (violates wire protocol)",
            pool_name
        )));
    }

    let result_handler = sec
        .get("result_handler")
        .cloned()
        .unwrap_or_else(default_result_handler);

    let priority = sec
        .get("priority")
        .map(|s| s.parse::<i32>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!(
                "EventListener '{}' invalid priority: {}",
                pool_name, e
            ))
        })?
        .unwrap_or(DEFAULT_EVENTLISTENER_PRIORITY);

    let autostart = sec
        .get("autostart")
        .map(|s| parse_loose_bool(s))
        .transpose()?;
    let autorestart = sec
        .get("autorestart")
        .map(|s| parse_autorestart(s))
        .transpose()?;
    let start_secs = sec
        .get("startsecs")
        .or_else(|| sec.get("start_secs"))
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!(
                "EventListener '{}' invalid startsecs: {}",
                pool_name, e
            ))
        })?;
    let start_retries = sec
        .get("startretries")
        .or_else(|| sec.get("start_retries"))
        .map(|s| s.parse::<u32>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!(
                "EventListener '{}' invalid startretries: {}",
                pool_name, e
            ))
        })?;
    let stop_signal = sec
        .get("stopsignal")
        .or_else(|| sec.get("stop_signal"))
        .map(|s| parse_stop_signal(s))
        .transpose()?;
    let stop_wait_secs = sec
        .get("stopwaitsecs")
        .or_else(|| sec.get("stop_wait_secs"))
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!(
                "EventListener '{}' invalid stopwaitsecs: {}",
                pool_name, e
            ))
        })?;

    let numprocs = sec
        .get("numprocs")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!(
                "EventListener '{}' invalid numprocs: {}",
                pool_name, e
            ))
        })?;
    let numprocs_start = sec
        .get("numprocs_start")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|e| {
            ProgramError::ConfigError(format!(
                "EventListener '{}' invalid numprocs_start: {}",
                pool_name, e
            ))
        })?;
    let process_name = sec.get("process_name").cloned();

    let directory = sec.get("directory").map(PathBuf::from);
    let user = sec.get("user").cloned();
    let umask = sec.get("umask").map(|s| parse_umask(s)).transpose()?;
    let environment = if let Some(env_str) = sec.get("environment") {
        parse_environment(env_str)?
    } else {
        HashMap::new()
    };
    let stdout_logfile = sec.get("stdout_logfile").and_then(|s| parse_log_path(s));
    let stderr_logfile = sec.get("stderr_logfile").and_then(|s| parse_log_path(s));

    Ok(crate::eventlistener::EventListenerConfigRaw {
        command,
        args: Vec::new(),
        events,
        buffer_size,
        result_handler,
        priority,
        numprocs,
        numprocs_start,
        process_name,
        autostart,
        autorestart,
        start_secs,
        start_retries,
        stop_signal,
        stop_wait_secs,
        directory,
        user,
        environment,
        umask,
        stdout_logfile,
        stderr_logfile,
        redirect_stderr,
    })
}

fn parse_group_config(sec: &HashMap<String, String>) -> Result<GroupConfigRaw, ProgramError> {
    let programs = sec
        .get("programs")
        .map(|s| parse_list(s))
        .unwrap_or_default();
    let priority = sec
        .get("priority")
        .map(|s| s.parse::<u32>())
        .transpose()
        .map_err(|e| ProgramError::ConfigError(format!("Invalid group priority: {}", e)))?;

    Ok(GroupConfigRaw { programs, priority })
}

fn parse_program_defaults(sec: &HashMap<String, String>) -> Result<ProgramDefaults, ProgramError> {
    let autostart = sec
        .get("autostart")
        .map(|s| parse_loose_bool(s))
        .transpose()?;
    let autorestart = sec
        .get("autorestart")
        .map(|s| parse_autorestart(s))
        .transpose()?;
    let start_secs = sec
        .get("startsecs")
        .or_else(|| sec.get("start_secs"))
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|e| ProgramError::ConfigError(format!("Invalid default startsecs: {}", e)))?;
    let start_retries = sec
        .get("startretries")
        .or_else(|| sec.get("start_retries"))
        .map(|s| s.parse::<u32>())
        .transpose()
        .map_err(|e| ProgramError::ConfigError(format!("Invalid default startretries: {}", e)))?;
    let stop_signal = sec
        .get("stopsignal")
        .or_else(|| sec.get("stop_signal"))
        .map(|s| parse_stop_signal(s))
        .transpose()?;
    let stop_wait_secs = sec
        .get("stopwaitsecs")
        .or_else(|| sec.get("stop_wait_secs"))
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|e| ProgramError::ConfigError(format!("Invalid default stopwaitsecs: {}", e)))?;
    let priority = sec
        .get("priority")
        .map(|s| s.parse::<u32>())
        .transpose()
        .map_err(|e| ProgramError::ConfigError(format!("Invalid default priority: {}", e)))?;

    let stdout_path = sec.get("stdout_logfile").and_then(|s| parse_log_path(s));
    let stderr_path = sec.get("stderr_logfile").and_then(|s| parse_log_path(s));
    let max_bytes = sec
        .get("stdout_logfile_maxbytes")
        .or_else(|| sec.get("stderr_logfile_maxbytes"))
        .cloned();
    let backups = sec
        .get("stdout_logfile_backups")
        .or_else(|| sec.get("stderr_logfile_backups"))
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|e| ProgramError::ConfigError(format!("Invalid default backups: {}", e)))?;
    let redirect_stderr = sec
        .get("redirect_stderr")
        .map(|s| parse_loose_bool(s))
        .transpose()?;
    let stdout_events_enabled = sec
        .get("stdout_events_enabled")
        .map(|s| parse_loose_bool(s))
        .transpose()?;
    let stderr_events_enabled = sec
        .get("stderr_events_enabled")
        .map(|s| parse_loose_bool(s))
        .transpose()?;

    let logs = if stdout_path.is_some()
        || stderr_path.is_some()
        || max_bytes.is_some()
        || backups.is_some()
        || redirect_stderr.is_some()
        || stdout_events_enabled.is_some()
        || stderr_events_enabled.is_some()
    {
        Some(ProgramLogsConfigRaw {
            enabled: Some(true),
            stdout: stdout_path,
            stderr: stderr_path,
            max_bytes,
            backups,
            redirect_stderr,
            stdout_events_enabled,
            stderr_events_enabled,
        })
    } else {
        None
    };

    let numprocs = sec
        .get("numprocs")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|e| ProgramError::ConfigError(format!("Invalid default numprocs: {}", e)))?;
    let numprocs_start = sec
        .get("numprocs_start")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|e| ProgramError::ConfigError(format!("Invalid default numprocs_start: {}", e)))?;
    let process_name = sec.get("process_name").cloned();

    Ok(ProgramDefaults {
        autostart,
        autorestart,
        start_secs,
        start_retries,
        stop_signal,
        stop_wait_secs,
        priority,
        logs,
        health_check: None,
        pre_start: sec
            .get("pre_start")
            .or_else(|| sec.get("pre_start_hook"))
            .cloned(),
        pre_stop: sec
            .get("pre_stop")
            .or_else(|| sec.get("pre_stop_hook"))
            .cloned(),
        pre_start_ignore_failure: sec
            .get("pre_start_ignore_failure")
            .map(|s| parse_loose_bool(s))
            .transpose()?,
        hook_timeout_secs: sec
            .get("hook_timeout_secs")
            .map(|s| s.parse::<u64>())
            .transpose()
            .map_err(|e| {
                ProgramError::ConfigError(format!("Invalid default hook_timeout_secs: {}", e))
            })?,
        numprocs,
        numprocs_start,
        process_name,
        restart_when_binary_changed: sec
            .get("restart_when_binary_changed")
            .map(|s| parse_loose_bool(s))
            .transpose()?,
        restart_signal_when_binary_changed: sec
            .get("restart_signal_when_binary_changed")
            .map(|s| parse_stop_signal(s))
            .transpose()?,
        restart_cmd_when_binary_changed: sec.get("restart_cmd_when_binary_changed").cloned(),
        restart_directory_monitor: sec.get("restart_directory_monitor").map(PathBuf::from),
        restart_file_pattern: sec.get("restart_file_pattern").cloned(),
        restart_signal_when_file_changed: sec
            .get("restart_signal_when_file_changed")
            .map(|s| parse_stop_signal(s))
            .transpose()?,
        restart_cmd_when_file_changed: sec.get("restart_cmd_when_file_changed").cloned(),
        restart_debounce_secs: sec
            .get("restart_debounce_secs")
            .map(|s| s.parse::<u64>())
            .transpose()
            .map_err(|e| {
                ProgramError::ConfigError(format!("Invalid default restart_debounce_secs: {}", e))
            })?,
    })
}
