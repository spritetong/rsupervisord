// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Derives the canonical daemon command name (`cmd_name`) from `argv[0]`.
/// Replaces trailing "ctl" (case-insensitive) with "d".
/// For example:
/// - `rsupervisord` -> `rsupervisord`
/// - `rsupervisorctl` -> `rsupervisord`
/// - `myctl` -> `myd`
/// - `myd` -> `myd`
/// - `ctl` -> `d`
pub fn derive_cmd_name(argv0: Option<&OsStr>) -> String {
    let raw = match argv0 {
        Some(s) => s.to_string_lossy(),
        None => match std::env::args_os().next() {
            Some(s) => s.to_string_lossy().into_owned().into(),
            None => return "rsupervisord".to_string(),
        },
    };

    let raw_str = raw.as_ref();
    let basename = raw_str.rsplit(['/', '\\']).next().unwrap_or(raw_str);
    let stem = match Path::new(basename).file_stem() {
        Some(s) => s.to_string_lossy(),
        None => return "rsupervisord".to_string(),
    };

    // Normalize Cargo test runner artifacts (e.g. `rsupervisord-097ddb1e4b724e0a` or `platform_tests-03321a0e25168acb`)
    let clean_stem = if let Some((base, hash)) = stem.rsplit_once('-') {
        if hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            base
        } else {
            stem.as_ref()
        }
    } else {
        stem.as_ref()
    };

    // If running under automated integration test harnesses, default to rsupervisord
    if clean_stem.ends_with("_tests")
        || clean_stem.starts_with("test_")
        || clean_stem == "deps"
        || clean_stem.is_empty()
    {
        return "rsupervisord".to_string();
    }

    derive_cmd_name_from_stem(clean_stem)
}

/// Helper to derive `cmd_name` from a clean filename stem.
pub fn derive_cmd_name_from_stem(stem: &str) -> String {
    let len = stem.len();
    if len >= 3 && stem[len - 3..].eq_ignore_ascii_case("ctl") {
        format!("{}d", &stem[..len - 3])
    } else {
        stem.to_string()
    }
}

/// Convenience function returning the active `cmd_name` for the current execution.
pub fn get_cmd_name() -> String {
    derive_cmd_name(None)
}

/// Returns the directory containing the executable binary.
pub fn get_executable_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        return parent.to_path_buf();
    }

    if let Some(argv0) = std::env::args_os().next() {
        let p = Path::new(&argv0);
        if let Some(parent) = p.parent()
            && !parent.as_os_str().is_empty()
        {
            return parent.to_path_buf();
        }
    }

    PathBuf::from(".")
}

/// Searches for the default configuration file location in strict priority order:
/// 1. Environment variable `<UPPERCASE_CMD_NAME>_CONFIG`
/// 2. `<executable path>/<cmd_name>.yaml`
/// 3. `<executable path>/<cmd_name>/config.yaml`
/// 4. OS-specific path:
///    - Windows: None
///    - Unix: `/etc/<cmd_name>/config.yaml`
pub fn find_default_config_path(cmd_name: &str) -> Option<PathBuf> {
    // 1. Environment variable `<UPPERCASE_CMD_NAME>_CONFIG`
    let env_name = format!("{}_CONFIG", cmd_name.to_uppercase());
    if let Ok(val) = std::env::var(&env_name) {
        let p = PathBuf::from(val.trim());
        if p.is_file() {
            return Some(p);
        }
    }

    let exe_dir = get_executable_dir();

    // 2. `<executable path>/<cmd_name>.yaml`
    let p2 = exe_dir.join(format!("{}.yaml", cmd_name));
    if p2.is_file() {
        return Some(p2);
    }
    // Also check symlink parent if argv[0] directory differs from exe_dir
    if let Some(argv0) = std::env::args_os().next() {
        let p = Path::new(&argv0);
        if let Some(parent) = p.parent()
            && !parent.as_os_str().is_empty()
            && parent != exe_dir
        {
            let p2_sym = parent.join(format!("{}.yaml", cmd_name));
            if p2_sym.is_file() {
                return Some(p2_sym);
            }
        }
    }

    // 3. `<executable path>/<cmd_name>/config.yaml`
    let p3 = exe_dir.join(cmd_name).join("config.yaml");
    if p3.is_file() {
        return Some(p3);
    }
    if let Some(argv0) = std::env::args_os().next() {
        let p = Path::new(&argv0);
        if let Some(parent) = p.parent()
            && !parent.as_os_str().is_empty()
            && parent != exe_dir
        {
            let p3_sym = parent.join(cmd_name).join("config.yaml");
            if p3_sym.is_file() {
                return Some(p3_sym);
            }
        }
    }

    // 4. OS-specific path (Unix: /etc/<cmd_name>/config.yaml)
    #[cfg(unix)]
    {
        let p4 = PathBuf::from(format!("/etc/{}/config.yaml", cmd_name));
        if p4.is_file() {
            return Some(p4);
        }
    }

    None
}

