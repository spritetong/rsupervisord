// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ConfigError;
use ahash::AHashMap;
use compact_str::CompactString;
use std::borrow::Cow;
use std::path::Path;

/// Evaluation context and replacement pattern engine compatible with Python Supervisor
/// format string syntax (`%(...)s`, `%(...)02d`, `%%`) and compatibility `$()` replacement syntax (`$(...)`, `$$`).
///
/// Designed to be reusable across YAML configurations, upcoming `.ini` configurations,
/// and program instance execution contexts.
#[derive(Debug, Clone, Default)]
pub struct StringExpression {
    env: AHashMap<CompactString, String>,
}

impl StringExpression {
    /// Creates an empty `StringExpression` context.
    pub fn new() -> Self {
        Self {
            env: AHashMap::new(),
        }
    }

    /// Creates a `StringExpression` populated with default variables:
    /// - `ENV_<KEY>`: all system environment variables
    /// - `host_node_name`: machine hostname queried from the platform backend
    pub fn with_defaults() -> Self {
        let mut se = Self::new();

        for (k, v) in std::env::vars() {
            let key = format!("ENV_{}", k);
            se.env.insert(CompactString::new(key), v);
        }

        let hostname = crate::platform::native_platform().hostname();
        se.env
            .insert(CompactString::new("host_node_name"), hostname);

        se
    }

    /// Creates a `StringExpression` populated with default variables plus `here` pointing
    /// to the directory of the configuration file.
    pub fn with_config_dir(dir: impl AsRef<Path>) -> Self {
        let mut se = Self::with_defaults();
        let here_str = dir.as_ref().to_string_lossy().to_string();
        se.env.insert(CompactString::new("here"), here_str);
        se
    }

    /// Adds a key-value variable to the expression context.
    pub fn add(&mut self, key: impl AsRef<str>, val: impl AsRef<str>) -> &mut Self {
        self.env
            .insert(CompactString::new(key.as_ref()), val.as_ref().to_string());
        self
    }

    /// Adds a key-value variable to the expression context (fluent builder taking self by value).
    pub fn with_var(mut self, key: impl AsRef<str>, val: impl AsRef<str>) -> Self {
        self.add(key, val);
        self
    }

    /// Populates contextual variables for a specific program process instance:
    /// - `program_name`: section/program identifier
    /// - `group_name`: process group identifier
    /// - `process_num`: zero-indexed process instance sequence number
    /// - `numprocs`: total number of processes in the group
    pub fn with_program_context(
        mut self,
        program_name: &str,
        group_name: &str,
        process_num: usize,
        numprocs: usize,
    ) -> Self {
        self.add("program_name", program_name);
        self.add("group_name", group_name);
        self.add("process_num", process_num.to_string());
        self.add("numprocs", numprocs.to_string());
        self
    }

