// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::error::ProgramError;
use crate::program::config::{
    AutoRestartPolicy, HealthCheckConfig, ProgramConfig, ProgramLogsConfig, StopSignal,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_uds_path")]
    pub uds_path: PathBuf,
    #[serde(default)]
    pub http_bind: Option<String>,
    #[serde(default)]
    pub auth_token: Option<String>,
}

fn default_uds_path() -> PathBuf {
    let cmd_name = crate::config::paths::get_cmd_name();
    crate::config::paths::default_uds_path(&cmd_name, None)
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            uds_path: default_uds_path(),
            http_bind: None,
            auth_token: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub file: Option<PathBuf>,
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub max_bytes: Option<String>,
    #[serde(default = "default_backups")]
    pub backups: usize,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_backups() -> usize {
    3
}

fn default_true() -> bool {
    true
}

fn default_metrics_idle_timeout() -> u64 {
    30
}

fn default_metrics_interval() -> u64 {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_metrics_idle_timeout")]
    pub idle_timeout_secs: u64,
    #[serde(default = "default_metrics_interval")]
    pub interval_secs: u64,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            idle_timeout_secs: default_metrics_idle_timeout(),
            interval_secs: default_metrics_interval(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            file: None,
            level: default_log_level(),
            max_bytes: Some("20MB".to_string()),
            backups: default_backups(),
        }
    }
}

/// Global defaults template for programs (equivalent to legacy [program-default]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProgramDefaults {
    #[serde(default)]
    pub autostart: Option<bool>,
    #[serde(default)]
    pub autorestart: Option<AutoRestartPolicy>,
    #[serde(default)]
    pub start_secs: Option<u64>,
    #[serde(default)]
    pub start_retries: Option<u32>,
    #[serde(default)]
    pub stop_signal: Option<StopSignal>,
    #[serde(default)]
    pub stop_wait_secs: Option<u64>,
    #[serde(default)]
    pub priority: Option<u8>,
    #[serde(default)]
    pub logs: Option<ProgramLogsConfigRaw>,
    #[serde(default)]
    pub health_check: Option<HealthCheckConfig>,
}

/// Raw representation of program log configuration with optional booleans for inheritance.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProgramLogsConfigRaw {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub stdout: Option<PathBuf>,
    #[serde(default)]
    pub stderr: Option<PathBuf>,
    #[serde(default)]
    pub max_bytes: Option<String>,
    #[serde(default)]
    pub backups: Option<usize>,
    #[serde(default)]
    pub redirect_stderr: Option<bool>,
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
    pub priority: Option<u8>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub autostart: Option<bool>,
    #[serde(default)]
    pub autorestart: Option<AutoRestartPolicy>,
    #[serde(default)]
    pub start_secs: Option<u64>,
    #[serde(default)]
    pub start_retries: Option<u32>,
    #[serde(default)]
    pub stop_signal: Option<StopSignal>,
    #[serde(default)]
    pub stop_wait_secs: Option<u64>,
    #[serde(default)]
    pub exit_codes: Option<Vec<i32>>,
    #[serde(default)]
    pub umask: Option<u32>,
    #[serde(default)]
    pub logs: Option<ProgramLogsConfigRaw>,
    #[serde(default)]
    pub health_check: Option<HealthCheckConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorConfig {
    #[serde(default)]
    pub worker_threads: Option<usize>,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub metrics: MetricsConfig,
    #[serde(default)]
    pub program_defaults: ProgramDefaults,
    #[serde(default)]
    pub programs: HashMap<String, ProgramConfigRaw>,
    #[serde(skip)]
    pub config_dir: Option<PathBuf>,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        let mut config = Self {
            worker_threads: None,
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            metrics: MetricsConfig::default(),
            program_defaults: ProgramDefaults::default(),
            programs: HashMap::new(),
            config_dir: None,
        };
        config.apply_default_paths();
        config
    }
}

impl SupervisorConfig {
    /// Loads and parses a SupervisorConfig from a YAML file, storing its directory for default path resolution.
    pub fn from_file<P: AsRef<std::path::Path>>(path: P) -> Result<Self, ProgramError> {
        let path_ref = path.as_ref();
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
    }

    pub fn from_yaml_str(yaml_content: &str) -> Result<Self, ProgramError> {
        Self::from_yaml_str_with_config_dir(yaml_content, None)
    }

    pub fn from_yaml_str_with_config_dir(
        yaml_content: &str,
        config_dir: Option<&std::path::Path>,
    ) -> Result<Self, ProgramError> {
        // Expand environment variables first
        let expanded = crate::config::expand::expand_env_vars(yaml_content);
        let mut config: Self = serde_yaml::from_str(&expanded).map_err(|e| {
            ProgramError::ConfigError(format!("Failed to parse YAML configuration: {}", e))
        })?;
        config.config_dir = config_dir.map(|p| p.to_path_buf());
        config.apply_default_paths();
        config.validate()?;
        Ok(config)
    }

    pub fn apply_default_paths(&mut self) {
        let cmd_name = crate::config::paths::get_cmd_name();
        let default_no_dir = crate::config::paths::default_uds_path(&cmd_name, None);
        if self.server.uds_path.as_os_str().is_empty() || self.server.uds_path == default_no_dir {
            self.server.uds_path =
                crate::config::paths::default_uds_path(&cmd_name, self.config_dir.as_deref());
        }
        if self.logging.enabled && self.logging.file.is_none() {
            self.logging.file = Some(crate::config::paths::default_daemon_log_path(
                &cmd_name,
                self.config_dir.as_deref(),
            ));
        }
    }

