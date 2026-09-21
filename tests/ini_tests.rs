// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use rsupervisord::config::schema::SupervisorConfig;
use std::path::Path;

#[test]
fn test_load_compat_supervisord_conf() {
    let conf_path = Path::new("compat/conf/supervisord.conf");
    assert!(
        conf_path.exists(),
        "compat/conf/supervisord.conf must exist"
    );

    let config = SupervisorConfig::from_file(conf_path)
        .expect("compat/conf/supervisord.conf should load successfully");

    // Server configuration
    assert!(
        config
            .server
            .uds_path
            .to_string_lossy()
            .ends_with("run/supervisor.sock")
            || config
                .server
                .uds_path
                .to_string_lossy()
                .ends_with("run\\supervisor.sock"),
        "UDS path must resolve correctly: {:?}",
        config.server.uds_path
    );
    assert_eq!(config.server.http_bind.as_deref(), Some("127.0.0.1:9011"));

    // Logging configuration
    assert!(config.logging.enabled);
    assert!(
        config
            .logging
            .file
            .as_ref()
            .unwrap()
            .to_string_lossy()
            .ends_with("logs/supervisord.log")
            || config
                .logging
                .file
                .as_ref()
                .unwrap()
                .to_string_lossy()
                .ends_with("logs\\supervisord.log")
    );
    assert_eq!(config.logging.max_bytes.as_deref(), Some("5MB"));
    assert_eq!(config.logging.backups, 3);
    assert_eq!(config.logging.level, "info");

    // Programs check (including [include] extra.ini)
    assert!(
        config.programs.contains_key("echo"),
        "Must contain program:echo"
    );
    assert!(
        config.programs.contains_key("ticker"),
        "Must contain program:ticker"
    );
    assert!(
        config.programs.contains_key("catx"),
        "Must contain program:catx"
    );
    assert!(
        config.programs.contains_key("worker"),
        "Must contain program:worker"
    );
    assert!(
        config.programs.contains_key("flaky"),
        "Must contain program:flaky"
    );
    assert!(
        config.programs.contains_key("nosuch"),
        "Must contain program:nosuch"
    );
    assert!(
        config.programs.contains_key("extra"),
        "Must contain program:extra included via [include] files = conf.d/*.ini"
    );

    // Groups check
    assert!(
        config.groups.contains_key("services"),
        "Must contain group:services"
    );
    let services = config.groups.get("services").unwrap();
    assert_eq!(services.programs, vec!["ticker", "catx"]);
    assert_eq!(services.priority, Some(999));

    // Resolve programs
    let resolved = config
        .resolve_programs()
        .expect("resolve_programs must succeed on supervisord.conf");

    // Worker multi-instance expansion
    assert!(resolved.contains_key("worker_00"), "Must expand worker_00");
    assert!(resolved.contains_key("worker_01"), "Must expand worker_01");
    let worker0 = resolved.get("worker_00").unwrap();
    assert_eq!(worker0.priority, 200);

    // Ticker program details
    let ticker = resolved.get("ticker").unwrap();
    assert_eq!(ticker.priority, 100);
    assert_eq!(ticker.group, "services");
    assert_eq!(
        ticker.stop_signal,
        rsupervisord::program::config::StopSignal::Term
    );
    assert_eq!(ticker.stop_wait_secs, 5);

    // Echo program details
    let echo = resolved.get("echo").unwrap();
    assert_eq!(echo.start_secs, 0);
    assert_eq!(echo.exit_codes, vec![0]);
    assert!(echo.logs.redirect_stderr);

    // Extra program details (included from conf.d/extra.ini)
    let extra = resolved.get("extra").unwrap();
    assert!(
        extra.logs.is_stdout_disabled(),
        "extra program should have stdout disabled"
    );
    assert!(
        extra.logs.is_stderr_disabled(),
        "extra program should have stderr disabled"
    );
}