    /// Queries the value of a variable in the expression context.
    #[inline]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.env.get(key).map(|s| s.as_str())
    }

    /// Checks if a variable is defined in the expression context.
    #[inline]
    pub fn contains_key(&self, key: &str) -> bool {
        self.env.contains_key(key)
    }

    /// Returns the number of defined variables in the expression context.
    #[inline]
    pub fn len(&self) -> usize {
        self.env.len()
    }

    /// Returns true if no variables are defined in the expression context.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.env.is_empty()
    }

    /// Evaluates replacement patterns in `text` using default context name `"expression"`.
    #[inline]
    pub fn eval(&self, text: &str) -> Result<String, ConfigError> {
        self.eval_named(text, "expression")
    }

    /// Evaluates replacement patterns in `text` with an explicit context/field name
    /// (e.g. `"command"`, `"stdout_logfile"`), matching Python Supervisord error diagnostics.
    pub fn eval_named(&self, text: &str, context_name: &str) -> Result<String, ConfigError> {
        if !text.contains('%') && !text.contains('$') {
            return Ok(text.to_string());
        }

        let mut result = String::with_capacity(text.len() + 32);
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let mut i = 0;

        while i < len {
            let ch = chars[i];

            if ch == '%' || ch == '$' {
                // Check for escape sequence: %% -> %, $$ -> $
                if i + 1 < len && chars[i + 1] == ch {
                    result.push(ch);
                    i += 2;
                    continue;
                }

                // Check for expression trigger: %( or $(
                if i + 1 < len && chars[i + 1] == '(' {
                    let trigger = ch;
                    let var_start = i + 2;
                    let mut end = var_start;

                    while end < len && chars[end] != ')' {
                        end += 1;
                    }

                    if end >= len {
                        return Err(ConfigError::Expansion {
                            message: format!(
                                "Format string '{text}' for '{context_name}' is badly formatted: unclosed parenthesis"
                            ),
                        });
                    }

                    let var_name_str: String = chars[var_start..end].iter().collect();
                    let var_name = var_name_str.trim();

                    if var_name.is_empty() {
                        return Err(ConfigError::Expansion {
                            message: format!(
                                "Format string '{text}' for '{context_name}' is badly formatted: empty variable name"
                            ),
                        });
                    }

                    let var_val = match self.env.get(var_name) {
                        Some(v) => v.as_str(),
                        None => {
                            if trigger == '$' {
                                // For $(...), preserve literal syntax if not defined in context
                                // to allow shell command substitutions like $(whoami) or $((i+1))
                                result.push('$');
                                result.push('(');
                                result.push_str(&var_name_str);
                                result.push(')');
                                i = end + 1;
                                continue;
                            }
                            let mut available: Vec<&str> =
                                self.env.keys().map(|k| k.as_str()).collect();
                            available.sort_unstable();
                            return Err(ConfigError::Expansion {
                                message: format!(
                                    "Format string '{text}' for '{context_name}' contains variable '{var_name}' which cannot be expanded. Available variables: {}",
                                    available.join(", ")
                                ),
                            });
                        }
                    };

                    // Inspect format specifier following ')' at index end + 1
                    let spec_idx = end + 1;
                    let mut temp_idx = spec_idx;

                    // Collect flags/width e.g. "02", "4", "-", "+"
                    while temp_idx < len
                        && (chars[temp_idx].is_ascii_digit()
                            || chars[temp_idx] == '-'
                            || chars[temp_idx] == '+')
                    {
                        temp_idx += 1;
                    }

                    let mut matched_spec = false;
                    let mut spec_str = String::new();

                    if temp_idx < len
                        && (chars[temp_idx] == 's'
                            || chars[temp_idx] == 'd'
                            || chars[temp_idx] == 'i')
                    {
                        spec_str = chars[spec_idx..=temp_idx].iter().collect();
                        temp_idx += 1;
                        matched_spec = true;
                    }

                    if matched_spec {
                        Self::format_value(
                            &mut result,
                            var_name,
                            var_val,
                            &spec_str,
                            text,
                            context_name,
                        )?;
                        i = temp_idx;
                    } else if trigger == '$' {
                        // Compatibility $(var) defaults to string substitution without consuming suffix
                        result.push_str(var_val);
                        i = end + 1;
                    } else {
                        // For %(...), if followed by unknown alphabetic char, report error
                        if spec_idx < len && chars[spec_idx].is_ascii_alphabetic() {
                            return Err(ConfigError::Expansion {
                                message: format!(
                                    "Format string '{text}' for '{context_name}' has unsupported format specifier '%{}'",
                                    chars[spec_idx]
                                ),
                            });
                        }
                        // Default lenient string substitution
                        result.push_str(var_val);
                        i = end + 1;
                    }

                    continue;
                }
            }

            result.push(ch);
            i += 1;
        }

        Ok(result)
    }

    fn format_value(
        result: &mut String,
        var_name: &str,
        var_val: &str,
        spec: &str,
        text: &str,
        context_name: &str,
    ) -> Result<(), ConfigError> {
        if spec == "s" {
            result.push_str(var_val);
            return Ok(());
        }

        if spec.ends_with('d') || spec.ends_with('i') {
            let int_val = var_val.trim().parse::<i64>().map_err(|e| {
                ConfigError::Expansion {
                    message: format!(
                        "Cannot convert variable '{var_name}' with value '{var_val}' to integer for format '{spec}' in '{context_name}': {e}"
                    ),
                }
            })?;

            let flags_width = &spec[..spec.len() - 1];
            if flags_width.is_empty() {
                result.push_str(&int_val.to_string());
            } else if flags_width.starts_with('0') {
                let width = flags_width
                    .parse::<usize>()
                    .map_err(|_| ConfigError::Expansion {
                        message: format!(
                            "Invalid integer format width in '{spec}' for format string '{text}'"
                        ),
                    })?;
                result.push_str(&format!("{:0width$}", int_val, width = width));
            } else {
                let width = flags_width
                    .parse::<usize>()
                    .map_err(|_| ConfigError::Expansion {
                        message: format!(
                            "Invalid integer format width in '{spec}' for format string '{text}'"
                        ),
                    })?;
                result.push_str(&format!("{:width$}", int_val, width = width));
            }
            return Ok(());
        }

        Err(ConfigError::Expansion {
            message: format!(
                "Unsupported format specifier '%{spec}' for variable '{var_name}' in '{text}'"
            ),
        })
    }
}

