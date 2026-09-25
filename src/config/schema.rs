// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::consts::*;
use crate::error::ProgramError;
use crate::program::config::{
    AutoRestartPolicy, HealthCheckConfig, LogMode, ProgramConfig, ProgramLogsConfig, StopSignal,
};
use crate::serde_util::duration_secs;
use serde::{Deserialize, Serialize};
use smart_default::SmartDefault;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Program field inheritance: prefer the program's own value, else `[program_defaults]`.
macro_rules! inherit {
    ($raw:expr, $defaults:expr, $field:ident) => {
        $raw.$field.or($defaults.$field)
    };
}

/// Borrowing variant of [`inherit`] for non-`Copy` fields (avoids cloning).
macro_rules! inherit_ref {
    ($raw:expr, $defaults:expr, $field:ident) => {
        $raw.$field.as_ref().or($defaults.$field.as_ref())
    };
}

/// Owned-clone variant of [`inherit`] for non-`Copy` fields used by value.
macro_rules! inherit_clone {
    ($raw:expr, $defaults:expr, $field:ident) => {
        $raw.$field.clone().or_else(|| $defaults.$field.clone())
    };
}

#[derive(Debug, Clone, SmartDefault, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_uds_path")]
    #[default(default_uds_path())]
    pub uds_path: PathBuf,
    /// Unix socket / Named Pipe DACL mode as an octal string (e.g. `"0700"`).
    /// Alias `chmod` matches `[unix_http_server]`. When omitted, defaults to
    /// `0o770` on Windows (owner + Administrators) or `0o700` on Unix, or
    /// `0o777` when `allow_unelevated` is true so local CLI can connect.
    #[serde(default, alias = "chmod", with = "crate::serde_util::option_chmod")]
    pub uds_chmod: Option<u32>,
    #[serde(default)]
    pub uds_username: Option<String>,
    #[serde(default)]
    pub uds_password: Option<String>,
    #[serde(default, deserialize_with = "crate::serde_util::optional_http_bind")]
    pub http_bind: Option<String>,
    #[serde(default, alias = "http_username")]
    pub username: Option<String>,
    #[serde(default, alias = "http_password")]
    pub password: Option<String>,
    #[serde(default)]
    pub auth_token: Option<String>,
    #[serde(default)]
    pub identifier: Option<String>,
    /// When true (default), relative paths in path fields are absolutized against the
    /// config file directory at the parse boundary. When false, relative paths are
    /// preserved and interpretated relative to the daemon working directory, matching
    /// python supervisor behavior.
    #[serde(default = "bool_value::<true>")]
    #[default(true)]
    pub path_translation: bool,
    /// When true, allows non-elevated (non-root on Unix, non-admin on Windows) callers
    /// to connect via local IPC when the daemon is running elevated. Default is false.
    #[serde(default)]
    pub allow_unelevated: bool,
    /// When true (default), empty [`SupervisorConfig::ctl`] fields are filled from this
    /// server section at load time so supervisorctl can connect without a ctl block.
    /// INI frontend forces false (Python/go parity: no server→ctl backfill).
    #[serde(default = "bool_value::<true>")]
    #[default(true)]
    pub ctl_defaults: bool,
}

fn default_uds_path() -> PathBuf {
    let cmd_name = crate::config::paths::get_cmd_name();
    crate::config::paths::default_uds_path(&cmd_name, None)
}

impl ServerConfig {
    /// Resolves the effective IPC mode: explicit `uds_chmod`, else `0o777` when
    /// `allow_unelevated` is set, else `0o770` on Windows (owner + Administrators)
    /// or `0o700` on Unix. Fails on invalid octal input.
    pub fn resolved_uds_chmod(&self) -> Result<u32, ProgramError> {
        Ok(self.uds_chmod.unwrap_or_else(|| self.default_uds_chmod()))
    }

    fn default_uds_chmod(&self) -> u32 {
        if self.allow_unelevated {
            UDS_CHMOD_UNELEVATED
        } else {
            // Windows: owner may be SYSTEM (service); include Administrators so an
            // elevated admin CLI can pass the OS authorization layer.
            UDS_CHMOD
        }
    }
}

#[derive(Debug, Clone, SmartDefault, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    #[serde(default, alias = "mode")]
    #[default(crate::program::config::LogMode::On)]
    pub enabled: crate::program::config::LogMode,
    #[serde(default)]
    pub file: Option<PathBuf>,
    #[serde(default = "default_log_level")]
    #[default(default_log_level())]
    pub level: String,
    #[serde(default, with = "crate::serde_util::option_byte_size")]
    pub buffer_size: Option<usize>,
    #[serde(default, with = "crate::serde_util::option_byte_size")]
    #[default(Some(DEFAULT_LOG_MAX_BYTES))]
    pub max_bytes: Option<usize>,
    #[serde(default = "default_log_backups")]
    #[default(DEFAULT_LOG_BACKUPS)]
    pub backups: usize,
    #[serde(default = "bool_value::<false>")]
    #[default(false)]
    pub timestamp_suffix: bool,
    /// When true, suppress console (stdout/stderr) log output (INI `silent`).
    #[serde(default)]
    pub silent: bool,
}

impl LoggingConfig {
    pub fn effective_buffer_size(&self) -> usize {
        self.buffer_size
            .filter(|&sz| sz > 0)
            .unwrap_or(crate::consts::DEFAULT_IN_MEMORY_LOG_BUFFER_SIZE)
    }

    /// Resolves the effective daemon log file path (either explicitly configured or default).
    pub fn resolved_file_path(&self) -> PathBuf {
        self.file.clone().unwrap_or_else(|| {
            crate::config::paths::PathResolver::from_current_exe().default_daemon_log_path()
        })
    }

