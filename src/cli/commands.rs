// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::cli::client::SupervisorClient;
use crate::control::protocol::ProgramStatusDto;
use anyhow::Result;
use colored::Colorize;
use tabled::Table;
use tabled::settings::Style;

/// Executes the 'status' command.
pub async fn handle_status(client: &SupervisorClient, names: &[String]) -> Result<()> {
    let programs = client.status(names).await?;
    if programs.is_empty() {
        println!("{}", "No programs currently configured or found.".dimmed());
        return Ok(());
    }

    let mut dtos = Vec::with_capacity(programs.len());
    for p in programs {
        let colored_state = match p.state.to_uppercase().as_str() {
            "RUNNING" => "RUNNING".green().bold().to_string(),
            "STARTING" => "STARTING".yellow().bold().to_string(),
            "BACKOFF" => "BACKOFF".yellow().to_string(),
            "STOPPING" => "STOPPING".cyan().to_string(),
            "STOPPED" => "STOPPED".dimmed().to_string(),
            "EXITED" => "EXITED".normal().to_string(),
            "FATAL" => "FATAL".red().bold().to_string(),
            other => other.to_string(),
        };

        let pid_str = if p.pid == "None" || p.pid.is_empty() {
            "-".dimmed().to_string()
        } else {
            p.pid
        };

        let display_name = if !p.group.is_empty() && p.group != p.name {
            format!("{}:{}", p.group, p.name).bold().to_string()
        } else {
            p.name.bold().to_string()
        };

        dtos.push(ProgramStatusDto {
            name: display_name,
            group: p.group,
            state: colored_state,
            health: p.health,
            pid: pid_str,
            cpu: p.cpu,
            mem: p.mem,
            uptime: p.uptime,
            cron: p.cron,
            description: p.description,
        });
    }

    let mut table = Table::new(dtos);
    table.with(Style::rounded());
    println!("{}", table);

    Ok(())
}