/// Environment macro expander supporting `${VAR}`, `${VAR:-default}`,
/// `%()` format strings, and `$()` replacement patterns.
#[derive(Debug, Clone, Copy, Default)]
pub struct MacroExpander;

impl MacroExpander {
    /// Creates a new `MacroExpander` instance.
    pub const fn new() -> Self {
        Self
    }

    /// Expands environment variables and patterns within a raw configuration string.
    /// Comment lines starting with `#` or `;` are preserved verbatim without expansion.
    pub fn expand(&self, raw: &str) -> String {
        let expr = StringExpression::with_defaults();
        self.expand_with_expr(raw, &expr)
    }

    /// Expands environment variables and patterns within a raw configuration string
    /// with an explicit config directory (populating `here`).
    pub fn expand_with_config_dir(&self, raw: &str, config_dir: Option<&Path>) -> String {
        let expr = match config_dir {
            Some(dir) => StringExpression::with_config_dir(dir),
            None => StringExpression::with_defaults(),
        };
        self.expand_with_expr(raw, &expr)
    }

    /// Expands environment variables and patterns using a specific `StringExpression` context.
    pub fn expand_with_expr(&self, raw: &str, expr: &StringExpression) -> String {
        let mut result = String::with_capacity(raw.len());

        for line in raw.split_inclusive('\n') {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') || trimmed.starts_with(';') {
                result.push_str(line);
                continue;
            }

            // Step 1: Expand ${VAR:-default} and ${VAR}
            let env_expanded = self.expand_line_env(line);

            // Step 2: If % or $( is present, evaluate through StringExpression
            if (env_expanded.contains('%') || env_expanded.contains("$("))
                && let Ok(evaled) = expr.eval(&env_expanded)
            {
                result.push_str(&evaled);
                continue;
            }

            result.push_str(&env_expanded);
        }

