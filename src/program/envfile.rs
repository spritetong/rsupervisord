// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

//! `.env` file loading for go-parity `envFiles` (OI-2).
//!
//! Format: one `KEY=VALUE` per line; `#` comments and blank lines ignored;
//! optional surrounding single/double quotes on the value; optional `export `
//! prefix. Missing files are skipped with a warning (go-aligned).

use std::collections::HashMap;
use std::path::Path;

/// Loads a single `.env` file into a map. Returns an empty map on missing file
/// (warned by the caller via [`load_env_files`]).
fn load_one(path: &Path) -> Result<HashMap<String, String>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {}", path.display(), e))?;
    let mut map = HashMap::new();
    for (idx, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let mut value = value.trim();
        if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            value = &value[1..value.len() - 1];
        }
        let _ = idx;
        map.insert(key.to_string(), value.to_string());
    }
    Ok(map)
}

/// Loads all env files in order. Later files override earlier keys.
/// Missing files are skipped with a warning (go-aligned; Python has no equivalent).
pub fn load_env_files(paths: &[std::path::PathBuf]) -> HashMap<String, String> {
    let mut merged = HashMap::new();
    for path in paths {
        match load_one(path) {
            Ok(map) => merged.extend(map),
            Err(e) => tracing::warn!(path = %path.display(), "env file skipped: {}", e),
        }
    }
    merged
}
