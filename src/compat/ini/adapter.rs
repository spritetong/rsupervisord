// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::compat::ini::parser::ParsedIni;
use crate::compat::ini::values::{
    parse_autorestart, parse_environment, parse_log_path, parse_stop_signal,
};
use crate::config::schema::SupervisorConfig;
use crate::consts::*;
use crate::error::ProgramError;
use crate::program::config::{HealthCheckConfig, HealthCheckType};
use crate::serde_util::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Transformation strategy for converting INI string values into JSON AST types.
#[derive(Clone, Copy)]
pub enum Transform {
    /// String preserved as JSON string.
    Identity,
    /// String trimmed and lowercased into JSON string.
    Lowercase,
    /// Boolean values (Python/go parity: true/false/yes/no/1/0/on/off) into JSON bool.
    Bool,
    /// Unsigned integer into JSON number.
    Usize,
    /// 32-bit unsigned integer into JSON number.
    U32,
    /// 32-bit signed integer into JSON number.
    I32,
    /// Duration in seconds into JSON number.
    DurationSecs,
    /// Human-readable byte size into JSON number of bytes.
    ByteSize,
    /// Octal permissions mode (e.g. 0755, 755) into JSON number.
    Chmod,
    /// Octal umask (e.g. 022, 22) into JSON number.
    Umask,
    /// Comma- or whitespace-separated list of strings into JSON array.
    StringList,
    /// Comma- or whitespace-separated list of event names into validated uppercase JSON array.
    EventList,
    /// Comma-separated list of 32-bit signed integers into JSON array.
    I32List,
    /// Python INI environment variable string into JSON object map.
    Environment,
    /// Supervisor log path normalization: AUTO/empty -> None, NONE/OFF/NULL//dev/null -> "/dev/null".
    LogPath,
    /// Process stop signal name into serialized JSON string.
    StopSignal,
    /// Process auto-restart policy into serialized JSON string.
    AutoRestart,
    /// Comma-separated list of environment file paths, absolutized against config_dir if relative.
    EnvFiles,
    /// HTTP bind address normalization (e.g. :9001, 9001 -> 0.0.0.0:9001).
    HttpBind,
}

