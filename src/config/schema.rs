use crate::error::ProgramError;
use crate::program::config::{AutoRestartPolicy, ProgramConfig, ProgramLogsConfig, StopSignal};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_uds_path")]
    pub uds_path: PathBuf,
    #[serde(default)]
    pub http_bind: Option<String>,
    #[serde(default)]
    pub auth_token: Option<String>,
}

fn default_uds_path() -> PathBuf {
    crate::platform::native_platform().default_uds_path()
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
pub struct LoggingConfig {
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

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            file: None,
            level: default_log_level(),
            max_bytes: Some("20MB".to_string()),
            backups: default_backups(),
        }
    }
}

/// Global defaults template for programs (equivalent to legacy [program-default]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    pub logs: Option<ProgramLogsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub logs: Option<ProgramLogsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SupervisorConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub program_defaults: ProgramDefaults,
    #[serde(default)]
    pub programs: HashMap<String, ProgramConfigRaw>,
}

impl SupervisorConfig {
    pub fn from_yaml_str(yaml_content: &str) -> Result<Self, ProgramError> {
        // Expand environment variables first
        let expanded = crate::config::expand::expand_env_vars(yaml_content);
        let config: Self = serde_yaml::from_str(&expanded).map_err(|e| {
            ProgramError::ConfigError(format!("Failed to parse YAML configuration: {}", e))
        })?;
        config.validate()?;
        Ok(config)
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
                let def = self.program_defaults.logs.clone().unwrap_or_default();
                if let Some(ref raw_logs) = raw.logs {
                    ProgramLogsConfig {
                        stdout: raw_logs.stdout.clone().or(def.stdout),
                        stderr: raw_logs.stderr.clone().or(def.stderr),
                        max_bytes: raw_logs.max_bytes.clone().or(def.max_bytes),
                        backups: raw_logs.backups.or(def.backups),
                        redirect_stderr: raw_logs.redirect_stderr || def.redirect_stderr,
                    }
                } else {
                    def
                }
            };

            let (command, args) = if raw.args.is_empty() {
                match shell_words::split(&raw.command) {
                    Ok(mut parts) if !parts.is_empty() => {
                        let cmd = parts.remove(0);
                        (cmd, parts)
                    }
                    _ => (raw.command.clone(), Vec::new()),
                }
            } else {
                (raw.command.clone(), raw.args.clone())
            };

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
            };

            resolved.insert(name.clone(), prog);
        }

        Ok(resolved)
    }
}
