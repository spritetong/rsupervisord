// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Path resolution engine for supervisor configurations, logs, and IPC sockets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathResolver<'a> {
    cmd_name: Cow<'a, str>,
    config_dir: Option<Cow<'a, Path>>,
}

impl<'a> PathResolver<'a> {
    /// Creates a new PathResolver bound to the given command name.
    pub fn new(cmd_name: impl Into<Cow<'a, str>>) -> Self {
        Self {
            cmd_name: cmd_name.into(),
            config_dir: None,
        }
    }

    /// Sets the optional configuration directory for path resolution.
    pub fn with_config_dir(mut self, config_dir: Option<impl Into<Cow<'a, Path>>>) -> Self {
        self.config_dir = config_dir.map(Into::into);
        self
    }

    /// Creates a PathResolver automatically deriving the command name from the current executable.
    pub fn from_current_exe() -> PathResolver<'static> {
        let cmd = derive_cmd_name(None);
        PathResolver {
            cmd_name: Cow::Owned(cmd),
            config_dir: None,
        }
    }

    /// Returns the command name.
    pub fn cmd_name(&self) -> &str {
        &self.cmd_name
    }

    /// Returns the active configuration directory, if configured.
    pub fn config_dir(&self) -> Option<&Path> {
        self.config_dir.as_deref()
    }

    /// Searches for the default configuration file location in strict priority order:
    /// 1. Environment variable `<UPPERCASE_CMD_NAME>_CONFIG`
    /// 2. Current Working Directory (CWD): `./<cmd_name>.<ext>`, `./etc/<cmd_name>.<ext>`
    /// 3. User XDG configuration directory (`$XDG_CONFIG_HOME/<cmd_name>/<cmd_name>.<ext>` or `~/.config/<cmd_name>/<cmd_name>.<ext>`)
    /// 4. Executable directory & relative `etc`: `<exe_dir>/<cmd_name>.<ext>`, `<exe_dir>/<cmd_name>/<cmd_name>.<ext>`, `<exe_dir>/../etc/<cmd_name>.<ext>`
    /// 5. OS-specific system path: `/etc/<cmd_name>/config.*`, `/etc/supervisor/supervisord.conf`, `/etc/supervisord.conf`
    pub fn find_default_config_path(&self) -> Option<PathBuf> {
        // 1. Environment variable <UPPERCASE_CMD_NAME>_CONFIG
        let env_name = format!("{}_CONFIG", self.cmd_name.to_uppercase());
        if let Ok(val) = std::env::var(&env_name) {
            let p = PathBuf::from(val.trim());
            if p.is_file() {
                return Some(p);
            }
        }

        // YAML is prior, do not change the order!
        let extensions = [".yaml", ".yml", ".conf", ".ini"];

        // 2. Current Working Directory (CWD) & ./etc/ (Python Supervisor compatibility)
        if let Ok(cwd) = std::env::current_dir() {
            for ext in &extensions {
                let p = cwd.join(format!("{}{}", self.cmd_name, ext));
                if p.is_file() {
                    return Some(p);
                }
            }
            let cwd_etc = cwd.join("etc");
            for ext in &extensions {
                let p = cwd_etc.join(format!("{}{}", self.cmd_name, ext));
                if p.is_file() {
                    return Some(p);
                }
            }
        }

        // 3. User configuration directory (~/.config/<cmd_name>/...)
        if let Some(user_config_home) = get_user_config_dir() {
            let user_dir = user_config_home.join(self.cmd_name.as_ref());
            for ext in &extensions {
                let p = user_dir.join(format!("{}{}", self.cmd_name, ext));
                if p.is_file() {
                    return Some(p);
                }
            }
        }

        // 4. Executable directory & sub-directory candidates
        let exe_dir = get_executable_dir();
        for ext in &extensions {
            let p = exe_dir.join(format!("{}{}", self.cmd_name, ext));
            if p.is_file() {
                return Some(p);
            }
        }
        let exe_sub_dir = exe_dir.join(self.cmd_name.as_ref());
        for ext in &extensions {
            let p = exe_sub_dir.join(format!("{}{}", self.cmd_name, ext));
            if p.is_file() {
                return Some(p);
            }
        }

        // Relative etc from executable directory
        if let Some(parent) = exe_dir.parent() {
            let exe_etc = parent.join("etc");
            for ext in &extensions {
                let p = exe_etc.join(format!("{}{}", self.cmd_name, ext));
                if p.is_file() {
                    return Some(p);
                }
            }
        }

        // 5. OS-specific system configuration paths
        if let Some(sys_dir) =
            crate::platform::native_platform().default_system_config_dir(&self.cmd_name)
        {
            for ext in &extensions {
                let p = sys_dir.join(format!("config{}", ext));
                if p.is_file() {
                    return Some(p);
                }
            }
        }

        let sys_candidates = [
            PathBuf::from("/etc/supervisor/supervisord.conf"),
            PathBuf::from("/etc/supervisord.conf"),
        ];
        for p in &sys_candidates {
            if p.is_file() {
                return Some(p.clone());
            }
        }

        None
    }

    /// Fallback path to attempt loading or reporting when no candidate in the search order exists.
    pub fn default_config_path_fallback(&self) -> PathBuf {
        let env_name = format!("{}_CONFIG", self.cmd_name.to_uppercase());
        if let Ok(val) = std::env::var(&env_name) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
        get_executable_dir().join(format!("{}.conf", self.cmd_name))
    }

    /// Returns the default local IPC path (named pipe on Windows, Unix domain socket on Unix).
    pub fn default_uds_path(&self) -> PathBuf {
        crate::platform::native_platform()
            .default_local_ipc_path(&self.cmd_name, self.config_dir.as_deref())
    }

    /// Returns the default Windows named pipe path (`\\.\pipe\<cmd_name>`).
    pub fn default_named_pipe_path(&self) -> PathBuf {
        PathBuf::from(format!(r"\\.\pipe\{}", self.cmd_name))
    }

    /// Returns the default log path for the daemon itself (`<cmd_name>.log`).
    pub fn default_daemon_log_path(&self) -> PathBuf {
        crate::platform::native_platform()
            .default_daemon_log_path(&self.cmd_name, self.config_dir.as_deref())
    }

    /// Returns the default log path for a supervised program (`<program_name>.log`).
    pub fn default_program_log_path(&self, program_name: &str) -> PathBuf {
        crate::platform::native_platform().default_program_log_path(
            &self.cmd_name,
            program_name,
            self.config_dir.as_deref(),
        )
    }
}

