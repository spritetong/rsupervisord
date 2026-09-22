// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::config::expand::{MacroExpander, StringExpression};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Context passed to [`transform`] describing the parse boundary.
pub struct Ctx<'a> {
    /// Directory of the configuration file; used as the anchor for relative paths.
    pub config_dir: Option<&'a Path>,
    /// When true, path-kind fields are absolutized against `config_dir`.
    /// When false, relative paths are preserved (daemon CWD semantics, python-compat mode).
    pub path_translation: bool,
}

/// Binary classification of a configuration key: a filesystem [`Path`] that is
/// absolutized (when `path_translation` is enabled) or a [`Command`] that is only
/// macro-expanded (together with all other string values).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Path,
    Command,
    Default,
}

/// Position in the typed configuration tree used to classify string keys.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Section {
    Root,
    Server,
    Logging,
    ProgramDefaults,
    ProgramsMap,
    EventListenersMap,
    Program,
    EventListener,
    Other,
}

/// Walks a `serde_json::Value` representation of a typed `SupervisorConfig`,
/// expanding `${VAR}` / `%(...)s` / `$(...)` macros on every string value and
/// absolutizing path-kind fields against `config_dir` when path translation is on.
pub fn transform(value: &mut Value, ctx: &Ctx) {
    let expander = MacroExpander::new();
    let expr = match ctx.config_dir {
        Some(dir) => StringExpression::with_config_dir(dir),
        None => StringExpression::with_defaults(),
    };
    walk_object(value, ctx, &expander, &expr, Section::Root, false);
}

fn walk_object(
    value: &mut Value,
    ctx: &Ctx,
    expander: &MacroExpander,
    expr: &StringExpression,
    section: Section,
    in_logs: bool,
) {
    if let Value::Object(map) = value {
        for (k, v) in map.iter_mut() {
            match v {
                Value::Object(_) => {
                    let (child_section, child_in_logs) = descend(section, in_logs, k);
                    walk_object(v, ctx, expander, expr, child_section, child_in_logs);
                }
                Value::Array(arr) => {
                    for item in arr.iter_mut() {
                        walk_scalar_or_container(item, ctx, expander, expr);
                    }
                }
                Value::String(s) => {
                    let kind = classify(section, in_logs, k);
                    *s = translate_string(s, kind, ctx, expander, expr);
                }
                _ => {}
            }
        }
    }
}

fn walk_scalar_or_container(
    value: &mut Value,
    ctx: &Ctx,
    expander: &MacroExpander,
    expr: &StringExpression,
) {
    match value {
        Value::Object(_) => walk_object(value, ctx, expander, expr, Section::Other, false),
        Value::Array(_) => {
            if let Value::Array(arr) = value {
                for item in arr.iter_mut() {
                    walk_scalar_or_container(item, ctx, expander, expr);
                }
            }
        }
        Value::String(s) => {
            *s = expander.expand_value_with_expr(s, expr);
        }
        _ => {}
    }
}

/// Computes the section context for the value of a given key.
fn descend(section: Section, in_logs: bool, key: &str) -> (Section, bool) {
    use Section::*;
    match section {
        Root => match key {
            "server" => (Server, false),
            "logging" => (Logging, false),
            "program_defaults" => (ProgramDefaults, false),
            "programs" => (ProgramsMap, false),
            "event_listeners" => (EventListenersMap, false),
            _ => (Other, false),
        },
        ProgramsMap => (Program, false),
        EventListenersMap => (EventListener, false),
        Program | ProgramDefaults | EventListener => match key {
            "logs" => (Program, true),
            "environment" => (Other, false),
            _ => (section, in_logs),
        },
        Server | Logging | Other => (section, in_logs),
    }
}

/// Classifies a string field as a filesystem path or command within its section.
fn classify(section: Section, in_logs: bool, key: &str) -> Kind {
    use Kind::*;
    use Section::*;
    match section {
        Server if key == "uds_path" => Path,
        Logging if key == "file" => Path,
        Program | ProgramDefaults if in_logs => {
            if key == "stdout" || key == "stderr" {
                Path
            } else {
                Default
            }
        }
        Program | ProgramDefaults => match key {
            "directory" | "restart_directory_monitor" => Path,
            "command" => Command,
            _ => Default,
        },
        EventListener => match key {
            "directory" | "stdout_logfile" | "stderr_logfile" => Path,
            "command" => Command,
            _ => Default,
        },
        _ => Default,
    }
}

/// Expands macros on a string, then absolutizes it if it is a path/command field and
/// path translation is enabled.
fn translate_string(
    s: &str,
    kind: Kind,
    ctx: &Ctx,
    expander: &MacroExpander,
    expr: &StringExpression,
) -> String {
    let expanded = expander.expand_value_with_expr(s, expr);
    match kind {
        Kind::Path if ctx.path_translation => absolutize(&expanded, ctx.config_dir)
            .to_string_lossy()
            .into_owned(),
        Kind::Command if ctx.path_translation => translate_command(&expanded, ctx.config_dir),
        _ => expanded,
    }
}

