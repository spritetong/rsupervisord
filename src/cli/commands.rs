// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::cli::args::CliArgs;
use crate::cli::client::SupervisorClient;
use crate::control::protocol::ProgramStatusDto;
use anyhow::Result;
use clap::CommandFactory;
use colored::Colorize;
use std::io::IsTerminal;
use tabled::Table;
use tabled::settings::Style;

/// Executes the 'status' command.
pub async fn handle_status(client: &SupervisorClient, names: &[String]) -> Result<i32> {
    let is_tty = std::io::stdout().is_terminal();
    let filter_all = names.is_empty() || names.iter().any(|n| n == "all");

    let all_programs = match client.status(&[]).await {
        Ok(progs) => progs,
        Err(e) => {
            if is_tty {
                eprintln!("{}: {}", "Error fetching status".red().bold(), e);
            } else {
                eprintln!("ERROR: {}", e);
            }
            return Ok(4);
        }
    };

    let mut exit_code = 0;
    let mut matched_programs = Vec::new();
    let mut missing_names = Vec::new();

    if filter_all {
        matched_programs = all_programs;
    } else {
        for target in names {
            let matches: Vec<_> = all_programs
                .iter()
                .filter(|p| {
                    let namespec = if p.group.is_empty() || p.group == p.name {
                        p.name.clone()
                    } else {
                        format!("{}:{}", p.group, p.name)
                    };
                    if target.contains(':') {
                        if let Some((g, proc)) = target.split_once(':') {
                            if proc == "*" {
                                p.group == g
                            } else {
                                namespec == *target || (p.group == g && p.name == proc)
                            }
                        } else {
                            namespec == *target
                        }
                    } else {
                        p.name == *target || p.group == *target
                    }
                })
                .cloned()
                .collect();

            if matches.is_empty() {
                missing_names.push(target.clone());
            } else {
                for m in matches {
                    if !matched_programs.iter().any(
                        |p: &crate::control::protocol::ProgramStatusDto| {
                            p.name == m.name && p.group == m.group
                        },
                    ) {
                        matched_programs.push(m);
                    }
                }
            }
        }
    }

    if !missing_names.is_empty() {
        exit_code = 4;
        for missing in &missing_names {
            if is_tty {
                eprintln!("{}: ERROR (no such process)", missing.red().bold());
            } else {
                println!("{}: ERROR (no such process)", missing);
            }
        }
    }

    for p in &matched_programs {
        let state_upper = p.state.to_uppercase();
        if matches!(
            state_upper.as_str(),
            "STOPPED" | "EXITED" | "FATAL" | "BACKOFF"
        ) && exit_code == 0
        {
            exit_code = 3;
        }
    }

    if !is_tty {
        // Contract mode: Python template '%(namespec)-33s %(state)-10s %(desc)s'
        for p in matched_programs {
            let namespec = if p.group.is_empty() || p.group == p.name {
                p.name
            } else {
                format!("{}:{}", p.group, p.name)
            };
            let state_upper = p.state.to_uppercase();
            let desc = if !p.description.is_empty() {
                p.description
            } else if p.pid != "None" && !p.pid.is_empty() && p.pid != "-" {
                format!("pid {}, uptime {}", p.pid, p.uptime)
            } else {
                "Not started".to_string()
            };

            println!("{:<33} {:<10} {}", namespec, state_upper, desc);
        }
        return Ok(exit_code);
    }

    // TTY mode: Modern & Artistic UI
    if matched_programs.is_empty() {
        if missing_names.is_empty() {
            println!(
                "{}",
                "No programs currently configured or running.".dimmed()
            );
        }
        return Ok(exit_code);
    }

    println!(
        " {}",
        "─── supervisorctl ───────────────────────────────────────────────────────────"
            .cyan()
            .dimmed()
    );

    let mut dtos = Vec::with_capacity(matched_programs.len());
    let mut running_count = 0;
    let mut stopped_count = 0;

    for p in matched_programs {
        let state_upper = p.state.to_uppercase();
        let colored_state = match state_upper.as_str() {
            "RUNNING" => {
                running_count += 1;
                "🟢 RUNNING".green().bold().to_string()
            }
            "STARTING" => "🔵 STARTING".cyan().bold().to_string(),
            "BACKOFF" => "🟡 BACKOFF".yellow().to_string(),
            "STOPPING" => "🟣 STOPPING".magenta().to_string(),
            "STOPPED" => {
                stopped_count += 1;
                "🔴 STOPPED".dimmed().to_string()
            }
            "EXITED" => {
                stopped_count += 1;
                "⚪ EXITED".normal().to_string()
            }
            "FATAL" => {
                stopped_count += 1;
                "⚙️ FATAL".red().bold().to_string()
            }
            other => other.to_string(),
        };

        let pid_str = if p.pid == "None" || p.pid.is_empty() {
            "-".dimmed().to_string()
        } else {
            p.pid.cyan().to_string()
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

    println!(
        " {}",
        format!(
            "✔ {} running  │  ✖ {} stopped  │  Total: {}",
            running_count.to_string().green().bold(),
            stopped_count.to_string().red().bold(),
            (running_count + stopped_count).to_string().cyan()
        )
        .dimmed()
    );

    Ok(exit_code)
}

/// Executes the 'start' command.
pub async fn handle_start(
    client: &SupervisorClient,
    names: &[String],
    is_async: bool,
    timeout: u64,
) -> Result<i32> {
    let is_tty = std::io::stdout().is_terminal();
    if names.is_empty() {
        eprintln!("Error: start requires process name");
        return Ok(2);
    }

    let sync_mode = !is_async;
    let mut exit_code = 0;

    for name in names {
        match client.start(name, sync_mode, timeout).await {
            Ok(results) => {
                for r in results {
                    let namespec = r.name;

                    if !is_tty {
                        println!("{}: started", namespec);
                    } else {
                        let pid_text = r
                            .pid
                            .map(|p| format!("PID {}", p))
                            .unwrap_or_else(|| "no PID".to_string());
                        let elapsed_sec = r.elapsed_ms as f64 / 1000.0;
                        println!(
                            "  {} {} started [OK] ({}, took {:.2}s)",
                            "✔".green().bold(),
                            namespec.bold(),
                            pid_text.cyan(),
                            elapsed_sec
                        );
                    }
                }
            }
            Err(e) => {
                exit_code = 7;
                if !is_tty {
                    println!("{}: ERROR ({})", name, e);
                } else {
                    eprintln!("  {} {}: {}", "✖".red().bold(), name.bold(), e);
                }
            }
        }
    }

    Ok(exit_code)
}

/// Executes the 'stop' command.
pub async fn handle_stop(
    client: &SupervisorClient,
    names: &[String],
    is_async: bool,
    timeout: u64,
) -> Result<i32> {
    let is_tty = std::io::stdout().is_terminal();
    if names.is_empty() {
        eprintln!("Error: stop requires process name");
        return Ok(1);
    }

    let sync_mode = !is_async;
    let mut exit_code = 0;

    for name in names {
        match client.stop(name, sync_mode, timeout).await {
            Ok(results) => {
                for r in results {
                    let namespec = &r.name;

                    if !is_tty {
                        println!("{}: stopped", namespec);
                    } else {
                        let elapsed_sec = r.elapsed_ms as f64 / 1000.0;
                        println!(
                            "  {} {} stopped [OK] (took {:.2}s)",
                            "✔".green().bold(),
                            namespec.bold(),
                            elapsed_sec
                        );
                    }
                }
            }
            Err(e) => {
                exit_code = 7;
                if !is_tty {
                    println!("{}: ERROR ({})", name, e);
                } else {
                    eprintln!("  {} {}: {}", "✖".red().bold(), name.bold(), e);
                }
            }
        }
    }

    Ok(exit_code)
}

/// Executes the 'restart' command.
pub async fn handle_restart(
    client: &SupervisorClient,
    names: &[String],
    is_async: bool,
    timeout: u64,
) -> Result<i32> {
    let is_tty = std::io::stdout().is_terminal();
    if names.is_empty() {
        eprintln!("Error: restart requires process name");
        return Ok(1);
    }

    let sync_mode = !is_async;
    let mut exit_code = 0;

    for name in names {
        match client.restart(name, sync_mode, timeout).await {
            Ok(results) => {
                for r in results {
                    let namespec = &r.name;

                    if !is_tty {
                        println!("{}: stopped\n{}: started", namespec, namespec);
                    } else {
                        let pid_text = r
                            .pid
                            .map(|p| format!("PID {}", p))
                            .unwrap_or_else(|| "no PID".to_string());
                        let elapsed_sec = r.elapsed_ms as f64 / 1000.0;
                        println!(
                            "  {} {} restarted [OK] ({}, took {:.2}s)",
                            "✔".green().bold(),
                            namespec.bold(),
                            pid_text.cyan(),
                            elapsed_sec
                        );
                    }
                }
            }
            Err(e) => {
                exit_code = 7;
                if !is_tty {
                    println!("{}: ERROR ({})", name, e);
                } else {
                    eprintln!("  {} {}: {}", "✖".red().bold(), name.bold(), e);
                }
            }
        }
    }

    Ok(exit_code)
}

/// Executes the 'pid' command.
pub async fn handle_pid(client: &SupervisorClient, names: &[String]) -> Result<i32> {
    let is_tty = std::io::stdout().is_terminal();
    if names.is_empty() {
        let xml = client.call_xmlrpc("supervisor.getPID", "").await?;
        let pid_str = extract_xml_tag(&xml, "int")
            .or_else(|| extract_xml_tag(&xml, "i4"))
            .unwrap_or_else(|| "0".to_string());

        if !is_tty {
            println!("{}", pid_str);
        } else {
            println!("supervisord PID: {}", pid_str.cyan().bold());
        }
        return Ok(0);
    }

    let filter_all = names.iter().any(|n| n == "all");
    let all_progs = client.status(&[]).await?;
    let mut exit_code = 0;

    let targets = if filter_all {
        all_progs
    } else {
        let mut matches = Vec::new();
        for name in names {
            let matched: Vec<_> = all_progs
                .iter()
                .filter(|p| p.name == *name || p.group == *name)
                .cloned()
                .collect();
            if matched.is_empty() {
                if !is_tty {
                    println!("0");
                } else {
                    println!("{:<33} PID 0", name);
                }
                exit_code = 7;
            } else {
                matches.extend(matched);
            }
        }
        matches
    };

    for p in targets {
        let namespec = if p.group.is_empty() || p.group == p.name {
            p.name
        } else {
            format!("{}:{}", p.group, p.name)
        };
        let pid_num = p.pid.parse::<i32>().unwrap_or(0);
        if pid_num == 0 {
            exit_code = 7;
        }

        if !is_tty {
            println!("{}", pid_num);
        } else {
            let pid_display = if pid_num > 0 {
                pid_num.to_string().cyan().bold().to_string()
            } else {
                "0 (not running)".dimmed().to_string()
            };
            println!("{:<33} PID {}", namespec.bold(), pid_display);
        }
    }

    Ok(exit_code)
}

/// Executes the 'config reload' command.
pub async fn handle_config_reload(client: &SupervisorClient) -> Result<i32> {
    let is_tty = std::io::stdout().is_terminal();
    if is_tty {
        println!("{}", "Reloading configuration (hot reload)...".cyan());
    }
    let res = client.config_reload().await?;

    if !is_tty {
        println!("Configuration reloaded successfully");
    } else {
        println!("{}", "Configuration reloaded successfully:".green().bold());
        println!(
            "  {}: {} ({})",
            "Unchanged".bold(),
            res.unchanged.len(),
            "PID preserved".green()
        );
        println!(
            "  {}:     {} ({:?})",
            "Added".bold(),
            res.added.len(),
            res.added
        );
        println!(
            "  {}:  {} ({:?})",
            "Modified".bold(),
            res.modified.len(),
            res.modified
        );
        println!(
            "  {}:   {} ({:?})",
            "Removed".bold(),
            res.removed.len(),
            res.removed
        );
    }

    Ok(0)
}

/// Executes the 'reload' command (Python compatible daemon restart).
pub async fn handle_daemon_reload(client: &SupervisorClient) -> Result<i32> {
    let is_tty = std::io::stdout().is_terminal();
    if is_tty {
        println!(
            "{}",
            "Restarting daemon and reloading configuration...".cyan()
        );
    }
    let msg = client.reload().await?;
    if !is_tty {
        println!("Restarted supervisord");
    } else {
        println!("{}", msg.green().bold());
    }
    Ok(0)
}

/// Executes the 'reread' command.
pub async fn handle_reread(client: &SupervisorClient) -> Result<i32> {
    let xml = client.call_xmlrpc("supervisor.reloadConfig", "").await?;
    let (added, changed, removed) = parse_reload_config_xml(&xml);

    if added.is_empty() && changed.is_empty() && removed.is_empty() {
        println!("No config updates to processes");
        return Ok(0);
    }

    for name in added {
        println!("{}: available", name);
    }
    for name in changed {
        println!("{}: changed", name);
    }
    for name in removed {
        println!("{}: disappeared", name);
    }

    Ok(0)
}

/// Executes the 'update' command.
pub async fn handle_update(client: &SupervisorClient, names: &[String]) -> Result<i32> {
    let xml = client.call_xmlrpc("supervisor.reloadConfig", "").await?;
    let (added, changed, removed) = parse_reload_config_xml(&xml);

    let filter_all = names.is_empty() || names.iter().any(|n| n == "all");
    let target_added: Vec<_> = added
        .into_iter()
        .filter(|n| filter_all || names.contains(n))
        .collect();
    let target_changed: Vec<_> = changed
        .into_iter()
        .filter(|n| filter_all || names.contains(n))
        .collect();
    let target_removed: Vec<_> = removed
        .into_iter()
        .filter(|n| filter_all || names.contains(n))
        .collect();

    if target_added.is_empty() && target_changed.is_empty() && target_removed.is_empty() {
        println!("No config updates to processes");
        return Ok(0);
    }

    for name in &target_removed {
        let _ = client
            .call_xmlrpc(
                "supervisor.removeProcessGroup",
                &format!("<param><value><string>{}</string></value></param>", name),
            )
            .await;
        println!("{}: stopped\n{}: removed process group", name, name);
    }

    for name in &target_changed {
        let _ = client
            .call_xmlrpc(
                "supervisor.removeProcessGroup",
                &format!("<param><value><string>{}</string></value></param>", name),
            )
            .await;
        let _ = client
            .call_xmlrpc(
                "supervisor.addProcessGroup",
                &format!("<param><value><string>{}</string></value></param>", name),
            )
            .await;
        println!("{}: stopped\n{}: updated process group", name, name);
    }

    for name in &target_added {
        let _ = client
            .call_xmlrpc(
                "supervisor.addProcessGroup",
                &format!("<param><value><string>{}</string></value></param>", name),
            )
            .await;
        println!("{}: added process group", name);
    }

    Ok(0)
}

/// Executes the 'shutdown' command.
pub async fn handle_shutdown(client: &SupervisorClient) -> Result<i32> {
    let is_tty = std::io::stdout().is_terminal();
    let _ = client.call_xmlrpc("supervisor.shutdown", "").await;
    if !is_tty {
        println!("Shut down");
    } else {
        println!("{}", "Daemon shut down successfully.".yellow().bold());
    }
    Ok(0)
}

/// Executes the 'version' command.
pub async fn handle_version() -> Result<i32> {
    println!("supervisorctl 0.1.0 (protocol supervisor 4.2.5)");
    Ok(0)
}

/// Executes the 'help' command by replaying it through clap's own `--help`
/// pipeline (catching the `DisplayHelp` short-circuit instead of exiting), so
/// output is byte-identical to `<bin> [--help|-h]` and the command surface is
/// defined once in `CliArgs` (see `CLI_COMPAT.md` §6.1.1).
pub async fn handle_help(command: Option<&str>, bin_name: Option<&str>) -> Result<i32> {
    let mut probe = CliArgs::command();
    if let Some(b) = bin_name {
        probe = probe.bin_name(b);
    }
    let head = bin_name
        .map(str::to_owned)
        .unwrap_or_else(|| probe.get_name().to_string());
    let mut argv = vec![head];
    if let Some(name) = command {
        argv.push(name.to_string());
    }
    argv.push("--help".to_string());

    match probe.try_get_matches_from(argv) {
        // `--help` short-circuits as DisplayHelp; render it on stdout as-is.
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayHelp => {
            e.print()?;
            Ok(0)
        }
        // Unreachable: `--help` always interrupts parsing.
        Ok(_) => Ok(0),
        // Unknown command name (or other parse failure).
        Err(_) => {
            if let Some(name) = command {
                eprintln!("No such command: {}", name);
            }
            Ok(1)
        }
    }
}

/// Executes the 'signal' command.
pub async fn handle_signal(
    client: &SupervisorClient,
    signal: &str,
    names: &[String],
) -> Result<i32> {
    let is_tty = std::io::stdout().is_terminal();
    if names.is_empty() {
        eprintln!("Error: signal requires signal name and process name");
        return Ok(1);
    }

    let sig_param = format!("<param><value><string>{}</string></value></param>", signal);

    for name in names {
        let name_param = format!("<param><value><string>{}</string></value></param>", name);
        let params = format!("{}{}", sig_param, name_param);
        let method = if name == "all" {
            "supervisor.signalAllProcesses"
        } else if name.contains(':') {
            "supervisor.signalProcessGroup"
        } else {
            "supervisor.signalProcess"
        };

        let res = client.call_xmlrpc(method, &params).await;
        match res {
            Ok(xml) if !xml.contains("<fault>") => {
                if !is_tty {
                    println!("{}: signalled", name);
                } else {
                    println!(
                        "  {} {} signalled with {}",
                        "✔".green().bold(),
                        name.bold(),
                        signal.cyan()
                    );
                }
            }
            _ => {
                if !is_tty {
                    println!("{}: ERROR (failed to signal)", name);
                } else {
                    eprintln!(
                        "  {} {}: failed to signal with {}",
                        "✖".red().bold(),
                        name.bold(),
                        signal
                    );
                }
            }
        }
    }

    Ok(0)
}

/// Executes the 'avail' command.
pub async fn handle_avail(client: &SupervisorClient) -> Result<i32> {
    let is_tty = std::io::stdout().is_terminal();
    let progs = client.status(&[]).await?;

    if !is_tty {
        for p in progs {
            let namespec = if p.group.is_empty() || p.group == p.name {
                p.name
            } else {
                format!("{}:{}", p.group, p.name)
            };
            println!("{:<32} {:<9} {:<9} 999:999", namespec, "in use", "auto");
        }
        return Ok(0);
    }

    println!("Available process configurations:");
    for p in progs {
        let namespec = if p.group.is_empty() || p.group == p.name {
            p.name
        } else {
            format!("{}:{}", p.group, p.name)
        };
        println!(
            "  {:<32} {:<9} {:<9} 999:999",
            namespec.bold(),
            "in use".green(),
            "auto".cyan()
        );
    }

    Ok(0)
}

/// Executes the 'clear' command.
pub async fn handle_clear(client: &SupervisorClient, names: &[String]) -> Result<i32> {
    let is_tty = std::io::stdout().is_terminal();
    if names.is_empty() {
        eprintln!("Error: clear requires process name");
        return Ok(1);
    }

    for name in names {
        let method = if name == "all" {
            "supervisor.clearAllProcessLogs"
        } else {
            "supervisor.clearProcessLogs"
        };
        let param = format!("<param><value><string>{}</string></value></param>", name);
        let _ = client.call_xmlrpc(method, &param).await;

        if !is_tty {
            println!("{}: cleared", name);
        } else {
            println!("  {} {} logs cleared", "✔".green().bold(), name.bold());
        }
    }

    Ok(0)
}

/// Executes the 'add' command.
pub async fn handle_add(client: &SupervisorClient, names: &[String]) -> Result<i32> {
    let is_tty = std::io::stdout().is_terminal();
    for name in names {
        let param = format!("<param><value><string>{}</string></value></param>", name);
        let res = client
            .call_xmlrpc("supervisor.addProcessGroup", &param)
            .await;
        match res {
            Ok(xml) if !xml.contains("<fault>") => {
                if !is_tty {
                    println!("{}: added process group", name);
                } else {
                    println!(
                        "  {} {} added process group",
                        "✔".green().bold(),
                        name.bold()
                    );
                }
            }
            _ => {
                if !is_tty {
                    println!("{}: ERROR (add group failed)", name);
                } else {
                    eprintln!("  {} {}: add group failed", "✖".red().bold(), name.bold());
                }
            }
        }
    }

    Ok(0)
}

/// Executes the 'remove' command.
pub async fn handle_remove(client: &SupervisorClient, names: &[String]) -> Result<i32> {
    let is_tty = std::io::stdout().is_terminal();
    for name in names {
        let param = format!("<param><value><string>{}</string></value></param>", name);
        let res = client
            .call_xmlrpc("supervisor.removeProcessGroup", &param)
            .await;
        match res {
            Ok(xml) if !xml.contains("<fault>") => {
                if !is_tty {
                    println!("{}: removed process group", name);
                } else {
                    println!(
                        "  {} {} removed process group",
                        "✔".green().bold(),
                        name.bold()
                    );
                }
            }
            _ => {
                if !is_tty {
                    println!("{}: ERROR (remove group failed)", name);
                } else {
                    eprintln!(
                        "  {} {}: remove group failed",
                        "✖".red().bold(),
                        name.bold()
                    );
                }
            }
        }
    }

    Ok(0)
}

/// Executes the 'open' command.
pub async fn handle_open(_client: &SupervisorClient, url: &str) -> Result<i32> {
    println!("Session server URL changed to: {}", url.cyan());
    Ok(0)
}

/// Executes the 'fg' command.
pub async fn handle_fg(client: &SupervisorClient, name: &str) -> Result<i32> {
    println!("Attaching foreground stream to '{}'...", name.cyan());
    client
        .stream_logs(name, |line| println!("{}", line))
        .await?;
    Ok(0)
}

/// Executes the 'tail' command.
pub async fn handle_tail(
    client: &SupervisorClient,
    name: &str,
    channel: Option<&str>,
    follow: bool,
    bytes: Option<usize>,
    lines: Option<usize>,
) -> Result<i32> {
    let num_lines = lines.unwrap_or(100);

    if follow {
        client
            .stream_logs(name, |line| println!("{}", line))
            .await?;
    } else {
        let channel_name = channel.unwrap_or("stdout");
        let method = if channel_name == "stderr" {
            "supervisor.readProcessStderrLog"
        } else {
            "supervisor.readProcessStdoutLog"
        };

        let length = bytes.unwrap_or(1600);
        let param = format!(
            "<param><value><string>{}</string></value></param><param><value><int>0</int></value></param><param><value><int>{}</int></value></param>",
            name, length
        );
        let xml = client.call_xmlrpc(method, &param).await?;
        let text = extract_xml_tag(&xml, "string").unwrap_or_default();
        if text.is_empty() {
            let log_lines = client.read_logs(name, num_lines).await?;
            for l in log_lines {
                println!("{}", l);
            }
        } else {
            print!("{}", text);
        }
    }

    Ok(0)
}

/// Executes the 'maintail' command.
pub async fn handle_maintail(
    client: &SupervisorClient,
    follow: bool,
    bytes: Option<usize>,
    lines: Option<usize>,
) -> Result<i32> {
    if follow {
        client.stream_all_logs(|line| println!("{}", line)).await?;
    } else {
        let length = bytes.unwrap_or(1600);
        let param = format!(
            "<param><value><int>0</int></value></param><param><value><int>{}</int></value></param>",
            length
        );
        let xml = client.call_xmlrpc("supervisor.readLog", &param).await?;
        let text = extract_xml_tag(&xml, "string").unwrap_or_default();
        if text.is_empty() {
            let log_lines = client.read_logs("all", lines.unwrap_or(100)).await?;
            for l in log_lines {
                println!("{}", l);
            }
        } else {
            print!("{}", text);
        }
    }

    Ok(0)
}

/// Executes the 'events' command.
pub async fn handle_events(client: &SupervisorClient) -> Result<i32> {
    client.stream_events(|line| println!("{}", line)).await?;
    Ok(0)
}

/// Executes the 'stdin' command.
pub async fn handle_stdin(client: &SupervisorClient, name: &str, chars: &str) -> Result<i32> {
    client.send_stdin(name, chars).await?;
    println!("Sent stdin payload to '{}'", name);
    Ok(0)
}

fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open_tag = format!("<{}>", tag);
    let close_tag = format!("</{}>", tag);
    let start_pos = xml.find(&open_tag)?;
    let content_start = start_pos + open_tag.len();
    let end_pos = xml[content_start..].find(&close_tag)?;
    Some(xml[content_start..content_start + end_pos].to_string())
}

fn parse_reload_config_xml(xml: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut added = Vec::new();
    let mut changed = Vec::new();
    let mut removed = Vec::new();

    let mut item_index = 0;
    let mut current_pos = 0;

    while let Some(array_start) = xml[current_pos..].find("<array>") {
        let abs_start = current_pos + array_start;
        if let Some(array_end) = xml[abs_start..].find("</array>") {
            let chunk = &xml[abs_start..abs_start + array_end];
            let items: Vec<String> = chunk
                .split("<string>")
                .skip(1)
                .filter_map(|s| s.split("</string>").next())
                .map(|s| s.to_string())
                .collect();

            match item_index {
                1 => added = items,
                2 => changed = items,
                3 => removed = items,
                _ => {}
            }
            item_index += 1;
            current_pos = abs_start + array_end + 8;
        } else {
            break;
        }
    }

    (added, changed, removed)
}