        result
    }

    /// Expands environment variables and patterns within a single configuration value
    /// (not a raw file), using an explicit config directory (populating `here`).
    ///
    /// Unlike [`Self::expand_with_config_dir`], this does not split on newlines or
    /// preserve comment lines; it expands the whole string as one value.
    pub fn expand_value(&self, value: &str, config_dir: Option<&Path>) -> String {
        let expr = match config_dir {
            Some(dir) => StringExpression::with_config_dir(dir),
            None => StringExpression::with_defaults(),
        };
        self.expand_value_with_expr(value, &expr)
    }

    /// Expands environment variables and patterns within a single configuration value
    /// using a specific `StringExpression` context.
    ///
    /// Mirrors [`Self::expand_with_expr`] semantics: `${VAR}` expansion first, then a
    /// lenient `%()`/`$()` evaluation that keeps literals when expansion fails.
    pub fn expand_value_with_expr(&self, value: &str, expr: &StringExpression) -> String {
        if !value.contains('$') && !value.contains('%') {
            return value.to_string();
        }

        // Step 1: Expand ${VAR:-default} and ${VAR}
        let env_expanded = self.expand_line_env(value);

        // Step 2: If % or $( is present, evaluate through StringExpression
        if (env_expanded.contains('%') || env_expanded.contains("$("))
            && let Ok(evaled) = expr.eval(&env_expanded)
        {
            return evaled;
        }

        env_expanded
    }

    fn expand_line_env(&self, raw: &str) -> String {
        let mut result = String::with_capacity(raw.len());
        let mut chars = raw.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '$' && chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut var_expr = String::new();
                let mut closed = false;

                for next_ch in chars.by_ref() {
                    if next_ch == '}' {
                        closed = true;
                        break;
                    }
                    var_expr.push(next_ch);
                }

                if closed {
                    let val = self.resolve_var_expr(&var_expr);
                    result.push_str(&val);
                } else {
                    // Unclosed '${' sequence: emit raw prefix unchanged
                    result.push_str("${");
                    result.push_str(&var_expr);
                }
            } else {
                result.push(ch);
            }
        }

        result
    }

    fn resolve_var_expr<'a>(&self, expr: &'a str) -> Cow<'a, str> {
        if let Some((var_name, default_val)) = expr.split_once(":-") {
            let var_name = var_name.trim();
            match std::env::var(var_name) {
                Ok(val) if !val.is_empty() => Cow::Owned(val),
                _ => Cow::Borrowed(default_val),
            }
        } else {
            let var_name = expr.trim();
            match std::env::var(var_name) {
                Ok(val) => Cow::Owned(val),
                Err(_) => Cow::Borrowed(""),
            }
        }
    }
}

