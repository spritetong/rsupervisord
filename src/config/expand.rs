use std::borrow::Cow;

/// Expands environment variables in the format `${VAR}` or `${VAR:-default}` within a raw string.
pub fn expand_env_vars(raw: &str) -> String {
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
                let val = resolve_var_expr(&var_expr);
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

fn resolve_var_expr(expr: &str) -> Cow<'_, str> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_env_vars() {
        unsafe {
            std::env::set_var("TEST_SUPERVISOR_PORT", "9001");
            std::env::remove_var("TEST_UNSET_VAR");
        }

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
    }
}
