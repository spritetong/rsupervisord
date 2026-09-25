// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::manager::supervisor::ManagerHandle;
use crate::program::config::{ProgramConfig, StopSignal};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Matcher helper for file pattern strings (e.g. `*`, `*.jar`, `*.conf`, `prefix*`).
pub fn matches_pattern(pattern: Option<&str>, file_name: &str) -> bool {
    let Some(pat) = pattern else {
        return true;
    };
    let trimmed = pat.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return true;
    }
    if let Some(suffix) = trimmed.strip_prefix('*') {
        return file_name.ends_with(suffix);
    }
    if let Some(prefix) = trimmed.strip_suffix('*') {
        return file_name.starts_with(prefix);
    }
    trimmed == file_name
}

/// A configured file or directory watch rule for a supervised program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchRule {
    Binary {
        program_name: String,
        target_path: PathBuf,
        parent_dir: PathBuf,
        directory: Option<PathBuf>,
        debounce_duration: Duration,
        signal: Option<StopSignal>,
        cmd: Option<String>,
    },
    Directory {
        program_name: String,
        dir_path: PathBuf,
        pattern: Option<String>,
        directory: Option<PathBuf>,
        debounce_duration: Duration,
        signal: Option<StopSignal>,
        cmd: Option<String>,
    },
}

impl WatchRule {
    #[inline]
    pub fn program_name(&self) -> &str {
        match self {
            Self::Binary { program_name, .. } | Self::Directory { program_name, .. } => {
                program_name
            }
        }
    }

    #[inline]
    pub fn debounce_duration(&self) -> Duration {
        match self {
            Self::Binary {
                debounce_duration, ..
            }
            | Self::Directory {
                debounce_duration, ..
            } => *debounce_duration,
        }
    }

    #[inline]
    pub fn signal(&self) -> Option<StopSignal> {
        match self {
            Self::Binary { signal, .. } | Self::Directory { signal, .. } => *signal,
        }
    }

    #[inline]
    pub fn cmd(&self) -> Option<&str> {
        match self {
            Self::Binary { cmd, .. } | Self::Directory { cmd, .. } => cmd.as_deref(),
        }
    }

    #[inline]
    pub fn directory(&self) -> Option<&Path> {
        match self {
            Self::Binary { directory, .. } | Self::Directory { directory, .. } => {
                directory.as_deref()
            }
        }
    }
}

/// Pending restart trigger awaiting debounce expiration.
#[derive(Debug)]
struct PendingTrigger {
    program_name: String,
    target_path: PathBuf,
    is_binary: bool,
    directory: Option<PathBuf>,
    signal: Option<StopSignal>,
    cmd: Option<String>,
    deadline: tokio::time::Instant,
    debounce_duration: Duration,
    consecutive_checks: usize,
}

/// Handle allowing caller to update watched program configurations during hot reload.
#[derive(Clone)]
pub struct WatchServiceHandle {
    reload_tx: mpsc::Sender<HashMap<String, ProgramConfig>>,
}

impl WatchServiceHandle {
    /// Updates the active watch rules and registered directory watchers.
    pub async fn update_configs(&self, configs: HashMap<String, ProgramConfig>) {
        let _ = self
            .reload_tx
            .send_timeout(configs, Duration::from_secs(2))
            .await;
    }
}

/// Filesystem watching service monitoring executable binaries and resource directories.
pub struct WatchService {
    manager: ManagerHandle,
    configs: HashMap<String, ProgramConfig>,
    cancel_token: CancellationToken,
}

impl WatchService {
    /// Spawns the watch service as an asynchronous background task.
    pub fn spawn(
        manager: ManagerHandle,
        configs: HashMap<String, ProgramConfig>,
        cancel_token: CancellationToken,
    ) -> WatchServiceHandle {
        let (reload_tx, reload_rx) = mpsc::channel(16);
        let service = Self {
            manager,
            configs,
            cancel_token,
        };

        tokio::spawn(service.run(reload_rx));

        WatchServiceHandle { reload_tx }
    }

