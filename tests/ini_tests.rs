// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use rsupervisord::config::schema::SupervisorConfig;
use std::path::Path;
use std::time::Duration;

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
    assert_eq!(config.server.uds_username.as_deref(), Some("ctluser"));
    assert_eq!(config.server.uds_password.as_deref(), Some("ctlpass"));
    assert_eq!(config.server.username.as_deref(), Some("rpcuser"));
    assert_eq!(config.server.password.as_deref(), Some("rpcpass"));

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
    assert_eq!(config.logging.max_bytes, Some(5 * 1024 * 1024));
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
    assert_eq!(ticker.stop_wait_secs, Duration::from_secs(5));

    // Echo program details
    let echo = resolved.get("echo").unwrap();
    assert_eq!(echo.start_secs, Duration::from_secs(0));
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
fn test_ini_chmod_maps_to_uds_chmod() {
    let ini_str = r#"
    [unix_http_server]
    file = /tmp/supervisor.sock
    chmod = 0755
    username = alice
    password = secret
    "#;
    let config = SupervisorConfig::from_ini_str(ini_str).expect("parse ini");

    assert_eq!(config.server.uds_chmod, Some(0o755));
    assert_eq!(config.server.resolved_uds_chmod().unwrap(), 0o755);
    assert_eq!(config.server.uds_username.as_deref(), Some("alice"));
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
      uds_username: testuser
      uds_password: secret
      http_bind: 127.0.0.1:9001
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
    assert_eq!(
        ini_config.server.uds_username,
        yaml_config.server.uds_username
    );
    assert_eq!(
        ini_config.server.uds_password,
        yaml_config.server.uds_password
    );

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
    assert_eq!(worker.start_secs, Duration::from_secs(10));
    assert_eq!(worker.start_retries, 5);
    assert_eq!(
        worker.stop_signal,
        rsupervisord::program::config::StopSignal::Int
    );
    assert_eq!(worker.stop_wait_secs, Duration::from_secs(20));
    assert_eq!(worker.priority, 300);
    assert_eq!(worker.logs.max_bytes, Some(50 * 1024 * 1024));
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

#[test]
fn test_yaml_credentials_defaulting() {
    // 1. When YAML defines username and password, UDS credentials default to them
    let yaml_default = r#"
server:
  username: "alice"
  password: "alice_password"
programs:
  dummy:
    command: "/bin/true"
"#;
    let config = SupervisorConfig::from_yaml_str(yaml_default).unwrap();
    assert_eq!(config.server.username.as_deref(), Some("alice"));
    assert_eq!(config.server.password.as_deref(), Some("alice_password"));
    assert_eq!(config.server.uds_username.as_deref(), Some("alice"));
    assert_eq!(
        config.server.uds_password.as_deref(),
        Some("alice_password")
    );

    // 2. When YAML explicitly provides uds_username and uds_password, explicit values are preserved
    let yaml_explicit = r#"
server:
  username: "alice"
  password: "alice_password"
  uds_username: "bob"
  uds_password: "bob_password"
programs:
  dummy:
    command: "/bin/true"
"#;
    let config_explicit = SupervisorConfig::from_yaml_str(yaml_explicit).unwrap();
    assert_eq!(config_explicit.server.username.as_deref(), Some("alice"));
    assert_eq!(
        config_explicit.server.password.as_deref(),
        Some("alice_password")
    );
    assert_eq!(config_explicit.server.uds_username.as_deref(), Some("bob"));
    assert_eq!(
        config_explicit.server.uds_password.as_deref(),
        Some("bob_password")
    );
}

#[test]
fn test_ini_independent_credentials() {
    // INI config with only [inet_http_server] must NOT populate UDS credentials
    let ini_str = r#"
[inet_http_server]
port = 127.0.0.1:9001
username = rpcuser
password = rpcpass

[unix_http_server]
file = /tmp/supervisor.sock

[program:dummy]
command = /bin/true
"#;
    let config = SupervisorConfig::from_ini_str(ini_str).unwrap();
    assert_eq!(config.server.username.as_deref(), Some("rpcuser"));
    assert_eq!(config.server.password.as_deref(), Some("rpcpass"));
    assert_eq!(config.server.uds_username, None);
    assert_eq!(config.server.uds_password, None);
}

#[test]
fn test_ini_baselines_path_translation_and_allow_unelevated() {
    // INI frontend must align with Python supervisor / go-supervisord baselines:
    // - path_translation=false: bare relative paths stay relative (CWD-resolved)
    // - allow_unelevated=true: no elevation gate on IPC
    let dir = tempfile::tempdir().unwrap();
    let conf_path = dir.path().join("supervisord.conf");
    std::fs::write(
        &conf_path,
        r#"
[unix_http_server]
file = /tmp/supervisor.sock

[supervisord]
logfile = relative/path/supervisord.log

[program:web]
command = /usr/bin/web
stdout_logfile = relative/path/web.out.log
"#,
    )
    .unwrap();

    let config = SupervisorConfig::from_file(&conf_path).expect("INI load must succeed");

    assert!(
        !config.server.path_translation,
        "INI frontend must force path_translation=false"
    );
    assert!(
        config.server.allow_unelevated,
        "INI frontend must force allow_unelevated=true"
    );

    // Bare relative paths must stay relative (not absolutized against config_dir)
    let log_file = config
        .logging
        .file
        .as_ref()
        .expect("logging.file must be set");
    assert!(
        log_file.is_relative(),
        "relative logfile must stay relative under INI path_translation=false, got {:?}",
        log_file
    );
    assert!(
        !log_file
            .to_string_lossy()
            .contains(dir.path().to_string_lossy().as_ref()),
        "relative logfile must not be absolutized against config_dir {:?}, got {:?}",
        dir.path(),
        log_file
    );

    let prog = config.programs.get("web").expect("program web must exist");
    let stdout = prog
        .logs
        .as_ref()
        .and_then(|l| l.stdout.as_ref())
        .expect("stdout_logfile must be set");
    assert!(
        stdout.is_relative(),
        "relative stdout_logfile must stay relative under INI path_translation=false, got {:?}",
        stdout
    );

    // resolved_uds_chmod with allow_unelevated=true (no explicit chmod) must be
    // the unelevated-friendly mode (0o777), matching Python/go which have no
    // elevation gate and rely on socket file permissions only.
    assert_eq!(
        config.server.resolved_uds_chmod().unwrap(),
        0o777,
        "allow_unelevated=true without explicit chmod must resolve to 0o777"
    );
}

/// OI-1 / OI-2 / OI-3 / OI-4 / OI-5 / OI-6 / OI-8 / OI-9 / OI-11 coverage.
#[test]
fn test_ini_open_issue_keys() {
    let dir = tempfile::tempdir().unwrap();
    let conf_path = dir.path().join("supervisord.conf");
    std::fs::write(
        &conf_path,
        r#"
[unix_http_server]
file = /tmp/supervisor.sock

[supervisord]
nodaemon = yes
silent = true
pidfile = /tmp/supervisord-test.pid
minfds = 1024
minprocs = 200
environment = OI6_TAG="alpha",OI6_MODE="test"
unknown_supervisord_key = ignored

[supervisorctl]
serverurl = unix:///tmp/supervisor.sock
username = ctluser
password = ctlpass

[program:web]
command = /usr/bin/web
envFiles = /tmp/web.env,rel.env
killwaitsecs = 5
stopasgroup = true
killasgroup = true
liveness_check_script = /usr/bin/true
liveness_check_period = 30
liveness_check_timeout = 10
liveness_check_initial_delay = 15
liveness_check_failure_threshold = 4
liveness_check_failure_action = restart
unknown_program_key = ignored

[program:badstop]
command = /usr/bin/bad
stopasgroup = true
killasgroup = false
"#,
    )
    .unwrap();

    let config = SupervisorConfig::from_file(&conf_path).expect("INI load must succeed");

    // OI-1
    let cli = config
        .cli_defaults
        .as_ref()
        .expect("cli_defaults must exist");
    assert_eq!(
        cli.serverurl.as_deref(),
        Some("unix:///tmp/supervisor.sock")
    );
    assert_eq!(cli.username.as_deref(), Some("ctluser"));
    assert_eq!(cli.password.as_deref(), Some("ctlpass"));

    // OI-4 / OI-6 / OI-8
    assert!(config.nodaemon, "nodaemon=yes must map to true");
    assert!(
        config.logging.silent,
        "silent=true must map to LoggingConfig.silent"
    );
    assert_eq!(
        config.pidfile.as_deref(),
        Some(std::path::Path::new("/tmp/supervisord-test.pid"))
    );
    assert_eq!(config.minfds, Some(1024));
    assert_eq!(config.minprocs, Some(200));
    assert_eq!(
        config.environment.get("OI6_TAG").map(String::as_str),
        Some("alpha")
    );
    assert_eq!(
        config.environment.get("OI6_MODE").map(String::as_str),
        Some("test")
    );

    // OI-2 / OI-3 / OI-5 / OI-9 on the raw program section
    let web = config.programs.get("web").expect("program web");
    let env_files = web.env_files.as_ref().expect("env_files must be set");
    assert_eq!(env_files.len(), 2);
    assert!(
        env_files[0].ends_with("web.env"),
        "absolute env file must be preserved, got {:?}",
        env_files[0]
    );
    assert!(
        env_files[1].ends_with("rel.env"),
        "relative env file must be absolutized against config_dir, got {:?}",
        env_files[1]
    );
    assert!(
        env_files[1].is_absolute(),
        "resolved relative env file must be absolute, got {:?}",
        env_files[1]
    );
    assert_eq!(
        web.kill_wait_secs,
        Some(Duration::from_secs(5)),
        "killwaitsecs must map to kill_wait_secs"
    );
    assert_eq!(web.stop_as_group, Some(true));
    assert_eq!(web.kill_as_group, Some(true));
    let hc = web
        .health_check
        .as_ref()
        .expect("liveness_check must map to health_check");
    assert_eq!(hc.interval_secs, Duration::from_secs(30));
    assert_eq!(hc.timeout_secs, Duration::from_secs(10));
    assert_eq!(hc.initial_delay_secs, Duration::from_secs(15));
    assert_eq!(hc.failure_threshold, 4);

    // OI-9 validation: stop_as_group without kill_as_group is rejected at resolve
    let err = config
        .resolve_programs()
        .expect_err("stop_as_group=true + kill_as_group=false must fail");
    assert!(
        err.to_string().contains("stop_as_group"),
        "error must mention stop_as_group, got: {}",
        err
    );

    // OI-6: environment must not be applied into the process env at parse time
    assert!(
        std::env::var_os("OI6_TAG").is_none(),
        "adapter must not set_var at parse time"
    );
}

/// OI-1: `resolve_endpoint_candidates` seeds from `[supervisorctl]` defaults.
#[test]
fn test_cli_resolve_from_supervisorctl_section() {
    use clap::Parser;
    use rsupervisord::cli::{CliArgs, resolve_endpoint_candidates};

    let dir = tempfile::tempdir().unwrap();
    let conf_path = dir.path().join("supervisord.conf");
    std::fs::write(
        &conf_path,
        r#"
[unix_http_server]
file = /tmp/supervisor.sock

[supervisorctl]
serverurl = http://127.0.0.1:9999
username = ctluser
password = ctlpass
"#,
    )
    .unwrap();

    let args = CliArgs::parse_from(["supervisorctl", "-c", conf_path.to_str().unwrap(), "status"]);
    let (candidates, basic, _token) =
        resolve_endpoint_candidates(&args).expect("resolve must succeed");

    assert_eq!(
        candidates.len(),
        1,
        "serverurl seeds a single candidate, got {:?}",
        candidates
    );
    let ep = format!("{}", candidates[0].endpoint);
    assert!(
        ep.contains("9999") || ep.contains("127.0.0.1"),
        "endpoint must come from serverurl, got {}",
        ep
    );
    assert_eq!(
        basic,
        Some(("ctluser".to_string(), "ctlpass".to_string())),
        "username/password must seed basic auth"
    );

    // CLI -u/-p override [supervisorctl]
    let args = CliArgs::parse_from([
        "supervisorctl",
        "-c",
        conf_path.to_str().unwrap(),
        "-u",
        "cliuser",
        "-p",
        "clipass",
        "status",
    ]);
    let (_, basic, _) = resolve_endpoint_candidates(&args).expect("resolve must succeed");
    assert_eq!(
        basic,
        Some(("cliuser".to_string(), "clipass".to_string())),
        "CLI credentials must override [supervisorctl]"
    );

    // Explicit -s overrides serverurl endpoint
    let args = CliArgs::parse_from([
        "supervisorctl",
        "-c",
        conf_path.to_str().unwrap(),
        "-s",
        "http://127.0.0.1:1234",
        "status",
    ]);
    let (candidates, basic, _) = resolve_endpoint_candidates(&args).expect("resolve must succeed");
    assert_eq!(candidates.len(), 1);
    let ep = format!("{}", candidates[0].endpoint);
    assert!(
        ep.contains("1234"),
        "explicit -s must win over serverurl, got {}",
        ep
    );
    assert_eq!(
        basic,
        Some(("ctluser".to_string(), "ctlpass".to_string())),
        "credentials still apply with explicit -s"
    );
}

/// Explicit `-c` pointing at an invalid config must hard-error (no silent fallback).
#[test]
fn test_cli_explicit_bad_config_is_hard_error() {
    use clap::Parser;
    use rsupervisord::cli::{CliArgs, resolve_endpoint_candidates};

    let dir = tempfile::tempdir().unwrap();
    let conf_path = dir.path().join("broken.conf");
    // Invalid chmod is a hard parse error (not a missing section).
    std::fs::write(
        &conf_path,
        "[unix_http_server]\nfile = /tmp/supervisor.sock\nchmod = not-an-octal\n",
    )
    .unwrap();

    let args = CliArgs::parse_from(["supervisorctl", "-c", conf_path.to_str().unwrap(), "status"]);
    let err = resolve_endpoint_candidates(&args)
        .expect_err("explicit invalid -c must return Err, not fall back");
    assert!(
        err.to_string().contains("Failed to load config"),
        "error must mention config load failure, got: {}",
        err
    );
}

/// OI-9: event listeners reject stop_as_group=true && kill_as_group=false.
#[test]
fn test_eventlistener_stop_kill_group_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let conf_path = dir.path().join("supervisord.conf");
    std::fs::write(
        &conf_path,
        r#"
[unix_http_server]
file = /tmp/supervisor.sock

[eventlistener:bad]
command = /usr/bin/true
events = PROCESS_STATE
stopasgroup = true
killasgroup = false
"#,
    )
    .unwrap();

    let config = SupervisorConfig::from_file(&conf_path).expect("INI load must succeed");
    let err = config
        .resolve_programs()
        .expect_err("EL stop_as_group=true + kill_as_group=false must fail");
    assert!(
        err.to_string().contains("stop_as_group") && err.to_string().contains("Event listener"),
        "error must mention event listener stop_as_group, got: {}",
        err
    );

    // Matching flags (both true) must succeed.
    std::fs::write(
        &conf_path,
        r#"
[unix_http_server]
file = /tmp/supervisor.sock

[eventlistener:ok]
command = /usr/bin/true
events = PROCESS_STATE
stopasgroup = true
killasgroup = true
"#,
    )
    .unwrap();
    let config = SupervisorConfig::from_file(&conf_path).expect("INI load must succeed");
    let resolved = config
        .resolve_programs()
        .expect("matching stop/kill group flags must resolve");
    let el = resolved.get("ok").expect("event listener instance");
    assert!(el.stop_as_group);
    assert!(el.kill_as_group);
}

/// OI-2: missing env files are skipped with warn (go parity), spawn still works.
#[test]
fn test_ini_env_files_missing_is_skipped() {
    use rsupervisord::program::envfile::load_env_files;

    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such.env");
    let map = load_env_files(std::slice::from_ref(&missing));
    assert!(
        map.is_empty(),
        "missing env file must yield empty map, got {:?}",
        map
    );

    let present = dir.path().join("ok.env");
    std::fs::write(&present, "FOO=bar\nexport BAZ=qux\n# comment\nEMPTY=\n").unwrap();
    let map = load_env_files(&[present, missing]);
    assert_eq!(map.get("FOO").map(String::as_str), Some("bar"));
    assert_eq!(map.get("BAZ").map(String::as_str), Some("qux"));
    assert_eq!(map.get("EMPTY").map(String::as_str), Some(""));
}
