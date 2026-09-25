// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;
use crate::program::config::{AutoRestartPolicy, StopSignal};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

/// Parses autorestart policies accepted by Python Supervisor:
/// `false`, `no`, `never`, `0` -> `AutoRestartPolicy::Never`
/// `true`, `yes`, `always`, `1` -> `AutoRestartPolicy::Always`
/// `unexpected` -> `AutoRestartPolicy::Unexpected`
pub fn parse_autorestart(s: &str) -> Result<AutoRestartPolicy, ProgramError> {
    let trimmed = s.trim();
    if trimmed.eq_ignore_ascii_case("false")
        || trimmed.eq_ignore_ascii_case("no")
        || trimmed.eq_ignore_ascii_case("never")
        || trimmed.eq_ignore_ascii_case("0")
    {
        Ok(AutoRestartPolicy::Never)
    } else if trimmed.eq_ignore_ascii_case("true")
        || trimmed.eq_ignore_ascii_case("yes")
        || trimmed.eq_ignore_ascii_case("always")
        || trimmed.eq_ignore_ascii_case("1")
    {
        Ok(AutoRestartPolicy::Always)
    } else if trimmed.eq_ignore_ascii_case("unexpected") {
        Ok(AutoRestartPolicy::Unexpected)
    } else {
        AutoRestartPolicy::from_str(trimmed).map_err(|_| {
            ProgramError::ConfigError(format!("Invalid autorestart policy '{}'", trimmed))
        })
    }
}

/// Parses Python Supervisor environment variable string:
/// Example: `KEY="val1",KEY2="val2",PORT="8080"`
/// Supports single/double quotes, escaped characters, and commas inside quotes.
pub fn parse_environment(s: &str) -> Result<HashMap<String, String>, ProgramError> {
    let mut env = HashMap::new();
    let mut cur_key = String::new();
    let mut cur_val = String::new();
    let mut in_key = true;
    let mut quote_char: Option<char> = None;
    let mut escape = false;

    let flush_entry = |env: &mut HashMap<String, String>,
                       k: &mut String,
                       v: &mut String,
                       in_key: &mut bool|
     -> Result<(), ProgramError> {
        let key = k.trim().to_string();
        if !key.is_empty() {
            if *in_key {
                return Err(ProgramError::ConfigError(format!(
                    "Environment entry '{}' missing '=' value delimiter",
                    key
                )));
            }
            env.insert(key, v.clone());
        }
        k.clear();
        v.clear();
        *in_key = true;
        Ok(())
    };

    for ch in s.chars() {
        if escape {
            if quote_char.is_some() {
                match ch {
                    'n' => cur_val.push('\n'),
                    't' => cur_val.push('\t'),
                    'r' => cur_val.push('\r'),
                    '"' => cur_val.push('"'),
                    '\'' => cur_val.push('\''),
                    '\\' => cur_val.push('\\'),
                    other => {
                        cur_val.push('\\');
                        cur_val.push(other);
                    }
                }
            } else if in_key {
                cur_key.push(ch);
            } else {
                cur_val.push(ch);
            }
            escape = false;
            continue;
        }

        if ch == '\\' {
            escape = true;
            continue;
        }

        if let Some(q) = quote_char {
            if ch == q {
                quote_char = None;
            } else {
                cur_val.push(ch);
            }
            continue;
        }

        match ch {
            '\'' | '"' => {
                quote_char = Some(ch);
            }
            '=' if in_key => {
                in_key = false;
            }
            ',' => {
                flush_entry(&mut env, &mut cur_key, &mut cur_val, &mut in_key)?;
            }
            _ => {
                if in_key {
                    cur_key.push(ch);
                } else {
                    cur_val.push(ch);
                }
            }
        }
    }

    if quote_char.is_some() {
        return Err(ProgramError::ConfigError(format!(
            "Unclosed quote in environment string: '{}'",
            s
        )));
    }

    flush_entry(&mut env, &mut cur_key, &mut cur_val, &mut in_key)?;

    Ok(env)
}

