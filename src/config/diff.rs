// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::program::config::ProgramConfig;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigDiff {
    pub added: Vec<ProgramConfig>,
    pub removed: Vec<String>,
    pub modified: Vec<ProgramConfig>,
    pub unchanged: Vec<String>,
}

impl ConfigDiff {
    pub fn compute(
        old_programs: &HashMap<String, ProgramConfig>,
        new_programs: &HashMap<String, ProgramConfig>,
    ) -> Self {
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut modified = Vec::new();
        let mut unchanged = Vec::new();

        for (name, new_cfg) in new_programs {
            match old_programs.get(name) {
                Some(old_cfg) => {
                    if old_cfg == new_cfg {
                        unchanged.push(name.clone());
                    } else {
                        modified.push(new_cfg.clone());
                    }
                }
                None => {
                    added.push(new_cfg.clone());
                }
            }
        }

        for name in old_programs.keys() {
            if !new_programs.contains_key(name) {
                removed.push(name.clone());
            }
        }

        // Sort entries to ensure deterministic ordering for test assertions and logs
        added.sort_by(|a, b| a.name.cmp(&b.name));
        removed.sort();
        modified.sort_by(|a, b| a.name.cmp(&b.name));
        unchanged.sort();

        Self {
            added,
            removed,
            modified,
            unchanged,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_diff() {
        let mut old_map = HashMap::new();
        let p_keep = ProgramConfig::new("keep_me", "sleep 10");
        let p_mod_old = ProgramConfig::new("mod_me", "echo old");
        let p_del = ProgramConfig::new("del_me", "echo bye");
        old_map.insert("keep_me".to_string(), p_keep.clone());
        old_map.insert("mod_me".to_string(), p_mod_old);
        old_map.insert("del_me".to_string(), p_del);

        let mut new_map = HashMap::new();
        let p_mod_new = ProgramConfig::new("mod_me", "echo new");
        let p_add = ProgramConfig::new("add_me", "echo hi");
        new_map.insert("keep_me".to_string(), p_keep);
        new_map.insert("mod_me".to_string(), p_mod_new);
        new_map.insert("add_me".to_string(), p_add);

        let diff = ConfigDiff::compute(&old_map, &new_map);
        assert_eq!(diff.unchanged, vec!["keep_me".to_string()]);
        assert_eq!(diff.removed, vec!["del_me".to_string()]);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.added[0].name, "add_me");
        assert_eq!(diff.modified.len(), 1);
        assert_eq!(diff.modified[0].name, "mod_me");
    }
}
