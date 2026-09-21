// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::program::config::ProgramConfig;
use chrono::{DateTime, Utc};
use croner::Cron;
use std::collections::HashMap;
use std::time::Duration;

/// An actionable trigger produced when a scheduled cron expression fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CronAction {
    Start {
        name: String,
        group: String,
        expression: String,
    },
    Stop {
        name: String,
        group: String,
        expression: String,
    },
}

#[derive(Debug, Clone)]
pub struct CronEntry {
    pub name: String,
    pub group: String,
    pub is_stop: bool,
    pub expression: String,
    pub cron: Cron,
    pub next_run: DateTime<Utc>,
}

/// In-memory table managing active cron schedules for supervised programs.
#[derive(Debug, Default, Clone)]
pub struct CronTable {
    entries: Vec<CronEntry>,
}

impl CronTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Constructs a CronTable from resolved program configurations at initialization or reload.
    pub fn from_configs(configs: &HashMap<String, ProgramConfig>) -> Self {
        let mut table = Self::new();
        let now = Utc::now();

        for (name, cfg) in configs {
            if let Some(ref expr) = cfg.cron
                && let Ok(cron) = expr.parse::<Cron>()
                && let Ok(next_run) = cron.find_next_occurrence(&now, false)
            {
                table.entries.push(CronEntry {
                    name: name.clone(),
                    group: cfg.group.clone(),
                    is_stop: false,
                    expression: expr.clone(),
                    cron,
                    next_run,
                });
            }

            if let Some(ref expr) = cfg.cron_stop
                && let Ok(cron) = expr.parse::<Cron>()
                && let Ok(next_run) = cron.find_next_occurrence(&now, false)
            {
                table.entries.push(CronEntry {
                    name: name.clone(),
                    group: cfg.group.clone(),
                    is_stop: true,
                    expression: expr.clone(),
                    cron,
                    next_run,
                });
            }
        }

        table
    }

    /// Returns the next scheduled run time for the specified program as an RFC 3339 string.
    pub fn get_next_run(&self, program_name: &str) -> Option<String> {
        self.entries
            .iter()
            .filter(|e| e.name == program_name && !e.is_stop)
            .map(|e| e.next_run)
            .min()
            .map(|dt| dt.to_rfc3339())
    }

    /// Returns the start cron expression for the specified program.
    pub fn get_cron_expr(&self, program_name: &str) -> Option<String> {
        self.entries
            .iter()
            .find(|e| e.name == program_name && !e.is_stop)
            .map(|e| e.expression.clone())
    }

    /// Calculates the earliest `tokio::time::Instant` when any cron job will become due.
    pub fn earliest_deadline(&self) -> Option<tokio::time::Instant> {
        let earliest_dt = self.entries.iter().map(|e| e.next_run).min()?;
        let now_dt = Utc::now();

        if earliest_dt <= now_dt {
            Some(tokio::time::Instant::now())
        } else {
            let dur = (earliest_dt - now_dt)
                .to_std()
                .unwrap_or(Duration::from_millis(10));
            Some(tokio::time::Instant::now() + dur)
        }
    }

    /// Pops and returns all actions that are due at or before `now`, advancing their next run time.
    pub fn pop_due_actions(&mut self, now: DateTime<Utc>) -> Vec<CronAction> {
        let mut actions = Vec::new();

        for entry in &mut self.entries {
            if entry.next_run <= now {
                if entry.is_stop {
                    actions.push(CronAction::Stop {
                        name: entry.name.clone(),
                        group: entry.group.clone(),
                        expression: entry.expression.clone(),
                    });
                } else {
                    actions.push(CronAction::Start {
                        name: entry.name.clone(),
                        group: entry.group.clone(),
                        expression: entry.expression.clone(),
                    });
                }

                if let Ok(next) = entry.cron.find_next_occurrence(&now, false) {
                    entry.next_run = next;
                }
            }
        }

        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_table_parsing_and_due_actions() {
        let mut configs = HashMap::new();
        let mut cfg = ProgramConfig::new("cron_worker", "echo 1");
        cfg.cron = Some("*/5 * * * *".to_string());
        configs.insert("cron_worker".to_string(), cfg);

        let mut table = CronTable::from_configs(&configs);
        assert_eq!(table.entries.len(), 1);
        assert!(table.get_next_run("cron_worker").is_some());
        assert_eq!(table.get_cron_expr("cron_worker").unwrap(), "*/5 * * * *");

        // Simulate time advancing into the future past next_run
        let future_time = table.entries[0].next_run + chrono::Duration::seconds(1);
        let due = table.pop_due_actions(future_time);
        assert_eq!(due.len(), 1);
        match &due[0] {
            CronAction::Start { name, .. } => assert_eq!(name, "cron_worker"),
            _ => panic!("Expected start action"),
        }

        // After pop, next_run should be advanced further into the future
        assert!(table.entries[0].next_run > future_time);
    }
}