    /// Builds an `InMemoryChannelRotator` for daemon logging if `is_in_memory_only()` is enabled,
    /// seeding from the configured file if present.
    pub fn build_main_rotator(
        &self,
    ) -> Option<std::sync::Arc<crate::logging::InMemoryChannelRotator>> {
        if self.enabled.is_in_memory_only() {
            let buffer_size = self.effective_buffer_size();
            let backups = self.backups;
            let rot = std::sync::Arc::new(
                crate::logging::InMemoryChannelRotator::with_total_capacity(buffer_size, backups),
            );
            if let Some(ref path) = self.file {
                let _ = rot.seed_from_file(path, rot.segment_size());
            }
            Some(rot)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, SmartDefault, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsConfig {
    #[serde(default = "bool_value::<true>")]
    #[default(true)]
    pub enabled: bool,
    #[serde(default = "default_metrics_idle_timeout", with = "duration_secs")]
    #[default(DEFAULT_METRICS_IDLE_TIMEOUT)]
    pub idle_timeout_secs: Duration,
    #[serde(default = "default_metrics_interval", with = "duration_secs")]
    #[default(DEFAULT_METRICS_INTERVAL)]
    pub interval_secs: Duration,
}

/// Global defaults template for programs (equivalent to legacy [program-default]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProgramDefaults {
    #[serde(default)]
    pub autostart: Option<bool>,
    #[serde(default)]
    pub autorestart: Option<AutoRestartPolicy>,
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub start_secs: Option<Duration>,
    #[serde(default)]
    pub start_retries: Option<u32>,
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub restart_pause_secs: Option<Duration>,
    #[serde(default)]
    pub stop_signal: Option<StopSignal>,
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub stop_wait_secs: Option<Duration>,
    #[serde(default)]
    pub priority: Option<u32>,
    #[serde(default)]
    pub logs: Option<ProgramLogsConfigRaw>,
    #[serde(default)]
    pub health_check: Option<HealthCheckConfig>,
    #[serde(default, alias = "pre_start_hook")]
    pub pre_start: Option<String>,
    #[serde(default, alias = "pre_stop_hook")]
    pub pre_stop: Option<String>,
    #[serde(default)]
    pub pre_start_ignore_failure: Option<bool>,
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub hook_timeout_secs: Option<Duration>,
    #[serde(default)]
    pub numprocs: Option<usize>,
    #[serde(default)]
    pub numprocs_start: Option<usize>,
    #[serde(default)]
    pub process_name: Option<String>,
    #[serde(default)]
    pub restart_when_binary_changed: Option<bool>,
    #[serde(default)]
    pub restart_signal_when_binary_changed: Option<StopSignal>,
    #[serde(default)]
    pub restart_cmd_when_binary_changed: Option<String>,
    #[serde(default)]
    pub restart_directory_monitor: Option<PathBuf>,
    #[serde(default)]
    pub restart_file_pattern: Option<String>,
    #[serde(default)]
    pub restart_signal_when_file_changed: Option<StopSignal>,
    #[serde(default)]
    pub restart_cmd_when_file_changed: Option<String>,
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub restart_debounce_secs: Option<Duration>,
    #[serde(default)]
    pub env_files: Option<Vec<PathBuf>>,
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub kill_wait_secs: Option<Duration>,
    #[serde(default)]
    pub stop_as_group: Option<bool>,
    #[serde(default)]
    pub kill_as_group: Option<bool>,
}

/// Raw representation of program log configuration with optional booleans for inheritance.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProgramLogsConfigRaw {
    #[serde(default, alias = "mode")]
    pub enabled: Option<crate::program::config::LogMode>,
    #[serde(default)]
    pub stdout: Option<PathBuf>,
    #[serde(default)]
    pub stderr: Option<PathBuf>,
    #[serde(default, with = "crate::serde_util::option_byte_size")]
    pub buffer_size: Option<usize>,
    #[serde(default, with = "crate::serde_util::option_byte_size")]
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub backups: Option<usize>,
    #[serde(default, with = "crate::serde_util::option_byte_size")]
    pub stdout_max_bytes: Option<usize>,
    #[serde(default, with = "crate::serde_util::option_byte_size")]
    pub stderr_max_bytes: Option<usize>,
    #[serde(default)]
    pub stdout_backups: Option<usize>,
    #[serde(default)]
    pub stderr_backups: Option<usize>,
    #[serde(default)]
    pub stdout_timestamp_suffix: Option<bool>,
    #[serde(default)]
    pub stderr_timestamp_suffix: Option<bool>,
    #[serde(default)]
    pub stdout_syslog: Option<bool>,
    #[serde(default)]
    pub stderr_syslog: Option<bool>,
    #[serde(default)]
    pub syslog_facility: Option<String>,
    #[serde(default)]
    pub syslog_tag: Option<String>,
    #[serde(default)]
    pub syslog_stdout_priority: Option<String>,
    #[serde(default)]
    pub syslog_stderr_priority: Option<String>,
    #[serde(default)]
    pub redirect_stderr: Option<bool>,
    #[serde(default)]
    pub stdout_events_enabled: Option<bool>,
    #[serde(default)]
    pub stderr_events_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramConfigRaw {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub directory: Option<PathBuf>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    #[serde(default)]
    pub priority: Option<u32>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub autostart: Option<bool>,
    #[serde(default)]
    pub autorestart: Option<AutoRestartPolicy>,
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub start_secs: Option<Duration>,
    #[serde(default)]
    pub start_retries: Option<u32>,
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub restart_pause_secs: Option<Duration>,
    #[serde(default)]
    pub stop_signal: Option<StopSignal>,
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub stop_wait_secs: Option<Duration>,
    #[serde(default)]
    pub exit_codes: Option<Vec<i32>>,
    #[serde(default, with = "crate::serde_util::option_chmod")]
    pub umask: Option<u32>,
    #[serde(default)]
    pub logs: Option<ProgramLogsConfigRaw>,
    #[serde(default)]
    pub health_check: Option<HealthCheckConfig>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub cron: Option<String>,
    #[serde(default, alias = "stop_cron")]
    pub cron_stop: Option<String>,
    #[serde(default, alias = "pre_start_hook")]
    pub pre_start: Option<String>,
    #[serde(default, alias = "pre_stop_hook")]
    pub pre_stop: Option<String>,
    #[serde(default)]
    pub pre_start_ignore_failure: Option<bool>,
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub hook_timeout_secs: Option<Duration>,
    #[serde(default)]
    pub numprocs: Option<usize>,
    #[serde(default)]
    pub numprocs_start: Option<usize>,
    #[serde(default)]
    pub process_name: Option<String>,
    #[serde(default)]
    pub restart_when_binary_changed: Option<bool>,
    #[serde(default)]
    pub restart_signal_when_binary_changed: Option<StopSignal>,
    #[serde(default)]
    pub restart_cmd_when_binary_changed: Option<String>,
    #[serde(default)]
    pub restart_directory_monitor: Option<PathBuf>,
    #[serde(default)]
    pub restart_file_pattern: Option<String>,
    #[serde(default)]
    pub restart_signal_when_file_changed: Option<StopSignal>,
    #[serde(default)]
    pub restart_cmd_when_file_changed: Option<String>,
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub restart_debounce_secs: Option<Duration>,
    #[serde(default)]
    pub stdout_events_enabled: Option<bool>,
    #[serde(default)]
    pub stderr_events_enabled: Option<bool>,
    #[serde(default)]
    pub env_files: Option<Vec<PathBuf>>,
    #[serde(default, with = "crate::serde_util::option_duration_secs")]
    pub kill_wait_secs: Option<Duration>,
    #[serde(default)]
    pub stop_as_group: Option<bool>,
    #[serde(default)]
    pub kill_as_group: Option<bool>,
}

/// Process group configuration definition.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct GroupConfigRaw {
    #[serde(default)]
    pub programs: Vec<String>,
    #[serde(default)]
    pub priority: Option<u32>,
}

/// Client connection config from `[supervisorctl]` / YAML `ctl` | `supervisorctl`.
///
/// Consumed only by `supervisorctl`; never applied to the daemon listener
/// credentials (`server.uds_*` / `server.username`). `None` on
/// [`SupervisorConfig`] means the source file had no ctl section and
/// `server.ctl_defaults` fill did not create one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CtlConfig {
    #[serde(default)]
    pub serverurl: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    /// Bearer token for protected endpoints (aligns with supervisorctl `-k`).
    #[serde(default)]
    pub auth_token: Option<String>,
}

impl CtlConfig {
    /// Fills `None` fields from the server listener section (one-way; never
    /// mutates `server`). Idempotent; safe on every load when
    /// `server.ctl_defaults` is true.
    pub fn fill_from_server(&mut self, server: &ServerConfig) {
        if self.serverurl.is_none() {
            if let Some(bind) = server.http_bind.as_ref().filter(|b| !b.is_empty()) {
                self.serverurl = Some(format!("http://{}", bind));
            } else {
                self.serverurl = Some(server.uds_path.to_string_lossy().into_owned());
            }
        }

        let is_tcp = self
            .serverurl
            .as_deref()
            .map(|u| u.starts_with("http://") || u.starts_with("tcp://"))
            .unwrap_or(false);

        if is_tcp {
            if self.username.is_none() {
                self.username = server.username.clone();
            }
            if self.password.is_none() {
                self.password = server.password.clone();
            }
        } else {
            if self.username.is_none() {
                self.username = server.uds_username.clone();
            }
            if self.password.is_none() {
                self.password = server.uds_password.clone();
            }
        }

        if self.auth_token.is_none() {
            self.auth_token = server.auth_token.clone();
        }
    }

