// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use clap::Parser;
use rsupervisord::DaemonArgs;
use rsupervisord::daemon::DaemonAction;
use rsupervisord::service::ServiceOp;

#[test]
fn test_daemon_args_service_subcommand() {
    for (op, expect) in [
        ("install", ServiceOp::Install),
        ("uninstall", ServiceOp::Uninstall),
        ("start", ServiceOp::Start),
        ("stop", ServiceOp::Stop),
        ("restart", ServiceOp::Restart),
    ] {
        let args = DaemonArgs::try_parse_from(["rsupervisord", "service", op]).unwrap();
        let actual = match args.action {
            Some(DaemonAction::Service { op }) => op,
            other => panic!("expected Service action for '{}', got {:?}", op, other),
        };
        assert_eq!(actual, expect, "operation '{}'", op);
    }
}

#[test]
fn test_daemon_args_service_global_config() {
    // -c after the subcommand (global arg)
    let args = DaemonArgs::try_parse_from([
        "rsupervisord",
        "service",
        "install",
        "-c",
        "/etc/rsupervisord/config.yaml",
    ])
    .unwrap();
    assert_eq!(
        args.config.as_deref(),
        Some(std::path::Path::new("/etc/rsupervisord/config.yaml"))
    );

    // -c before the subcommand
    let args = DaemonArgs::try_parse_from([
        "rsupervisord",
        "-c",
        "/etc/rsupervisord/config.yaml",
        "service",
        "install",
    ])
    .unwrap();
    assert_eq!(
        args.config.as_deref(),
        Some(std::path::Path::new("/etc/rsupervisord/config.yaml"))
    );
}

#[test]
fn test_daemon_args_service_flag_still_present() {
    // `--service` (Windows SCM re-entry) is a flag, not the `service` subcommand.
    let args = DaemonArgs::try_parse_from(["rsupervisord", "--service"]).unwrap();
    assert!(args.service);
    assert!(args.action.is_none());
}

#[test]
fn test_daemon_args_no_service_action_by_default() {
    let args = DaemonArgs::try_parse_from(["rsupervisord"]).unwrap();
    assert!(args.action.is_none());
    assert!(!args.service);
}

#[test]
fn test_daemon_args_legacy_service_flags_rejected() {
    for flag in ["--install", "--uninstall", "--start", "--stop", "--restart"] {
        assert!(
            DaemonArgs::try_parse_from(["rsupervisord", flag]).is_err(),
            "legacy flag '{}' should no longer parse",
            flag
        );
    }
}

#[test]
fn test_daemon_args_service_mutual_exclusion() {
    // Bare `service` requires an operation.
    assert!(DaemonArgs::try_parse_from(["rsupervisord", "service"]).is_err());
    // Two operations in one invocation are a parse error, structurally
    // replacing the old "Only one service management flag" manual check.
    assert!(DaemonArgs::try_parse_from(["rsupervisord", "service", "install", "start"]).is_err());
}