/// Executes the 'start' command.
pub async fn handle_start(
    client: &SupervisorClient,
    names: &[String],
    is_async: bool,
    timeout: u64,
) -> Result<()> {
    let sync_mode = !is_async;
    for name in names {
        match client.start(name, sync_mode, timeout).await {
            Ok(results) => {
                for r in results {
                    if is_async {
                        println!(
                            "{}: start requested (state: {})",
                            r.name.bold(),
                            format!("{:?}", r.state).yellow()
                        );
                    } else {
                        let pid_text = r
                            .pid
                            .map(|p| format!("PID {}", p))
                            .unwrap_or_else(|| "no PID".to_string());
                        let elapsed_sec = r.elapsed_ms as f64 / 1000.0;
                        println!(
                            "{}: started [OK] ({}, took {:.2}s)",
                            r.name.bold(),
                            pid_text.cyan(),
                            elapsed_sec
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "{}: {}",
                    format!("Error starting '{}'", name).red().bold(),
                    e
                );
            }
        }
    }
    Ok(())
}

/// Executes the 'stop' command.
pub async fn handle_stop(
    client: &SupervisorClient,
    names: &[String],
    is_async: bool,
    timeout: u64,
) -> Result<()> {
    let sync_mode = !is_async;
    for name in names {
        match client.stop(name, sync_mode, timeout).await {
            Ok(results) => {
                for r in results {
                    if is_async {
                        println!(
                            "{}: stop requested (state: {})",
                            r.name.bold(),
                            format!("{:?}", r.state).cyan()
                        );
                    } else {
                        let elapsed_sec = r.elapsed_ms as f64 / 1000.0;
                        println!("{}: stopped [OK] (took {:.2}s)", r.name.bold(), elapsed_sec);
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "{}: {}",
                    format!("Error stopping '{}'", name).red().bold(),
                    e
                );
            }
        }
    }
    Ok(())
}

/// Executes the 'restart' command.
pub async fn handle_restart(
    client: &SupervisorClient,
    names: &[String],
    is_async: bool,
    timeout: u64,
) -> Result<()> {
    let sync_mode = !is_async;
    for name in names {
        match client.restart(name, sync_mode, timeout).await {
            Ok(results) => {
                for r in results {
                    if is_async {
                        println!(
                            "{}: restart requested (state: {})",
                            r.name.bold(),
                            format!("{:?}", r.state).yellow()
                        );
                    } else {
                        let pid_text = r
                            .pid
                            .map(|p| format!("PID {}", p))
                            .unwrap_or_else(|| "no PID".to_string());
                        let elapsed_sec = r.elapsed_ms as f64 / 1000.0;
                        println!(
                            "{}: restarted [OK] ({}, took {:.2}s)",
                            r.name.bold(),
                            pid_text.cyan(),
                            elapsed_sec
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "{}: {}",
                    format!("Error restarting '{}'", name).red().bold(),
                    e
                );
            }
        }
    }
    Ok(())
}

/// Executes the 'reload' command.
pub async fn handle_reload(client: &SupervisorClient) -> Result<()> {
    println!("{}", "Reloading configuration...".cyan());
    let res = client.reload().await?;

    println!("{}", "Configuration reloaded successfully:".green().bold());
    println!(
        "  {}: {} ({})",
        "Unchanged".bold(),
        res.unchanged.len(),
        "PID preserved".green()
    );
    if !res.added.is_empty() {
        println!(
            "  {}:     {} ({:?})",
            "Added".bold(),
            res.added.len(),
            res.added
        );
    } else {
        println!("  {}:     0", "Added".bold());
    }
    if !res.modified.is_empty() {
        println!(
            "  {}:  {} ({:?})",
            "Modified".bold(),
            res.modified.len(),
            res.modified
        );
    } else {
        println!("  {}:  0", "Modified".bold());
    }
    if !res.removed.is_empty() {
        println!(
            "  {}:   {} ({:?})",
            "Removed".bold(),
            res.removed.len(),
            res.removed
        );
    } else {
        println!("  {}:   0", "Removed".bold());
    }

    Ok(())
}

/// Executes the 'tail' command.
pub async fn handle_tail(
    client: &SupervisorClient,
    name: &str,
    follow: bool,
    lines: usize,
) -> Result<()> {
    if name == "all" {
        if follow {
            println!(
                "{}",
                "Streaming aggregated logs for all programs... (Press Ctrl+C to exit)"
                    .cyan()
                    .bold()
            );
            client
                .stream_all_logs(|line| {
                    if let Ok(entry) = serde_json::from_str::<crate::manager::LogEntry>(line) {
                        let stream_tag = if entry.stream == "stderr" {
                            "[stderr]".red()
                        } else {
                            "[stdout]".dimmed()
                        };
                        println!(
                            "[{}] {} {}",
                            entry.program.cyan().bold(),
                            stream_tag,
                            entry.line
                        );
                    } else {
                        println!("{}", line);
                    }
                })
                .await?;
        } else {
            println!(
                "{}",
                "Specify -f/--follow to tail all logs continuously: rsupervisorctl tail -f all"
                    .yellow()
            );
        }
        return Ok(());
    }

    let initial_lines = client.read_logs(name, lines).await?;
    for line in initial_lines {
        println!("{}", line);
    }

    if follow {
        client
            .stream_logs(name, |line| {
                println!("{}", line);
            })
            .await?;
    }

    Ok(())
}

/// Executes the 'events' command to stream live system events.
pub async fn handle_events(client: &SupervisorClient) -> Result<()> {
    println!(
        "{}",
        "Subscribing to system event bus... (Press Ctrl+C to exit)"
            .cyan()
            .bold()
    );
    client
        .stream_events(|line| {
            if let Ok(event) = serde_json::from_str::<crate::manager::SystemEvent>(line) {
                match event {
                    crate::manager::SystemEvent::StateChanged {
                        name,
                        old_state,
                        new_state,
                        pid,
                        exit_code,
                        description,
                    } => {
                        let pid_str = pid.map(|p| format!(" (PID {})", p)).unwrap_or_default();
                        let exit_str = exit_code
                            .map(|c| format!(" exit={}", c))
                            .unwrap_or_default();
                        println!(
                            "[{}] StateChanged: {:?} -> {:?}{}{} - {}",
                            name.cyan().bold(),
                            old_state,
                            new_state,
                            pid_str,
                            exit_str,
                            description
                        );
                    }
                    crate::manager::SystemEvent::HealthChanged {
                        name,
                        healthy,
                        status,
                        reason,
                    } => {
                        let tag = if healthy {
                            "HEALTHY".green()
                        } else {
                            "UNHEALTHY".red()
                        };
                        let r = reason.map(|s| format!(" ({})", s)).unwrap_or_default();
                        println!(
                            "[{}] HealthChanged: [{}] {:?}{}",
                            name.cyan().bold(),
                            tag,
                            status,
                            r
                        );
                    }
                    crate::manager::SystemEvent::ConfigReloaded {
                        added,
                        removed,
                        modified,
                        unchanged,
                    } => {
                        println!(
                            "{}: added={}, removed={}, modified={}, unchanged={}",
                            "ConfigReloaded".magenta().bold(),
                            added.len(),
                            removed.len(),
                            modified.len(),
                            unchanged.len()
                        );
                    }
                    crate::manager::SystemEvent::DaemonLifecycle {
                        action,
                        timestamp_secs,
                    } => {
                        println!(
                            "{}: action={} at {}",
                            "DaemonLifecycle".yellow().bold(),
                            action,
                            timestamp_secs
                        );
                    }
                    crate::manager::SystemEvent::CronTriggered {
                        name,
                        group,
                        action,
                        expression,
                    } => {
                        println!(
                            "[{}] CronTriggered ({}): action={} expr='{}'",
                            name.cyan().bold(),
                            group,
                            action.green().bold(),
                            expression
                        );
                    }
                    crate::manager::SystemEvent::ProcessPreStart {
                        name,
                        group: _,
                        command,
                    } => {
                        println!(
                            "[{}] ProcessPreStart: command='{}'",
                            name.cyan().bold(),
                            command
                        );
                    }
                    crate::manager::SystemEvent::ProcessPreStartFailed {
                        name,
                        group: _,
                        error,
                    } => {
                        println!(
                            "[{}] {}: {}",
                            name.cyan().bold(),
                            "ProcessPreStartFailed".red().bold(),
                            error
                        );
                    }
                    crate::manager::SystemEvent::ProcessPreStop {
                        name,
                        group: _,
                        command,
                    } => {
                        println!(
                            "[{}] ProcessPreStop: command='{}'",
                            name.cyan().bold(),
                            command
                        );
                    }
                    crate::manager::SystemEvent::ProcessPreStopFailed {
                        name,
                        group: _,
                        error,
                    } => {
                        println!(
                            "[{}] {}: {}",
                            name.cyan().bold(),
                            "ProcessPreStopFailed".red().bold(),
                            error
                        );
                    }
                }
            } else {
                println!("{}", line);
            }
        })
        .await?;
    Ok(())
}

/// Executes the 'stdin' command to send input characters to a running program.
pub async fn handle_stdin(client: &SupervisorClient, name: &str, chars: &str) -> Result<()> {
    match client.send_stdin(name, chars).await {
        Ok(()) => {
            println!(
                "{} Sent {} byte(s) to stdin of '{}'",
                "SUCCESS:".green().bold(),
                chars.len(),
                name.cyan().bold()
            );
            Ok(())
        }
        Err(e) => {
            eprintln!(
                "{} Failed to send stdin to '{}': {}",
                "ERROR:".red().bold(),
                name.cyan().bold(),
                e
            );
            Err(e)
        }
    }
}