    /// Builds the full server-derived candidate chain when no `ctl` section was
    /// written but `server.ctl_defaults` is true (YAML). Order: local IPC first,
    /// then TCP when `http_bind` is set. Each entry carries every connection
    /// field for that listener (endpoint URL, matching credentials, auth token).
    pub fn vec_from_server(server: &ServerConfig) -> Vec<Self> {
        let mut out = Vec::with_capacity(2);
        out.push(Self {
            serverurl: Some(server.uds_path.to_string_lossy().into_owned()),
            username: server.uds_username.clone(),
            password: server.uds_password.clone(),
            auth_token: server.auth_token.clone(),
        });
        if let Some(bind) = server.http_bind.as_ref().filter(|b| !b.is_empty()) {
            out.push(Self {
                serverurl: Some(format!("http://{}", bind)),
                username: server.username.clone(),
                password: server.password.clone(),
                auth_token: server.auth_token.clone(),
            });
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorConfig {
    #[serde(default)]
    pub worker_threads: Option<usize>,
    /// Run in foreground (Python parity `nodaemon`). Daemonize is not yet
    /// implemented; when false the process still runs in the foreground.
    // TODO: Python parity double-fork daemonize when nodaemon is false.
    #[serde(default)]
    pub nodaemon: bool,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub metrics: MetricsConfig,
    #[serde(default)]
    pub program_defaults: ProgramDefaults,
    #[serde(default)]
    pub groups: HashMap<String, GroupConfigRaw>,
    #[serde(default)]
    pub programs: HashMap<String, ProgramConfigRaw>,
    #[serde(default)]
    pub event_listeners: HashMap<String, crate::eventlistener::EventListenerConfigRaw>,
    /// Daemon-level environment from `[supervisord] environment`.
    /// Applied at daemon startup (before children spawn), never at parse time.
    #[serde(default)]
    pub environment: HashMap<String, String>,
    /// Optional pidfile path written on startup and removed on shutdown.
    #[serde(default)]
    pub pidfile: Option<PathBuf>,
    /// Minimum open-file soft limit (`minfds`); applied best-effort on Unix.
    #[serde(default)]
    pub minfds: Option<u32>,
    /// Minimum process soft limit (`minprocs`); applied best-effort on Unix.
    #[serde(default)]
    pub minprocs: Option<u32>,
    /// Client connection config from `[supervisorctl]` / YAML `ctl`|`supervisorctl`
    /// (not consumed by the daemon). `None` = section absent after optional fill.
    #[serde(default, alias = "supervisorctl")]
    pub ctl: Option<CtlConfig>,
    #[serde(skip)]
    pub config_dir: Option<PathBuf>,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        let mut config = Self {
            worker_threads: None,
            nodaemon: false,
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            metrics: MetricsConfig::default(),
            program_defaults: ProgramDefaults::default(),
            groups: HashMap::new(),
            programs: HashMap::new(),
            event_listeners: HashMap::new(),
            environment: HashMap::new(),
            pidfile: None,
            minfds: None,
            minprocs: None,
            ctl: None,
            config_dir: None,
        };
        config.apply_default_paths();
        config
    }
}

impl SupervisorConfig {
    /// Loads and parses a SupervisorConfig from a file, automatically determining format:
    /// - `.yaml` or `.yml` files are parsed via the YAML pipeline
    /// - all other files (e.g. `.conf`, `.ini`, or extensionless) are parsed via the compatibility INI pipeline
    pub fn from_file<P: AsRef<std::path::Path>>(path: P) -> Result<Self, ProgramError> {
        let path_buf = crate::platform::abs_path(path.as_ref());
        let path_ref = path_buf.as_path();
        let is_yaml = match path_ref.extension().and_then(|ext| ext.to_str()) {
            Some(ext) => ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"),
            None => false,
        };

        if is_yaml {
            let content = std::fs::read_to_string(path_ref).map_err(|e| {
                ProgramError::ConfigError(format!(
                    "Failed to read config file '{:?}': {}",
                    path_ref, e
                ))
            })?;
            let config_dir = path_ref.parent().map(|p| {
                if p.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    p.to_path_buf()
                }
            });
            Self::from_yaml_str_with_config_dir(&content, config_dir.as_deref())
        } else {
            crate::compat::ini::load_ini_config(path_ref)
        }
    }

    pub fn from_ini_str(ini_content: &str) -> Result<Self, ProgramError> {
        Self::from_ini_str_with_config_dir(ini_content, None)
    }

    pub fn from_ini_str_with_config_dir(
        ini_content: &str,
        config_dir: Option<&std::path::Path>,
    ) -> Result<Self, ProgramError> {
        crate::compat::ini::parse_ini_str(ini_content, config_dir)
    }

    pub fn from_yaml_str(yaml_content: &str) -> Result<Self, ProgramError> {
        Self::from_yaml_str_with_config_dir(yaml_content, None)
    }

    pub fn from_yaml_str_with_config_dir(
        yaml_content: &str,
        config_dir: Option<&std::path::Path>,
    ) -> Result<Self, ProgramError> {
        // No text-level macro expansion here: `${VAR}`/`%()`/`$()` expansion and path
        // absolutization happen per-value inside the shared translate_paths boundary.
        let mut config: Self = serde_yaml::from_str(yaml_content).map_err(|e| {
            ProgramError::ConfigError(format!("Failed to parse YAML configuration: {}", e))
        })?;
        // Default UDS credentials from HTTP credentials in YAML frontend if omitted (Scheme B)
        if config.server.uds_username.is_none() && config.server.username.is_some() {
            config.server.uds_username = config.server.username.clone();
        }
        if config.server.uds_password.is_none() && config.server.password.is_some() {
            config.server.uds_password = config.server.password.clone();
        }
        config.config_dir = config_dir.map(|p| p.to_path_buf());
        config.apply_default_paths();
        config = config.translate_paths()?;
        config.validate()?;
        config.fill_ctl_defaults();
        Ok(config)
    }

    /// When `server.ctl_defaults` is true, fills empty fields on an existing
    /// `ctl` section from the server section. Does **not** synthesize a section
    /// when absent: the CLI resolves a missing section to
    /// [`CtlConfig::vec_from_server`] (multi-candidate) or a hard error when
    /// `ctl_defaults` is false (INI / Python parity).
    fn fill_ctl_defaults(&mut self) {
        if !self.server.ctl_defaults {
            return;
        }
        if let Some(ctl) = self.ctl.as_mut() {
            ctl.fill_from_server(&self.server);
        }
    }

    /// Applies the path-translation boundary transform shared by both the YAML and the
    /// INI pipelines: serializes the typed config to JSON, walks the tree expanding
    /// `${VAR}`/`%()`/`$()` macros (leniently, keeping literals on failure) and
    /// absolutizing path-kind fields against `config_dir` when `server.path_translation`
    /// is enabled, then deserializes back into a typed config.
    pub(crate) fn translate_paths(self) -> Result<Self, ProgramError> {
        let path_translation = self.server.path_translation;
        let config_dir = self.config_dir.clone();
        let mut value = serde_json::to_value(&self).map_err(|e| {
            ProgramError::ConfigError(format!(
                "Failed to serialize config for path translation: {}",
                e
            ))
        })?;
        crate::config::transform::transform(
            &mut value,
            &crate::config::transform::Ctx {
                config_dir: config_dir.as_deref(),
                path_translation,
            },
        );
        let mut config: Self = serde_json::from_value(value).map_err(|e| {
            ProgramError::ConfigError(format!(
                "Failed to deserialize path-translated config: {}",
                e
            ))
        })?;
        // config_dir is #[serde(skip)]: restore it after the round-trip.
        config.config_dir = config_dir;
        Ok(config)
    }

    pub fn apply_default_paths(&mut self) {
        let resolver = crate::config::paths::PathResolver::from_current_exe()
            .with_config_dir(self.config_dir.as_deref());
        let default_no_dir =
            crate::config::paths::PathResolver::from_current_exe().default_uds_path();
        if self.server.uds_path == default_no_dir {
            self.server.uds_path = resolver.default_uds_path();
        }
        if self.logging.enabled.is_on() && self.logging.file.is_none() {
            self.logging.file = Some(resolver.default_daemon_log_path());
        }
    }

    pub fn validate(&self) -> Result<(), ProgramError> {
        // Fail fast on bad IPC mode before any listener bind.
        self.server.resolved_uds_chmod()?;
        for (name, raw) in &self.programs {
            if raw.command.trim().is_empty() {
                return Err(ProgramError::ConfigError(format!(
                    "Program '{}' command cannot be empty",
                    name
                )));
            }
            let priority =
                inherit!(raw, self.program_defaults, priority).unwrap_or(DEFAULT_PRIORITY);
            if priority > MAX_PRIORITY {
                return Err(ProgramError::ConfigError(format!(
                    "Program '{}' priority {} must be in range [0, 999]",
                    name, priority
                )));
            }
            let stop_wait =
                inherit!(raw, self.program_defaults, stop_wait_secs).unwrap_or(DEFAULT_STOP_WAIT);
            if stop_wait > MAX_TIMEOUT {
                return Err(ProgramError::ConfigError(format!(
                    "Program '{}' stop_wait_secs {} exceeds maximum 86400",
                    name,
                    stop_wait.as_secs()
                )));
            }
            if let Some(ref expr) = raw.cron
                && let Err(e) = expr.parse::<croner::Cron>()
            {
                return Err(ProgramError::InvalidCronExpression {
                    name: name.clone(),
                    expression: expr.clone(),
                    reason: e.to_string(),
                });
            }
            if let Some(ref expr) = raw.cron_stop
                && let Err(e) = expr.parse::<croner::Cron>()
            {
                return Err(ProgramError::InvalidCronExpression {
                    name: name.clone(),
                    expression: expr.clone(),
                    reason: e.to_string(),
                });
            }

            let numprocs = inherit!(raw, self.program_defaults, numprocs).unwrap_or(1);
            if numprocs == 0 {
                return Err(ProgramError::ConfigError(format!(
                    "Program '{}' numprocs must be greater than 0",
                    name
                )));
            }
            if numprocs > 1
                && let Some(p_template) = inherit_ref!(raw, self.program_defaults, process_name)
                && !p_template.contains("process_num")
            {
                return Err(ProgramError::ConfigError(format!(
                    "Program '{}' with numprocs > 1 specifies process_name '{}' which does not contain 'process_num'",
                    name, p_template
                )));
            }

            // Log destination / syslog validation (fail loud before spawn).
            {
                use crate::logging::{LogDestination, SyslogFacility, SyslogSeverity};

                let raw_logs = raw.logs.as_ref();
                let def_logs = self.program_defaults.logs.as_ref();

                let stdout_syslog = raw_logs
                    .and_then(|l| l.stdout_syslog)
                    .or_else(|| def_logs.and_then(|l| l.stdout_syslog))
                    .unwrap_or(false);
                let stderr_syslog = raw_logs
                    .and_then(|l| l.stderr_syslog)
                    .or_else(|| def_logs.and_then(|l| l.stderr_syslog))
                    .unwrap_or(false);

                if let Some(f) = raw_logs
                    .and_then(|l| l.syslog_facility.as_deref())
                    .or_else(|| def_logs.and_then(|l| l.syslog_facility.as_deref()))
                {
                    f.parse::<SyslogFacility>().map_err(|e| {
                        ProgramError::ConfigError(format!("Program '{}': {}", name, e))
                    })?;
                }
                if let Some(p) = raw_logs
                    .and_then(|l| l.syslog_stdout_priority.as_deref())
                    .or_else(|| def_logs.and_then(|l| l.syslog_stdout_priority.as_deref()))
                {
                    p.parse::<SyslogSeverity>().map_err(|e| {
                        ProgramError::ConfigError(format!("Program '{}': {}", name, e))
                    })?;
                }
                if let Some(p) = raw_logs
                    .and_then(|l| l.syslog_stderr_priority.as_deref())
                    .or_else(|| def_logs.and_then(|l| l.syslog_stderr_priority.as_deref()))
                {
                    p.parse::<SyslogSeverity>().map_err(|e| {
                        ProgramError::ConfigError(format!("Program '{}': {}", name, e))
                    })?;
                }

                let validate_dest = |stream: &str, path: &PathBuf| -> Result<(), ProgramError> {
                    let s = path.to_string_lossy();
                    let dest = LogDestination::parse(&s).map_err(|e| {
                        ProgramError::ConfigError(format!(
                            "Program '{}' invalid {} log destination '{}': {}",
                            name, stream, s, e
                        ))
                    })?;
                    #[cfg(windows)]
                    if dest.contains_syslog() {
                        return Err(ProgramError::ConfigError(format!(
                            "Program '{}' {} log destination '{}' uses syslog, which is not supported on Windows",
                            name, stream, s
                        )));
                    }
                    let _ = &dest;
                    Ok(())
                };

                if let Some(p) = raw_logs
                    .and_then(|l| l.stdout.as_ref())
                    .or_else(|| def_logs.and_then(|l| l.stdout.as_ref()))
                {
                    validate_dest("stdout", p)?;
                }
                if let Some(p) = raw_logs
                    .and_then(|l| l.stderr.as_ref())
                    .or_else(|| def_logs.and_then(|l| l.stderr.as_ref()))
                {
                    validate_dest("stderr", p)?;
                }

                #[cfg(windows)]
                if stdout_syslog || stderr_syslog {
                    return Err(ProgramError::ConfigError(format!(
                        "Program '{}' stdout_syslog/stderr_syslog is not supported on Windows",
                        name
                    )));
                }
                #[cfg(not(windows))]
                let _ = (stdout_syslog, stderr_syslog);
            }
        }

        // Daemon main log destination validation.
        {
            use crate::logging::LogDestination;
            if let Some(ref f) = self.logging.file {
                let s = f.to_string_lossy();
                let dest = LogDestination::parse(&s).map_err(|e| {
                    ProgramError::ConfigError(format!(
                        "Invalid daemon log destination '{}': {}",
                        s, e
                    ))
                })?;
                #[cfg(windows)]
                if dest.contains_syslog() {
                    return Err(ProgramError::ConfigError(format!(
                        "Daemon log destination '{}' uses syslog, which is not supported on Windows",
                        s
                    )));
                }
                let _ = &dest;
            }
        }

        for (group_name, group_cfg) in &self.groups {
            if let Some(p) = group_cfg.priority
                && p > MAX_PRIORITY
            {
                return Err(ProgramError::ConfigError(format!(
                    "Group '{}' priority {} must be in range [0, 999]",
                    group_name, p
                )));
            }
            for prog in &group_cfg.programs {
                if !self.programs.contains_key(prog) {
                    return Err(ProgramError::ConfigError(format!(
                        "Group '{}' references unknown program '{}'",
                        group_name, prog
                    )));
                }
            }
        }

        for (listener_name, listener_cfg) in &self.event_listeners {
            if listener_cfg.command.trim().is_empty() {
                return Err(ProgramError::ConfigError(format!(
                    "EventListener '{}' command cannot be empty",
                    listener_name
                )));
            }
            crate::eventlistener::validate_event_list(&listener_cfg.events)?;
            if listener_cfg.buffer_size < 1 {
                return Err(ProgramError::ConfigError(format!(
                    "EventListener '{}' buffer_size must be >= 1",
                    listener_name
                )));
            }
            if listener_cfg.redirect_stderr == Some(true) {
                return Err(ProgramError::ConfigError(format!(
                    "EventListener '{}' redirect_stderr cannot be true (violates wire protocol)",
                    listener_name
                )));
            }
        }

        Ok(())
    }

    /// Resolves all raw program definitions by expanding numprocs, applying program_defaults,
    /// interpolating template expressions, and mapping inter-program dependencies.
    pub fn resolve_programs(&self) -> Result<HashMap<String, ProgramConfig>, ProgramError> {
        // Phase 1: Determine instance names for every raw program definition
        let mut program_instances: HashMap<String, Vec<String>> =
            HashMap::with_capacity(self.programs.len());

        for (base_name, raw) in &self.programs {
            let numprocs = inherit!(raw, self.program_defaults, numprocs).unwrap_or(1);
            let numprocs_start = inherit!(raw, self.program_defaults, numprocs_start).unwrap_or(0);
            let process_name_template = inherit_ref!(raw, self.program_defaults, process_name);

            let group = if let Some(ref g) = raw.group {
                g.clone()
            } else {
                let mut found_group = None;
                for (g_name, g_cfg) in &self.groups {
                    if g_cfg.programs.iter().any(|p| p == base_name) {
                        found_group = Some(g_name.clone());
                        break;
                    }
                }
                found_group.unwrap_or_else(|| base_name.clone())
            };

            let mut instances = Vec::with_capacity(numprocs);
            if numprocs == 1 && process_name_template.is_none() {
                instances.push(base_name.clone());
            } else {
                for idx in 0..numprocs {
                    let process_num = numprocs_start + idx;
                    let instance_name = if let Some(template) = process_name_template {
                        let expr = crate::config::expand::StringExpression::with_config_dir(
                            self.config_dir
                                .as_deref()
                                .unwrap_or(std::path::Path::new(".")),
                        )
                        .with_program_context(
                            base_name,
                            &group,
                            process_num,
                            numprocs,
                        );
                        expr.eval_named(template, "process_name")
                            .map_err(|e| ProgramError::ConfigError(e.to_string()))?
                    } else {
                        format!("{}:{}", base_name, process_num)
                    };
                    instances.push(instance_name);
                }
            }
            program_instances.insert(base_name.clone(), instances);
        }

        // Phase 2: Expand and resolve ProgramConfig for each instance
        let mut resolved: HashMap<String, ProgramConfig> = HashMap::new();

        for (base_name, raw) in &self.programs {
            let numprocs = inherit!(raw, self.program_defaults, numprocs).unwrap_or(1);
            let numprocs_start = inherit!(raw, self.program_defaults, numprocs_start).unwrap_or(0);

            let priority =
                inherit!(raw, self.program_defaults, priority).unwrap_or(DEFAULT_PRIORITY);
            if priority > MAX_PRIORITY {
                return Err(ProgramError::ConfigError(format!(
                    "Program '{}' priority {} must be in range [0, 999]",
                    base_name, priority
                )));
            }

            let autostart =
                inherit!(raw, self.program_defaults, autostart).unwrap_or(raw.cron.is_none());

            let autorestart = inherit!(raw, self.program_defaults, autorestart).unwrap_or_default();

            let start_secs =
                inherit!(raw, self.program_defaults, start_secs).unwrap_or(DEFAULT_START);

            let start_retries = inherit!(raw, self.program_defaults, start_retries)
                .unwrap_or(DEFAULT_START_RETRIES);

            let restart_pause_secs = inherit!(raw, self.program_defaults, restart_pause_secs)
                .unwrap_or(DEFAULT_RESTART_PAUSE);

            let stop_signal = inherit!(raw, self.program_defaults, stop_signal).unwrap_or_default();

            let stop_wait_secs =
                inherit!(raw, self.program_defaults, stop_wait_secs).unwrap_or(DEFAULT_STOP_WAIT);

            let exit_codes = raw.exit_codes.clone().unwrap_or_else(default_exit_codes);

            let pre_start_ignore_failure =
                inherit!(raw, self.program_defaults, pre_start_ignore_failure).unwrap_or(false);

            let hook_timeout_secs = inherit!(raw, self.program_defaults, hook_timeout_secs)
                .unwrap_or(DEFAULT_HOOK_TIMEOUT);

            let group = if let Some(ref g) = raw.group {
                g.clone()
            } else {
                let mut found_group = None;
                for (g_name, g_cfg) in &self.groups {
                    if g_cfg.programs.iter().any(|p| p == base_name) {
                        found_group = Some(g_name.clone());
                        break;
                    }
                }
                found_group.unwrap_or_else(|| base_name.clone())
            };

            let group_priority = self
                .groups
                .get(&group)
                .and_then(|g| g.priority)
                .unwrap_or(DEFAULT_GROUP_PRIORITY);

            // Expand depends_on: map multi-instance program dependencies to all their instances
            let mut resolved_depends_on = Vec::new();
            for dep in &raw.depends_on {
                if let Some(dep_instances) = program_instances.get(dep) {
                    resolved_depends_on.extend(dep_instances.clone());
                } else {
                    resolved_depends_on.push(dep.clone());
                }
            }

            let instances = program_instances.get(base_name).unwrap();

            for (idx, instance_name) in instances.iter().enumerate() {
                let process_num = numprocs_start + idx;

                let mut expr = crate::config::expand::StringExpression::with_config_dir(
                    self.config_dir
                        .as_deref()
                        .unwrap_or(std::path::Path::new(".")),
                )
                .with_program_context(base_name, &group, process_num, numprocs);

                // OI-6: seed daemon-level environment into expansion context without
                // mutating the process env (environment values may reference these).
                for (k, v) in &self.environment {
                    expr.add(format!("ENV_{}", k), v);
                }
                for (k, v) in &raw.environment {
                    expr.add(format!("ENV_{}", k), v);
                }

                // Evaluate command and args
                let raw_cmd = expr
                    .eval_named(&raw.command, "command")
                    .map_err(|e| ProgramError::ConfigError(e.to_string()))?;

                let platform = crate::platform::native_platform();
                let (command, args) = if raw.args.is_empty() {
                    match platform.split_command_line(&raw_cmd) {
                        Ok(mut parts) if !parts.is_empty() => {
                            let cmd = parts.remove(0);
                            (cmd, parts)
                        }
                        _ => (raw_cmd, Vec::new()),
                    }
                } else {
                    let mut evaled_args = Vec::with_capacity(raw.args.len());
                    for a in &raw.args {
                        let evaled_a = expr
                            .eval_named(a, "args")
                            .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                        evaled_args.push(evaled_a);
                    }
                    (raw_cmd, evaled_args)
                };

                // Evaluate directory
                let directory = if let Some(ref dir) = raw.directory {
                    let dir_str = dir.to_string_lossy();
                    let evaled_dir = expr
                        .eval_named(&dir_str, "directory")
                        .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                    Some(PathBuf::from(evaled_dir))
                } else {
                    None
                };

                // Evaluate environment
                let mut environment = HashMap::with_capacity(raw.environment.len());
                for (k, v) in &raw.environment {
                    let evaled_v = expr
                        .eval_named(v, "environment")
                        .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                    environment.insert(k.clone(), evaled_v);
                }

                // Evaluate logs
                let logs = {
                    let raw_logs = raw.logs.as_ref();
                    let def_logs = self.program_defaults.logs.as_ref();

                    let enabled = raw_logs
                        .and_then(|l| l.enabled)
                        .or_else(|| def_logs.and_then(|l| l.enabled))
                        .unwrap_or(crate::program::config::LogMode::On);

                    let buffer_size = raw_logs
                        .and_then(|l| l.buffer_size)
                        .or_else(|| def_logs.and_then(|l| l.buffer_size));

                    let resolver = crate::config::paths::PathResolver::from_current_exe()
                        .with_config_dir(self.config_dir.as_deref());

                    let stdout = if let Some(raw_stdout) = raw_logs
                        .and_then(|l| l.stdout.as_ref())
                        .or_else(|| def_logs.and_then(|l| l.stdout.as_ref()))
                    {
                        let stdout_str = raw_stdout.to_string_lossy();
                        let evaled_stdout = expr
                            .eval_named(&stdout_str, "stdout_logfile")
                            .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                        Some(PathBuf::from(evaled_stdout))
                    } else if enabled == crate::program::config::LogMode::On {
                        Some(resolver.default_program_log_path(instance_name))
                    } else {
                        None
                    };

                    let stderr = if let Some(raw_stderr) = raw_logs
                        .and_then(|l| l.stderr.as_ref())
                        .or_else(|| def_logs.and_then(|l| l.stderr.as_ref()))
                    {
                        let stderr_str = raw_stderr.to_string_lossy();
                        let evaled_stderr = expr
                            .eval_named(&stderr_str, "stderr_logfile")
                            .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                        Some(PathBuf::from(evaled_stderr))
                    } else {
                        None
                    };

                    let max_bytes = raw_logs
                        .and_then(|l| l.max_bytes)
                        .or_else(|| def_logs.and_then(|l| l.max_bytes));

                    let backups = raw_logs
                        .and_then(|l| l.backups)
                        .or_else(|| def_logs.and_then(|l| l.backups));

                    let stdout_max_bytes = raw_logs
                        .and_then(|l| l.stdout_max_bytes)
                        .or_else(|| def_logs.and_then(|l| l.stdout_max_bytes));

                    let stderr_max_bytes = raw_logs
                        .and_then(|l| l.stderr_max_bytes)
                        .or_else(|| def_logs.and_then(|l| l.stderr_max_bytes));

                    let stdout_backups = raw_logs
                        .and_then(|l| l.stdout_backups)
                        .or_else(|| def_logs.and_then(|l| l.stdout_backups));

                    let stderr_backups = raw_logs
                        .and_then(|l| l.stderr_backups)
                        .or_else(|| def_logs.and_then(|l| l.stderr_backups));

                    let stdout_timestamp_suffix = raw_logs
                        .and_then(|l| l.stdout_timestamp_suffix)
                        .or_else(|| def_logs.and_then(|l| l.stdout_timestamp_suffix))
                        .unwrap_or(false);

                    let stderr_timestamp_suffix = raw_logs
                        .and_then(|l| l.stderr_timestamp_suffix)
                        .or_else(|| def_logs.and_then(|l| l.stderr_timestamp_suffix))
                        .unwrap_or(false);

                    let stdout_syslog = raw_logs
                        .and_then(|l| l.stdout_syslog)
                        .or_else(|| def_logs.and_then(|l| l.stdout_syslog))
                        .unwrap_or(false);

                    let stderr_syslog = raw_logs
                        .and_then(|l| l.stderr_syslog)
                        .or_else(|| def_logs.and_then(|l| l.stderr_syslog))
                        .unwrap_or(false);

                    let syslog_facility = raw_logs
                        .and_then(|l| l.syslog_facility.clone())
                        .or_else(|| def_logs.and_then(|l| l.syslog_facility.clone()));

                    let syslog_tag = raw_logs
                        .and_then(|l| l.syslog_tag.clone())
                        .or_else(|| def_logs.and_then(|l| l.syslog_tag.clone()));

                    let syslog_stdout_priority = raw_logs
                        .and_then(|l| l.syslog_stdout_priority.clone())
                        .or_else(|| def_logs.and_then(|l| l.syslog_stdout_priority.clone()));

                    let syslog_stderr_priority = raw_logs
                        .and_then(|l| l.syslog_stderr_priority.clone())
                        .or_else(|| def_logs.and_then(|l| l.syslog_stderr_priority.clone()));

                    let redirect_stderr = raw_logs
                        .and_then(|l| l.redirect_stderr)
                        .or_else(|| def_logs.and_then(|l| l.redirect_stderr))
                        .unwrap_or(false);

                    let stdout_events_enabled = raw_logs
                        .and_then(|l| l.stdout_events_enabled)
                        .or(raw.stdout_events_enabled)
                        .unwrap_or(false);

                    let stderr_events_enabled = raw_logs
                        .and_then(|l| l.stderr_events_enabled)
                        .or(raw.stderr_events_enabled)
                        .unwrap_or(false);

                    ProgramLogsConfig {
                        enabled,
                        stdout,
                        stderr,
                        buffer_size,
                        max_bytes,
                        backups,
                        stdout_max_bytes,
                        stderr_max_bytes,
                        stdout_backups,
                        stderr_backups,
                        stdout_timestamp_suffix,
                        stderr_timestamp_suffix,
                        stdout_syslog,
                        stderr_syslog,
                        syslog_facility,
                        syslog_tag,
                        syslog_stdout_priority,
                        syslog_stderr_priority,
                        redirect_stderr,
                        stdout_events_enabled,
                        stderr_events_enabled,
                    }
                };

                // Evaluate health_check
                let health_check = {
                    let raw_hc = raw
                        .health_check
                        .clone()
                        .or_else(|| self.program_defaults.health_check.clone());
                    if let Some(mut hc) = raw_hc {
                        match hc.check_type {
                            crate::program::config::HealthCheckType::Http {
                                ref mut url, ..
                            } => {
                                *url = expr
                                    .eval_named(url, "health_check.http.url")
                                    .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                            }
                            crate::program::config::HealthCheckType::Tcp { ref mut endpoint } => {
                                *endpoint = expr
                                    .eval_named(endpoint, "health_check.tcp.endpoint")
                                    .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                            }
                            crate::program::config::HealthCheckType::Exec { ref mut command } => {
                                *command = expr
                                    .eval_named(command, "health_check.exec.command")
                                    .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                            }
                        }
                        Some(hc)
                    } else {
                        None
                    }
                };

                // Evaluate hooks
                let pre_start =
                    if let Some(hook) = inherit_ref!(raw, self.program_defaults, pre_start) {
                        Some(
                            expr.eval_named(hook, "pre_start")
                                .map_err(|e| ProgramError::ConfigError(e.to_string()))?,
                        )
                    } else {
                        None
                    };

                let pre_stop =
                    if let Some(hook) = inherit_ref!(raw, self.program_defaults, pre_stop) {
                        Some(
                            expr.eval_named(hook, "pre_stop")
                                .map_err(|e| ProgramError::ConfigError(e.to_string()))?,
                        )
                    } else {
                        None
                    };

                let restart_when_binary_changed =
                    inherit!(raw, self.program_defaults, restart_when_binary_changed)
                        .unwrap_or(false);

                let restart_signal_when_binary_changed = inherit!(
                    raw,
                    self.program_defaults,
                    restart_signal_when_binary_changed
                );

                let restart_cmd_when_binary_changed = if let Some(cmd) =
                    inherit_ref!(raw, self.program_defaults, restart_cmd_when_binary_changed)
                {
                    Some(
                        expr.eval_named(cmd, "restart_cmd_when_binary_changed")
                            .map_err(|e| ProgramError::ConfigError(e.to_string()))?,
                    )
                } else {
                    None
                };

                let restart_directory_monitor = if let Some(dir) =
                    inherit_ref!(raw, self.program_defaults, restart_directory_monitor)
                {
                    let evaluated_dir = expr
                        .eval_named(&dir.to_string_lossy(), "restart_directory_monitor")
                        .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                    Some(PathBuf::from(evaluated_dir))
                } else {
                    None
                };

                let restart_file_pattern =
                    inherit_clone!(raw, self.program_defaults, restart_file_pattern);

                let restart_signal_when_file_changed =
                    inherit!(raw, self.program_defaults, restart_signal_when_file_changed);

                let restart_cmd_when_file_changed = if let Some(cmd) =
                    inherit_ref!(raw, self.program_defaults, restart_cmd_when_file_changed)
                {
                    Some(
                        expr.eval_named(cmd, "restart_cmd_when_file_changed")
                            .map_err(|e| ProgramError::ConfigError(e.to_string()))?,
                    )
                } else {
                    None
                };

                let restart_debounce_secs =
                    inherit!(raw, self.program_defaults, restart_debounce_secs)
                        .unwrap_or(DEFAULT_RESTART_DEBOUNCE);

                let env_files =
                    inherit_clone!(raw, self.program_defaults, env_files).unwrap_or_default();
                let kill_wait_secs = inherit!(raw, self.program_defaults, kill_wait_secs)
                    .unwrap_or(DEFAULT_KILL_WAIT);
                let stop_as_group =
                    inherit!(raw, self.program_defaults, stop_as_group).unwrap_or(false);
                let kill_as_group =
                    inherit!(raw, self.program_defaults, kill_as_group).unwrap_or(stop_as_group);
                if stop_as_group && !kill_as_group {
                    return Err(ProgramError::ConfigError(format!(
                        "Program '{}' cannot set stop_as_group=true and kill_as_group=false",
                        base_name
                    )));
                }

                let prog = ProgramConfig {
                    name: instance_name.clone(),
                    command,
                    args,
                    directory,
                    user: raw.user.clone(),
                    environment,
                    priority,
                    depends_on: resolved_depends_on.clone(),
                    autostart,
                    autorestart,
                    start_secs,
                    start_retries,
                    restart_pause_secs,
                    stop_signal,
                    stop_wait_secs,
                    exit_codes: exit_codes.clone(),
                    umask: raw.umask,
                    logs,
                    health_check,
                    group: group.clone(),
                    group_priority,
                    cron: raw.cron.clone(),
                    cron_stop: raw.cron_stop.clone(),
                    pre_start,
                    pre_stop,
                    pre_start_ignore_failure,
                    hook_timeout_secs,
                    restart_when_binary_changed,
                    restart_signal_when_binary_changed,
                    restart_cmd_when_binary_changed,
                    restart_directory_monitor,
                    restart_file_pattern,
                    restart_signal_when_file_changed,
                    restart_cmd_when_file_changed,
                    restart_debounce_secs,
                    env_files,
                    kill_wait_secs,
                    stop_as_group,
                    kill_as_group,
                    event_listener: None,
                };

                if resolved.contains_key(instance_name) {
                    return Err(ProgramError::ConfigError(format!(
                        "Duplicate program instance name '{}' produced during expansion",
                        instance_name
                    )));
                }

                resolved.insert(instance_name.clone(), prog);
            }
        }

        // Phase 3: Resolve event listener pools into ProgramConfig instances
        for (listener_name, raw) in &self.event_listeners {
            let numprocs = raw.numprocs.unwrap_or(1);
            let numprocs_start = raw.numprocs_start.unwrap_or(0);
            let process_name_template = raw.process_name.as_ref();
            let group = listener_name.clone();

            let priority = if raw.priority < 0 {
                0
            } else {
                raw.priority as u32
            };
            let autostart = raw.autostart.unwrap_or(true);
            let autorestart = raw.autorestart.unwrap_or_default();
            let start_secs = raw.start_secs.unwrap_or(DEFAULT_START);
            let start_retries = raw.start_retries.unwrap_or(DEFAULT_START_RETRIES);
            let stop_signal = raw.stop_signal.unwrap_or_default();
            let stop_wait_secs = raw.stop_wait_secs.unwrap_or(DEFAULT_STOP_WAIT);
            let group_priority = 0;

            let mut instances = Vec::with_capacity(numprocs);
            if numprocs == 1 && process_name_template.is_none() {
                instances.push(listener_name.clone());
            } else {
                for idx in 0..numprocs {
                    let process_num = numprocs_start + idx;
                    let instance_name = if let Some(template) = process_name_template {
                        let expr = crate::config::expand::StringExpression::with_config_dir(
                            self.config_dir
                                .as_deref()
                                .unwrap_or(std::path::Path::new(".")),
                        )
                        .with_program_context(
                            listener_name,
                            &group,
                            process_num,
                            numprocs,
                        );
                        expr.eval_named(template, "process_name")
                            .map_err(|e| ProgramError::ConfigError(e.to_string()))?
                    } else {
                        format!("{}:{}", listener_name, process_num)
                    };
                    instances.push(instance_name);
                }
            }

            for (idx, instance_name) in instances.iter().enumerate() {
                let process_num = numprocs_start + idx;
                let mut expr = crate::config::expand::StringExpression::with_config_dir(
                    self.config_dir
                        .as_deref()
                        .unwrap_or(std::path::Path::new(".")),
                )
                .with_program_context(listener_name, &group, process_num, numprocs);

                // OI-6: seed daemon-level environment into expansion context (see programs).
                for (k, v) in &self.environment {
                    expr.add(format!("ENV_{}", k), v);
                }
                for (k, v) in &raw.environment {
                    expr.add(format!("ENV_{}", k), v);
                }

                let raw_cmd = expr
                    .eval_named(&raw.command, "command")
                    .map_err(|e| ProgramError::ConfigError(e.to_string()))?;

                let platform = crate::platform::native_platform();
                let (command, args) = if raw.args.is_empty() {
                    match platform.split_command_line(&raw_cmd) {
                        Ok(mut parts) if !parts.is_empty() => {
                            let cmd = parts.remove(0);
                            (cmd, parts)
                        }
                        _ => (raw_cmd, Vec::new()),
                    }
                } else {
                    let mut evaled_args = Vec::with_capacity(raw.args.len());
                    for a in &raw.args {
                        let evaled_a = expr
                            .eval_named(a, "args")
                            .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                        evaled_args.push(evaled_a);
                    }
                    (raw_cmd, evaled_args)
                };

                let directory = if let Some(ref dir) = raw.directory {
                    let dir_str = dir.to_string_lossy();
                    let evaled_dir = expr
                        .eval_named(&dir_str, "directory")
                        .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                    Some(PathBuf::from(evaled_dir))
                } else {
                    None
                };

                let mut environment = HashMap::new();
                for (k, v) in &raw.environment {
                    let evaled_val = expr
                        .eval_named(v, &format!("environment.{}", k))
                        .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                    environment.insert(k.clone(), evaled_val);
                }

                let stderr = if let Some(ref raw_stderr) = raw.stderr_logfile {
                    let stderr_str = raw_stderr.to_string_lossy();
                    let evaled = expr
                        .eval_named(&stderr_str, "stderr_logfile")
                        .map_err(|e| ProgramError::ConfigError(e.to_string()))?;
                    Some(PathBuf::from(evaled))
                } else {
                    None
                };

                let logs = ProgramLogsConfig {
                    enabled: LogMode::On,
                    stdout: None, // stdout is reserved for the wire protocol
                    stderr,
                    ..Default::default()
                };

                let event_listener = Some(crate::eventlistener::EventListenerConfig::new(
                    listener_name.clone(),
                    raw.events.clone(),
                    raw.buffer_size,
                    raw.result_handler.clone(),
                ));

                let stop_as_group = raw.stop_as_group.unwrap_or(false);
                let kill_as_group = raw.kill_as_group.unwrap_or(stop_as_group);
                if stop_as_group && !kill_as_group {
                    return Err(ProgramError::ConfigError(format!(
                        "Event listener '{}' cannot set stop_as_group=true and kill_as_group=false",
                        listener_name
                    )));
                }

                let prog = ProgramConfig {
                    name: instance_name.clone(),
                    command,
                    args,
                    directory,
                    user: raw.user.clone(),
                    environment,
                    priority,
                    depends_on: Vec::new(),
                    autostart,
                    autorestart,
                    start_secs,
                    start_retries,
                    restart_pause_secs: DEFAULT_RESTART_PAUSE,
                    stop_signal,
                    stop_wait_secs,
                    exit_codes: default_exit_codes(),
                    umask: raw.umask,
                    logs,
                    health_check: None,
                    group: group.clone(),
                    group_priority,
                    cron: None,
                    cron_stop: None,
                    pre_start: None,
                    pre_stop: None,
                    pre_start_ignore_failure: false,
                    hook_timeout_secs: DEFAULT_HOOK_TIMEOUT,
                    restart_when_binary_changed: false,
                    restart_signal_when_binary_changed: None,
                    restart_cmd_when_binary_changed: None,
                    restart_directory_monitor: None,
                    restart_file_pattern: None,
                    restart_signal_when_file_changed: None,
                    restart_cmd_when_file_changed: None,
                    restart_debounce_secs: DEFAULT_RESTART_DEBOUNCE,
                    env_files: raw.env_files.clone().unwrap_or_default(),
                    kill_wait_secs: DEFAULT_KILL_WAIT,
                    stop_as_group,
                    kill_as_group,
                    event_listener,
                };

                if resolved.contains_key(instance_name) {
                    return Err(ProgramError::ConfigError(format!(
                        "Duplicate program/eventlistener instance name '{}'",
                        instance_name
                    )));
                }

                resolved.insert(instance_name.clone(), prog);
            }
        }

        // Phase 4: Warn if multiple log streams target the same rotating file with max_bytes > 0
        let mut rotating_files: HashMap<PathBuf, Vec<String>> = HashMap::new();
        for (prog_name, prog) in &resolved {
            if !prog.logs.is_enabled() {
                continue;
            }

            if prog.logs.effective_stdout_max_bytes() > 0
                && let Some(dest) = prog.logs.stdout.as_ref().and_then(|p| {
                    crate::logging::destination::LogDestination::parse(&p.to_string_lossy()).ok()
                })
            {
                for file_path in dest.file_paths() {
                    rotating_files
                        .entry(file_path.to_path_buf())
                        .or_default()
                        .push(format!("{}:stdout", prog_name));
                }
            }

            if !prog.logs.redirect_stderr
                && prog.logs.effective_stderr_max_bytes() > 0
                && let Some(dest) = prog.logs.stderr.as_ref().and_then(|p| {
                    crate::logging::destination::LogDestination::parse(&p.to_string_lossy()).ok()
                })
            {
                for file_path in dest.file_paths() {
                    rotating_files
                        .entry(file_path.to_path_buf())
                        .or_default()
                        .push(format!("{}:stderr", prog_name));
                }
            }
        }

        for (path, streams) in rotating_files {
            if streams.len() > 1 {
                tracing::warn!(
                    path = %path.display(),
                    streams = ?streams,
                    "Multiple log streams target rotating file '{}'. Concurrent rotation may lead to lost logs or premature rotation.",
                    path.display()
                );
            }
        }

        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deny_unknown_fields() {
        let yaml_with_unknown = r#"
programs:
  app:
    command: "sleep 10"
    unknown_custom_field: 4
"#;
        let res = SupervisorConfig::from_yaml_str(yaml_with_unknown);
        assert!(
            res.is_err(),
            "Expected error on unknown field 'unknown_custom_field'"
        );
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("unknown field `unknown_custom_field`"),
            "Expected unknown field error message, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_numprocs_expansion_default_naming() {
        let yaml = r#"
programs:
  worker:
    command: "worker --id=%(process_num)02d --port=80%(process_num)02d"
    numprocs: 3
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).unwrap();
        let resolved = config.resolve_programs().unwrap();

        assert_eq!(resolved.len(), 3);
        assert!(resolved.contains_key("worker:0"));
        assert!(resolved.contains_key("worker:1"));
        assert!(resolved.contains_key("worker:2"));

        assert_eq!(resolved["worker:0"].command, "worker");
        assert_eq!(resolved["worker:0"].args, vec!["--id=00", "--port=8000"]);
        assert_eq!(resolved["worker:1"].args, vec!["--id=01", "--port=8001"]);
        assert_eq!(resolved["worker:2"].args, vec!["--id=02", "--port=8002"]);

        // Shared template group defaults to program base name
        assert_eq!(resolved["worker:0"].group, "worker");
        assert_eq!(resolved["worker:1"].group, "worker");
        assert_eq!(resolved["worker:2"].group, "worker");
    }

    #[test]
    fn test_numprocs_custom_process_name() {
        let yaml = r#"
programs:
  worker:
    command: "worker --id=%(process_num)d"
    numprocs: 2
    numprocs_start: 1
    process_name: "%(program_name)s_proc%(process_num)02d"
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).unwrap();
        let resolved = config.resolve_programs().unwrap();

        assert_eq!(resolved.len(), 2);
        assert!(resolved.contains_key("worker_proc01"));
        assert!(resolved.contains_key("worker_proc02"));

        assert_eq!(resolved["worker_proc01"].args, vec!["--id=1"]);
        assert_eq!(resolved["worker_proc02"].args, vec!["--id=2"]);
    }