/// Fallback path to attempt loading or reporting when no candidate in the search order exists.
pub fn get_default_config_path_fallback(cmd_name: &str) -> PathBuf {
    let env_name = format!("{}_CONFIG", cmd_name.to_uppercase());
    if let Ok(val) = std::env::var(&env_name) {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    get_executable_dir().join(format!("{}.yaml", cmd_name))
}

/// Returns the default local IPC path:
/// - Windows: Named Pipe `\\.\pipe\<cmd_name>`
/// - Unix: Unix Domain Socket `/var/run/<cmd_name>.sock`
pub fn default_uds_path(cmd_name: &str, config_dir: Option<&Path>) -> PathBuf {
    #[cfg(unix)]
    {
        let _ = config_dir;
        PathBuf::from(format!("/var/run/{}.sock", cmd_name))
    }
    #[cfg(windows)]
    {
        let _ = config_dir;
        default_named_pipe_path(cmd_name)
    }
}

/// Returns the default Windows named pipe path (`\\.\pipe\<cmd_name>`).
pub fn default_named_pipe_path(cmd_name: &str) -> PathBuf {
    PathBuf::from(format!(r"\\.\pipe\{}", cmd_name))
}

/// Returns the default log path for the daemon itself (`<cmd_name>.log`).
/// - Windows: `<config dir>/logs/<cmd_name>.log`
/// - Unix: `/var/log/<cmd_name>/<cmd_name>.log`
pub fn default_daemon_log_path(cmd_name: &str, config_dir: Option<&Path>) -> PathBuf {
    #[cfg(unix)]
    {
        let _ = config_dir;
        PathBuf::from(format!("/var/log/{}/{}.log", cmd_name, cmd_name))
    }
    #[cfg(windows)]
    {
        let dir = config_dir.unwrap_or_else(|| Path::new("."));
        dir.join("logs").join(format!("{}.log", cmd_name))
    }
}

/// Returns the default log path for a supervised program (`<program_name>.log`).
/// - Windows: `<config dir>/logs/<program_name>.log`
/// - Unix: `/var/log/<cmd_name>/<program_name>.log`
pub fn default_program_log_path(
    cmd_name: &str,
    program_name: &str,
    config_dir: Option<&Path>,
) -> PathBuf {
    #[cfg(unix)]
    {
        let _ = config_dir;
        PathBuf::from(format!("/var/log/{}/{}.log", cmd_name, program_name))
    }
    #[cfg(windows)]
    {
        let _ = cmd_name;
        let dir = config_dir.unwrap_or_else(|| Path::new("."));
        dir.join("logs").join(format!("{}.log", program_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use tempfile::tempdir;

    #[test]
    fn test_derive_cmd_name() {
        assert_eq!(derive_cmd_name_from_stem("rsupervisord"), "rsupervisord");
        assert_eq!(derive_cmd_name_from_stem("rsupervisorctl"), "rsupervisord");
        assert_eq!(derive_cmd_name_from_stem("myctl"), "myd");
        assert_eq!(derive_cmd_name_from_stem("MYCTL"), "MYd");
        assert_eq!(derive_cmd_name_from_stem("ctl"), "d");
        assert_eq!(derive_cmd_name_from_stem("supervisord"), "supervisord");
        assert_eq!(derive_cmd_name_from_stem("worker"), "worker");

        assert_eq!(
            derive_cmd_name(Some(OsStr::new("rsupervisorctl"))),
            "rsupervisord"
        );
        assert_eq!(
            derive_cmd_name(Some(OsStr::new("rsupervisorctl.exe"))),
            "rsupervisord"
        );
        assert_eq!(
            derive_cmd_name(Some(OsStr::new("/usr/local/bin/myctl"))),
            "myd"
        );
        assert_eq!(
            derive_cmd_name(Some(OsStr::new(r"C:\bin\customctl.exe"))),
            "customd"
        );
    }

    #[test]
    fn test_default_paths_structure() {
        #[cfg(unix)]
        {
            let config_dir = PathBuf::from("/etc/myapp");
            assert_eq!(
                default_uds_path("myappd", Some(&config_dir)),
                PathBuf::from("/var/run/myappd.sock")
            );
            assert_eq!(
                default_daemon_log_path("myappd", Some(&config_dir)),
                PathBuf::from("/var/log/myappd/myappd.log")
            );
            assert_eq!(
                default_program_log_path("myappd", "worker1", Some(&config_dir)),
                PathBuf::from("/var/log/myappd/worker1.log")
            );
        }

        #[cfg(windows)]
        {
            let win_dir = PathBuf::from(r"C:\myapp");
            assert_eq!(
                default_uds_path("myappd", Some(&win_dir)),
                PathBuf::from(r"\\.\pipe\myappd")
            );
            assert_eq!(
                default_daemon_log_path("myappd", Some(&win_dir)),
                PathBuf::from(r"C:\myapp\logs\myappd.log")
            );
            assert_eq!(
                default_program_log_path("myappd", "worker1", Some(&win_dir)),
                PathBuf::from(r"C:\myapp\logs\worker1.log")
            );
        }
    }

    #[test]
    fn test_find_default_config_env_priority() {
        let dir = tempdir().unwrap();
        let env_cfg = dir.path().join("env_override.yaml");
        std::fs::write(&env_cfg, "test: true").unwrap();

        let env_var = "TESTCMD_CONFIG";
        unsafe {
            std::env::set_var(env_var, env_cfg.to_str().unwrap());
        }

        let found = find_default_config_path("testcmd");
        assert_eq!(found, Some(env_cfg));

        unsafe {
            std::env::remove_var(env_var);
        }
    }
}