impl Transform {
    fn apply(
        &self,
        raw: &str,
        key: &str,
        context: &str,
        config_dir: Option<&Path>,
    ) -> Result<Option<serde_json::Value>, ProgramError> {
        match self {
            Self::Identity => Ok(Some(serde_json::Value::String(raw.to_string()))),
            Self::Lowercase => Ok(Some(serde_json::Value::String(
                raw.trim().to_ascii_lowercase(),
            ))),
            Self::Bool => {
                let b = string_to_bool(raw).map_err(|e| {
                    ProgramError::ConfigError(format!("{}: invalid {}: {}", context, key, e))
                })?;
                Ok(Some(serde_json::Value::Bool(b)))
            }
            Self::Usize => {
                let n = raw.trim().parse::<usize>().map_err(|e| {
                    ProgramError::ConfigError(format!("{}: invalid {}: {}", context, key, e))
                })?;
                Ok(Some(serde_json::json!(n)))
            }
            Self::U32 => {
                let n = raw.trim().parse::<u32>().map_err(|e| {
                    ProgramError::ConfigError(format!("{}: invalid {}: {}", context, key, e))
                })?;
                Ok(Some(serde_json::json!(n)))
            }
            Self::I32 => {
                let n = raw.trim().parse::<i32>().map_err(|e| {
                    ProgramError::ConfigError(format!("{}: invalid {}: {}", context, key, e))
                })?;
                Ok(Some(serde_json::json!(n)))
            }
            Self::DurationSecs => {
                let d = string_to_duration(raw).map_err(|e| {
                    ProgramError::ConfigError(format!("{}: invalid {}: {}", context, key, e))
                })?;
                Ok(Some(serde_json::json!(d.as_secs())))
            }
            Self::ByteSize => {
                let bytes = string_to_bytes(raw).map_err(|e| {
                    ProgramError::ConfigError(format!("{}: invalid {}: {}", context, key, e))
                })?;
                Ok(Some(serde_json::json!(bytes)))
            }
            Self::Chmod => {
                let mode = string_to_chmod(raw).map_err(|e| {
                    ProgramError::ConfigError(format!("{}: invalid {}: {}", context, key, e))
                })?;
                Ok(Some(serde_json::json!(mode)))
            }
            Self::Umask => {
                let mask = string_to_umask(raw).map_err(|e| {
                    ProgramError::ConfigError(format!("{}: invalid {}: {}", context, key, e))
                })?;
                Ok(Some(serde_json::json!(mask)))
            }
            Self::StringList => {
                let list = string_to_str_list(raw);
                Ok(Some(serde_json::json!(list)))
            }
            Self::EventList => {
                let events: Vec<String> = raw
                    .split(&[',', ' ', '\t'][..])
                    .map(|s| s.trim().to_ascii_uppercase())
                    .filter(|s| !s.is_empty())
                    .collect();
                crate::eventlistener::validate_event_list(&events)?;
                Ok(Some(serde_json::json!(events)))
            }
            Self::I32List => {
                let list = string_to_i32_list(raw).map_err(|e| {
                    ProgramError::ConfigError(format!("{}: invalid {}: {}", context, key, e))
                })?;
                Ok(Some(serde_json::json!(list)))
            }
            Self::Environment => {
                let map = parse_environment(raw)?;
                Ok(Some(serde_json::json!(map)))
            }
            Self::LogPath => {
                let opt = parse_log_path(raw);
                Ok(opt.map(|p| serde_json::Value::String(p.to_string_lossy().into_owned())))
            }
            Self::StopSignal => {
                let sig = parse_stop_signal(raw)?;
                let val = serde_json::to_value(sig).map_err(|e| {
                    ProgramError::ConfigError(format!("{}: invalid {}: {}", context, key, e))
                })?;
                Ok(Some(val))
            }
            Self::AutoRestart => {
                let policy = parse_autorestart(raw)?;
                let val = serde_json::to_value(policy).map_err(|e| {
                    ProgramError::ConfigError(format!("{}: invalid {}: {}", context, key, e))
                })?;
                Ok(Some(val))
            }
            Self::EnvFiles => {
                let files: Vec<PathBuf> = string_to_str_list(raw)
                    .into_iter()
                    .map(PathBuf::from)
                    .collect();
                let resolved = resolve_env_files(files, config_dir);
                let paths: Vec<String> = resolved
                    .into_iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                Ok(Some(serde_json::json!(paths)))
            }
            Self::HttpBind => {
                let norm = normalize_http_bind(raw);
                Ok(Some(serde_json::Value::String(norm)))
            }
        }
    }
}

/// Declarative mapping between INI source keys (with aliases) and target JSON AST paths.
#[derive(Clone, Copy)]
pub struct FieldMapping {
    pub src_keys: &'static [&'static str],
    pub dst_path: &'static str,
    pub transform: Transform,
}

/// Sets a value in a JSON AST object at a dot-separated path (e.g. "logs.stdout").
/// Intermediate objects are created automatically; "logs" defaults to `{ "enabled": true }`.
fn set_json_path(root: &mut serde_json::Value, path: &str, val: serde_json::Value) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = root;
    for &part in &parts[..parts.len() - 1] {
        if !current[part].is_object() {
            if part == "logs" {
                current[part] = serde_json::json!({ "enabled": true });
            } else {
                current[part] = serde_json::json!({});
            }
        }
        current = &mut current[part];
    }
    if let Some(&last) = parts.last() {
        current[last] = val;
    }
}

/// Applies a table of declarative field mappings onto a target JSON AST value.
fn apply_mappings(
    sec: &HashMap<String, String>,
    mappings: &[FieldMapping],
    target: &mut serde_json::Value,
    context: &str,
    config_dir: Option<&Path>,
) -> Result<(), ProgramError> {
    for m in mappings {
        if let Some((k, raw)) = m.src_keys.iter().find_map(|&k| sec.get(k).map(|v| (k, v)))
            && let Some(val) = m.transform.apply(raw, k, context, config_dir)?
        {
            set_json_path(target, m.dst_path, val);
        }
    }
    Ok(())
}