    pub fn validate(&self) -> Result<(), ProgramError> {
        for (name, raw) in &self.programs {
            if raw.command.trim().is_empty() {
                return Err(ProgramError::ConfigError(format!(
                    "Program '{}' command cannot be empty",
                    name
                )));
            }
            let priority = raw
                .priority
                .or(self.program_defaults.priority)
                .unwrap_or(50);
            if priority > 99 {
                return Err(ProgramError::ConfigError(format!(
                    "Program '{}' priority {} must be in range [0, 99]",
                    name, priority
                )));
            }
            let stop_wait = raw
                .stop_wait_secs
                .or(self.program_defaults.stop_wait_secs)
                .unwrap_or(10);
            if stop_wait > 86400 {
                return Err(ProgramError::ConfigError(format!(
                    "Program '{}' stop_wait_secs {} exceeds maximum 86400",
                    name, stop_wait
                )));
            }
        }
        Ok(())
    }

    /// Resolves all raw program definitions by applying program_defaults and splitting commands.
    pub fn resolve_programs(&self) -> Result<HashMap<String, ProgramConfig>, ProgramError> {
        let mut resolved = HashMap::new();

        for (name, raw) in &self.programs {
            let priority = raw
                .priority
                .or(self.program_defaults.priority)
                .unwrap_or(50);

            if priority > 99 {
                return Err(ProgramError::ConfigError(format!(
                    "Program '{}' priority {} must be in range [0, 99]",
                    name, priority
                )));
            }

            let autostart = raw
                .autostart
                .or(self.program_defaults.autostart)
                .unwrap_or(true);

            let autorestart = raw
                .autorestart
                .or(self.program_defaults.autorestart)
                .unwrap_or_default();

            let start_secs = raw
                .start_secs
                .or(self.program_defaults.start_secs)
                .unwrap_or(3);

            let start_retries = raw
                .start_retries
                .or(self.program_defaults.start_retries)
                .unwrap_or(3);

            let stop_signal = raw
                .stop_signal
                .or(self.program_defaults.stop_signal)
                .unwrap_or_default();

            let stop_wait_secs = raw
                .stop_wait_secs
                .or(self.program_defaults.stop_wait_secs)
                .unwrap_or(10);

            let exit_codes = raw.exit_codes.clone().unwrap_or_else(|| vec![0]);

            let logs = {
                let raw_logs = raw.logs.as_ref();
                let def_logs = self.program_defaults.logs.as_ref();

                let enabled = raw_logs
                    .and_then(|l| l.enabled)
                    .or_else(|| def_logs.and_then(|l| l.enabled))
                    .unwrap_or(true);

                let cmd_name = crate::config::paths::get_cmd_name();
                let stdout = raw_logs
                    .and_then(|l| l.stdout.clone())
                    .or_else(|| def_logs.and_then(|l| l.stdout.clone()))
                    .or_else(|| {
                        if enabled {
                            Some(crate::config::paths::default_program_log_path(
                                &cmd_name,
                                name,
                                self.config_dir.as_deref(),
                            ))
                        } else {
                            None
                        }
                    });

                let stderr = raw_logs
                    .and_then(|l| l.stderr.clone())
                    .or_else(|| def_logs.and_then(|l| l.stderr.clone()));

                let max_bytes = raw_logs
                    .and_then(|l| l.max_bytes.clone())
                    .or_else(|| def_logs.and_then(|l| l.max_bytes.clone()));

                let backups = raw_logs
                    .and_then(|l| l.backups)
                    .or_else(|| def_logs.and_then(|l| l.backups));

                let redirect_stderr = raw_logs
                    .and_then(|l| l.redirect_stderr)
                    .or_else(|| def_logs.and_then(|l| l.redirect_stderr))
                    .unwrap_or(false);

                ProgramLogsConfig {
                    enabled,
                    stdout,
                    stderr,
                    max_bytes,
                    backups,
                    redirect_stderr,
                }
            };

            let (command, args) = if raw.args.is_empty() {
                if std::path::Path::new(&raw.command).is_file() {
                    (raw.command.clone(), Vec::new())
                } else {
                    match shell_words::split(&raw.command) {
                        Ok(mut parts) if !parts.is_empty() => {
                            let cmd = parts.remove(0);
                            (cmd, parts)
                        }
                        _ => (raw.command.clone(), Vec::new()),
                    }
                }
            } else {
                (raw.command.clone(), raw.args.clone())
            };

            let health_check = raw
                .health_check
                .clone()
                .or_else(|| self.program_defaults.health_check.clone());

            let prog = ProgramConfig {
                name: name.clone(),
                command,
                args,
                directory: raw.directory.clone(),
                user: raw.user.clone(),
                environment: raw.environment.clone(),
                priority,
                depends_on: raw.depends_on.clone(),
                autostart,
                autorestart,
                start_secs,
                start_retries,
                stop_signal,
                stop_wait_secs,
                exit_codes,
                umask: raw.umask,
                logs,
                health_check,
            };

            resolved.insert(name.clone(), prog);
        }

        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deny_unknown_fields() {
        let yaml_with_numprocs = r#"
programs:
  app:
    command: "sleep 10"
    numprocs: 4
"#;
        let res = SupervisorConfig::from_yaml_str(yaml_with_numprocs);
        assert!(res.is_err(), "Expected error on unknown field 'numprocs'");
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("unknown field `numprocs`"),
            "Expected unknown field error message, got: {}",
            err_msg
        );
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
        assert!(!inherit_logs.enabled);
        assert!(inherit_logs.redirect_stderr);
        assert_eq!(inherit_logs.stdout, Some(PathBuf::from("/tmp/app.log")));

        // app_override should explicitly override both
        let override_logs = &resolved["app_override"].logs;
        assert!(override_logs.enabled);
        assert!(!override_logs.redirect_stderr);
        assert!(override_logs.stdout.is_some());
    }
}