fn quote_token_if_needed(token: &str) -> String {
    if token.is_empty() {
        return "\"\"".to_string();
    }
    if token.contains(' ') || token.contains('\t') || token.contains('"') {
        let escaped = token.replace('"', "\\\"");
        format!("\"{}\"", escaped)
    } else {
        token.to_string()
    }
}

fn join_command_tokens(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|t| quote_token_if_needed(t))
        .collect::<Vec<_>>()
        .join(" ")
}

fn translate_command(cmd_str: &str, config_dir: Option<&Path>) -> String {
    let platform = crate::platform::native_platform();
    if let Ok(mut tokens) = platform.split_command_line(cmd_str)
        && let Some(first) = tokens.first_mut()
        && (first.contains('/') || first.contains('\\'))
    {
        let abs = absolutize(first, config_dir);
        *first = abs.to_string_lossy().into_owned();
        return join_command_tokens(&tokens);
    }
    cmd_str.to_string()
}

/// Converts a relative path into an absolute path anchored at `config_dir`
/// (falling back to the current working directory).
/// Sentinel and special values are preserved verbatim:
/// - `AUTO` / `NONE` (case-insensitive, python compat sentinels)
/// - Windows named pipes (`\\.\pipe\...`)
/// - paths that already look absolute (`/`, `\\`, or drive-letter prefixed)
/// - empty strings
pub fn absolutize(value: &str, config_dir: Option<&Path>) -> PathBuf {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return PathBuf::from(value);
    }
    if trimmed.eq_ignore_ascii_case("AUTO") || trimmed.eq_ignore_ascii_case("NONE") {
        return PathBuf::from(value);
    }
    #[cfg(windows)]
    if trimmed.starts_with(r"\\.\pipe\") {
        return PathBuf::from(value);
    }
    if is_abs_like(trimmed) {
        return PathBuf::from(value);
    }
    let base = config_dir.unwrap_or(Path::new("."));
    base.join(value)
}

