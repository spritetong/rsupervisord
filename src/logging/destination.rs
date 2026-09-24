// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

/// Transport protocol for remote syslog targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyslogProto {
    Udp,
    Tcp,
}

impl fmt::Display for SyslogProto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Udp => write!(f, "udp"),
            Self::Tcp => write!(f, "tcp"),
        }
    }
}

/// Address destination target for syslog emission.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SyslogTarget {
    /// Local Unix domain datagram/stream socket (/dev/log, /var/run/syslog, /var/run/log).
    Local,
    /// Remote syslog server via UDP or TCP.
    Remote {
        proto: SyslogProto,
        host: String,
        port: u16,
    },
}

impl fmt::Display for SyslogTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => write!(f, "syslog"),
            Self::Remote { proto, host, port } => write!(f, "syslog@{}:{}:{}", proto, host, port),
        }
    }
}

/// Standard syslog facility per RFC 3164.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum SyslogFacility {
    Kern = 0,
    User = 1,
    Mail = 2,
    Daemon = 3,
    Auth = 4,
    Syslog = 5,
    Lpr = 6,
    News = 7,
    Uucp = 8,
    Cron = 9,
    Authpriv = 10,
    Ftp = 11,
    #[default]
    Local0 = 16,
    Local1 = 17,
    Local2 = 18,
    Local3 = 19,
    Local4 = 20,
    Local5 = 21,
    Local6 = 22,
    Local7 = 23,
}

impl SyslogFacility {
    #[inline]
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

impl FromStr for SyslogFacility {
    type Err = ProgramError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let clean = s.trim().to_uppercase();
        let name = clean.strip_prefix("LOG_").unwrap_or(&clean);
        match name {
            "KERN" => Ok(Self::Kern),
            "USER" => Ok(Self::User),
            "MAIL" => Ok(Self::Mail),
            "DAEMON" => Ok(Self::Daemon),
            "AUTH" => Ok(Self::Auth),
            "SYSLOG" => Ok(Self::Syslog),
            "LPR" => Ok(Self::Lpr),
            "NEWS" => Ok(Self::News),
            "UUCP" => Ok(Self::Uucp),
            "CRON" => Ok(Self::Cron),
            "AUTHPRIV" => Ok(Self::Authpriv),
            "FTP" => Ok(Self::Ftp),
            "LOCAL0" => Ok(Self::Local0),
            "LOCAL1" => Ok(Self::Local1),
            "LOCAL2" => Ok(Self::Local2),
            "LOCAL3" => Ok(Self::Local3),
            "LOCAL4" => Ok(Self::Local4),
            "LOCAL5" => Ok(Self::Local5),
            "LOCAL6" => Ok(Self::Local6),
            "LOCAL7" => Ok(Self::Local7),
            _ => Err(ProgramError::ConfigError(format!(
                "Invalid syslog facility '{}'. Supported: KERN, USER, MAIL, DAEMON, AUTH, SYSLOG, LPR, NEWS, UUCP, CRON, AUTHPRIV, FTP, LOCAL0..LOCAL7",
                s
            ))),
        }
    }
}

/// Standard syslog severity / priority per RFC 3164.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum SyslogSeverity {
    Emerg = 0,
    Alert = 1,
    Crit = 2,
    Err = 3,
    Warning = 4,
    #[default]
    Notice = 5,
    Info = 6,
    Debug = 7,
}

impl SyslogSeverity {
    #[inline]
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

impl FromStr for SyslogSeverity {
    type Err = ProgramError;

