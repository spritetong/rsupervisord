// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

pub mod adapter;
pub mod parser;
pub mod values;

use crate::config::schema::SupervisorConfig;
use crate::error::ProgramError;
use std::collections::HashSet;
use std::path::Path;

/// Loads and parses a `supervisord.conf` (or `.ini` / `.conf`) file, processing any `[include]` directives.
pub fn load_ini_config(path: &Path) -> Result<SupervisorConfig, ProgramError> {
    let mut visited = HashSet::new();
    let parsed = parser::load_ini_file_with_includes(path, &mut visited, true)?;
    let here = path.parent().map(|p| {
        if p.as_os_str().is_empty() {
            Path::new(".")
        } else {
            p
        }
    });
    adapter::adapt_ini_to_config(&parsed, here)
}

/// Parses an INI configuration string with an optional configuration directory.
pub fn parse_ini_str(
    content: &str,
    config_dir: Option<&Path>,
) -> Result<SupervisorConfig, ProgramError> {
    let parsed = parser::parse_ini_content(content, config_dir);
    adapter::adapt_ini_to_config(&parsed, config_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ini_str_end_to_end() {
        let ini = r#"
        [unix_http_server]
        file = /tmp/supervisor.sock
        username = user1
        password = pass1

        [inet_http_server]
        port = :9001

        [supervisord]
        logfile = /tmp/supervisord.log
        loglevel = warn

        [program:web]
        command = /usr/bin/python -m http.server 8080
        autostart = true
        autorestart = unexpected
        priority = 20
        stdout_logfile = /tmp/web.out.log
        redirect_stderr = true
        environment = FOO="bar",NUM="123"

        [group:all_web]
        programs = web
        priority = 100
        "#;

        let config = parse_ini_str(ini, None).expect("Should parse valid INI");
        assert_eq!(
            config.server.uds_path,
            std::path::PathBuf::from("/tmp/supervisor.sock")
        );
        assert_eq!(config.server.uds_username.as_deref(), Some("user1"));
        assert_eq!(config.server.uds_password.as_deref(), Some("pass1"));
        assert_eq!(config.server.http_bind.as_deref(), Some("0.0.0.0:9001"));
        assert_eq!(config.logging.level, "warn");

        let web = config
            .programs
            .get("web")
            .expect("web program should exist");
        assert_eq!(web.command, "/usr/bin/python -m http.server 8080");
        assert_eq!(web.autostart, Some(true));
        assert_eq!(
            web.autorestart,
            Some(crate::program::config::AutoRestartPolicy::Unexpected)
        );
        assert_eq!(web.priority, Some(20));
        assert_eq!(web.environment.get("FOO").unwrap(), "bar");
        assert_eq!(web.environment.get("NUM").unwrap(), "123");

        let group = config.groups.get("all_web").expect("group should exist");
        assert_eq!(group.programs, vec!["web"]);
        assert_eq!(group.priority, Some(100));
    }
}