#[test]
fn test_ini_yaml_equivalence() {
    let ini_str = r#"
    [unix_http_server]
    file = /tmp/supervisor.sock
    username = testuser
    password = secret

    [inet_http_server]
    port = 127.0.0.1:9001

    [supervisord]
    logfile = /var/log/supervisord.log
    logfile_maxbytes = 10MB
    logfile_backups = 5
    loglevel = debug

    [program:web]
    command = /usr/bin/web --port 8080
    priority = 20
    autostart = true
    autorestart = unexpected
    startsecs = 5
    startretries = 2
    stopsignal = QUIT
    stopwaitsecs = 12
    exitcodes = 0,2
    redirect_stderr = true
    stdout_logfile = /var/log/web.out.log
    stdout_logfile_maxbytes = 5MB
    stdout_logfile_backups = 2
    environment = ENV_MODE="test",PORT="8080"

    [group:apps]
    programs = web
    priority = 50
    "#;

    let yaml_str = r#"
    server:
      uds_path: /tmp/supervisor.sock
      http_bind: 127.0.0.1:9001
      username: testuser
      password: secret
    logging:
      enabled: true
      file: /var/log/supervisord.log
      max_bytes: 10MB
      backups: 5
      level: debug
    programs:
      web:
        command: /usr/bin/web --port 8080
        priority: 20
        autostart: true
        autorestart: unexpected
        start_secs: 5
        start_retries: 2
        stop_signal: QUIT
        stop_wait_secs: 12
        exit_codes: [0, 2]
        logs:
          redirect_stderr: true
          stdout: /var/log/web.out.log
          max_bytes: 5MB
          backups: 2
        environment:
          ENV_MODE: test
          PORT: "8080"
    groups:
      apps:
        programs: [web]
        priority: 50
    "#;

    let ini_config = SupervisorConfig::from_ini_str(ini_str).unwrap();
    let yaml_config = SupervisorConfig::from_yaml_str(yaml_str).unwrap();

    let ini_resolved = ini_config.resolve_programs().unwrap();
    let yaml_resolved = yaml_config.resolve_programs().unwrap();

    assert_eq!(ini_resolved.len(), yaml_resolved.len());
    let ini_web = ini_resolved.get("web").unwrap();
    let yaml_web = yaml_resolved.get("web").unwrap();

    assert_eq!(ini_web.name, yaml_web.name);
    assert_eq!(ini_web.command, yaml_web.command);
    assert_eq!(ini_web.priority, yaml_web.priority);
    assert_eq!(ini_web.autostart, yaml_web.autostart);
    assert_eq!(ini_web.autorestart, yaml_web.autorestart);
    assert_eq!(ini_web.start_secs, yaml_web.start_secs);
    assert_eq!(ini_web.start_retries, yaml_web.start_retries);
    assert_eq!(ini_web.stop_signal, yaml_web.stop_signal);
    assert_eq!(ini_web.stop_wait_secs, yaml_web.stop_wait_secs);
    assert_eq!(ini_web.exit_codes, yaml_web.exit_codes);
    assert_eq!(ini_web.environment, yaml_web.environment);
    assert_eq!(ini_web.group, yaml_web.group);
    assert_eq!(ini_web.logs.stdout, yaml_web.logs.stdout);
    assert_eq!(ini_web.logs.max_bytes, yaml_web.logs.max_bytes);
    assert_eq!(ini_web.logs.backups, yaml_web.logs.backups);
    assert_eq!(ini_web.logs.redirect_stderr, yaml_web.logs.redirect_stderr);
}

#[test]
fn test_program_default_inheritance() {
    let ini_str = r#"
    [program-default]
    autostart = false
    autorestart = always
    startsecs = 10
    startretries = 5
    stopsignal = INT
    stopwaitsecs = 20
    priority = 300
    stdout_logfile_maxbytes = 50MB
    stdout_logfile_backups = 10

    [program:worker]
    command = /usr/bin/worker
    "#;

    let config = SupervisorConfig::from_ini_str(ini_str).unwrap();
    let resolved = config.resolve_programs().unwrap();
    let worker = resolved.get("worker").unwrap();

    assert!(!worker.autostart);
    assert_eq!(
        worker.autorestart,
        rsupervisord::program::config::AutoRestartPolicy::Always
    );
    assert_eq!(worker.start_secs, 10);
    assert_eq!(worker.start_retries, 5);
    assert_eq!(
        worker.stop_signal,
        rsupervisord::program::config::StopSignal::Int
    );
    assert_eq!(worker.stop_wait_secs, 20);
    assert_eq!(worker.priority, 300);
    assert_eq!(worker.logs.max_bytes.as_deref(), Some("50MB"));
    assert_eq!(worker.logs.backups, Some(10));
}

#[test]
fn test_circular_include_protection() {
    let dir = tempfile::tempdir().unwrap();
    let file_a = dir.path().join("a.conf");
    let file_b = dir.path().join("b.conf");

    std::fs::write(
        &file_a,
        format!(
            "[program:a]\ncommand = /bin/true\n[include]\nfiles = {}\n",
            file_b.display()
        ),
    )
    .unwrap();

    std::fs::write(
        &file_b,
        format!(
            "[program:b]\ncommand = /bin/true\n[include]\nfiles = {}\n",
            file_a.display()
        ),
    )
    .unwrap();

    let result = SupervisorConfig::from_file(&file_a);
    // Since b.conf has [include] which is either rejected or circular, it must error cleanly
    assert!(result.is_err());
}