/// Normalizes log file path:
/// - `AUTO`, `memory`, `in_memory`, `in_memory_only` (case-insensitive) -> `None`
/// - `NONE` / `OFF` / `NULL` / `/dev/null` -> `Some(PathBuf::from("/dev/null"))`
/// - empty -> `None`
/// - any other path -> `Some(PathBuf::from(s))`
pub fn parse_log_path(s: &str) -> Option<PathBuf> {
    let trimmed = s.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("AUTO")
        || trimmed.eq_ignore_ascii_case("memory")
        || trimmed.eq_ignore_ascii_case("in_memory")
        || trimmed.eq_ignore_ascii_case("in_memory_only")
    {
        None
    } else if trimmed.eq_ignore_ascii_case("NONE")
        || trimmed.eq_ignore_ascii_case("OFF")
        || trimmed.eq_ignore_ascii_case("NULL")
        || trimmed == "/dev/null"
    {
        Some(PathBuf::from("/dev/null"))
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// Parses a process stop signal by name (e.g. `TERM`, `SIGTERM`, `INT`, `QUIT`, `KILL`).
pub fn parse_stop_signal(s: &str) -> Result<StopSignal, ProgramError> {
    let trimmed = s.trim();
    StopSignal::from_str(trimmed)
        .map_err(|_| ProgramError::ConfigError(format!("Invalid stop signal '{}'", trimmed)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_autorestart() {
        assert_eq!(
            parse_autorestart("false").unwrap(),
            AutoRestartPolicy::Never
        );
        assert_eq!(parse_autorestart("0").unwrap(), AutoRestartPolicy::Never);
        assert_eq!(
            parse_autorestart("never").unwrap(),
            AutoRestartPolicy::Never
        );

        assert_eq!(
            parse_autorestart("true").unwrap(),
            AutoRestartPolicy::Always
        );
        assert_eq!(parse_autorestart("1").unwrap(), AutoRestartPolicy::Always);
        assert_eq!(
            parse_autorestart("always").unwrap(),
            AutoRestartPolicy::Always
        );

        assert_eq!(
            parse_autorestart("unexpected").unwrap(),
            AutoRestartPolicy::Unexpected
        );
    }

    #[test]
    fn test_parse_environment() {
        let env = parse_environment(r#"KEY="val1",KEY2="val2",PORT="8080""#).unwrap();
        assert_eq!(env.get("KEY").unwrap(), "val1");
        assert_eq!(env.get("KEY2").unwrap(), "val2");
        assert_eq!(env.get("PORT").unwrap(), "8080");

        // Embedded comma and quotes
        let env2 = parse_environment(r#"MSG="hello, world",ESCAPED="a\"b\"c""#).unwrap();
        assert_eq!(env2.get("MSG").unwrap(), "hello, world");
        assert_eq!(env2.get("ESCAPED").unwrap(), "a\"b\"c");

        // Macro preservation
        let env3 =
            parse_environment(r#"PROC="%(process_num)02d",TAG="%(ENV_COMPAT_TAG)s""#).unwrap();
        assert_eq!(env3.get("PROC").unwrap(), "%(process_num)02d");
        assert_eq!(env3.get("TAG").unwrap(), "%(ENV_COMPAT_TAG)s");
    }

    #[test]
    fn test_parse_log_path() {
        assert_eq!(parse_log_path("AUTO"), None);
        assert_eq!(parse_log_path("auto"), None);
        assert_eq!(parse_log_path(""), None);
        assert_eq!(parse_log_path("NONE"), Some(PathBuf::from("/dev/null")));
        assert_eq!(
            parse_log_path("/dev/null"),
            Some(PathBuf::from("/dev/null"))
        );
        assert_eq!(
            parse_log_path("/var/log/my.log"),
            Some(PathBuf::from("/var/log/my.log"))
        );
    }

    #[test]
    fn test_parse_stop_signal() {
        assert_eq!(parse_stop_signal("TERM").unwrap(), StopSignal::Term);
        assert_eq!(parse_stop_signal("SIGTERM").unwrap(), StopSignal::Term);
        assert_eq!(parse_stop_signal("QUIT").unwrap(), StopSignal::Quit);
        assert_eq!(parse_stop_signal("KILL").unwrap(), StopSignal::Kill);
    }
}