/// Expands environment variables in the format `${VAR}` or `${VAR:-default}` within a raw string.
/// Comment lines starting with `#` or `;` are preserved verbatim without expansion.
#[inline]
pub fn expand_env_vars(raw: &str) -> String {
    MacroExpander::new().expand(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_expression_python_percent_syntax() {
        let mut se = StringExpression::new();
        se.add("here", "/etc/supervisor")
            .add("program_name", "web_worker")
            .add("ENV_LOGLEVEL", "DEBUG");

        assert_eq!(
            se.eval("%(here)s/bin/%(program_name)s --level=%(ENV_LOGLEVEL)s")
                .unwrap(),
            "/etc/supervisor/bin/web_worker --level=DEBUG"
        );
    }

    #[test]
    fn test_string_expression_format_specifiers() {
        let mut se = StringExpression::new();
        se.add("var1", "ok").add("var2", "2").add("var3", "12");

        // Exactly matches Go's test: %(var1)s_test_%(var2)02d -> ok_test_02
        assert_eq!(se.eval("%(var1)s_test_%(var2)02d").unwrap(), "ok_test_02");

        assert_eq!(se.eval("%(var1)s_test_%(var3)02d").unwrap(), "ok_test_12");

        assert_eq!(se.eval("%(var2)d and %(var2)3d").unwrap(), "2 and   2");
    }

    #[test]
    fn test_string_expression_dollar_syntax() {
        let mut se = StringExpression::new();
        se.add("var1", "ok")
            .add("var2", "2")
            .add("program_name", "api")
            .add("ENV_PORT", "8080");

        // Bare $(var) without 's'
        assert_eq!(
            se.eval("http://127.0.0.1:$(ENV_PORT)/$(program_name)")
                .unwrap(),
            "http://127.0.0.1:8080/api"
        );

        // Formatted integer with $(var)02d
        assert_eq!(se.eval("$(var1)_test_$(var2)02d").unwrap(), "ok_test_02");

        // Mixed $() and %()
        assert_eq!(se.eval("$(var1)_%(var2)02d").unwrap(), "ok_02");
    }

    #[test]
    fn test_string_expression_escapes() {
        let se = StringExpression::new();
        assert_eq!(se.eval("%%100 and $$200").unwrap(), "%100 and $200");
    }

    #[test]
    fn test_string_expression_missing_variable_error() {
        let mut se = StringExpression::new();
        se.add("b_var", "1").add("a_var", "2");

        let err = se.eval_named("%(c_var)s", "command").unwrap_err();
        let err_msg = err.to_string();

        assert!(err_msg.contains("variable 'c_var' which cannot be expanded"));
        assert!(err_msg.contains("Available variables: a_var, b_var"));
    }

    #[test]
    fn test_string_expression_integer_format_error() {
        let mut se = StringExpression::new();
        se.add("non_num", "not_a_number");

        let err = se.eval("%(non_num)02d").unwrap_err();
        assert!(
            err.to_string()
                .contains("Cannot convert variable 'non_num'")
        );
    }

    #[test]
    fn test_string_expression_builder_and_program_context() {
        let se = StringExpression::new().with_program_context("worker", "workers", 3, 10);

        assert_eq!(
            se.eval("%(program_name)s_%(process_num)02d of %(numprocs)d (group: %(group_name)s)")
                .unwrap(),
            "worker_03 of 10 (group: workers)"
        );
    }

    #[test]
    fn test_ini_compatibility_simulation() {
        let mut se = StringExpression::new();
        se.add("here", "/app")
            .add("program_name", "srv")
            .add("process_num", "1")
            .add("numprocs", "4")
            .add("group_name", "srv_group");

        let ini_cmd = "%(here)s/bin/%(program_name)s --id=%(process_num)02d";
        let ini_logfile = "%(here)s/logs/%(program_name)s-%(process_num)d.log";

        assert_eq!(
            se.eval_named(ini_cmd, "command").unwrap(),
            "/app/bin/srv --id=01"
        );
        assert_eq!(
            se.eval_named(ini_logfile, "stdout_logfile").unwrap(),
            "/app/logs/srv-1.log"
        );
    }

    #[test]
    fn test_macro_expander_struct_and_function() {
        unsafe {
            std::env::set_var("TEST_SUPERVISOR_PORT", "9001");
            std::env::remove_var("TEST_UNSET_VAR");
        }

        let expander = MacroExpander::new();

        assert_eq!(
            expander.expand("http://127.0.0.1:${TEST_SUPERVISOR_PORT}/api"),
            "http://127.0.0.1:9001/api"
        );

        assert_eq!(
            expand_env_vars("http://127.0.0.1:${TEST_SUPERVISOR_PORT}/api"),
            "http://127.0.0.1:9001/api"
        );

        assert_eq!(
            expand_env_vars("value is ${TEST_UNSET_VAR:-fallback_default}"),
            "value is fallback_default"
        );

        assert_eq!(
            expand_env_vars("plain string without vars"),
            "plain string without vars"
        );

        assert_eq!(
            expand_env_vars("unclosed ${VAR_NAME and more"),
            "unclosed ${VAR_NAME and more"
        );

        // Test comment preservation for # and ;
        assert_eq!(
            expand_env_vars(
                "# Comment with ${TEST_UNSET_VAR} unset\nport: ${TEST_SUPERVISOR_PORT}"
            ),
            "# Comment with ${TEST_UNSET_VAR} unset\nport: 9001"
        );

        assert_eq!(
            expand_env_vars("  # Indented comment ${TEST_UNSET_VAR}\nname: test"),
            "  # Indented comment ${TEST_UNSET_VAR}\nname: test"
        );

        assert_eq!(
            expand_env_vars("; INI style comment with ${TEST_UNSET_VAR}\nname: test"),
            "; INI style comment with ${TEST_UNSET_VAR}\nname: test"
        );
    }

    #[test]
    fn test_macro_expander_with_expressions() {
        unsafe {
            std::env::set_var("TEST_MACRO_VAR", "expanded_macro");
        }

        let mut se = StringExpression::with_defaults();
        se.add("here", "/config/dir");

        let expander = MacroExpander::new();
        let input = "path: %(here)s/app\nenv: $(ENV_TEST_MACRO_VAR)\nlegacy: ${TEST_MACRO_VAR}";
        let output = expander.expand_with_expr(input, &se);

        assert_eq!(
            output,
            "path: /config/dir/app\nenv: expanded_macro\nlegacy: expanded_macro"
        );
    }
}