/// Derives the canonical daemon command name (`cmd_name`) from `exe_path()`
/// (or an explicit path override). Replaces trailing "ctl" (case-insensitive)
/// with "d".
pub fn derive_cmd_name(argv0: Option<&OsStr>) -> String {
    let path = match argv0.map(Path::new) {
        Some(p) => p,
        None => exe_path(),
    };
    let raw_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let file_name = raw_name.rsplit('\\').next().unwrap_or(raw_name);
    let stem = match Path::new(file_name)
        .file_stem()
        .map(|s| s.to_string_lossy())
    {
        Some(s) if !s.is_empty() => s,
        _ => return "supervisord".to_string(),
    };

    // Normalize Cargo test runner artifacts (e.g. `rsupervisord-097ddb1e4b724e0a`)
    let clean_stem = if let Some((base, hash)) = stem.rsplit_once('-') {
        if hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            base
        } else {
            stem.as_ref()
        }
    } else {
        stem.as_ref()
    };

    // If running under automated integration test harnesses, default to supervisord
    if clean_stem.ends_with("_tests")
        || clean_stem.starts_with("test_")
        || clean_stem == "deps"
        || clean_stem.is_empty()
    {
        return "supervisord".to_string();
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
#[inline]
pub fn get_cmd_name() -> String {
    derive_cmd_name(None)
}

/// Resolves the user-level configuration base directory (e.g. `$XDG_CONFIG_HOME`, `~/.config`, `%APPDATA%`).
pub fn get_user_config_dir() -> Option<PathBuf> {
    if let Ok(val) = std::env::var("XDG_CONFIG_HOME") {
        let p = PathBuf::from(val.trim());
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    if let Ok(val) = std::env::var("HOME") {
        let p = PathBuf::from(val.trim());
        if !p.as_os_str().is_empty() {
            return Some(p.join(".config"));
        }
    }
    if let Ok(val) = std::env::var("APPDATA") {
        let p = PathBuf::from(val.trim());
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    if let Ok(val) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(val.trim());
        if !p.as_os_str().is_empty() {
            return Some(p.join(".config"));
        }
    }
    None
}

/// Returns the directory containing the executable binary.
pub fn get_executable_dir() -> &'static Path {
    if let Some(parent) = exe_path().parent()
        && !parent.as_os_str().is_empty()
    {
        parent
    } else {
        Path::new(".")
    }
}

/// Returns the absolute path of the running executable derived from `argv[0]`.
///
/// Intended to be read before CLI argument parsing. Computed once and cached in
/// a function-local `static LazyLock`. The path is standardized with
/// [`crate::platform::abs_path`] (never resolves symbolic links).
///
/// Processing pipeline:
/// 1. Reads `argv[0]`. If it is a bare name without path separators, attempts
///    `PATH` lookup via [`which`] (preserving the symlink path in `PATH`).
/// 2. On Windows, appends `.exe` if the extension is missing.
/// 3. Normalizes to an absolute path via `std::path::absolute` (which never
///    resolves symlinks).
pub fn exe_path() -> &'static Path {
    static EXE_PATH: LazyLock<PathBuf> = LazyLock::new(|| {
        std::env::args_os()
            .next()
            .map(PathBuf::from)
            .and_then(|mut argv0| {
                which::which(&argv0).ok().or_else(|| {
                    if cfg!(windows)
                        && argv0
                            .extension()
                            .and_then(|e| e.to_str())
                            .is_none_or(|e| !e.eq_ignore_ascii_case("exe"))
                    {
                        let mut os = argv0.into_os_string();
                        os.push(".exe");
                        argv0 = os.into();
                    }
                    let path = crate::platform::abs_path(&argv0);
                    if path.is_file() { Some(path) } else { None }
                })
            })
            .ok_or_else(std::env::current_exe)
            .unwrap()
    });
    &EXE_PATH
}

