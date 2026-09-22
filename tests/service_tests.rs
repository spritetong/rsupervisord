// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use clap::Parser;
use rsupervisord::DaemonArgs;
use rsupervisord::service::handle_service_command;

#[test]
fn test_daemon_args_service_flags() {
    // Test --install flag
    let args = DaemonArgs::try_parse_from(["rsupervisord", "--install"]).unwrap();
    assert!(args.install);
    assert!(!args.uninstall);
    assert!(!args.start);
    assert!(!args.stop);
    assert!(!args.restart);

    // Test --uninstall flag
    let args = DaemonArgs::try_parse_from(["rsupervisord", "--uninstall"]).unwrap();
    assert!(!args.install);
    assert!(args.uninstall);

    // Test --start flag
    let args = DaemonArgs::try_parse_from(["rsupervisord", "--start"]).unwrap();
    assert!(args.start);

    // Test --stop flag
    let args = DaemonArgs::try_parse_from(["rsupervisord", "--stop"]).unwrap();
    assert!(args.stop);

    // Test --restart flag
    let args = DaemonArgs::try_parse_from(["rsupervisord", "--restart"]).unwrap();
    assert!(args.restart);

    // Test combined with -c / --config
    let args = DaemonArgs::try_parse_from([
        "rsupervisord",
        "--install",
        "-c",
        "/etc/rsupervisord/config.yaml",
    ])
    .unwrap();
    assert!(args.install);
    assert_eq!(
        args.config.as_deref(),
        Some(std::path::Path::new("/etc/rsupervisord/config.yaml"))
    );

    // Test --service flag
    let args = DaemonArgs::try_parse_from(["rsupervisord", "--service"]).unwrap();
    assert!(args.service);
}

#[test]
fn test_handle_service_command_no_flags() {
    let args = DaemonArgs::try_parse_from(["rsupervisord"]).unwrap();
    let res = handle_service_command(&args, "rsupervisord", None).unwrap();
    // When no service actions are given, it should return Ok(false)
    assert!(!res);
}

#[test]
fn test_handle_service_command_multiple_flags_rejected() {
    let mut args = DaemonArgs::try_parse_from(["rsupervisord"]).unwrap();
    args.install = true;
    args.start = true;

    let res = handle_service_command(&args, "rsupervisord", None);
    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(err.contains("Only one service management flag"));
}
