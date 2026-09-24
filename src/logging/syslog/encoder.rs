// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::logging::destination::{SyslogFacility, SyslogSeverity};
use chrono::{DateTime, Local};
use std::time::SystemTime;

/// Maximum length of an RFC 3164 syslog packet.
pub const RFC3164_MAX_LEN: usize = 1024;

/// Formats a syslog record according to RFC 3164 (BSD syslog protocol).
///
/// Wire format: `<PRI>TIMESTAMP HOST TAG: MESSAGE`
/// - `PRI = (facility * 8) + severity`
/// - `TIMESTAMP`: `Mon dd hh:mm:ss` (e.g. `Sep 24 23:30:00`)
/// - `HOST`: Hostname of the originating system
/// - `TAG`: Process or service tag (e.g. `worker-1` or `supervisord`)
/// - Output is truncated to `RFC3164_MAX_LEN` (1024 bytes) per RFC 3164 guidelines.
pub fn format_rfc3164(
    facility: SyslogFacility,
    severity: SyslogSeverity,
    timestamp: SystemTime,
    hostname: &str,
    tag: &str,
    message: &str,
) -> Vec<u8> {
    let pri = (facility.as_u8() * 8) + severity.as_u8();
    let local_time: DateTime<Local> = timestamp.into();
    let ts_str = local_time.format("%b %e %H:%M:%S").to_string();

    let tag_clean = tag.trim_end_matches(':');
    let prefix = format!("<{}>{} {} {}: ", pri, ts_str, hostname, tag_clean);

    let prefix_bytes = prefix.as_bytes();
    let msg_bytes = message.trim_end().as_bytes();

    let mut result =
        Vec::with_capacity(RFC3164_MAX_LEN.min(prefix_bytes.len() + msg_bytes.len() + 1));
    result.extend_from_slice(prefix_bytes);

    let remaining = RFC3164_MAX_LEN.saturating_sub(result.len() + 1);
    if msg_bytes.len() > remaining {
        result.extend_from_slice(&msg_bytes[..remaining]);
    } else {
        result.extend_from_slice(msg_bytes);
    }
    result.push(b'\n');

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_rfc3164_basic() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1727220000);
        let bytes = format_rfc3164(
            SyslogFacility::Local0,
            SyslogSeverity::Notice,
            now,
            "myhost",
            "myprogram",
            "Worker started successfully",
        );
        let s = String::from_utf8(bytes).unwrap();
        // PRI for Local0(16) * 8 + Notice(5) = 133
        assert!(s.starts_with("<133>"));
        assert!(s.contains("myhost myprogram: Worker started successfully\n"));
    }

    #[test]
    fn test_format_rfc3164_truncation() {
        let now = SystemTime::now();
        let long_msg = "A".repeat(2000);
        let bytes = format_rfc3164(
            SyslogFacility::Daemon,
            SyslogSeverity::Err,
            now,
            "localhost",
            "app",
            &long_msg,
        );
        assert!(bytes.len() <= RFC3164_MAX_LEN);
        assert_eq!(*bytes.last().unwrap(), b'\n');
    }
}