    fn from_str(s: &str) -> Result<Self, ProgramError> {
        let clean = s.trim().to_uppercase();
        let name = clean.strip_prefix("LOG_").unwrap_or(&clean);
        match name {
            "EMERG" | "EMERGENCY" => Ok(Self::Emerg),
            "ALERT" => Ok(Self::Alert),
            "CRIT" | "CRITICAL" => Ok(Self::Crit),
            "ERR" | "ERROR" => Ok(Self::Err),
            "WARN" | "WARNING" => Ok(Self::Warning),
            "NOTICE" => Ok(Self::Notice),
            "INFO" => Ok(Self::Info),
            "DEBUG" => Ok(Self::Debug),
            _ => Err(ProgramError::ConfigError(format!(
                "Invalid syslog severity '{}'. Supported: EMERG, ALERT, CRIT, ERR, WARNING, NOTICE, INFO, DEBUG",
                s
            ))),
        }
    }
}

/// Resolved log destination specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogDestination {
    /// In-memory ring buffer (Go AUTO parity; 0 disk writes).
    Auto,
    /// Output discarded completely (/dev/null, none, off).
    Null,
    /// Direct output into parent daemon process stdout.
    DevStdout,
    /// Direct output into parent daemon process stderr.
    DevStderr,
    /// Rotating file on disk.
    File(PathBuf),
    /// Syslog destination (RFC 3164).
    Syslog(SyslogTarget),
    /// Multi-destination fan-out (comma-separated list).
    Composite(Vec<LogDestination>),
}

impl LogDestination {
    /// Parses a log destination string into a `LogDestination`.
    ///
    /// Supports comma-separated multi-destination lists (e.g. `test.log, /dev/stdout`),
    /// `AUTO` memory buffering, `/dev/stdout`, `/dev/stderr`, `syslog`, `syslog@host:port`,
    /// and standard file paths.
    pub fn parse(s: &str) -> Result<Self, ProgramError> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Ok(Self::Null);
        }

        // Check for comma-separated multi-destinations
        if trimmed.contains(',') {
            let tokens: Vec<&str> = trimmed
                .split(',')
                .map(|t| t.trim())
                .filter(|t| !t.is_empty())
                .collect();

            if tokens.is_empty() {
                return Ok(Self::Null);
            }
            if tokens.len() == 1 {
                return Self::parse_single(tokens[0]);
            }

            let mut destinations = Vec::with_capacity(tokens.len());
            for token in tokens {
                destinations.push(Self::parse_single(token)?);
            }
            return Ok(Self::Composite(destinations));
        }

        Self::parse_single(trimmed)
    }

    /// Parses a single trimmed destination token.
    fn parse_single(token: &str) -> Result<Self, ProgramError> {
        if token.is_empty() {
            return Ok(Self::Null);
        }

        let lower = token.to_ascii_lowercase();
        match lower.as_str() {
            "none" | "off" | "null" | "/dev/null" => return Ok(Self::Null),
            "auto" | "memory" => return Ok(Self::Auto),
            "/dev/stdout" => return Ok(Self::DevStdout),
            "/dev/stderr" => return Ok(Self::DevStderr),
            "syslog" => return Ok(Self::Syslog(SyslogTarget::Local)),
            _ => {}
        }

        if let Some(rest) = lower.strip_prefix("syslog@") {
            return Self::parse_syslog_remote(rest);
        }

        // Standard file path destination
        Ok(Self::File(PathBuf::from(token)))
    }

    /// Parses remote syslog address `[proto:]host[:port]`.
    fn parse_syslog_remote(address: &str) -> Result<Self, ProgramError> {
        let (proto, host_port) = if let Some(rest) = address.strip_prefix("udp:") {
            (SyslogProto::Udp, rest)
        } else if let Some(rest) = address.strip_prefix("tcp:") {
            (SyslogProto::Tcp, rest)
        } else if address.contains(':')
            && !address
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == ':')
        {
            // Check for unknown protocol prefix e.g. "http:host"
            let parts: Vec<&str> = address.splitn(2, ':').collect();
            if parts.len() == 2
                && !parts[0].contains('.')
                && parts[0].chars().all(|c| c.is_ascii_alphabetic())
            {
                return Err(ProgramError::ConfigError(format!(
                    "Unsupported syslog protocol '{}' in 'syslog@{}'. Only 'udp' and 'tcp' are supported.",
                    parts[0], address
                )));
            }
            (SyslogProto::Udp, address)
        } else {
            (SyslogProto::Udp, address)
        };

        let (host, port) = if let Some((h, p_str)) = host_port.rsplit_once(':') {
            let port = p_str.parse::<u16>().map_err(|_| {
                ProgramError::ConfigError(format!(
                    "Invalid port '{}' in syslog destination 'syslog@{}'",
                    p_str, address
                ))
            })?;
            (h.trim(), port)
        } else {
            let default_port = match proto {
                SyslogProto::Udp => 514,
                SyslogProto::Tcp => 6514,
            };
            (host_port.trim(), default_port)
        };

        if host.is_empty() {
            return Err(ProgramError::ConfigError(format!(
                "Host cannot be empty in syslog destination 'syslog@{}'",
                address
            )));
        }

        Ok(Self::Syslog(SyslogTarget::Remote {
            proto,
            host: host.to_string(),
            port,
        }))
    }

    /// Returns true if this destination produces no output.
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Returns true if this destination (or any composite member) targets syslog.
    pub fn contains_syslog(&self) -> bool {
        match self {
            Self::Syslog(_) => true,
            Self::Composite(list) => list.iter().any(|d| d.contains_syslog()),
            _ => false,
        }
    }

    /// Returns the primary file path if this destination contains a file sink.
    pub fn primary_file_path(&self) -> Option<&std::path::Path> {
        match self {
            Self::File(p) => Some(p.as_path()),
            Self::Composite(list) => list.first().and_then(|d| d.primary_file_path()),
            _ => None,
        }
    }

    /// Returns all file paths targeted by this destination (including within composite destinations).
    pub fn file_paths(&self) -> Vec<&std::path::Path> {
        let mut paths = Vec::new();
        self.collect_file_paths(&mut paths);
        paths
    }

    fn collect_file_paths<'a>(&'a self, out: &mut Vec<&'a std::path::Path>) {
        match self {
            Self::File(p) => out.push(p.as_path()),
            Self::Composite(list) => {
                for d in list {
                    d.collect_file_paths(out);
                }
            }
            _ => {}
        }
    }
}

