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

        dtos.push(ProgramStatusDto {
            name: p.name.bold().to_string(),
            state: colored_state,
            pid: pid_str,
            priority: p.priority,
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