/// Resolves the effective configuration path for the daemon:
/// explicit `-c` value → default search → fallback; result is always
/// standardized with [`crate::platform::abs_path`] (absolute + lexical norm,
/// never resolving symbolic links).
pub fn resolve_config_path(cmd_name: &str, explicit: Option<&Path>) -> anyhow::Result<PathBuf> {
    let resolved = if let Some(p) = explicit {
        p.to_path_buf()
    } else {
        find_default_config_path(cmd_name)
            .unwrap_or_else(|| get_default_config_path_fallback(cmd_name))
    };
    Ok(crate::platform::abs_path(&resolved))
}

/// Resolves the daemon executable path for service installation.
///
/// Uses [`exe_path`] (derived from `argv[0]`). The companion `*ctl` binary
/// resolves the sibling `<cmd_name>[.exe]` next to itself (reusing
/// `derive_cmd_name_from_stem`'s ctl detection); the daemon returns its own
/// executable path.
pub fn find_daemon_exe(cmd_name: &str) -> PathBuf {
    let exe = exe_path();
    let is_ctl = exe.file_stem().map(|s| {
        let s = s.to_string_lossy();
        derive_cmd_name_from_stem(&s) != s.as_ref()
    });
    if is_ctl == Some(true) {
        exe.with_file_name(format!("{}{}", cmd_name, std::env::consts::EXE_SUFFIX))
    } else {
        exe.to_owned()
    }
}

/// Searches for the default configuration file location in strict priority order.
#[inline]
pub fn find_default_config_path(cmd_name: &str) -> Option<PathBuf> {
    PathResolver::new(cmd_name).find_default_config_path()
}

/// Fallback path to attempt loading or reporting when no candidate in the search order exists.
#[inline]
pub fn get_default_config_path_fallback(cmd_name: &str) -> PathBuf {
    PathResolver::new(cmd_name).default_config_path_fallback()
}

/// Returns the default local IPC path (named pipe on Windows, Unix domain socket on Unix).
#[inline]
pub fn default_uds_path(cmd_name: &str, config_dir: Option<&Path>) -> PathBuf {
    PathResolver::new(cmd_name)
        .with_config_dir(config_dir)
        .default_uds_path()
}

/// Returns the default Windows named pipe path (`\\.\pipe\<cmd_name>`).
#[inline]
pub fn default_named_pipe_path(cmd_name: &str) -> PathBuf {
    PathResolver::new(cmd_name).default_named_pipe_path()
}