    #[test]
    fn test_numprocs_validation_zero() {
        let yaml = r#"
programs:
  worker:
    command: "worker"
    numprocs: 0
"#;
        let res = SupervisorConfig::from_yaml_str(yaml);
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains("numprocs must be greater than 0")
        );
    }

    #[test]
    fn test_numprocs_dag_dependency_expansion() {
        let yaml = r#"
programs:
  redis:
    command: "redis-server"
  worker:
    command: "worker"
    numprocs: 2
    depends_on: ["redis"]
  api:
    command: "api-server"
    depends_on: ["worker"]
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).unwrap();
        let resolved = config.resolve_programs().unwrap();

        // worker:0 and worker:1 depend on redis
        assert_eq!(resolved["worker:0"].depends_on, vec!["redis"]);
        assert_eq!(resolved["worker:1"].depends_on, vec!["redis"]);

        // api depends on all instances of worker
        let api_deps = &resolved["api"].depends_on;
        assert!(api_deps.contains(&"worker:0".to_string()));
        assert!(api_deps.contains(&"worker:1".to_string()));

        // DAG builds successfully with all expanded instances
        let dag = crate::manager::DependencyGraph::build(&resolved).unwrap();
        assert_eq!(dag.start_layers[0], vec!["redis"]);
        assert!(dag.start_layers[1].contains(&"worker:0".to_string()));
        assert!(dag.start_layers[1].contains(&"worker:1".to_string()));
        assert_eq!(dag.start_layers[2], vec!["api"]);
    }

    #[test]
    fn test_numprocs_isolated_log_paths() {
        let yaml = r#"
programs:
  worker:
    command: "worker"
    numprocs: 2
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).unwrap();
        let resolved = config.resolve_programs().unwrap();

        let stdout0 = resolved["worker:0"]
            .logs
            .stdout
            .as_ref()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let stdout1 = resolved["worker:1"]
            .logs
            .stdout
            .as_ref()
            .unwrap()
            .to_string_lossy()
            .to_string();

        assert_ne!(stdout0, stdout1);
        assert!(stdout0.contains("worker_0") || stdout0.contains("worker:0"));
        assert!(stdout1.contains("worker_1") || stdout1.contains("worker:1"));
    }

    #[test]
    fn test_stop_signal_aliases() {
        let yaml = r#"
programs:
  sig_test:
    command: "echo test"
    stop_signal: SIGTERM
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).expect("Failed to parse SIGTERM");
        let resolved = config.resolve_programs().unwrap();
        assert_eq!(resolved["sig_test"].stop_signal, StopSignal::Term);

        let yaml_lower = r#"
