// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::compat::ini::values::parse_list;
use crate::error::ProgramError;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Represents a parsed INI document organized by section and key-value entries.
#[derive(Debug, Clone, Default)]
pub struct ParsedIni {
    /// Section name -> map of (lowercase_key -> value)
    pub sections: HashMap<String, HashMap<String, String>>,
    /// Preserves original ordering of sections
    pub section_order: Vec<String>,
}

impl ParsedIni {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or updates a section entry.
    pub fn insert_entry(&mut self, section: &str, key: &str, val: String) {
        if !self.sections.contains_key(section) {
            self.section_order.push(section.to_string());
            self.sections.insert(section.to_string(), HashMap::new());
        }
        if let Some(sec) = self.sections.get_mut(section) {
            sec.insert(key.to_ascii_lowercase(), val);
        }
    }

    /// Merges another `ParsedIni` into this one. Later values overwrite earlier ones.
    pub fn merge(&mut self, other: ParsedIni) {
        for section in other.section_order {
            if let Some(entries) = other.sections.get(&section) {
                if !self.sections.contains_key(&section) {
                    self.section_order.push(section.clone());
                    self.sections.insert(section.clone(), HashMap::new());
                }
                if let Some(target) = self.sections.get_mut(&section) {
                    for (k, v) in entries {
                        target.insert(k.clone(), v.clone());
                    }
                }
            }
        }
    }

    /// Retrieves an entry by section and key (key lookup is case-insensitive).
    pub fn get(&self, section: &str, key: &str) -> Option<&String> {
        self.sections
            .get(section)
            .and_then(|sec| sec.get(&key.to_ascii_lowercase()))
    }
}

/// Strips comments according to Python Supervisor / ConfigParser rules:
/// - Full-line comments start with `;` or `#` (optionally indented)
/// - Inline comments require preceding whitespace before `;` or `#`
/// - Characters `;` or `#` within quotes (`'` or `"`) are treated as literal
pub fn strip_inline_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;
    let chars: Vec<(usize, char)> = line.char_indices().collect();

    for i in 0..chars.len() {
        let (byte_idx, ch) = chars[i];
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            continue;
        }
        if !in_single && !in_double && (ch == ';' || ch == '#') {
            if i == 0 {
                return "";
            }
            let (_, prev_ch) = chars[i - 1];
            if prev_ch.is_whitespace() {
                let mut cut_idx = byte_idx;
                while cut_idx > 0 && line.as_bytes()[cut_idx - 1].is_ascii_whitespace() {
                    cut_idx -= 1;
                }
                return &line[..cut_idx];
            }
        }
    }
    line
}

/// Wildcard matching supporting `*` (any sequence) and `?` (any single character).
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p_chars: Vec<char> = pattern.chars().collect();
    let t_chars: Vec<char> = text.chars().collect();
    let (mut p_idx, mut t_idx) = (0, 0);
    let (mut star_idx, mut match_idx) = (None, 0);

    while t_idx < t_chars.len() {
        if p_idx < p_chars.len() && (p_chars[p_idx] == '?' || p_chars[p_idx] == t_chars[t_idx]) {
            p_idx += 1;
            t_idx += 1;
        } else if p_idx < p_chars.len() && p_chars[p_idx] == '*' {
            star_idx = Some(p_idx);
            match_idx = t_idx;
            p_idx += 1;
        } else if let Some(star) = star_idx {
            p_idx = star + 1;
            match_idx += 1;
            t_idx = match_idx;
        } else {
            return false;
        }
    }

    while p_idx < p_chars.len() && p_chars[p_idx] == '*' {
        p_idx += 1;
    }

    p_idx == p_chars.len()
}

/// Resolves glob patterns across file paths.
pub fn resolve_glob(pattern: &Path) -> Vec<PathBuf> {
    let parent = pattern.parent().unwrap_or(Path::new("."));
    let file_pattern = pattern
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    if !file_pattern.contains('*') && !file_pattern.contains('?') {
        if pattern.exists() {
            return vec![pattern.to_path_buf()];
        }
        return Vec::new();
    }

    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(fname) = path.file_name().and_then(|f| f.to_str())
                && wildcard_match(file_pattern, fname)
            {
                results.push(path);
            }
        }
    }
    results.sort();
    results
}

/// Parses raw INI content string, stripping comments and handling line continuations.
pub fn parse_ini_content(content: &str, here_dir: Option<&Path>) -> ParsedIni {
    let mut ini = ParsedIni::new();
    let mut current_section = String::new();
    let mut current_key = String::new();
    let mut current_val = String::new();
    let mut current_key_indent = 0usize;

    let here_str = here_dir
        .map(|p| {
            let s = p.to_string_lossy().to_string();
            s.replace('\\', "/")
        })
        .unwrap_or_else(|| ".".to_string());

    let flush_kv =
        |ini: &mut ParsedIni, sec: &str, k: &mut String, v: &mut String, here_replacement: &str| {
            if !sec.is_empty() && !k.is_empty() {
                let val = v.replace("%(here)s", here_replacement);
                ini.insert_entry(sec, k, val);
            }
            k.clear();
            v.clear();
        };

    for raw_line in content.lines() {
        let line = raw_line.trim_end_matches(['\r', '\n']);

        // Check if line is empty or purely a comment
        let trimmed_start = line.trim_start();
        if trimmed_start.is_empty()
            || trimmed_start.starts_with(';')
            || trimmed_start.starts_with('#')
        {
            continue;
        }

        let indent = line.len() - trimmed_start.len();

        // Strip inline comments
        let stripped = strip_inline_comment(line).trim();
        if stripped.is_empty() {
            continue;
        }

        // Section header: [section_name]
        if stripped.starts_with('[') && stripped.ends_with(']') {
            flush_kv(
                &mut ini,
                &current_section,
                &mut current_key,
                &mut current_val,
                &here_str,
            );
            current_section = stripped[1..stripped.len() - 1].trim().to_string();
            current_key_indent = 0;
            continue;
        }

        // Line continuation: line is indented deeper than the current key's indentation
        if !current_key.is_empty() && indent > current_key_indent {
            if !current_val.is_empty() {
                current_val.push('\n');
            }
            current_val.push_str(stripped);
            continue;
        }

        // Key-value pair delimiter '=' or ':'
        let delimiter_pos = stripped.find('=').or_else(|| stripped.find(':'));
        if let Some(pos) = delimiter_pos {
            flush_kv(
                &mut ini,
                &current_section,
                &mut current_key,
                &mut current_val,
                &here_str,
            );
            let k = stripped[..pos].trim().to_string();
            let v = stripped[pos + 1..].trim().to_string();
            current_key = k;
            current_val = v;
            current_key_indent = indent;
        }
    }

    flush_kv(
        &mut ini,
        &current_section,
        &mut current_key,
        &mut current_val,
        &here_str,
    );

    ini
}

