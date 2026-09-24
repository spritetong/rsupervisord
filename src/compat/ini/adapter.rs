// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::compat::ini::parser::ParsedIni;
use crate::compat::ini::values::{
    parse_autorestart, parse_environment, parse_log_path, parse_stop_signal,
};
use crate::config::schema::{
    CtlConfig, GroupConfigRaw, ProgramConfigRaw, ProgramDefaults, ProgramLogsConfigRaw,
    SupervisorConfig,
};
use crate::consts::*;
use crate::error::ProgramError;
use crate::program::config::{HealthCheckConfig, HealthCheckType};
use crate::serde_util::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn parse_opt_duration(
    map: &HashMap<String, String>,
    keys: &[&str],
) -> Result<Option<Duration>, ProgramError> {
    let raw = keys.iter().find_map(|k| map.get(*k));
    raw.map(|s| string_to_duration(s)).transpose()
}

fn parse_opt_bytesize(
    map: &HashMap<String, String>,
    keys: &[&str],
) -> Result<Option<usize>, ProgramError> {
    keys.iter()
        .find_map(|k| map.get(*k))
        .map(|s| string_to_bytes(s))
        .transpose()
}

/// Parses a comma-separated list of env file paths (`envFiles`).
/// INI keys are lowercased by the parser, so lookup uses `envfiles`.
fn parse_env_files(sec: &HashMap<String, String>) -> Option<Vec<PathBuf>> {
    sec.get("envfiles")
        .or_else(|| sec.get("env_files"))
        .map(|s| {
            string_to_str_list(s)
                .into_iter()
                .map(PathBuf::from)
                .collect()
        })
}

/// Absolutizes env file paths against `config_dir` when relative (OI-2).
fn resolve_env_files(files: Vec<PathBuf>, config_dir: Option<&Path>) -> Vec<PathBuf> {
    files
        .into_iter()
        .map(|p| {
            if p.is_absolute() {
                p
            } else if let Some(dir) = config_dir {
                crate::platform::abs_path(&dir.join(p))
            } else {
                crate::platform::abs_path(&p)
            }
        })
        .collect()
}

/// Maps go `liveness_check_*` keys onto [`HealthCheckConfig`] (OI-5).
fn parse_liveness_check(
    sec: &HashMap<String, String>,
    context: &str,
) -> Result<Option<HealthCheckConfig>, ProgramError> {
    let script = sec.get("liveness_check_script").map(|s| s.trim());
    let has_any = sec.keys().any(|k| k.starts_with("liveness_check"));
    if !has_any {
        return Ok(None);
    }
    let Some(script) = script.filter(|s| !s.is_empty()) else {
        return Err(ProgramError::ConfigError(format!(
            "{}: liveness_check_* keys require non-empty 'liveness_check_script'",
            context
        )));
    };

    let interval = match sec.get("liveness_check_period") {
        Some(s) => string_to_duration(s).map_err(|e| {
            ProgramError::ConfigError(format!("{}: invalid liveness_check_period: {}", context, e))
        })?,
        None => DEFAULT_LIVENESS_INTERVAL,
    };
    let timeout = match sec.get("liveness_check_timeout") {
        Some(s) => string_to_duration(s).map_err(|e| {
            ProgramError::ConfigError(format!(
                "{}: invalid liveness_check_timeout: {}",
                context, e
            ))
        })?,
        None => DEFAULT_LIVENESS_TIMEOUT,
    };
    let initial_delay = match sec.get("liveness_check_initial_delay") {
        Some(s) => string_to_duration(s).map_err(|e| {
            ProgramError::ConfigError(format!(
                "{}: invalid liveness_check_initial_delay: {}",
                context, e
            ))
        })?,
        None => DEFAULT_LIVENESS_INITIAL_DELAY,
    };
    let failure_threshold = match sec.get("liveness_check_failure_threshold") {
        Some(s) => s.parse::<u32>().map_err(|e| {
            ProgramError::ConfigError(format!(
                "{}: invalid liveness_check_failure_threshold: {}",
                context, e
            ))
        })?,
        None => DEFAULT_HEALTH_FAILURE_THRESHOLD,
    };

    // Only `restart` failure action is supported; empty success action is fine.
    if let Some(action) = sec.get("liveness_check_failure_action")
        && !action.trim().is_empty()
        && !action.trim().eq_ignore_ascii_case("restart")
    {
        tracing::warn!(
            section = context,
            action = %action,
            "unsupported liveness_check_failure_action; only 'restart' is mapped"
        );
    }
    if let Some(action) = sec.get("liveness_check_success_action")
        && !action.trim().is_empty()
    {
        tracing::warn!(
            section = context,
            action = %action,
            "liveness_check_success_action is not supported and is ignored"
        );
    }
    if let Some(th) = sec.get("liveness_check_success_threshold")
        && th.trim() != "1"
    {
        tracing::warn!(
            section = context,
            threshold = %th,
            "liveness_check_success_threshold other than 1 is ignored"
        );
    }

    Ok(Some(HealthCheckConfig {
        check_type: HealthCheckType::Exec {
            command: script.to_string(),
        },
        interval_secs: interval,
        timeout_secs: timeout,
        failure_threshold,
        initial_delay_secs: initial_delay,
    }))
}

