// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

pub mod cli;
pub mod compat;
pub mod config;
pub mod consts;
pub mod control;
pub mod daemon;
pub mod error;
pub mod eventlistener;
pub mod logging;
pub mod manager;
pub mod platform;
pub mod program;
pub mod serde_util;
pub mod server;
pub mod service;

pub use config::{ConfigDiff, StringExpression, SupervisorConfig};
pub use daemon::{DaemonArgs, SupervisorDaemon, run_daemon};
pub use error::{CliError, ConfigError, ProgramError, ServiceError, SupervisorError};
pub use manager::{
    DependencyGraph, ManagerHandle, ReloadSummary, SupervisorManager, SupervisorManagerBuilder,
};
pub use platform::{PlatformBackend, PlatformProcessGuard, native_platform};
pub use program::{
    AutoRestartPolicy, ProcessProgram, Program, ProgramConfig, ProgramState, ProgramStatus,
    StopSignal,
};
pub use server::ServerEngine;

/// Builds a Tokio runtime according to the configured number of worker threads.
/// If threads == 1, builds a single-threaded (`current_thread`) runtime.
/// If threads > 1, builds a multi-threaded runtime with that exact worker count.
/// If None, checks the `TOKIO_WORKER_THREADS` environment variable or defaults to available CPU cores.
pub fn build_tokio_runtime(
    nb_worker_threads: Option<u32>,
) -> std::io::Result<tokio::runtime::Runtime> {
    let threads = nb_worker_threads.or_else(|| {
        std::env::var("TOKIO_WORKER_THREADS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
    });

    let mut builder = match threads {
        Some(1) => {
            tracing::info!("Initialized single-threaded (current_thread) Tokio runtime");
            tokio::runtime::Builder::new_current_thread()
        }
        Some(n) => {
            tracing::info!(
                "Initialized multi-threaded Tokio runtime with {} worker threads",
                n
            );
            let mut b = tokio::runtime::Builder::new_multi_thread();
            b.worker_threads(n as usize);
            b
        }
        None => tokio::runtime::Builder::new_multi_thread(),
    };

    builder.enable_all().build()
}