/// Recursively loads an INI file and processes any `[include]` sections.
pub fn load_ini_file_with_includes(
    file_path: &Path,
    visited_files: &mut HashSet<PathBuf>,
    allow_includes: bool,
) -> Result<ParsedIni, ProgramError> {
    let canonical = file_path.canonicalize().unwrap_or_else(|_| {
        if file_path.is_relative() {
            std::env::current_dir()
                .map(|cwd| cwd.join(file_path))
                .unwrap_or_else(|_| file_path.to_path_buf())
        } else {
            file_path.to_path_buf()
        }
    });

    if visited_files.contains(&canonical) {
        return Err(ProgramError::ConfigError(format!(
            "Circular or duplicate include detected for '{:?}'",
            file_path
        )));
    }
    visited_files.insert(canonical.clone());

    let content = std::fs::read_to_string(file_path).map_err(|e| {
        ProgramError::ConfigError(format!(
            "Failed to read INI config file '{:?}': {}",
            file_path, e
        ))
    })?;

    let here_dir = file_path.parent().unwrap_or(Path::new("."));
    let mut main_ini = parse_ini_content(&content, Some(here_dir));

    if !allow_includes && main_ini.sections.contains_key("include") {
        return Err(ProgramError::ConfigError(format!(
            "Included config file '{:?}' is not permitted to contain an [include] section",
            file_path
        )));
    }

    if allow_includes && let Some(include_files_val) = main_ini.get("include", "files") {
        let here_str = here_dir.to_string_lossy().to_string().replace('\\', "/");
        let patterns = parse_list(include_files_val);

        for pattern_raw in patterns {
            let pattern_expanded = pattern_raw.replace("%(here)s", &here_str);
            let pattern_path = PathBuf::from(&pattern_expanded);

            let target_path = if pattern_path.is_relative() {
                here_dir.join(pattern_path)
            } else {
                pattern_path
            };

            let matched_files = resolve_glob(&target_path);
            for sub_file in matched_files {
                // Included files must not contain further [include] sections
                let sub_ini = load_ini_file_with_includes(&sub_file, visited_files, false)?;
                main_ini.merge(sub_ini);
            }
        }
    }

    Ok(main_ini)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_inline_comment() {
        assert_eq!(
            strip_inline_comment("file=/tmp/sock ; socket path"),
            "file=/tmp/sock"
        );
        assert_eq!(
            strip_inline_comment("command=/bin/sh -c \"echo tick; sleep 1\" ; comment"),
            "command=/bin/sh -c \"echo tick; sleep 1\""
        );
        assert_eq!(strip_inline_comment("a=b;c"), "a=b;c");
        assert_eq!(strip_inline_comment("; full line"), "");
        assert_eq!(strip_inline_comment("   # indented full line"), "");
    }

    #[test]
    fn test_wildcard_match() {
        assert!(wildcard_match("*.ini", "extra.ini"));
        assert!(!wildcard_match("*.ini", "extra.conf"));
        assert!(wildcard_match("test_*.txt", "test_123.txt"));
        assert!(wildcard_match("foo?bar", "fooxbar"));
        assert!(!wildcard_match("foo?bar", "foobar"));
    }

    #[test]
    fn test_parse_ini_content_basic() {
        let text = r#"
        ; supervisord configuration file
        [supervisord]
        logfile = %(here)s/supervisord.log
        loglevel = info ; comment here

        [program:cat]
        command = /bin/cat
        autostart = true
        autorestart = unexpected
        "#;

        let parsed = parse_ini_content(text, Some(Path::new("/tmp/test")));
        assert_eq!(
            parsed.get("supervisord", "logfile").unwrap(),
            "/tmp/test/supervisord.log"
        );
        assert_eq!(parsed.get("supervisord", "loglevel").unwrap(), "info");
        assert_eq!(parsed.get("program:cat", "command").unwrap(), "/bin/cat");
        assert_eq!(parsed.get("program:cat", "autostart").unwrap(), "true");
        assert_eq!(
            parsed.get("program:cat", "autorestart").unwrap(),
            "unexpected"
        );
    }

    #[test]
    fn test_line_continuation() {
        let text = r#"
        [include]
        files =
            conf.d/*.ini
            other/*.conf
        "#;
        let parsed = parse_ini_content(text, None);
        let files = parsed.get("include", "files").unwrap();
        assert!(files.contains("conf.d/*.ini"));
        assert!(files.contains("other/*.conf"));
    }
}