/// Options and configuration parameters used when building a concrete [`LogBackend`].
#[derive(Debug, Clone, Copy)]
pub struct BackendBuildOptions<'a> {
    pub program_name: &'a str,
    pub channel: crate::logging::types::LogChannel,
    pub max_bytes: usize,
    pub backups: usize,
    pub timestamp_suffix: bool,
    pub syslog_facility: Option<&'a str>,
    pub syslog_tag: Option<&'a str>,
    pub syslog_priority: Option<&'a str>,
}

impl LogDestination {
    /// Instantiates the concrete `LogBackend` pipeline corresponding to this destination.
    pub fn build_backend(
        &self,
        opts: &BackendBuildOptions<'_>,
    ) -> Result<Option<std::sync::Arc<dyn crate::logging::backend::LogBackend>>, ProgramError> {
        use crate::logging::backend::LogBackend;
        use crate::logging::composite::{CompositeLogBackend, NullLogBackend, StdIoLogBackend};
        use crate::logging::rotator::LogRotator;
        use crate::logging::syslog::SyslogLogBackend;
        use std::sync::Arc;

        match self {
            Self::Null => Ok(Some(Arc::new(NullLogBackend))),
            Self::Auto => Ok(None),
            Self::DevStdout => Ok(Some(Arc::new(StdIoLogBackend::stdout()))),
            Self::DevStderr => Ok(Some(Arc::new(StdIoLogBackend::stderr()))),
            Self::File(path) => {
                let rot = LogRotator::with_options(
                    path,
                    opts.max_bytes,
                    opts.backups,
                    opts.timestamp_suffix,
                )?;
                Ok(Some(Arc::new(rot)))
            }
            Self::Syslog(target) => {
                let facility = opts
                    .syslog_facility
                    .and_then(|f| f.parse::<SyslogFacility>().ok())
                    .unwrap_or_default();
                let severity = opts
                    .syslog_priority
                    .and_then(|p| p.parse::<SyslogSeverity>().ok())
                    .unwrap_or_default();
                let tag = opts.syslog_tag.unwrap_or(opts.program_name);
                let backend = SyslogLogBackend::new(target.clone(), facility, severity, tag)?;
                Ok(Some(Arc::new(backend)))
            }
            Self::Composite(list) => {
                let mut backends: Vec<Arc<dyn LogBackend>> = Vec::new();
                for item in list {
                    if let Some(b) = item.build_backend(opts)? {
                        backends.push(b);
                    }
                }
                if backends.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(Arc::new(CompositeLogBackend::new(backends))))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_destination_parsing() {
        assert_eq!(LogDestination::parse("").unwrap(), LogDestination::Null);
        assert_eq!(LogDestination::parse("none").unwrap(), LogDestination::Null);
        assert_eq!(LogDestination::parse("NONE").unwrap(), LogDestination::Null);
        assert_eq!(
            LogDestination::parse("/dev/null").unwrap(),
            LogDestination::Null
        );
        assert_eq!(LogDestination::parse("AUTO").unwrap(), LogDestination::Auto);
        assert_eq!(LogDestination::parse("auto").unwrap(), LogDestination::Auto);
        assert_eq!(
            LogDestination::parse("memory").unwrap(),
            LogDestination::Auto
        );
        assert_eq!(
            LogDestination::parse("/dev/stdout").unwrap(),
            LogDestination::DevStdout
        );
        assert_eq!(
            LogDestination::parse("/dev/stderr").unwrap(),
            LogDestination::DevStderr
        );
        assert_eq!(
            LogDestination::parse("syslog").unwrap(),
            LogDestination::Syslog(SyslogTarget::Local)
        );
        assert_eq!(
            LogDestination::parse("/var/log/app.log").unwrap(),
            LogDestination::File(PathBuf::from("/var/log/app.log"))
        );
    }

    #[test]
    fn test_remote_syslog_parsing() {
        assert_eq!(
            LogDestination::parse("syslog@logs.internal").unwrap(),
            LogDestination::Syslog(SyslogTarget::Remote {
                proto: SyslogProto::Udp,
                host: "logs.internal".to_string(),
                port: 514,
            })
        );
        assert_eq!(
            LogDestination::parse("syslog@127.0.0.1:1514").unwrap(),
            LogDestination::Syslog(SyslogTarget::Remote {
                proto: SyslogProto::Udp,
                host: "127.0.0.1".to_string(),
                port: 1514,
            })
        );
        assert_eq!(
            LogDestination::parse("syslog@tcp:127.0.0.1").unwrap(),
            LogDestination::Syslog(SyslogTarget::Remote {
                proto: SyslogProto::Tcp,
                host: "127.0.0.1".to_string(),
                port: 6514,
            })
        );
        assert_eq!(
            LogDestination::parse("syslog@tcp:logs.example.com:7001").unwrap(),
            LogDestination::Syslog(SyslogTarget::Remote {
                proto: SyslogProto::Tcp,
                host: "logs.example.com".to_string(),
                port: 7001,
            })
        );
        assert!(LogDestination::parse("syslog@http:example.com").is_err());
    }

    #[test]
    fn test_composite_destination_parsing() {
        let dest = LogDestination::parse("test.log, /dev/stdout").unwrap();
        assert_eq!(
            dest,
            LogDestination::Composite(vec![
                LogDestination::File(PathBuf::from("test.log")),
                LogDestination::DevStdout,
            ])
        );
        assert_eq!(
            dest.primary_file_path(),
            Some(std::path::Path::new("test.log"))
        );

        let multi = LogDestination::parse("a.log, syslog@udp:127.0.0.1:514, /dev/stderr").unwrap();
        assert_eq!(
            multi,
            LogDestination::Composite(vec![
                LogDestination::File(PathBuf::from("a.log")),
                LogDestination::Syslog(SyslogTarget::Remote {
                    proto: SyslogProto::Udp,
                    host: "127.0.0.1".to_string(),
                    port: 514,
                }),
                LogDestination::DevStderr,
            ])
        );
    }

    #[test]
    fn test_syslog_facility_and_severity() {
        assert_eq!(
            "local0".parse::<SyslogFacility>().unwrap(),
            SyslogFacility::Local0
        );
        assert_eq!(
            "LOG_LOCAL7".parse::<SyslogFacility>().unwrap(),
            SyslogFacility::Local7
        );
        assert_eq!(
            "daemon".parse::<SyslogFacility>().unwrap(),
            SyslogFacility::Daemon
        );

        assert_eq!(
            "notice".parse::<SyslogSeverity>().unwrap(),
            SyslogSeverity::Notice
        );
        assert_eq!(
            "LOG_ERR".parse::<SyslogSeverity>().unwrap(),
            SyslogSeverity::Err
        );
        assert_eq!(
            "DEBUG".parse::<SyslogSeverity>().unwrap(),
            SyslogSeverity::Debug
        );
    }
}