    async fn run(mut self, mut reload_rx: mpsc::Receiver<HashMap<String, ProgramConfig>>) {
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Event>();

        let watcher_res = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = event_tx.send(event);
                }
            },
            notify::Config::default(),
        );

        let mut watcher = match watcher_res {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("Failed to initialize filesystem RecommendedWatcher: {}", e);
                return;
            }
        };

        let mut current_watched_dirs: HashSet<PathBuf> = HashSet::new();
        let mut rules = Self::build_rules(&self.configs);
        Self::sync_watcher(&mut watcher, &mut current_watched_dirs, &rules);

        let mut pending_triggers: HashMap<String, PendingTrigger> = HashMap::new();

        loop {
            let earliest_deadline = pending_triggers.values().map(|p| p.deadline).min();

            tokio::select! {
                biased;

                _ = self.cancel_token.cancelled() => {
                    tracing::debug!("WatchService cancelled, shutting down filesystem monitoring");
                    break;
                }

                // Debounce deadline expired
                _ = async {
                    match earliest_deadline {
                        Some(instant) => tokio::time::sleep_until(instant).await,
                        None => std::future::pending().await,
                    }
                }, if earliest_deadline.is_some() => {
                    let now = tokio::time::Instant::now();
                    let due_keys: Vec<String> = pending_triggers
                        .iter()
                        .filter(|(_, trig)| trig.deadline <= now)
                        .map(|(k, _)| k.clone())
                        .collect();

                    for key in due_keys {
                        if let Some(mut trigger) = pending_triggers.remove(&key) {
                            // Verify file stability (avoid triggering while still locked by compiler)
                            if trigger.is_binary && trigger.target_path.exists() {
                                let is_readable = match std::fs::File::open(&trigger.target_path) {
                                    Ok(f) => f.metadata().map(|m| m.len() > 0).unwrap_or(true),
                                    Err(_) => false,
                                };

                                if !is_readable && trigger.consecutive_checks < 5 {
                                    trigger.consecutive_checks += 1;
                                    trigger.deadline = tokio::time::Instant::now() + crate::consts::SHORT_RETRY_DELAY;
                                    pending_triggers.insert(key, trigger);
                                    continue;
                                }
                            }

                            Self::execute_trigger(&self.manager, trigger).await;
                        }
                    }
                }

                // Filesystem event received from OS
                Some(event) = event_rx.recv() => {
                    if !Self::is_relevant_event(&event) {
                        continue;
                    }

                    for path in &event.paths {
                        for rule in &rules {
                            if Self::matches_rule(rule, path) {
                                let prog_name = rule.program_name().to_string();
                                let debounce_dur = rule.debounce_duration();
                                let deadline = tokio::time::Instant::now() + debounce_dur;

                                if let Some(existing) = pending_triggers.get_mut(&prog_name) {
                                    existing.deadline = tokio::time::Instant::now() + existing.debounce_duration;
                                    existing.target_path = path.clone();
                                    tracing::debug!(
                                        program = %prog_name,
                                        path = ?path,
                                        debounce_secs = existing.debounce_duration.as_secs(),
                                        "Extended debounce deadline for program restart"
                                    );
                                } else {
                                    tracing::info!(
                                        program = %prog_name,
                                        path = ?path,
                                        debounce_secs = debounce_dur.as_secs(),
                                        "Filesystem change detected, debouncing restart trigger"
                                    );
                                    pending_triggers.insert(
                                        prog_name.clone(),
                                        PendingTrigger {
                                            program_name: prog_name,
                                            target_path: path.clone(),
                                            is_binary: matches!(rule, WatchRule::Binary { .. }),
                                            directory: rule.directory().map(|d| d.to_path_buf()),
                                            signal: rule.signal(),
                                            cmd: rule.cmd().map(String::from),
                                            deadline,
                                            debounce_duration: debounce_dur,
                                            consecutive_checks: 0,
                                        },
                                    );
                                }
                            }
                        }
                    }
                }

                // Configuration reload
                Some(new_configs) = reload_rx.recv() => {
                    tracing::info!("WatchService reloading rules for {} program(s)", new_configs.len());
                    self.configs = new_configs;
                    rules = Self::build_rules(&self.configs);
                    Self::sync_watcher(&mut watcher, &mut current_watched_dirs, &rules);
                }
            }
        }
    }

    /// Evaluates whether a filesystem event warrants a restart evaluation.
    fn is_relevant_event(event: &Event) -> bool {
        matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
        )
    }

    /// Checks whether an event path satisfies the given watch rule.
    fn matches_rule(rule: &WatchRule, event_path: &Path) -> bool {
        match rule {
            WatchRule::Binary { target_path, .. } => {
                if event_path == target_path {
                    return true;
                }
                if let (Some(ev_name), Some(tgt_name)) =
                    (event_path.file_name(), target_path.file_name())
                    && ev_name == tgt_name
                {
                    return true;
                }
                false
            }
            WatchRule::Directory {
                dir_path, pattern, ..
            } => {
                if !event_path.starts_with(dir_path) {
                    return false;
                }
                if let Some(name) = event_path.file_name().and_then(|n| n.to_str()) {
                    matches_pattern(pattern.as_deref(), name)
                } else {
                    false
                }
            }
        }
    }

    /// Extracts watch rules from all resolved program configurations.
    pub fn build_rules(configs: &HashMap<String, ProgramConfig>) -> Vec<WatchRule> {
        let mut rules = Vec::new();
        let platform = crate::platform::native_platform();

        for (name, cfg) in configs {
            let debounce = cfg.restart_debounce_secs;

            // 1. Binary change monitor
            if cfg.restart_when_binary_changed {
                let resolved_bin =
                    platform.resolve_executable(&cfg.command, cfg.directory.as_deref());

                if let Some(target_path) = resolved_bin {
                    if let Some(parent_dir) = target_path.parent() {
                        rules.push(WatchRule::Binary {
                            program_name: name.clone(),
                            target_path: target_path.clone(),
                            parent_dir: parent_dir.to_path_buf(),
                            directory: cfg.directory.clone(),
                            debounce_duration: debounce,
                            signal: cfg.restart_signal_when_binary_changed,
                            cmd: cfg.restart_cmd_when_binary_changed.clone(),
                        });
                    }
                } else {
                    tracing::warn!(
                        program = %name,
                        command = %cfg.command,
                        "Could not resolve binary executable path for restart_when_binary_changed"
                    );
                }
            }

            // 2. Directory monitor
            if let Some(ref dir) = cfg.restart_directory_monitor {
                let abs_dir = if dir.is_absolute() {
                    dir.clone()
                } else if let Some(ref wd) = cfg.directory {
                    wd.join(dir)
                } else {
                    std::env::current_dir()
                        .map(|cwd| cwd.join(dir))
                        .unwrap_or_else(|_| dir.clone())
                };

                let watch_dir = crate::platform::abs_path(&abs_dir);

                rules.push(WatchRule::Directory {
                    program_name: name.clone(),
                    dir_path: watch_dir,
                    pattern: cfg.restart_file_pattern.clone(),
                    directory: cfg.directory.clone(),
                    debounce_duration: debounce,
                    signal: cfg.restart_signal_when_file_changed,
                    cmd: cfg.restart_cmd_when_file_changed.clone(),
                });
            }
        }

        rules
    }

    /// Synchronizes active OS directory watches with the current set of rules.
    fn sync_watcher(
        watcher: &mut RecommendedWatcher,
        current_dirs: &mut HashSet<PathBuf>,
        rules: &[WatchRule],
    ) {
        let mut target_dirs = HashSet::new();

        for rule in rules {
            match rule {
                WatchRule::Binary { parent_dir, .. } => {
                    if parent_dir.is_dir() {
                        target_dirs.insert((parent_dir.clone(), RecursiveMode::NonRecursive));
                    }
                }
                WatchRule::Directory { dir_path, .. } => {
                    if dir_path.is_dir() {
                        target_dirs.insert((dir_path.clone(), RecursiveMode::Recursive));
                    }
                }
            }
        }

        // Unwatch obsolete directories
        let to_remove: Vec<PathBuf> = current_dirs
            .iter()
            .filter(|d| !target_dirs.iter().any(|(td, _)| td == *d))
            .cloned()
            .collect();

        for dir in to_remove {
            let _ = watcher.unwatch(&dir);
            current_dirs.remove(&dir);
            tracing::debug!(dir = ?dir, "Unwatched directory");
        }

        // Add new directory watches
        for (dir, mode) in target_dirs {
            if !current_dirs.contains(&dir) {
                match watcher.watch(&dir, mode) {
                    Ok(_) => {
                        tracing::info!(dir = ?dir, ?mode, "Registered filesystem watcher");
                        current_dirs.insert(dir);
                    }
                    Err(e) => {
                        tracing::warn!(dir = ?dir, error = %e, "Failed to watch directory");
                    }
                }
            }
        }
    }

    /// Executes the restart or reload trigger after debounce has expired.
    async fn execute_trigger(manager: &ManagerHandle, trigger: PendingTrigger) {
        tracing::info!(
            program = %trigger.program_name,
            target = ?trigger.target_path,
            is_binary = trigger.is_binary,
            "Debounce window elapsed, executing auto-restart action"
        );

        if let Some(ref cmd) = trigger.cmd {
            tracing::info!(
                program = %trigger.program_name,
                command = %cmd,
                "Executing custom restart command"
            );
            let mut shell_cmd = crate::platform::native_platform().build_shell_command(cmd);
            if let Some(ref dir) = trigger.directory {
                shell_cmd.current_dir(dir);
            }
            match tokio::time::timeout(Duration::from_secs(30), shell_cmd.status()).await {
                Ok(Ok(status)) => {
                    tracing::info!(
                        program = %trigger.program_name,
                        exit_status = %status,
                        "Custom restart command finished successfully"
                    );
                }
                Ok(Err(e)) => {
                    tracing::error!(
                        program = %trigger.program_name,
                        error = %e,
                        "Custom restart command failed"
                    );
                }
                Err(_) => {
                    tracing::error!(
                        program = %trigger.program_name,
                        "Custom restart command timed out after 30s"
                    );
                }
            }
        } else if let Some(sig) = trigger.signal {
            tracing::info!(
                program = %trigger.program_name,
                signal = %sig,
                "Sending reload signal to program"
            );
            if let Err(e) = manager.signal_program(&trigger.program_name, sig).await {
                tracing::warn!(
                    program = %trigger.program_name,
                    error = %e,
                    "Failed to signal program; falling back to full restart"
                );
                let _ = manager.restart_program(&trigger.program_name, None).await;
            }
        } else {
            tracing::info!(
                program = %trigger.program_name,
                "Restarting program via supervisor manager"
            );
            if let Err(e) = manager.restart_program(&trigger.program_name, None).await {
                tracing::error!(
                    program = %trigger.program_name,
                    error = %e,
                    "Failed to restart program after file change"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_pattern_wildcards() {
        assert!(matches_pattern(None, "anything.txt"));
        assert!(matches_pattern(Some("*"), "anything.txt"));
        assert!(matches_pattern(Some(""), "anything.txt"));
        assert!(matches_pattern(Some("*.jar"), "app.jar"));
        assert!(matches_pattern(Some("*.jar"), "my-app-1.0.jar"));
        assert!(!matches_pattern(Some("*.jar"), "app.war"));
        assert!(matches_pattern(Some("prefix_*"), "prefix_foo.txt"));
        assert!(!matches_pattern(Some("prefix_*"), "other_foo.txt"));
        assert!(matches_pattern(Some("exact.conf"), "exact.conf"));
        assert!(!matches_pattern(Some("exact.conf"), "inexact.conf"));
    }
}