/// Reports whether a string looks like an absolute path, independent of the
/// host platform (python uses `ntpath.isabs`/`posixpath.isabs` semantics).
fn is_abs_like(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() {
        return false;
    }
    if b[0] == b'/' {
        return true;
    }
    if b.starts_with(br"\\") {
        return true;
    }
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(dir: &Path) -> Ctx<'_> {
        Ctx {
            config_dir: Some(dir),
            path_translation: true,
        }
    }

    #[test]
    fn test_absolutize_relative_joins_config_dir() {
        let dir = Path::new("/etc/supervisor");
        assert_eq!(
            absolutize("logs/app.log", Some(dir)),
            dir.join("logs/app.log")
        );
        assert_eq!(
            absolutize("./svc/a.out", Some(dir)),
            dir.join("./svc/a.out")
        );
    }

    #[test]
    fn test_absolutize_keeps_absolute() {
        let dir = Path::new("/etc/supervisor");
        assert_eq!(
            absolutize("/var/log/x.log", Some(dir)),
            PathBuf::from("/var/log/x.log")
        );
        assert_eq!(
            absolutize(r"C:\Program Files\bin\app.exe", Some(dir)),
            PathBuf::from(r"C:\Program Files\bin\app.exe")
        );
        assert_eq!(
            absolutize(r"\\.\pipe\svc", Some(dir)),
            PathBuf::from(r"\\.\pipe\svc")
        );
    }

    #[test]
    fn test_absolutize_keeps_sentinels() {
        let dir = Path::new("/etc/supervisor");
        assert_eq!(absolutize("AUTO", Some(dir)), PathBuf::from("AUTO"));
        assert_eq!(absolutize("NONE", Some(dir)), PathBuf::from("NONE"));
        assert_eq!(absolutize("auto", Some(dir)), PathBuf::from("auto"));
        assert_eq!(absolutize("", Some(dir)), PathBuf::from(""));
    }

    #[test]
    fn test_absolutize_none_dir_uses_cwd() {
        let rel = "logs/app.log";
        let got = absolutize(rel, None);
        assert_eq!(got, Path::new(".").join(rel));
    }

    #[test]
    fn test_walker_absolutizes_path_fields() {
        let dir = Path::new("/etc/supervisor");
        let mut value = json!({
            "server": { "uds_path": "run/ipc.sock" },
            "logging": { "file": "logs/daemon.log" },
            "programs": {
                "web": {
                    "command": "/usr/bin/python -m http.server",
                    "directory": "./services/api",
                    "logs": { "stdout": "logs/web.log", "stderr": "logs/web.err" }
                }
            },
            "event_listeners": {
                "ev": {
                    "command": "bin/handler",
                    "stdout_logfile": "logs/ev.out",
                    "stderr_logfile": "logs/ev.err"
                }
            }
        });
        transform(&mut value, &ctx(dir));

        assert_eq!(
            value["server"]["uds_path"],
            dir.join("run/ipc.sock").to_string_lossy().as_ref()
        );
        assert_eq!(
            value["logging"]["file"],
            dir.join("logs/daemon.log").to_string_lossy().as_ref()
        );
        let web = &value["programs"]["web"];
        assert_eq!(
            web["directory"],
            dir.join("./services/api").to_string_lossy().as_ref()
        );
        assert_eq!(
            web["logs"]["stdout"],
            dir.join("logs/web.log").to_string_lossy().as_ref()
        );
        assert_eq!(
            web["logs"]["stderr"],
            dir.join("logs/web.err").to_string_lossy().as_ref()
        );
        // Absolute command remains unchanged.
        assert_eq!(web["command"], json!("/usr/bin/python -m http.server"));
        let ev = &value["event_listeners"]["ev"];
        assert_eq!(
            ev["stdout_logfile"],
            dir.join("logs/ev.out").to_string_lossy().as_ref()
        );
        assert_eq!(
            ev["stderr_logfile"],
            dir.join("logs/ev.err").to_string_lossy().as_ref()
        );
        // Relative command containing path separators is absolutized against config_dir.
        assert_eq!(
            ev["command"],
            dir.join("bin/handler").to_string_lossy().as_ref()
        );
    }

    #[test]
    fn test_walker_expands_macros_in_commands_only() {
        let dir = Path::new("/etc/supervisor");
        let mut value = json!({
            "server": { "uds_path": "${RSV_UNSET_LSOCKET:-/var/run/x.sock}" },
            "programs": {
                "web": {
                    "command": "%(here)s/bin/%(program_name)s",
                    "directory": "%(here)s/work",
                    "logs": { "stdout": "logs/%(process_num)s.log", "stderr": "logs/e.log" }
                }
            }
        });
        let c = Ctx {
            config_dir: Some(dir),
            path_translation: true,
        };
        transform(&mut value, &c);

        assert_eq!(value["server"]["uds_path"], "/var/run/x.sock");
        let web = &value["programs"]["web"];
        // Unknown program-scoped macro keeps the literal token: absolutization joins
        // the (still literal) relative path with config_dir, leaving %(program_name)s for resolve_programs.
        assert_eq!(
            web["command"],
            dir.join("%(here)s/bin/%(program_name)s")
                .to_string_lossy()
                .as_ref()
        );
        // Pure config-scoped macro (here) expands via string concat before absolutization.
        assert_eq!(web["directory"], format!("{}/work", dir.display()));
        // Unknown program-scoped macro keeps the literal token: absolutization joins
        // the (still literal) relative path with config_dir via PathBuf::join, leaving
        // the remaining %(process_num)s for resolve_programs to substitute later.
        assert_eq!(
            web["logs"]["stdout"],
            dir.join("logs/%(process_num)s.log")
                .to_string_lossy()
                .as_ref(),
        );
        assert_eq!(
            web["logs"]["stderr"],
            dir.join("logs/e.log").to_string_lossy().as_ref()
        );
    }

    #[test]
    fn test_walker_disabled_path_translation_preserves_relative() {
        let dir = Path::new("/etc/supervisor");
        let mut value = json!({
            "server": { "uds_path": "run/ipc.sock" },
            "programs": {
                "web": {
                    "command": "python -m http.server",
                    "directory": "work",
                    "logs": { "stdout": "logs/a.log", "stderr": "logs/b.log" }
                }
            }
        });
        let c = Ctx {
            config_dir: Some(dir),
            path_translation: false,
        };
        transform(&mut value, &c);

        // Everything keeps its relative spelling but macros still expand.
        assert_eq!(value["server"]["uds_path"], "run/ipc.sock");
        assert_eq!(value["programs"]["web"]["directory"], "work");
        assert_eq!(value["programs"]["web"]["logs"]["stdout"], "logs/a.log");
        assert_eq!(value["programs"]["web"]["command"], "python -m http.server");
    }

    #[test]
    fn test_walker_environment_values_not_treated_as_paths() {
        let dir = Path::new("/etc/supervisor");
        let mut value = json!({
            "programs": {
                "web": {
                    "command": "env",
                    "environment": {
                        "HOME": "logs",
                        "PORT": "8080",
                        "PATH": "some/path"
                    }
                }
            }
        });
        transform(&mut value, &ctx(dir));
        let env = &value["programs"]["web"]["environment"];
        assert_eq!(env["HOME"], "logs");
        assert_eq!(env["PORT"], "8080");
        assert_eq!(env["PATH"], "some/path");
    }

    #[test]
    fn test_walker_health_check_command_stays_literal() {
        let dir = Path::new("/etc/supervisor");
        let mut value = json!({
            "programs": {
                "web": {
                    "command": "python -m http.server",
                    "health_check": {
                        "type": "http",
                        "url": "http://127.0.0.1:8080/health",
                        "interval_secs": 10
                    }
                }
            }
        });
        transform(&mut value, &ctx(dir));
        let hc = &value["programs"]["web"]["health_check"];
        assert_eq!(hc["type"], "http");
        assert_eq!(hc["url"], "http://127.0.0.1:8080/health");
        assert_eq!(hc["interval_secs"], 10);
    }
}