programs:
  sig_test:
    command: "echo test"
    stop_signal: sigterm
"#;
        let config_lower =
            SupervisorConfig::from_yaml_str(yaml_lower).expect("Failed to parse sigterm");
        let resolved_lower = config_lower.resolve_programs().unwrap();
        assert_eq!(resolved_lower["sig_test"].stop_signal, StopSignal::Term);
    }

    #[test]
    fn test_stop_wait_secs_overflow_validation() {
        let yaml = r#"
programs:
  app:
    command: "echo test"
    stop_wait_secs: 100000
"#;
        let res = SupervisorConfig::from_yaml_str(yaml);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("exceeds maximum"));
    }

    #[test]
    fn test_logs_inheritance_override() {
        let yaml = r#"
program_defaults:
  logs:
    enabled: false
    redirect_stderr: true
programs:
  app_inherit:
    command: "echo inherit"
    logs:
      stdout: "/tmp/app.log"
  app_override:
    command: "echo override"
    logs:
      enabled: true
      redirect_stderr: false
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).expect("Valid YAML");
        let resolved = config.resolve_programs().unwrap();

        // app_inherit should inherit enabled: false and redirect_stderr: true
        let inherit_logs = &resolved["app_inherit"].logs;
        assert!(!inherit_logs.enabled.is_enabled());
        assert!(inherit_logs.redirect_stderr);
        assert_eq!(inherit_logs.stdout, Some(PathBuf::from("/tmp/app.log")));

        // app_override should explicitly override both
        let override_logs = &resolved["app_override"].logs;
        assert!(override_logs.enabled.is_enabled());
        assert!(!override_logs.redirect_stderr);
        assert!(override_logs.stdout.is_some());
    }

    #[test]
    fn test_server_config_basic_auth_fields() {
        let yaml = r#"
server:
  http_bind: ":9001"
  username: "admin"
  password: "thepassword"
programs: {}
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).expect("Valid YAML");
        assert_eq!(config.server.http_bind.as_deref(), Some("0.0.0.0:9001"));
        assert_eq!(config.server.username.as_deref(), Some("admin"));
        assert_eq!(config.server.password.as_deref(), Some("thepassword"));
    }

    #[test]
    fn test_resolved_uds_chmod_defaults() {
        // allow_unelevated=false → 0o770 on Windows, 0o700 elsewhere
        let s = ServerConfig {
            allow_unelevated: false,
            uds_chmod: None,
            ..Default::default()
        };
        #[cfg(windows)]
        assert_eq!(s.resolved_uds_chmod().unwrap(), 0o770);
        #[cfg(not(windows))]
        assert_eq!(s.resolved_uds_chmod().unwrap(), 0o700);

        // allow_unelevated=true → 0o777
        let s = ServerConfig {
            allow_unelevated: true,
            uds_chmod: None,
            ..Default::default()
        };
        assert_eq!(s.resolved_uds_chmod().unwrap(), 0o777);

        // Explicit uds_chmod always wins
        let s = ServerConfig {
            allow_unelevated: true,
            uds_chmod: Some(0o755),
            ..Default::default()
        };
        assert_eq!(s.resolved_uds_chmod().unwrap(), 0o755);

        // Empty string becomes None at deserialize → platform default
        // (covered by optional_chmod; construct None explicitly here)
        let s = ServerConfig {
            allow_unelevated: true,
            uds_chmod: None,
            ..Default::default()
        };
        assert_eq!(s.resolved_uds_chmod().unwrap(), 0o777);

        // Invalid explicit value fails at parse boundary (deserialize_with)
        let bad = r#"
server:
  uds_chmod: "not-octal"
programs: {}
"#;
        assert!(SupervisorConfig::from_yaml_str(bad).is_err());
    }

    #[test]
    fn test_yaml_chmod_alias_and_deny_unknown() {
        // Alias `chmod` maps to uds_chmod
        let yaml = r#"
server:
  chmod: "0750"
programs: {}
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).expect("valid yaml");
        assert_eq!(config.server.uds_chmod, Some(0o750));
        assert_eq!(config.server.resolved_uds_chmod().unwrap(), 0o750);

        // Quoted string form (primary field name)
        let yaml2 = r#"