/// Warns about keys under a known section that were not consumed (OI-11).
fn warn_unknown_keys(section: &str, sec: &HashMap<String, String>, known: &[&str]) {
    for key in sec.keys() {
        if !known.contains(&key.as_str()) {
            tracing::warn!(
                section = section,
                key = %key,
                "unknown INI key ignored"
            );
        }
    }
}

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
            config.server.uds_chmod = Some(string_to_chmod(chmod)?);
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
            config.logging.max_bytes = Some(string_to_bytes(maxbytes)?);
        }
        if let Some(backups_str) = sec.get("logfile_backups") {
            // OI-4: hard error on parse failure (was silently dropped).
            config.logging.backups = backups_str.parse::<usize>().map_err(|e| {
                ProgramError::ConfigError(format!(
                    "[supervisord] invalid logfile_backups '{}': {}",
                    backups_str, e
                ))
            })?;
        }
        if let Some(level) = sec.get("loglevel") {
            config.logging.level = level.to_ascii_lowercase();
        }
        if let Some(ident) = sec.get("identifier") {
            config.server.identifier = Some(ident.clone());
        }
        // OI-4: accept Python-style booleans (yes/no/1/0/on/off).
        if let Some(nodaemon) = sec.get("nodaemon") {
            config.nodaemon = string_to_bool(nodaemon)?;
        }
        // OI-4: `silent` suppresses console logging (file layer still applies).
        if let Some(silent) = sec.get("silent") {
            config.logging.silent = string_to_bool(silent)?;
        }
        // OI-6: store daemon environment; applied at daemon startup, never at parse time.
        if let Some(env_str) = sec.get("environment") {
            config.environment = parse_environment(env_str)?;
        }
        // OI-8: runtime surface fields.
        if let Some(pidfile) = sec.get("pidfile") {
            config.pidfile = Some(PathBuf::from(pidfile));
        }
        if let Some(minfds) = sec.get("minfds") {
            config.minfds = Some(minfds.parse::<u32>().map_err(|e| {
                ProgramError::ConfigError(format!(
                    "[supervisord] invalid minfds '{}': {}",
                    minfds, e
                ))
            })?);
        }
        if let Some(minprocs) = sec.get("minprocs") {
            config.minprocs = Some(minprocs.parse::<u32>().map_err(|e| {
                ProgramError::ConfigError(format!(
                    "[supervisord] invalid minprocs '{}': {}",
                    minprocs, e
                ))
            })?);
        }

        warn_unknown_keys(
            "supervisord",
            sec,
            &[
                "logfile",
                "logfile_maxbytes",
                "logfile_backups",
                "loglevel",
                "identifier",
                "nodaemon",
                "silent",
                "environment",
                "pidfile",
                "minfds",
                "minprocs",
                // Python keys accepted but not yet mapped (documented).
                "umask",
                "directory",
                "childlogdir",
            ],
        );
    }

    // 3b. Process [supervisorctl] → client config (OI-1 / Python parity).
    // Section presence maps to Some(ctl); missing section stays None so
    // supervisorctl can hard-error (Python requires the section).
    if let Some(sec) = ini.sections.get("supervisorctl") {
        let mut ctl = CtlConfig::default();
        if let Some(v) = sec.get("serverurl") {
            ctl.serverurl = Some(v.clone());
        }
        if let Some(v) = sec.get("username") {
            ctl.username = Some(v.clone());
        }
        if let Some(v) = sec.get("password") {
            ctl.password = Some(v.clone());
        }
        if let Some(v) = sec.get("auth_token") {
            ctl.auth_token = Some(v.clone());
        }
        config.ctl = Some(ctl);
        warn_unknown_keys(
            "supervisorctl",
            sec,
            &[
                "serverurl",
                "username",
                "password",
                "auth_token",
                // Known Python keys we do not map yet.
                "prompt",
                "history_file",
            ],
        );
    }

    warn_unknown_keys(
        "unix_http_server",
        ini.sections
            .get("unix_http_server")
            .unwrap_or(&HashMap::new()),
        &["file", "chmod", "username", "password"],
    );
    warn_unknown_keys(
        "inet_http_server",
        ini.sections
            .get("inet_http_server")
            .unwrap_or(&HashMap::new()),
        &["port", "username", "password"],
    );

    // 4. Process [program-default]
    if let Some(sec) = ini.sections.get("program-default") {
        let mut defaults = parse_program_defaults(sec)?;
        if let Some(files) = parse_env_files(sec) {
            defaults.env_files = Some(resolve_env_files(files, config_dir));
        }
        config.program_defaults = defaults;
    }

    // 5. Process sections
    for section_name in &ini.section_order {
        if let Some(prog_name) = section_name.strip_prefix("program:") {
            if let Some(sec) = ini.sections.get(section_name) {
                let mut prog = parse_program_config(prog_name, sec)?;
                if let Some(files) = prog.env_files.take() {
                    prog.env_files = Some(resolve_env_files(files, config_dir));
                }
                config.programs.insert(prog_name.to_string(), prog);
            }
        } else if let Some(group_name) = section_name.strip_prefix("group:") {
            if let Some(sec) = ini.sections.get(section_name) {
                let group = parse_group_config(sec)?;
                config.groups.insert(group_name.to_string(), group);
            }
        } else if section_name.starts_with("rpcinterface:")
            || section_name == "include"
            || section_name == "unix_http_server"
            || section_name == "inet_http_server"
            || section_name == "supervisord"
            || section_name == "supervisorctl"
            || section_name == "program-default"
        {
            // Already handled or standard ignored section
        } else if let Some(pool_name) = section_name.strip_prefix("eventlistener:") {
            if let Some(sec) = ini.sections.get(section_name) {
                let mut el_cfg = parse_event_listener_config(pool_name, sec)?;
                if let Some(files) = el_cfg.env_files.take() {
                    el_cfg.env_files = Some(resolve_env_files(files, config_dir));
                }
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

    // Align INI frontend with Python supervisor / go-supervisord baselines:
    // - path_translation=false: bare relative paths stay relative (resolved against
    //   the daemon working directory at runtime), matching Python/go behavior.
    // - allow_unelevated=true: no elevation gate on IPC, matching Python/go which
    //   only rely on socket file permissions.
    // - ctl_defaults=false: no server→ctl backfill; missing [supervisorctl] stays
    //   None so supervisorctl hard-errors like Python (options.py).
    config.server.path_translation = false;
    config.server.allow_unelevated = true;
    config.server.ctl_defaults = false;

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
        .map(|s| string_to_bool(s))
        .transpose()?;
    let autorestart = sec
        .get("autorestart")
        .map(|s| parse_autorestart(s))
        .transpose()?;

    let start_secs = parse_opt_duration(sec, &["startsecs", "start_secs"]).map_err(|e| {
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
        .map(|s| string_to_i32_list(s))
        .transpose()?;

    let stop_signal = sec
        .get("stopsignal")
        .or_else(|| sec.get("stop_signal"))
        .map(|s| parse_stop_signal(s))
        .transpose()?;

    let stop_wait_secs =
        parse_opt_duration(sec, &["stopwaitsecs", "stop_wait_secs"]).map_err(|e| {
            ProgramError::ConfigError(format!(
                "Program '{}' invalid stopwaitsecs: {}",
                prog_name, e
            ))
        })?;

    let directory = sec.get("directory").map(PathBuf::from);
    let user = sec.get("user").cloned();
    let umask = sec.get("umask").map(|s| string_to_umask(s)).transpose()?;

    let environment = if let Some(env_str) = sec.get("environment") {
        parse_environment(env_str)?
    } else {
        HashMap::new()
    };

    let redirect_stderr = sec
        .get("redirect_stderr")
        .map(|s| string_to_bool(s))
        .transpose()?;

    // Build log configuration
    let stdout_path = sec.get("stdout_logfile").and_then(|s| parse_log_path(s));
    let stderr_path = sec.get("stderr_logfile").and_then(|s| parse_log_path(s));

    let max_bytes =
        parse_opt_bytesize(sec, &["stdout_logfile_maxbytes", "stderr_logfile_maxbytes"])?;

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
        .map(|s| string_to_bool(s))
        .transpose()?;
    let stderr_events_enabled = sec
        .get("stderr_events_enabled")
        .map(|s| string_to_bool(s))
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
        .map(|s| string_to_str_list(s))
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
        .map(|s| string_to_bool(s))
        .transpose()?;
    let hook_timeout_secs = parse_opt_duration(sec, &["hook_timeout_secs"]).map_err(|e| {
        ProgramError::ConfigError(format!(
            "Program '{}' invalid hook_timeout_secs: {}",
            prog_name, e
        ))
    })?;

    let restart_when_binary_changed = sec
        .get("restart_when_binary_changed")
        .map(|s| string_to_bool(s))
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
    let restart_debounce_secs =
        parse_opt_duration(sec, &["restart_debounce_secs"]).map_err(|e| {
            ProgramError::ConfigError(format!(
                "Program '{}' invalid restart_debounce_secs: {}",
                prog_name, e
            ))
        })?;

    // OI-2 / OI-3 / OI-9: go-parity extension keys.
    let env_files = parse_env_files(sec);
    let kill_wait_secs =
        parse_opt_duration(sec, &["killwaitsecs", "kill_wait_secs"]).map_err(|e| {
            ProgramError::ConfigError(format!(
                "Program '{}' invalid killwaitsecs: {}",
                prog_name, e
            ))
        })?;
    let stop_as_group = sec
        .get("stopasgroup")
        .or_else(|| sec.get("stop_as_group"))
        .map(|s| string_to_bool(s))
        .transpose()?;
    let kill_as_group = sec
        .get("killasgroup")
        .or_else(|| sec.get("kill_as_group"))
        .map(|s| string_to_bool(s))
        .transpose()?;
    // OI-5: map liveness_check_* onto health_check.
    let health_check = parse_liveness_check(sec, &format!("[program:{}]", prog_name))?;

    // OI-11: warn about unconsumed keys under this program section.
    let known: &[&str] = &[
        "command",
        "process_name",
        "numprocs",
        "numprocs_start",
        "priority",
        "autostart",
        "autorestart",
        "startsecs",
        "start_secs",
        "startretries",
        "start_retries",
        "exitcodes",
        "exit_codes",
        "stopsignal",
        "stop_signal",
        "stopwaitsecs",
        "stop_wait_secs",
        "directory",
        "user",
        "umask",
        "environment",
        "redirect_stderr",
        "stdout_logfile",
        "stderr_logfile",
        "stdout_logfile_maxbytes",
        "stderr_logfile_maxbytes",
        "stdout_logfile_backups",
        "stderr_logfile_backups",
        "stdout_events_enabled",
        "stderr_events_enabled",
        "depends_on",
        "cron",
        "cron_stop",
        "stop_cron",
        "pre_start",
        "pre_start_hook",
        "pre_stop",
        "pre_stop_hook",
        "pre_start_ignore_failure",
        "hook_timeout_secs",
        "restart_when_binary_changed",
        "restart_signal_when_binary_changed",
        "restart_cmd_when_binary_changed",
        "restart_directory_monitor",
        "restart_file_pattern",
        "restart_signal_when_file_changed",
        "restart_cmd_when_file_changed",
        "restart_debounce_secs",
        // INI keys are lowercased by the parser.
        "envfiles",
        "env_files",
        "killwaitsecs",
        "kill_wait_secs",
        "stopasgroup",
        "stop_as_group",
        "killasgroup",
        "kill_as_group",
        // liveness_check_* (validated by parse_liveness_check)
        "liveness_check_script",
        "liveness_check_period",
        "liveness_check_timeout",
        "liveness_check_initial_delay",
        "liveness_check_success_threshold",
        "liveness_check_success_action",
        "liveness_check_failure_threshold",
        "liveness_check_failure_action",
        // Python keys accepted without mapping today.
        "stdout_logfile_bytes",
        "stderr_logfile_bytes",
        "stdout_syslog",
        "stderr_syslog",
        "serverurl",
        "environment_set",
    ];
    // Prefix-check liveness keys already listed; still allow any liveness_check_*.
    for key in sec.keys() {
        if !known.contains(&key.as_str()) && !key.starts_with("liveness_check") {
            tracing::warn!(
                section = %format!("[program:{}]", prog_name),
                key = %key,
                "unknown INI key ignored"
            );
        }
    }

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
        health_check,
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
        env_files,
        kill_wait_secs,
        stop_as_group,
        kill_as_group,
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
        .map(|s| string_to_bool(s))
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
        .map(|s| string_to_bool(s))
        .transpose()?;
    let autorestart = sec
        .get("autorestart")
        .map(|s| parse_autorestart(s))
        .transpose()?;
    let start_secs = parse_opt_duration(sec, &["startsecs", "start_secs"]).map_err(|e| {
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
    let stop_wait_secs =
        parse_opt_duration(sec, &["stopwaitsecs", "stop_wait_secs"]).map_err(|e| {
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
    let umask = sec.get("umask").map(|s| string_to_umask(s)).transpose()?;
    let environment = if let Some(env_str) = sec.get("environment") {
        parse_environment(env_str)?
    } else {
        HashMap::new()
    };
    let stdout_logfile = sec.get("stdout_logfile").and_then(|s| parse_log_path(s));
    let stderr_logfile = sec.get("stderr_logfile").and_then(|s| parse_log_path(s));
    let env_files = parse_env_files(sec);
    let stop_as_group = sec
        .get("stopasgroup")
        .or_else(|| sec.get("stop_as_group"))
        .map(|s| string_to_bool(s))
        .transpose()?;
    let kill_as_group = sec
        .get("killasgroup")
        .or_else(|| sec.get("kill_as_group"))
        .map(|s| string_to_bool(s))
        .transpose()?;

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
        env_files,
        stop_as_group,
        kill_as_group,
    })
}

fn parse_group_config(sec: &HashMap<String, String>) -> Result<GroupConfigRaw, ProgramError> {
    let programs = sec
        .get("programs")
        .map(|s| string_to_str_list(s))
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
        .map(|s| string_to_bool(s))
        .transpose()?;
    let autorestart = sec
        .get("autorestart")
        .map(|s| parse_autorestart(s))
        .transpose()?;
    let start_secs = parse_opt_duration(sec, &["startsecs", "start_secs"])
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
    let stop_wait_secs = parse_opt_duration(sec, &["stopwaitsecs", "stop_wait_secs"])
        .map_err(|e| ProgramError::ConfigError(format!("Invalid default stopwaitsecs: {}", e)))?;
    let priority = sec
        .get("priority")
        .map(|s| s.parse::<u32>())
        .transpose()
        .map_err(|e| ProgramError::ConfigError(format!("Invalid default priority: {}", e)))?;

    let stdout_path = sec.get("stdout_logfile").and_then(|s| parse_log_path(s));
    let stderr_path = sec.get("stderr_logfile").and_then(|s| parse_log_path(s));
    let max_bytes =
        parse_opt_bytesize(sec, &["stdout_logfile_maxbytes", "stderr_logfile_maxbytes"])?;
    let backups = sec
        .get("stdout_logfile_backups")
        .or_else(|| sec.get("stderr_logfile_backups"))
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|e| ProgramError::ConfigError(format!("Invalid default backups: {}", e)))?;
    let redirect_stderr = sec
        .get("redirect_stderr")
        .map(|s| string_to_bool(s))
        .transpose()?;
    let stdout_events_enabled = sec
        .get("stdout_events_enabled")
        .map(|s| string_to_bool(s))
        .transpose()?;
    let stderr_events_enabled = sec
        .get("stderr_events_enabled")
        .map(|s| string_to_bool(s))
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
        health_check: parse_liveness_check(sec, "[program-default]")?,
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
            .map(|s| string_to_bool(s))
            .transpose()?,
        hook_timeout_secs: parse_opt_duration(sec, &["hook_timeout_secs"]).map_err(|e| {
            ProgramError::ConfigError(format!("Invalid default hook_timeout_secs: {}", e))
        })?,
        numprocs,
        numprocs_start,
        process_name,
        restart_when_binary_changed: sec
            .get("restart_when_binary_changed")
            .map(|s| string_to_bool(s))
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
        restart_debounce_secs: parse_opt_duration(sec, &["restart_debounce_secs"]).map_err(
            |e| ProgramError::ConfigError(format!("Invalid default restart_debounce_secs: {}", e)),
        )?,
        env_files: parse_env_files(sec),
        kill_wait_secs: parse_opt_duration(sec, &["killwaitsecs", "kill_wait_secs"]).map_err(
            |e| ProgramError::ConfigError(format!("Invalid default killwaitsecs: {}", e)),
        )?,
        stop_as_group: sec
            .get("stopasgroup")
            .or_else(|| sec.get("stop_as_group"))
            .map(|s| string_to_bool(s))
            .transpose()?,
        kill_as_group: sec
            .get("killasgroup")
            .or_else(|| sec.get("kill_as_group"))
            .map(|s| string_to_bool(s))
            .transpose()?,
    })
}
