// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use rsupervisord::cli::client::SupervisorClient;
use rsupervisord::cli::transport::Endpoint;
use rsupervisord::config::SupervisorConfig;
use rsupervisord::manager::SupervisorManager;
use rsupervisord::server::ServerEngine;
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn get_worker_command(secs: u64) -> String {
    #[cfg(unix)]
    {
        format!("sh -c 'sleep {}'", secs)
    }
    #[cfg(windows)]
    {
        format!(
            "powershell.exe -NoProfile -Command \"Start-Sleep -Seconds {}\"",
            secs
        )
    }
}

static IPC_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn get_test_ipc_path(prefix: &str) -> PathBuf {
    let id = IPC_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    #[cfg(windows)]
    {
        PathBuf::from(format!(
            r"\\.\pipe\rsupervisord_test_{}_{}_{}",
            prefix,
            std::process::id(),
            id
        ))
    }
    #[cfg(unix)]
    {
        let temp = std::env::temp_dir();
        temp.join(format!(
            "rsupervisord_test_{}_{}_{}.sock",
            prefix,
            std::process::id(),
            id
        ))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_process_resource_metrics_and_program_details_api() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let config_path = temp_dir.path().join("rsupervisord.yaml");
    let ipc_path = get_test_ipc_path("metrics");

    let yaml = format!(
        r#"
server:
  uds_path: "{ipc_path}"

programs:
  metric_worker:
    command: |-
      {cmd}
    autostart: true
    start_secs: 0
"#,
        ipc_path = ipc_path.to_string_lossy().replace('\\', "\\\\"),
        cmd = get_worker_command(30),
    );

    std::fs::write(&config_path, &yaml).expect("write config");
    let config = SupervisorConfig::from_file(&config_path).expect("parse config");

    let mut manager = SupervisorManager::new(&config).expect("create manager");
    let manager_handle = manager.handle();
    manager_handle.start_all().await.expect("start all");

    let server_cancel = CancellationToken::new();
    let server = ServerEngine::new(
        manager_handle.clone(),
        Some(config_path.clone()),
        config.server.clone(),
    );
    let server_token = server_cancel.clone();
    let server_task = tokio::spawn(async move {
        let _ = server.run(server_token).await;
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Connect via native client
    let endpoint = Endpoint::parse(&ipc_path.to_string_lossy());
    let client = SupervisorClient::new(endpoint, None);

    // Wait for the metrics ticker (every 2s) to sample the process
    let mut sampled = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let details = client
            .get_program("metric_worker")
            .await
            .expect("get_program");
        if let Some(mem) = details.memory_rss_bytes
            && mem > 0
        {
            sampled = true;
            assert!(details.cpu_percent.is_some());
            assert!(details.pid.is_some());
            assert!(details.uptime_secs.is_some());
            break;
        }
    }

    assert!(
        sampled,
        "Expected process metrics (RSS Memory > 0 and CPU %) to be sampled"
    );

    // Also verify status list has formatted columns
    let statuses = client.status(&[]).await.expect("get status list");
    assert_eq!(statuses.len(), 1);
    let worker_status = &statuses[0];
    assert_eq!(worker_status.name, "metric_worker");
    assert_ne!(worker_status.mem, "-");
    assert_ne!(worker_status.cpu, "-");
    assert_ne!(worker_status.uptime, "-");

    server_cancel.cancel();
    let _ = server_task.await;
    manager.shutdown().await.expect("shutdown manager");
}