server:
  uds_chmod: "0700"
programs: {}
"#;
        let config2 = SupervisorConfig::from_yaml_str(yaml2).expect("valid yaml");
        assert_eq!(config2.server.uds_chmod, Some(0o700));

        // deny_unknown_fields still rejects unknown keys
        let bad = r#"
server:
  not_a_real_field: 1
programs: {}
"#;
        assert!(SupervisorConfig::from_yaml_str(bad).is_err());
    }

    #[test]
    fn test_validate_rejects_bad_uds_chmod() {
        let yaml = r#"
server:
  uds_chmod: "not-octal"
programs: {}
"#;
        // Load path already calls validate(); bad mode must fail-fast at parse.
        let err = SupervisorConfig::from_yaml_str(yaml).expect_err("bad uds_chmod must fail");
        let msg = err.to_string();
        assert!(msg.contains("not-octal"), "error must name the mode: {msg}");
    }

    #[test]
    fn test_group_config_resolution_and_validation() {
        let yaml = r#"
groups:
  web:
    programs:
      - frontend
      - backend
    priority: 80
programs:
  frontend:
    command: "echo front"
  backend:
    command: "echo back"
  worker:
    command: "echo worker"
    group: "jobs"
  standalone:
    command: "echo alone"
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).expect("Valid YAML");
        let resolved = config.resolve_programs().unwrap();

        assert_eq!(resolved["frontend"].group, "web");
        assert_eq!(resolved["frontend"].group_priority, 80);
        assert_eq!(resolved["backend"].group, "web");
        assert_eq!(resolved["backend"].group_priority, 80);
        assert_eq!(resolved["worker"].group, "jobs");
        assert_eq!(resolved["worker"].group_priority, 999);
        // standalone has no explicit group, so default group is its own name
        assert_eq!(resolved["standalone"].group, "standalone");
        assert_eq!(resolved["standalone"].group_priority, 999);

        assert_eq!(resolved["frontend"].full_name(), "web:frontend");
        assert_eq!(resolved["worker"].full_name(), "jobs:worker");
        assert_eq!(resolved["standalone"].full_name(), "standalone");

        // Test unknown program in group validation
        let invalid_yaml = r#"
