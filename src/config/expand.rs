// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use std::borrow::Cow;

/// Environment macro expander supporting `${VAR}` and `${VAR:-default}` syntax.
#[derive(Debug, Clone, Copy, Default)]
pub struct MacroExpander;

impl MacroExpander {
    /// Creates a new MacroExpander instance.
    pub const fn new() -> Self {
        Self
    }

    /// Expands environment variables in the format `${VAR}` or `${VAR:-default}` within a raw string.
    /// Comment lines starting with `#` are preserved verbatim without expansion.
    pub fn expand(&self, raw: &str) -> String {
        let mut result = String::with_capacity(raw.len());

        for line in raw.split_inclusive('\n') {
            if line.trim_start().starts_with('#') {
                result.push_str(line);
                continue;
            }
            result.push_str(&self.expand_line(line));
        }

        result
    }

    fn expand_line(&self, raw: &str) -> String {
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
/// Comment lines starting with `#` are preserved verbatim without expansion.
#[inline]
pub fn expand_env_vars(raw: &str) -> String {
    MacroExpander::new().expand(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

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

        // Test comment preservation
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
    }
}