/// Warns about keys in `sec` that are not consumed by any mapping or extra known list (OI-11).
fn warn_unconsumed_keys_multi(
    section: &str,
    sec: &HashMap<String, String>,
    mapping_groups: &[&[FieldMapping]],
    extra_known: &[&str],
) {
    for key in sec.keys() {
        let is_mapped = mapping_groups
            .iter()
            .any(|mappings| mappings.iter().any(|m| m.src_keys.contains(&key.as_str())));
        if !is_mapped && !extra_known.contains(&key.as_str()) {
            tracing::warn!(
                section = section,
                key = %key,
                "unknown INI key ignored"
            );
        }
    }
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

// ---------------------------------------------------------------------------
// Declarative Mapping Tables
// ---------------------------------------------------------------------------

const SERVER_UNIX_MAPPINGS: &[FieldMapping] = &[
    FieldMapping {
        src_keys: &["file"],
        dst_path: "server.uds_path",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["chmod"],
        dst_path: "server.uds_chmod",
        transform: Transform::Chmod,
    },
    FieldMapping {
        src_keys: &["username"],
        dst_path: "server.uds_username",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["password"],
        dst_path: "server.uds_password",
        transform: Transform::Identity,
    },
];

const SERVER_INET_MAPPINGS: &[FieldMapping] = &[
    FieldMapping {
        src_keys: &["port"],
        dst_path: "server.http_bind",
        transform: Transform::HttpBind,
    },
    FieldMapping {
        src_keys: &["username"],
        dst_path: "server.username",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["password"],
        dst_path: "server.password",
        transform: Transform::Identity,
    },
];

const SUPERVISORD_MAPPINGS: &[FieldMapping] = &[
    FieldMapping {
        src_keys: &["logfile_maxbytes"],
        dst_path: "logging.max_bytes",
        transform: Transform::ByteSize,
    },
    FieldMapping {
        src_keys: &["logfile_backups"],
        dst_path: "logging.backups",
        transform: Transform::Usize,
    },
    FieldMapping {
        src_keys: &["loglevel"],
        dst_path: "logging.level",
        transform: Transform::Lowercase,
    },
    FieldMapping {
        src_keys: &["silent"],
        dst_path: "logging.silent",
        transform: Transform::Bool,
    },
    FieldMapping {
        src_keys: &["identifier"],
        dst_path: "server.identifier",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["nodaemon"],
        dst_path: "nodaemon",
        transform: Transform::Bool,
    },
    FieldMapping {
        src_keys: &["environment"],
        dst_path: "environment",
        transform: Transform::Environment,
    },
    FieldMapping {
        src_keys: &["pidfile"],
        dst_path: "pidfile",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["minfds"],
        dst_path: "minfds",
        transform: Transform::U32,
    },
    FieldMapping {
        src_keys: &["minprocs"],
        dst_path: "minprocs",
        transform: Transform::U32,
    },
];

const CTL_MAPPINGS: &[FieldMapping] = &[
    FieldMapping {
        src_keys: &["serverurl"],
        dst_path: "serverurl",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["username"],
        dst_path: "username",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["password"],
        dst_path: "password",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["auth_token"],
        dst_path: "auth_token",
        transform: Transform::Identity,
    },
];

const GROUP_MAPPINGS: &[FieldMapping] = &[
    FieldMapping {
        src_keys: &["programs"],
        dst_path: "programs",
        transform: Transform::StringList,
    },
    FieldMapping {
        src_keys: &["priority"],
        dst_path: "priority",
        transform: Transform::U32,
    },
];

/// Shared program mappings used by both `[program:x]` and `[program-default]`.
const PROGRAM_SHARED_MAPPINGS: &[FieldMapping] = &[
    FieldMapping {
        src_keys: &["autostart"],
        dst_path: "autostart",
        transform: Transform::Bool,
    },
    FieldMapping {
        src_keys: &["autorestart"],
        dst_path: "autorestart",
        transform: Transform::AutoRestart,
    },
    FieldMapping {
        src_keys: &["startsecs", "start_secs"],
        dst_path: "start_secs",
        transform: Transform::DurationSecs,
    },
    FieldMapping {
        src_keys: &["startretries", "start_retries"],
        dst_path: "start_retries",
        transform: Transform::U32,
    },
    FieldMapping {
        src_keys: &["restartpause", "restart_pause", "restart_pause_secs"],
        dst_path: "restart_pause_secs",
        transform: Transform::DurationSecs,
    },
    FieldMapping {
        src_keys: &["stopsignal", "stop_signal"],
        dst_path: "stop_signal",
        transform: Transform::StopSignal,
    },
    FieldMapping {
        src_keys: &["stopwaitsecs", "stop_wait_secs"],
        dst_path: "stop_wait_secs",
        transform: Transform::DurationSecs,
    },
    FieldMapping {
        src_keys: &["priority"],
        dst_path: "priority",
        transform: Transform::U32,
    },
    FieldMapping {
        src_keys: &["numprocs"],
        dst_path: "numprocs",
        transform: Transform::Usize,
    },
    FieldMapping {
        src_keys: &["numprocs_start"],
        dst_path: "numprocs_start",
        transform: Transform::Usize,
    },
    FieldMapping {
        src_keys: &["process_name"],
        dst_path: "process_name",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["stdout_logfile"],
        dst_path: "logs.stdout",
        transform: Transform::LogPath,
    },
    FieldMapping {
        src_keys: &["stderr_logfile"],
        dst_path: "logs.stderr",
        transform: Transform::LogPath,
    },
    FieldMapping {
        src_keys: &["stdout_logfile_maxbytes", "stderr_logfile_maxbytes"],
        dst_path: "logs.max_bytes",
        transform: Transform::ByteSize,
    },
    FieldMapping {
        src_keys: &["stdout_logfile_backups", "stderr_logfile_backups"],
        dst_path: "logs.backups",
        transform: Transform::Usize,
    },
    FieldMapping {
        src_keys: &["redirect_stderr"],
        dst_path: "logs.redirect_stderr",
        transform: Transform::Bool,
    },
    FieldMapping {
        src_keys: &["stdout_events_enabled"],
        dst_path: "logs.stdout_events_enabled",
        transform: Transform::Bool,
    },
    FieldMapping {
        src_keys: &["stderr_events_enabled"],
        dst_path: "logs.stderr_events_enabled",
        transform: Transform::Bool,
    },
    FieldMapping {
        src_keys: &["pre_start", "pre_start_hook"],
        dst_path: "pre_start",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["pre_stop", "pre_stop_hook"],
        dst_path: "pre_stop",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["pre_start_ignore_failure"],
        dst_path: "pre_start_ignore_failure",
        transform: Transform::Bool,
    },
    FieldMapping {
        src_keys: &["hook_timeout_secs"],
        dst_path: "hook_timeout_secs",
        transform: Transform::DurationSecs,
    },
    FieldMapping {
        src_keys: &["restart_when_binary_changed"],
        dst_path: "restart_when_binary_changed",
        transform: Transform::Bool,
    },
    FieldMapping {
        src_keys: &["restart_signal_when_binary_changed"],
        dst_path: "restart_signal_when_binary_changed",
        transform: Transform::StopSignal,
    },
    FieldMapping {
        src_keys: &["restart_cmd_when_binary_changed"],
        dst_path: "restart_cmd_when_binary_changed",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["restart_directory_monitor"],
        dst_path: "restart_directory_monitor",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["restart_file_pattern"],
        dst_path: "restart_file_pattern",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["restart_signal_when_file_changed"],
        dst_path: "restart_signal_when_file_changed",
        transform: Transform::StopSignal,
    },
    FieldMapping {
        src_keys: &["restart_cmd_when_file_changed"],
        dst_path: "restart_cmd_when_file_changed",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["restart_debounce_secs"],
        dst_path: "restart_debounce_secs",
        transform: Transform::DurationSecs,
    },
    FieldMapping {
        src_keys: &["envfiles", "env_files"],
        dst_path: "env_files",
        transform: Transform::EnvFiles,
    },
    FieldMapping {
        src_keys: &["killwaitsecs", "kill_wait_secs"],
        dst_path: "kill_wait_secs",
        transform: Transform::DurationSecs,
    },
    FieldMapping {
        src_keys: &["stopasgroup", "stop_as_group"],
        dst_path: "stop_as_group",
        transform: Transform::Bool,
    },
    FieldMapping {
        src_keys: &["killasgroup", "kill_as_group"],
        dst_path: "kill_as_group",
        transform: Transform::Bool,
    },
];

/// Program-only mappings that are not supported on `[program-default]`.
const PROGRAM_ONLY_MAPPINGS: &[FieldMapping] = &[
    FieldMapping {
        src_keys: &["command"],
        dst_path: "command",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["exitcodes", "exit_codes"],
        dst_path: "exit_codes",
        transform: Transform::I32List,
    },
    FieldMapping {
        src_keys: &["directory"],
        dst_path: "directory",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["user"],
        dst_path: "user",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["umask"],
        dst_path: "umask",
        transform: Transform::Umask,
    },
    FieldMapping {
        src_keys: &["environment"],
        dst_path: "environment",
        transform: Transform::Environment,
    },
    FieldMapping {
        src_keys: &["depends_on"],
        dst_path: "depends_on",
        transform: Transform::StringList,
    },
    FieldMapping {
        src_keys: &["cron"],
        dst_path: "cron",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["cron_stop", "stop_cron"],
        dst_path: "cron_stop",
        transform: Transform::Identity,
    },
];

/// Mappings for `[eventlistener:x]` sections.
const EVENT_LISTENER_MAPPINGS: &[FieldMapping] = &[
    FieldMapping {
        src_keys: &["command"],
        dst_path: "command",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["events"],
        dst_path: "events",
        transform: Transform::EventList,
    },
    FieldMapping {
        src_keys: &["buffer_size", "buffersize"],
        dst_path: "buffer_size",
        transform: Transform::Usize,
    },
    FieldMapping {
        src_keys: &["result_handler"],
        dst_path: "result_handler",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["priority"],
        dst_path: "priority",
        transform: Transform::I32,
    },
    FieldMapping {
        src_keys: &["autostart"],
        dst_path: "autostart",
        transform: Transform::Bool,
    },
    FieldMapping {
        src_keys: &["autorestart"],
        dst_path: "autorestart",
        transform: Transform::AutoRestart,
    },
    FieldMapping {
        src_keys: &["startsecs", "start_secs"],
        dst_path: "start_secs",
        transform: Transform::DurationSecs,
    },
    FieldMapping {
        src_keys: &["startretries", "start_retries"],
        dst_path: "start_retries",
        transform: Transform::U32,
    },
    FieldMapping {
        src_keys: &["stopsignal", "stop_signal"],
        dst_path: "stop_signal",
        transform: Transform::StopSignal,
    },
    FieldMapping {
        src_keys: &["stopwaitsecs", "stop_wait_secs"],
        dst_path: "stop_wait_secs",
        transform: Transform::DurationSecs,
    },
    FieldMapping {
        src_keys: &["directory"],
        dst_path: "directory",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["user"],
        dst_path: "user",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["umask"],
        dst_path: "umask",
        transform: Transform::Umask,
    },
    FieldMapping {
        src_keys: &["environment"],
        dst_path: "environment",
        transform: Transform::Environment,
    },
    FieldMapping {
        src_keys: &["stdout_logfile"],
        dst_path: "stdout_logfile",
        transform: Transform::LogPath,
    },
    FieldMapping {
        src_keys: &["stderr_logfile"],
        dst_path: "stderr_logfile",
        transform: Transform::LogPath,
    },
    FieldMapping {
        src_keys: &["redirect_stderr"],
        dst_path: "redirect_stderr",
        transform: Transform::Bool,
    },
    FieldMapping {
        src_keys: &["numprocs"],
        dst_path: "numprocs",
        transform: Transform::Usize,
    },
    FieldMapping {
        src_keys: &["numprocs_start"],
        dst_path: "numprocs_start",
        transform: Transform::Usize,
    },
    FieldMapping {
        src_keys: &["process_name"],
        dst_path: "process_name",
        transform: Transform::Identity,
    },
    FieldMapping {
        src_keys: &["envfiles", "env_files"],
        dst_path: "env_files",
        transform: Transform::EnvFiles,
    },
    FieldMapping {
        src_keys: &["stopasgroup", "stop_as_group"],
        dst_path: "stop_as_group",
        transform: Transform::Bool,
    },
    FieldMapping {
        src_keys: &["killasgroup", "kill_as_group"],
        dst_path: "kill_as_group",
        transform: Transform::Bool,
    },
];

const LIVENESS_CHECK_KEYS: &[&str] = &[
    "liveness_check_script",
    "liveness_check_period",
    "liveness_check_timeout",
    "liveness_check_initial_delay",
    "liveness_check_failure_threshold",
    "liveness_check_failure_action",
    "liveness_check_success_threshold",
    "liveness_check_success_action",
];

// ---------------------------------------------------------------------------
// Main Adapter Pipeline
// ---------------------------------------------------------------------------

/// Adapts a `ParsedIni` into a standard `SupervisorConfig` by translating
/// the INI sections into a structured JSON AST, then deserializing via `serde_json`.
pub fn adapt_ini_to_config(
    ini: &ParsedIni,
    config_dir: Option<&Path>,
) -> Result<SupervisorConfig, ProgramError> {
    let mut root = serde_json::json!({
        "server": {
            "path_translation": false,
            "allow_unelevated": true,
            "ctl_defaults": false,
        },
        "programs": {},
        "groups": {},
        "event_listeners": {},
        "program_defaults": {},
    });

    // 1. Process [unix_http_server]
    if let Some(sec) = ini.sections.get("unix_http_server") {
        apply_mappings(
            sec,
            SERVER_UNIX_MAPPINGS,
            &mut root,
            "[unix_http_server]",
            config_dir,
        )?;
        warn_unconsumed_keys_multi("unix_http_server", sec, &[SERVER_UNIX_MAPPINGS], &[]);
    }

    // 2. Process [inet_http_server]
    if let Some(sec) = ini.sections.get("inet_http_server") {
        apply_mappings(
            sec,
            SERVER_INET_MAPPINGS,
            &mut root,
            "[inet_http_server]",
            config_dir,
        )?;
        warn_unconsumed_keys_multi("inet_http_server", sec, &[SERVER_INET_MAPPINGS], &[]);
    }

    // 3. Process [supervisord]
    if let Some(sec) = ini.sections.get("supervisord") {
        if let Some(logfile) = sec.get("logfile") {
            if logfile.eq_ignore_ascii_case("NONE") {
                set_json_path(&mut root, "logging.enabled", serde_json::json!(false));
                set_json_path(&mut root, "logging.file", serde_json::Value::Null);
            } else {
                set_json_path(&mut root, "logging.enabled", serde_json::json!(true));
                set_json_path(&mut root, "logging.file", serde_json::json!(logfile));
            }
        }
        apply_mappings(
            sec,
            SUPERVISORD_MAPPINGS,
            &mut root,
            "[supervisord]",
            config_dir,
        )?;
        warn_unconsumed_keys_multi(
            "supervisord",
            sec,
            &[SUPERVISORD_MAPPINGS],
            &["logfile", "umask", "directory", "childlogdir"],
        );
    }

    // 3b. Process [supervisorctl] -> client config (OI-1 / Python parity).
    // Section presence maps to Some(ctl); missing section stays None so
    // supervisorctl can hard-error (Python requires the section).
    if let Some(sec) = ini.sections.get("supervisorctl") {
        let mut ctl = serde_json::json!({});
        apply_mappings(sec, CTL_MAPPINGS, &mut ctl, "[supervisorctl]", config_dir)?;
        root["ctl"] = ctl;
        warn_unconsumed_keys_multi(
            "supervisorctl",
            sec,
            &[CTL_MAPPINGS],
            &["prompt", "history_file"],
        );
    }

    // 4. Process [program-default]
    if let Some(sec) = ini.sections.get("program-default") {
        let mut defs = serde_json::json!({});
        apply_mappings(
            sec,
            PROGRAM_SHARED_MAPPINGS,
            &mut defs,
            "[program-default]",
            config_dir,
        )?;
        if let Some(hc) = parse_liveness_check(sec, "[program-default]")? {
            defs["health_check"] =
                serde_json::to_value(&hc).map_err(|e| ProgramError::ConfigError(e.to_string()))?;
        }
        root["program_defaults"] = defs;
        warn_unconsumed_keys_multi(
            "program-default",
            sec,
            &[PROGRAM_SHARED_MAPPINGS],
            LIVENESS_CHECK_KEYS,
        );
    }

    // 5. Process sections
    for section_name in &ini.section_order {
        if let Some(prog_name) = section_name.strip_prefix("program:") {
            if let Some(sec) = ini.sections.get(section_name) {
                if !sec.contains_key("command") {
                    return Err(ProgramError::ConfigError(format!(
                        "Program '{}' missing required 'command' field",
                        prog_name
                    )));
                }
                let context = format!("Program '{}'", prog_name);
                let mut prog = serde_json::json!({});
                apply_mappings(
                    sec,
                    PROGRAM_SHARED_MAPPINGS,
                    &mut prog,
                    &context,
                    config_dir,
                )?;
                apply_mappings(sec, PROGRAM_ONLY_MAPPINGS, &mut prog, &context, config_dir)?;
                if let Some(hc) = parse_liveness_check(sec, &format!("[program:{}]", prog_name))? {
                    prog["health_check"] = serde_json::to_value(&hc)
                        .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                }
                root["programs"][prog_name] = prog;
                warn_unconsumed_keys_multi(
                    &format!("program:{}", prog_name),
                    sec,
                    &[PROGRAM_SHARED_MAPPINGS, PROGRAM_ONLY_MAPPINGS],
                    LIVENESS_CHECK_KEYS,
                );
            }
        } else if let Some(group_name) = section_name.strip_prefix("group:") {
            if let Some(sec) = ini.sections.get(section_name) {
                let context = format!("group:{}", group_name);
                let mut grp = serde_json::json!({});
                apply_mappings(sec, GROUP_MAPPINGS, &mut grp, &context, config_dir)?;
                root["groups"][group_name] = grp;
                warn_unconsumed_keys_multi(&context, sec, &[GROUP_MAPPINGS], &[]);
            }
        } else if let Some(pool_name) = section_name.strip_prefix("eventlistener:") {
            if let Some(sec) = ini.sections.get(section_name) {
                if !sec.contains_key("command") {
                    return Err(ProgramError::ConfigError(format!(
                        "EventListener '{}' missing required 'command' field",
                        pool_name
                    )));
                }
                if !sec.contains_key("events") {
                    return Err(ProgramError::ConfigError(format!(
                        "EventListener '{}' missing required 'events' field",
                        pool_name
                    )));
                }
                let context = format!("EventListener '{}'", pool_name);
                let mut el = serde_json::json!({});
                apply_mappings(sec, EVENT_LISTENER_MAPPINGS, &mut el, &context, config_dir)?;

                if let Some(redirect_stderr) = el.get("redirect_stderr").and_then(|v| v.as_bool())
                    && redirect_stderr
                {
                    return Err(ProgramError::ConfigError(format!(
                        "EventListener '{}' redirect_stderr cannot be true (violates wire protocol)",
                        pool_name
                    )));
                }
                if let Some(buf_size) = el.get("buffer_size").and_then(|v| v.as_u64())
                    && buf_size < 1
                {
                    return Err(ProgramError::ConfigError(format!(
                        "EventListener '{}' buffer_size must be >= 1",
                        pool_name
                    )));
                }

                root["event_listeners"][pool_name] = el;
                warn_unconsumed_keys_multi(
                    &format!("eventlistener:{}", pool_name),
                    sec,
                    &[EVENT_LISTENER_MAPPINGS],
                    &[],
                );
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

    let mut config: SupervisorConfig = serde_json::from_value(root).map_err(|e| {
        ProgramError::ConfigError(format!("Failed to deserialize INI configuration: {}", e))
    })?;

    config.config_dir = config_dir.map(|p| p.to_path_buf());
    config.apply_default_paths();
    config = config.translate_paths()?;
    config.validate()?;

    Ok(config)
}