groups:
  web:
    programs:
      - nonexistent
programs:
  frontend:
    command: "echo front"
"#;
        let invalid_res = SupervisorConfig::from_yaml_str(invalid_yaml);
        assert!(invalid_res.is_err());
        assert!(
            invalid_res
                .unwrap_err()
                .to_string()
                .contains("unknown program")
        );
    }

    #[test]
    fn test_restart_watch_fields_and_debounce_defaults() {
        let yaml = r#"
server:
  path_translation: false
program_defaults:
  restart_debounce_secs: 10
programs:
  default_debounce_prog:
    command: "app1"
    restart_when_binary_changed: true
    restart_signal_when_binary_changed: "HUP"
  custom_debounce_prog:
    command: "app2"
    restart_directory_monitor: "src/%(program_name)s"
    restart_file_pattern: "*.rs"
    restart_debounce_secs: 2
    restart_signal_when_file_changed: "SIGTERM"
  vanilla_prog:
    command: "app3"
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).unwrap();
        let resolved = config.resolve_programs().unwrap();

        let p1 = &resolved["default_debounce_prog"];
        assert!(p1.restart_when_binary_changed);
        assert_eq!(p1.restart_signal_when_binary_changed, Some(StopSignal::Hup));
        assert_eq!(p1.restart_debounce_secs, Duration::from_secs(10)); // Inherited from defaults

        let p2 = &resolved["custom_debounce_prog"];
        assert_eq!(
            p2.restart_directory_monitor,
            Some(PathBuf::from("src/custom_debounce_prog"))
        );
        assert_eq!(p2.restart_file_pattern.as_deref(), Some("*.rs"));
        assert_eq!(p2.restart_debounce_secs, Duration::from_secs(2)); // Overridden
        assert_eq!(p2.restart_signal_when_file_changed, Some(StopSignal::Term));

        let p3 = &resolved["vanilla_prog"];
        assert!(!p3.restart_when_binary_changed);
        assert_eq!(p3.restart_debounce_secs, Duration::from_secs(10)); // Inherited from defaults
    }

    #[test]
    fn test_restart_pause_inheritance() {
        let yaml = r#"
server:
  path_translation: false
program_defaults:
  restart_pause_secs: 7
programs:
  inherited_pause:
    command: "app1"
  overridden_pause:
    command: "app2"
    restart_pause_secs: 2
  vanilla_pause:
    command: "app3"
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).unwrap();
        let resolved = config.resolve_programs().unwrap();

        assert_eq!(
            resolved["inherited_pause"].restart_pause_secs,
            Duration::from_secs(7)
        );
        assert_eq!(
            resolved["overridden_pause"].restart_pause_secs,
            Duration::from_secs(2)
        );
        assert_eq!(
            resolved["vanilla_pause"].restart_pause_secs,
            Duration::from_secs(7)
        );
    }

    #[test]
    fn test_ctl_absent_section_not_synthesized_on_load() {
        // No ctl section: load leaves None; resolve builds Vec via vec_from_server.
        let yaml = r#"
server:
  http_bind: "127.0.0.1:9001"
  username: "admin"
  password: "secret"
  auth_token: "tok"
programs: {}
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).expect("Valid YAML");
        assert!(config.server.ctl_defaults, "YAML default must be true");
        assert!(
            config.ctl.is_none(),
            "load must not synthesize a ctl section"
        );
        let chain = CtlConfig::vec_from_server(&config.server);
        assert_eq!(chain.len(), 2, "IPC + TCP candidates");
        assert_eq!(
            chain[0].username.as_deref(),
            Some("admin"),
            "IPC inherits uds_username (Scheme B defaults from username)"
        );
        assert_eq!(chain[0].auth_token.as_deref(), Some("tok"));
        assert_eq!(chain[1].serverurl.as_deref(), Some("http://127.0.0.1:9001"));
        assert_eq!(chain[1].username.as_deref(), Some("admin"));
        assert_eq!(chain[1].password.as_deref(), Some("secret"));
        assert_eq!(chain[1].auth_token.as_deref(), Some("tok"));
    }

    #[test]
    fn test_ctl_fill_preserves_existing_none_only_fields() {
        let yaml = r#"
server:
  http_bind: "127.0.0.1:9001"
  username: "admin"
  password: "secret"
ctl:
  serverurl: "http://custom:1234"
programs: {}
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).expect("Valid YAML");
        let ctl = config.ctl.as_ref().expect("ctl must exist");
        assert_eq!(ctl.serverurl.as_deref(), Some("http://custom:1234"));
        assert_eq!(ctl.username.as_deref(), Some("admin"));
        assert_eq!(ctl.password.as_deref(), Some("secret"));
    }

    #[test]
    fn test_ctl_fill_disabled_when_defaults_false() {
        let yaml = r#"
server:
  http_bind: "127.0.0.1:9001"
  ctl_defaults: false
programs: {}
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).expect("Valid YAML");
        assert!(!config.server.ctl_defaults);
        assert!(
            config.ctl.is_none(),
            "ctl_defaults=false must leave ctl as None"
        );
    }

    #[test]
    fn test_ctl_supervisorctl_yaml_alias() {
        let yaml = r#"
supervisorctl:
  serverurl: "http://127.0.0.1:9001"
  username: "u"
  password: "p"
programs: {}
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).expect("Valid YAML");
        let ctl = config.ctl.as_ref().expect("alias must map to ctl");
        assert_eq!(ctl.serverurl.as_deref(), Some("http://127.0.0.1:9001"));
        assert_eq!(ctl.username.as_deref(), Some("u"));
        assert_eq!(ctl.password.as_deref(), Some("p"));
    }

    #[test]
    fn test_ctl_fill_credentials_follow_endpoint_type() {
        // Partial section with IPC serverurl: fill picks uds credentials.
        let yaml = r#"
server:
  uds_path: "/tmp/s.sock"
  uds_username: "ipcuser"
  uds_password: "ipcpass"
  username: "tcpuser"
  password: "tcppass"
ctl:
  serverurl: "/tmp/s.sock"
programs: {}
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).expect("Valid YAML");
        let ctl = config.ctl.as_ref().expect("section must exist");
        assert_eq!(ctl.serverurl.as_deref(), Some("/tmp/s.sock"));
        assert_eq!(ctl.username.as_deref(), Some("ipcuser"));
        assert_eq!(ctl.password.as_deref(), Some("ipcpass"));

        // TCP serverurl: fill picks server username/password instead.
        let yaml = r#"
server:
  uds_path: "/tmp/s.sock"
  uds_username: "ipcuser"
  uds_password: "ipcpass"
  username: "tcpuser"
  password: "tcppass"
ctl:
  serverurl: "http://127.0.0.1:9001"
programs: {}
"#;
        let config = SupervisorConfig::from_yaml_str(yaml).expect("Valid YAML");
        let ctl = config.ctl.as_ref().expect("section must exist");
        assert_eq!(ctl.username.as_deref(), Some("tcpuser"));
        assert_eq!(ctl.password.as_deref(), Some("tcppass"));
    }
}