/// Returns the default log path for the daemon itself (`<cmd_name>.log`).
#[inline]
pub fn default_daemon_log_path(cmd_name: &str, config_dir: Option<&Path>) -> PathBuf {
    PathResolver::new(cmd_name)
        .with_config_dir(config_dir)
        .default_daemon_log_path()
}

/// Returns the default log path for a supervised program (`<program_name>.log`).
#[inline]
pub fn default_program_log_path(
    cmd_name: &str,
    program_name: &str,
    config_dir: Option<&Path>,
) -> PathBuf {
    PathResolver::new(cmd_name)
        .with_config_dir(config_dir)
        .default_program_log_path(program_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use tempfile::tempdir;

    #[test]
    fn test_derive_cmd_name() {
        assert_eq!(derive_cmd_name_from_stem("supervisord"), "supervisord");
        assert_eq!(derive_cmd_name_from_stem("supervisorctl"), "supervisord");
        assert_eq!(derive_cmd_name_from_stem("myctl"), "myd");
        assert_eq!(derive_cmd_name_from_stem("MYCTL"), "MYd");
        assert_eq!(derive_cmd_name_from_stem("ctl"), "d");
        assert_eq!(derive_cmd_name_from_stem("worker"), "worker");

        assert_eq!(
            derive_cmd_name(Some(OsStr::new("supervisorctl"))),
            "supervisord"
        );
        assert_eq!(
            derive_cmd_name(Some(OsStr::new("supervisorctl.exe"))),
            "supervisord"
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
    fn test_path_resolver_api() {
        let test_dir = PathBuf::from("myapp_test_dir");
        let resolver = PathResolver::new("myappd").with_config_dir(Some(&test_dir));

        assert_eq!(resolver.cmd_name(), "myappd");
        assert_eq!(resolver.config_dir(), Some(test_dir.as_path()));

        let uds = resolver.default_uds_path();
        assert!(!uds.as_os_str().is_empty());
        let daemon_log = resolver.default_daemon_log_path();
        assert!(daemon_log.to_string_lossy().contains("myappd.log"));
        let prog_log = resolver.default_program_log_path("worker1");
        assert!(prog_log.to_string_lossy().contains("worker1.log"));
    }

    #[test]
    fn test_default_paths_structure() {
        let test_dir = PathBuf::from("myapp_test_dir");
        let uds = default_uds_path("myappd", Some(&test_dir));
        assert!(!uds.as_os_str().is_empty());
        let daemon_log = default_daemon_log_path("myappd", Some(&test_dir));
        assert!(daemon_log.to_string_lossy().contains("myappd.log"));
        let prog_log = default_program_log_path("myappd", "worker1", Some(&test_dir));
        assert!(prog_log.to_string_lossy().contains("worker1.log"));
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

    #[test]
    fn test_resolve_config_path_explicit() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            resolve_config_path("testcmd", Some(Path::new("explicit.yaml"))).unwrap(),
            cwd.join("explicit.yaml")
        );

        let abs = if cfg!(windows) {
            PathBuf::from(r"C:\cfg\daemon.yaml")
        } else {
            PathBuf::from("/etc/daemon.yaml")
        };
        assert_eq!(resolve_config_path("testcmd", Some(&abs)).unwrap(), abs);
    }

    #[test]
    fn test_find_default_config_cwd_priority() {
        let dir = tempdir().unwrap();
        let old_cwd = std::env::current_dir().unwrap();

        let cwd_conf = dir.path().join("mycustomd.conf");
        let cwd_ini = dir.path().join("mycustomd.ini");
        std::fs::write(&cwd_conf, "test: conf").unwrap();
        std::fs::write(&cwd_ini, "test: ini").unwrap();

        std::env::set_current_dir(dir.path()).unwrap();
        let found = find_default_config_path("mycustomd");
        // Restore CWD before asserting so a failure cannot leak the temp dir.
        let _ = std::env::set_current_dir(&old_cwd);

        // .conf takes precedence over .ini when both exist.
        assert_eq!(found.as_ref(), Some(&cwd_conf));
    }

    #[test]
    fn test_find_daemon_exe_resolves_current_executable() {
        // Integration-test harness stems never end in "ctl", so exe_path()
        // (argv[0]-derived) is returned (the ctl sibling branch needs a
        // ctl-named binary).
        let exe = find_daemon_exe("supervisord");
        assert_eq!(exe, exe_path());
    }
}
