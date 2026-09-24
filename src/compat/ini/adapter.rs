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
use crate::program::config::{AutoRestartPolicy, HealthCheckConfig, HealthCheckType};
use crate::serde_util::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Transformation strategy for converting INI string values into JSON AST types.
#[derive(Clone, Copy)]
pub enum Transform {
    Identity,
    Lowercase,
    Bool,
    Usize,
    U32,
    I32,
    DurationSecs,
    ByteSize,
    Chmod,
    Umask,
    StringList,
    EventList,
    I32List,
    Environment,
    LogPath,
    StopSignal,
    AutoRestart,
    EnvFiles,
    HttpBind,
}

macro_rules! transform_value {
    ($context:ident, $key:ident, $value:expr) => {{
        let jsn = $value
            .map_err(|e| ProgramError::ConfigError(format!("{} {}: {}", $context, $key, e)))?;
        Ok(Some(serde_json::json!(jsn)))
    }};
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
            Self::Bool => transform_value!(context, key, string_to_bool(raw)),
            Self::Usize => transform_value!(context, key, raw.trim().parse::<usize>()),
            Self::U32 => transform_value!(context, key, raw.trim().parse::<u32>()),
            Self::I32 => transform_value!(context, key, raw.trim().parse::<i32>()),
            Self::DurationSecs => {
                transform_value!(context, key, string_to_duration(raw).map(|d| d.as_secs()))
            }
            Self::ByteSize => transform_value!(context, key, string_to_bytes(raw)),
            Self::Chmod => transform_value!(context, key, string_to_chmod(raw)),
            Self::Umask => transform_value!(context, key, string_to_umask(raw)),
            Self::StringList => Ok(Some(serde_json::json!(string_to_str_list(raw)))),
            Self::EventList => {
                let events: Vec<String> = raw
                    .split(&[',', ' ', '\t'][..])
                    .map(|s| s.trim().to_ascii_uppercase())
                    .filter(|s| !s.is_empty())
                    .collect();
                crate::eventlistener::validate_event_list(&events)?;
                Ok(Some(serde_json::json!(events)))
            }
            Self::I32List => transform_value!(context, key, string_to_i32_list(raw)),
            Self::Environment => Ok(Some(serde_json::json!(parse_environment(raw)?))),
            Self::LogPath => {
                let opt = parse_log_path(raw);
                Ok(opt.map(|p| serde_json::Value::String(p.to_string_lossy().into_owned())))
            }
            Self::StopSignal => {
                transform_value!(context, key, parse_stop_signal(raw).map(|s| s.to_string()))
            }
            Self::AutoRestart => transform_value!(
                context,
                key,
                parse_autorestart(raw).map(|p| match p {
                    AutoRestartPolicy::Always => "always",
                    AutoRestartPolicy::Unexpected => "unexpected",
                    AutoRestartPolicy::Never => "never",
                })
            ),
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
            Self::HttpBind => Ok(Some(serde_json::Value::String(normalize_http_bind(raw)))),
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

macro_rules! map_field {
    ($key:literal, $transform:ident) => {
        FieldMapping {
            src_keys: &[$key],
            dst_path: $key,
            transform: Transform::$transform,
        }
    };
    ($src_keys:expr, $dst_path:literal, $transform:ident) => {
        FieldMapping {
            src_keys: $src_keys,
            dst_path: $dst_path,
            transform: Transform::$transform,
        }
    };
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

/// Maps one or more field mapping groups into a newly created JSON object.
fn map_section(
    sec: &HashMap<String, String>,
    mapping_groups: &[&[FieldMapping]],
    context: &str,
    config_dir: Option<&Path>,
) -> Result<serde_json::Value, ProgramError> {
    let mut target = serde_json::json!({});
    for mappings in mapping_groups {
        apply_mappings(sec, mappings, &mut target, context, config_dir)?;
    }
    Ok(target)
}

/// Warns about keys in `sec` that are not consumed by any mapping or extra known list (OI-11).
/// `extra_prefixes` silences keys with those prefixes (e.g. `liveness_check` on programs).
fn warn_unconsumed_keys_multi(
    section: &str,
    sec: &HashMap<String, String>,
    mapping_groups: &[&[FieldMapping]],
    extra_known: &[&str],
    extra_prefixes: &[&str],
) {
    for key in sec.keys() {
        let is_mapped = mapping_groups
            .iter()
            .any(|mappings| mappings.iter().any(|m| m.src_keys.contains(&key.as_str())));
        let is_known = extra_known.contains(&key.as_str())
            || extra_prefixes.iter().any(|p| key.starts_with(p));
        if !is_mapped && !is_known {
            tracing::warn!(
                section = section,
                key = %key,
                "unknown INI key ignored"
            );
        }
    }
}

/// Enforces that a required key exists in the section.
fn require_key(
    sec: &HashMap<String, String>,
    key: &str,
    entity: &str,
    name: &str,
) -> Result<(), ProgramError> {
    if !sec.contains_key(key) {
        Err(ProgramError::ConfigError(format!(
            "{} '{}' missing required '{}' field",
            entity, name, key
        )))
    } else {
        Ok(())
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

/// Evaluates and attaches liveness health check configuration if present in the section.
fn attach_liveness_check(
    sec: &HashMap<String, String>,
    target: &mut serde_json::Value,
    context: &str,
) -> Result<(), ProgramError> {
    if let Some(hc) = parse_liveness_check(sec, context)? {
        target["health_check"] =
            serde_json::to_value(&hc).map_err(|e| ProgramError::ConfigError(e.to_string()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Declarative Mapping Tables
// ---------------------------------------------------------------------------

const SERVER_UNIX_MAPPINGS: &[FieldMapping] = &[
    map_field!(&["file"], "server.uds_path", Identity),
    map_field!(&["chmod"], "server.uds_chmod", Chmod),
    map_field!(&["username"], "server.uds_username", Identity),
    map_field!(&["password"], "server.uds_password", Identity),
];

const SERVER_INET_MAPPINGS: &[FieldMapping] = &[
    map_field!(&["port"], "server.http_bind", HttpBind),
    map_field!(&["username"], "server.username", Identity),
    map_field!(&["password"], "server.password", Identity),
];

const SUPERVISORD_MAPPINGS: &[FieldMapping] = &[
    map_field!(&["logfile_maxbytes"], "logging.max_bytes", ByteSize),
    map_field!(&["logfile_backups"], "logging.backups", Usize),
    map_field!(&["loglevel"], "logging.level", Lowercase),
    map_field!(&["silent"], "logging.silent", Bool),
    map_field!(&["identifier"], "server.identifier", Identity),
    map_field!("nodaemon", Bool),
    map_field!("environment", Environment),
    map_field!("pidfile", Identity),
    map_field!("minfds", U32),
    map_field!("minprocs", U32),
];

const CTL_MAPPINGS: &[FieldMapping] = &[
    map_field!("serverurl", Identity),
    map_field!("username", Identity),
    map_field!("password", Identity),
    map_field!("auth_token", Identity),
];

const GROUP_MAPPINGS: &[FieldMapping] = &[
    map_field!("programs", StringList),
    map_field!("priority", U32),
];

/// Shared program mappings used by both `[program:x]` and `[program-default]`.
const PROGRAM_SHARED_MAPPINGS: &[FieldMapping] = &[
    map_field!("autostart", Bool),
    map_field!("autorestart", AutoRestart),
    map_field!(&["startsecs", "start_secs"], "start_secs", DurationSecs),
    map_field!(&["startretries", "start_retries"], "start_retries", U32),
    map_field!(
        &["restartpause", "restart_pause", "restart_pause_secs"],
        "restart_pause_secs",
        DurationSecs
    ),
    map_field!(&["stopsignal", "stop_signal"], "stop_signal", StopSignal),
    map_field!(
        &["stopwaitsecs", "stop_wait_secs"],
        "stop_wait_secs",
        DurationSecs
    ),
    map_field!("priority", U32),
    map_field!("numprocs", Usize),
    map_field!("numprocs_start", Usize),
    map_field!("process_name", Identity),
    map_field!(&["stdout_logfile"], "logs.stdout", LogPath),
    map_field!(&["stderr_logfile"], "logs.stderr", LogPath),
    map_field!(
        &["stdout_logfile_maxbytes", "stderr_logfile_maxbytes"],
        "logs.max_bytes",
        ByteSize
    ),
    map_field!(
        &["stdout_logfile_backups", "stderr_logfile_backups"],
        "logs.backups",
        Usize
    ),
    map_field!(&["redirect_stderr"], "logs.redirect_stderr", Bool),
    map_field!(
        &["stdout_events_enabled"],
        "logs.stdout_events_enabled",
        Bool
    ),
    map_field!(
        &["stderr_events_enabled"],
        "logs.stderr_events_enabled",
        Bool
    ),
    map_field!(&["pre_start", "pre_start_hook"], "pre_start", Identity),
    map_field!(&["pre_stop", "pre_stop_hook"], "pre_stop", Identity),
    map_field!("pre_start_ignore_failure", Bool),
    map_field!(&["hook_timeout_secs"], "hook_timeout_secs", DurationSecs),
    map_field!("restart_when_binary_changed", Bool),
    map_field!("restart_signal_when_binary_changed", StopSignal),
    map_field!("restart_cmd_when_binary_changed", Identity),
    map_field!("restart_directory_monitor", Identity),
    map_field!("restart_file_pattern", Identity),
    map_field!("restart_signal_when_file_changed", StopSignal),
    map_field!("restart_cmd_when_file_changed", Identity),
    map_field!("restart_debounce_secs", DurationSecs),
    map_field!(&["envfiles", "env_files"], "env_files", EnvFiles),
    map_field!(
        &["killwaitsecs", "kill_wait_secs"],
        "kill_wait_secs",
        DurationSecs
    ),
    map_field!(&["stopasgroup", "stop_as_group"], "stop_as_group", Bool),
    map_field!(&["killasgroup", "kill_as_group"], "kill_as_group", Bool),
];

/// Program-only mappings that are not supported on `[program-default]`.
const PROGRAM_ONLY_MAPPINGS: &[FieldMapping] = &[
    map_field!("command", Identity),
    map_field!(&["exitcodes", "exit_codes"], "exit_codes", I32List),
    map_field!("directory", Identity),
    map_field!("user", Identity),
    map_field!("umask", Umask),
    map_field!("environment", Environment),
    map_field!("depends_on", StringList),
    map_field!("cron", Identity),
    map_field!(&["cron_stop", "stop_cron"], "cron_stop", Identity),
];

/// Mappings for `[eventlistener:x]` sections.
const EVENT_LISTENER_MAPPINGS: &[FieldMapping] = &[
    map_field!("command", Identity),
    map_field!("events", EventList),
    map_field!(&["buffer_size", "buffersize"], "buffer_size", Usize),
    map_field!("result_handler", Identity),
    map_field!("priority", I32),
    map_field!("autostart", Bool),
    map_field!("autorestart", AutoRestart),
    map_field!(&["startsecs", "start_secs"], "start_secs", DurationSecs),
    map_field!(&["startretries", "start_retries"], "start_retries", U32),
    map_field!(&["stopsignal", "stop_signal"], "stop_signal", StopSignal),
    map_field!(
        &["stopwaitsecs", "stop_wait_secs"],
        "stop_wait_secs",
        DurationSecs
    ),
    map_field!("directory", Identity),
    map_field!("user", Identity),
    map_field!("umask", Umask),
    map_field!("environment", Environment),
    map_field!("stdout_logfile", LogPath),
    map_field!("stderr_logfile", LogPath),
    map_field!("redirect_stderr", Bool),
    map_field!("numprocs", Usize),
    map_field!("numprocs_start", Usize),
    map_field!("process_name", Identity),
    map_field!(&["envfiles", "env_files"], "env_files", EnvFiles),
    map_field!(&["stopasgroup", "stop_as_group"], "stop_as_group", Bool),
    map_field!(&["killasgroup", "kill_as_group"], "kill_as_group", Bool),
];

/// Python supervisor keys accepted under `[program:*]` / `[program-default]`
/// but not yet mapped (OI-11; silent, matching the pre-table allowlist).
const PYTHON_UNMAPPED_PROGRAM_KEYS: &[&str] = &[
    "environment_set",
    "serverurl",
    "stdout_logfile_bytes",
    "stderr_logfile_bytes",
    "stdout_syslog",
    "stderr_syslog",
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
        warn_unconsumed_keys_multi("unix_http_server", sec, &[SERVER_UNIX_MAPPINGS], &[], &[]);
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
        warn_unconsumed_keys_multi("inet_http_server", sec, &[SERVER_INET_MAPPINGS], &[], &[]);
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
            &[],
        );
    }

    // 3b. Process [supervisorctl] -> client config (OI-1 / Python parity).
    // Section presence maps to Some(ctl); missing section stays None so
    // supervisorctl can hard-error (Python requires the section).
    if let Some(sec) = ini.sections.get("supervisorctl") {
        root["ctl"] = map_section(sec, &[CTL_MAPPINGS], "[supervisorctl]", config_dir)?;
        warn_unconsumed_keys_multi(
            "supervisorctl",
            sec,
            &[CTL_MAPPINGS],
            &["prompt", "history_file"],
            &[],
        );
    }

    // 4. Process [program-default]
    if let Some(sec) = ini.sections.get("program-default") {
        // Error context matches legacy `Invalid default <key>` messages.
        let mut defs = map_section(
            sec,
            &[PROGRAM_SHARED_MAPPINGS],
            "Invalid default",
            config_dir,
        )?;
        attach_liveness_check(sec, &mut defs, "[program-default]")?;
        root["program_defaults"] = defs;
        warn_unconsumed_keys_multi(
            "program-default",
            sec,
            &[PROGRAM_SHARED_MAPPINGS],
            PYTHON_UNMAPPED_PROGRAM_KEYS,
            &["liveness_check"],
        );
    }

    // 5. Process sections
    for section_name in &ini.section_order {
        if let Some(prog_name) = section_name.strip_prefix("program:") {
            if let Some(sec) = ini.sections.get(section_name) {
                require_key(sec, "command", "Program", prog_name)?;
                let context = format!("Program '{}' invalid", prog_name);
                let mut prog = map_section(
                    sec,
                    &[PROGRAM_SHARED_MAPPINGS, PROGRAM_ONLY_MAPPINGS],
                    &context,
                    config_dir,
                )?;
                attach_liveness_check(sec, &mut prog, &format!("Program '{}'", prog_name))?;
                root["programs"][prog_name] = prog;
                warn_unconsumed_keys_multi(
                    &format!("program:{}", prog_name),
                    sec,
                    &[PROGRAM_SHARED_MAPPINGS, PROGRAM_ONLY_MAPPINGS],
                    PYTHON_UNMAPPED_PROGRAM_KEYS,
                    &["liveness_check"],
                );
            }
        } else if let Some(group_name) = section_name.strip_prefix("group:") {
            if let Some(sec) = ini.sections.get(section_name) {
                // Error context matches legacy `Invalid group priority` message.
                root["groups"][group_name] =
                    map_section(sec, &[GROUP_MAPPINGS], "Invalid group", config_dir)?;
                warn_unconsumed_keys_multi(
                    &format!("group:{}", group_name),
                    sec,
                    &[GROUP_MAPPINGS],
                    &[],
                    &[],
                );
            }
        } else if let Some(pool_name) = section_name.strip_prefix("eventlistener:") {
            if let Some(sec) = ini.sections.get(section_name) {
                require_key(sec, "command", "EventListener", pool_name)?;
                require_key(sec, "events", "EventListener", pool_name)?;
                let context = format!("EventListener '{}' invalid", pool_name);
                let el = map_section(sec, &[EVENT_LISTENER_MAPPINGS], &context, config_dir)?;

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
