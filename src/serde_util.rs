// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

//! Boundary serde helpers: wire-compatible conversions so config/JSON value
//! types stay unchanged while runtime structs hold parsed forms.

use crate::error::ProgramError;
use serde::{Deserialize, Deserializer};
use std::fmt;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Basic type conversions (string_to_* / *_to_string / numeric)
// ---------------------------------------------------------------------------

/// Parses loose booleans accepted by Python Supervisor:
/// `1`, `true`, `yes`, `on` -> `true`
/// `0`, `false`, `no`, `off` -> `false`
pub fn string_to_bool(s: &str) -> Result<bool, ProgramError> {
    let trimmed = s.trim();
    if trimmed.eq_ignore_ascii_case("1")
        || trimmed.eq_ignore_ascii_case("true")
        || trimmed.eq_ignore_ascii_case("yes")
        || trimmed.eq_ignore_ascii_case("on")
    {
        Ok(true)
    } else if trimmed.eq_ignore_ascii_case("0")
        || trimmed.eq_ignore_ascii_case("false")
        || trimmed.eq_ignore_ascii_case("no")
        || trimmed.eq_ignore_ascii_case("off")
    {
        Ok(false)
    } else {
        Err(ProgramError::ConfigError(format!(
            "Invalid boolean value '{}'",
            trimmed
        )))
    }
}

/// Parses human-readable byte sizes into numeric bytes.
/// Supports units: B, KB, K, MB, M, GB, G (case-insensitive).
pub fn string_to_bytes(s: &str) -> Result<usize, ProgramError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(ProgramError::ConfigError(
            "Byte size string cannot be empty".to_string(),
        ));
    }

    let upper = trimmed.to_uppercase();
    if let Some(num) = upper.strip_suffix("GB").or_else(|| upper.strip_suffix('G')) {
        let val: f64 = num.trim().parse().map_err(|_| {
            ProgramError::ConfigError(format!("Invalid gigabyte value: '{}'", trimmed))
        })?;
        Ok((val * 1024.0 * 1024.0 * 1024.0) as usize)
    } else if let Some(num) = upper.strip_suffix("MB").or_else(|| upper.strip_suffix('M')) {
        let val: f64 = num.trim().parse().map_err(|_| {
            ProgramError::ConfigError(format!("Invalid megabyte value: '{}'", trimmed))
        })?;
        Ok((val * 1024.0 * 1024.0) as usize)
    } else if let Some(num) = upper.strip_suffix("KB").or_else(|| upper.strip_suffix('K')) {
        let val: f64 = num.trim().parse().map_err(|_| {
            ProgramError::ConfigError(format!("Invalid kilobyte value: '{}'", trimmed))
        })?;
        Ok((val * 1024.0) as usize)
    } else if let Some(num) = upper.strip_suffix('B') {
        let val: usize = num
            .trim()
            .parse()
            .map_err(|_| ProgramError::ConfigError(format!("Invalid byte value: '{}'", trimmed)))?;
        Ok(val)
    } else {
        let val: usize = upper.parse().map_err(|_| {
            ProgramError::ConfigError(format!("Invalid numeric byte value: '{}'", trimmed))
        })?;
        Ok(val)
    }
}

/// Formats numeric bytes into a canonical human-readable string (e.g. "50MB", "10KB", "25B").
pub fn bytes_to_string(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = 1024 * KB;
    const GB: usize = 1024 * MB;

    if bytes > 0 && bytes.is_multiple_of(GB) {
        format!("{}GB", bytes / GB)
    } else if bytes > 0 && bytes.is_multiple_of(MB) {
        format!("{}MB", bytes / MB)
    } else if bytes > 0 && bytes.is_multiple_of(KB) {
        format!("{}KB", bytes / KB)
    } else {
        format!("{}B", bytes)
    }
}

/// Parses an octal file mode string (`"0700"`, `"0o700"`, `"700"`).
/// Result is masked to `0o7777` (permission + setuid/setgid/sticky bits).
pub fn string_to_chmod(s: &str) -> Result<u32, ProgramError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(ProgramError::ConfigError(
            "Invalid octal mode '': empty value".to_string(),
        ));
    }
    let digits = trimmed
        .strip_prefix("0o")
        .or_else(|| trimmed.strip_prefix("0O"))
        .unwrap_or(trimmed);
    u32::from_str_radix(digits, 8)
        .map(|mode| mode & crate::consts::CHMOD_MASK)
        .map_err(|e| ProgramError::ConfigError(format!("Invalid octal mode '{}': {}", trimmed, e)))
}

/// Parses umask, supporting octal notations like `022` or `0o22`.
pub fn string_to_umask(s: &str) -> Result<u32, ProgramError> {
    let trimmed = s.trim();
    if let Some(rest) = trimmed
        .strip_prefix("0o")
        .or_else(|| trimmed.strip_prefix("0O"))
    {
        u32::from_str_radix(rest, 8).map_err(|e| {
            ProgramError::ConfigError(format!("Invalid octal umask '{}': {}", trimmed, e))
        })
    } else if trimmed.starts_with('0') && trimmed.len() > 1 {
        u32::from_str_radix(trimmed, 8).map_err(|e| {
            ProgramError::ConfigError(format!("Invalid octal umask '{}': {}", trimmed, e))
        })
    } else {
        trimmed
            .parse::<u32>()
            .map_err(|e| ProgramError::ConfigError(format!("Invalid umask '{}': {}", trimmed, e)))
    }
}

/// Parses a comma- or whitespace-separated list of `i32` values (e.g. exit codes).
/// Example: `0,2` -> `vec![0, 2]`
pub fn string_to_i32_list(s: &str) -> Result<Vec<i32>, ProgramError> {
    let mut codes = Vec::new();
    for token in s.split([',', ' ', '\t', '\r', '\n']) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let code = token.parse::<i32>().map_err(|e| {
            ProgramError::ConfigError(format!("Invalid integer '{}' in list: {}", token, e))
        })?;
        codes.push(code);
    }
    Ok(codes)
}

/// Splits a comma- or whitespace-separated list of string items
/// (e.g. `programs`, `files`, or `include` globs).
pub fn string_to_str_list(s: &str) -> Vec<String> {
    s.split([',', ' ', '\t', '\r', '\n'])
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Parses a whole-second duration string (e.g. `"10"`) into `Duration`.
pub fn string_to_duration(s: &str) -> Result<Duration, ProgramError> {
    s.parse::<u64>()
        .map(Duration::from_secs)
        .map_err(|e| ProgramError::ConfigError(format!("invalid duration '{}': {}", s, e)))
}

/// Formats whole seconds into a human readable duration
/// (e.g. 45s, 12m 30s, 1h 45m).
pub fn duration_secs_to_string(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Normalizes an HTTP bind address according to standard supervisor conventions:
/// - `:9001` -> `0.0.0.0:9001`
/// - `*:9001` -> `0.0.0.0:9001`
/// - `9001` -> `0.0.0.0:9001`
/// - `127.0.0.1:9001` -> `127.0.0.1:9001`
pub fn normalize_http_bind(bind: &str) -> String {
    let trimmed = bind.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some(port) = trimmed.strip_prefix('*') {
        let port_part = port.strip_prefix(':').unwrap_or(port);
        format!("0.0.0.0:{}", port_part)
    } else if let Some(port) = trimmed.strip_prefix(':') {
        format!("0.0.0.0:{}", port)
    } else if trimmed.chars().all(|c| c.is_ascii_digit()) {
        format!("0.0.0.0:{}", trimmed)
    } else {
        trimmed.to_string()
    }
}

/// Percent-decodes a string (`%XX` hex and `+` as space).
pub fn percent_decode_string(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let next_two = (chars.next(), chars.next());
            match next_two {
                (Some(h1), Some(h2)) => {
                    let hex = [h1, h2];
                    if let Ok(hex_str) = std::str::from_utf8(&hex)
                        && let Ok(val) = u8::from_str_radix(hex_str, 16)
                    {
                        bytes.push(val);
                        continue;
                    }
                    bytes.push(b'%');
                    bytes.push(h1);
                    bytes.push(h2);
                }
                (Some(h1), None) => {
                    bytes.push(b'%');
                    bytes.push(h1);
                }
                _ => {
                    bytes.push(b'%');
                }
            }
        } else if b == b'+' {
            bytes.push(b' ');
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| s.to_string())
}

/// Saturating `u64` -> `i32` conversion (protects 32-bit wire integers).
#[inline]
pub fn u64_to_i32(v: u64) -> i32 {
    if v > (i32::MAX as u64) {
        i32::MAX
    } else {
        v as i32
    }
}

// ---------------------------------------------------------------------------
// Duration ↔ integer seconds (wire: plain u64)
// ---------------------------------------------------------------------------

/// Serializes/deserializes `Duration` as whole seconds on the wire, accepting
/// both plain integers and string forms (e.g. 10, "10", "10s").
pub mod duration_secs {
    use super::*;
    use serde::{Deserializer, Serializer, de};

    pub fn serialize<S>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(value.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DurationVisitor;

        impl<'de> de::Visitor<'de> for DurationVisitor {
            type Value = Duration;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a duration in seconds as an integer or string (e.g. 10, '10s')")
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Duration::from_secs(v))
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v < 0 {
                    return Err(de::Error::custom("duration cannot be negative"));
                }
                Ok(Duration::from_secs(v as u64))
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let trimmed = v.trim();
                let s = trimmed.strip_suffix('s').unwrap_or(trimmed).trim();
                s.parse::<u64>()
                    .map(Duration::from_secs)
                    .map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_any(DurationVisitor)
    }
}

/// Serde helpers for `Option<Duration>` fields: absent stays `None`,
/// present integers or strings become `Duration`. Wire stays integer-or-null.
pub mod option_duration_secs {
    use super::*;
    use serde::{Deserializer, Serializer, de};

    pub fn serialize<S>(value: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(d) => serializer.serialize_some(&d.as_secs()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OptionDurationVisitor;

        impl<'de> de::Visitor<'de> for OptionDurationVisitor {
            type Value = Option<Duration>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "an optional duration in seconds as an integer or string (e.g. 10, '10s')",
                )
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Some(Duration::from_secs(v)))
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v < 0 {
                    return Err(de::Error::custom("duration cannot be negative"));
                }
                Ok(Some(Duration::from_secs(v as u64)))
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let trimmed = v.trim();
                if trimmed.is_empty() {
                    return Ok(None);
                }
                let s = trimmed.strip_suffix('s').unwrap_or(trimmed).trim();
                s.parse::<u64>()
                    .map(|secs| Some(Duration::from_secs(secs)))
                    .map_err(de::Error::custom)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(None)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_any(self)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(None)
            }
        }

        deserializer.deserialize_option(OptionDurationVisitor)
    }
}

// ---------------------------------------------------------------------------
// Byte size ↔ human-readable string (wire: "50MB", "10KB", "25B")
// ---------------------------------------------------------------------------

/// Serializes/deserializes a numeric byte size `usize` as a canonical
/// human-readable string on the wire (e.g. `"50MB"`, `"10KB"`), while accepting
/// both strings and plain integer byte counts during deserialization.
pub mod byte_size {
    use super::*;
    use serde::{Deserializer, Serializer, de};

    pub fn serialize<S>(value: &usize, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&bytes_to_string(*value))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<usize, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ByteSizeVisitor;

        impl<'de> de::Visitor<'de> for ByteSizeVisitor {
            type Value = usize;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a byte size string (e.g. '50MB') or an integer byte count")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                string_to_bytes(v).map_err(de::Error::custom)
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(v as usize)
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v < 0 {
                    return Err(de::Error::custom("byte size cannot be negative"));
                }
                Ok(v as usize)
            }
        }

        deserializer.deserialize_any(ByteSizeVisitor)
    }
}

/// Serde helpers for `Option<usize>` byte size fields: absent stays `None`,
/// present values are serialized as canonical human-readable strings and
/// deserialized from strings (`"50MB"`, `"10KB"`) or raw integers.
pub mod option_byte_size {
    use super::*;
    use serde::{Deserializer, Serializer, de};

    pub fn serialize<S>(value: &Option<usize>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => serializer.serialize_str(&bytes_to_string(*bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OptionByteSizeVisitor;

        impl<'de> de::Visitor<'de> for OptionByteSizeVisitor {
            type Value = Option<usize>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an optional byte size string (e.g. '50MB') or integer byte count")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let trimmed = v.trim();
                if trimmed.is_empty() {
                    return Ok(None);
                }
                string_to_bytes(trimmed)
                    .map(Some)
                    .map_err(de::Error::custom)
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Some(v as usize))
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v < 0 {
                    return Err(de::Error::custom("byte size cannot be negative"));
                }
                Ok(Some(v as usize))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(None)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_any(self)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(None)
            }
        }

        deserializer.deserialize_option(OptionByteSizeVisitor)
    }
}

// ---------------------------------------------------------------------------
// Octal chmod mode ↔ string (wire: "0700", "0755", "700")
// ---------------------------------------------------------------------------

/// Formats a mode as a canonical 4-digit octal string (e.g. "0700", "0755").
pub fn chmod_to_string(mode: u32) -> String {
    format!("{:04o}", mode & crate::consts::CHMOD_MASK)
}

/// Serde helpers for `u32` chmod mode fields: serializes as canonical 4-digit
/// octal string (e.g. `"0700"`), deserializes from octal strings or integers.
pub mod chmod {
    use super::*;
    use serde::{Deserializer, Serializer, de};

    pub fn serialize<S>(value: &u32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&chmod_to_string(*value))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u32, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ChmodVisitor;

        impl<'de> de::Visitor<'de> for ChmodVisitor {
            type Value = u32;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an octal mode string (e.g. '0700') or an integer mode")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                string_to_chmod(v).map_err(de::Error::custom)
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok((v as u32) & crate::consts::CHMOD_MASK)
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v < 0 {
                    return Err(de::Error::custom("chmod mode cannot be negative"));
                }
                Ok((v as u32) & crate::consts::CHMOD_MASK)
            }
        }

        deserializer.deserialize_any(ChmodVisitor)
    }
}

/// Serde helpers for `Option<u32>` chmod mode fields: absent stays `None`,
/// present values serialize as canonical 4-digit octal strings and
/// deserialize from octal strings (`"0700"`, `"0755"`, `"700"`) or integers.
/// Empty/whitespace strings map to `None` ("use platform default").
pub mod option_chmod {
    use super::*;
    use serde::{Deserializer, Serializer, de};

    pub fn serialize<S>(value: &Option<u32>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(mode) => serializer.serialize_str(&chmod_to_string(*mode)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OptionChmodVisitor;

        impl<'de> de::Visitor<'de> for OptionChmodVisitor {
            type Value = Option<u32>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an optional octal mode string (e.g. '0700') or integer mode")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let trimmed = v.trim();
                if trimmed.is_empty() {
                    return Ok(None);
                }
                string_to_chmod(trimmed)
                    .map(Some)
                    .map_err(de::Error::custom)
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Some((v as u32) & crate::consts::CHMOD_MASK))
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v < 0 {
                    return Err(de::Error::custom("chmod mode cannot be negative"));
                }
                Ok(Some((v as u32) & crate::consts::CHMOD_MASK))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(None)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_any(self)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(None)
            }
        }

        deserializer.deserialize_option(OptionChmodVisitor)
    }
}

// ---------------------------------------------------------------------------
// http_bind: normalize at the deserialize boundary
// ---------------------------------------------------------------------------

/// Deserializes an optional HTTP bind address, normalizing `:9001`/`*:9001`/`9001`.
pub fn optional_http_bind<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.map(|s| normalize_http_bind(&s)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct DummyByteSizeConfig {
        #[serde(default, with = "option_byte_size")]
        max_bytes: Option<usize>,
    }

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct DummyChmodConfig {
        #[serde(default, with = "option_chmod")]
        mode: Option<u32>,
    }

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct DummyDurationConfig {
        #[serde(default, with = "option_duration_secs")]
        secs: Option<Duration>,
    }

    #[test]
    fn test_option_byte_size_serde_round_trip() {
        // String deserialization
        let json_input = r#"{"max_bytes":"50MB"}"#;
        let parsed: DummyByteSizeConfig = serde_json::from_str(json_input).unwrap();
        assert_eq!(parsed.max_bytes, Some(50 * 1024 * 1024));

        // Serialization perfectly restores canonical string
        let reserialized = serde_json::to_string(&parsed).unwrap();
        assert_eq!(reserialized, r#"{"max_bytes":"50MB"}"#);

        // Numeric deserialization also works
        let num_input = r#"{"max_bytes":52428800}"#;
        let parsed_num: DummyByteSizeConfig = serde_json::from_str(num_input).unwrap();
        assert_eq!(parsed_num.max_bytes, Some(50 * 1024 * 1024));
        assert_eq!(
            serde_json::to_string(&parsed_num).unwrap(),
            r#"{"max_bytes":"50MB"}"#
        );

        // Small byte sizes
        let small_input = r#"{"max_bytes":"25B"}"#;
        let parsed_small: DummyByteSizeConfig = serde_json::from_str(small_input).unwrap();
        assert_eq!(parsed_small.max_bytes, Some(25));
        assert_eq!(
            serde_json::to_string(&parsed_small).unwrap(),
            r#"{"max_bytes":"25B"}"#
        );

        // None / null handling
        let null_input = r#"{"max_bytes":null}"#;
        let parsed_null: DummyByteSizeConfig = serde_json::from_str(null_input).unwrap();
        assert_eq!(parsed_null.max_bytes, None);
        assert_eq!(
            serde_json::to_string(&parsed_null).unwrap(),
            r#"{"max_bytes":null}"#
        );

        // Absent field
        let empty_input = r#"{}"#;
        let parsed_empty: DummyByteSizeConfig = serde_json::from_str(empty_input).unwrap();
        assert_eq!(parsed_empty.max_bytes, None);
    }

    #[test]
    fn test_option_chmod_serde_round_trip() {
        // Octal string deserialization
        let json_input = r#"{"mode":"0750"}"#;
        let parsed: DummyChmodConfig = serde_json::from_str(json_input).unwrap();
        assert_eq!(parsed.mode, Some(0o750));

        // Serialization perfectly restores canonical 4-digit octal string
        let reserialized = serde_json::to_string(&parsed).unwrap();
        assert_eq!(reserialized, r#"{"mode":"0750"}"#);

        // 3-digit octal string "700" -> serialized to canonical "0700"
        let short_input = r#"{"mode":"700"}"#;
        let parsed_short: DummyChmodConfig = serde_json::from_str(short_input).unwrap();
        assert_eq!(parsed_short.mode, Some(0o700));
        assert_eq!(
            serde_json::to_string(&parsed_short).unwrap(),
            r#"{"mode":"0700"}"#
        );

        // Numeric deserialization (e.g. from YAML 0o755 = 493)
        let num_input = r#"{"mode":488}"#; // 488 == 0o750
        let parsed_num: DummyChmodConfig = serde_json::from_str(num_input).unwrap();
        assert_eq!(parsed_num.mode, Some(0o750));
        assert_eq!(
            serde_json::to_string(&parsed_num).unwrap(),
            r#"{"mode":"0750"}"#
        );

        // Empty string -> None
        let empty_str_input = r#"{"mode":""}"#;
        let parsed_empty_str: DummyChmodConfig = serde_json::from_str(empty_str_input).unwrap();
        assert_eq!(parsed_empty_str.mode, None);

        // null -> None
        let null_input = r#"{"mode":null}"#;
        let parsed_null: DummyChmodConfig = serde_json::from_str(null_input).unwrap();
        assert_eq!(parsed_null.mode, None);
        assert_eq!(
            serde_json::to_string(&parsed_null).unwrap(),
            r#"{"mode":null}"#
        );
    }

    #[test]
    fn test_option_duration_secs_serde_round_trip() {
        // Integer
        let int_input = r#"{"secs":10}"#;
        let parsed: DummyDurationConfig = serde_json::from_str(int_input).unwrap();
        assert_eq!(parsed.secs, Some(Duration::from_secs(10)));
        assert_eq!(serde_json::to_string(&parsed).unwrap(), r#"{"secs":10}"#);

        // String with "s" suffix
        let str_s_input = r#"{"secs":"10s"}"#;
        let parsed_s: DummyDurationConfig = serde_json::from_str(str_s_input).unwrap();
        assert_eq!(parsed_s.secs, Some(Duration::from_secs(10)));

        // String plain
        let str_input = r#"{"secs":"15"}"#;
        let parsed_str: DummyDurationConfig = serde_json::from_str(str_input).unwrap();
        assert_eq!(parsed_str.secs, Some(Duration::from_secs(15)));

        // null / empty
        let null_input = r#"{"secs":null}"#;
        let parsed_null: DummyDurationConfig = serde_json::from_str(null_input).unwrap();
        assert_eq!(parsed_null.secs, None);
    }

    #[test]
    fn test_string_to_bool() {
        assert!(string_to_bool("true").unwrap());
        assert!(string_to_bool("TRUE").unwrap());
        assert!(string_to_bool("yes").unwrap());
        assert!(string_to_bool("1").unwrap());
        assert!(string_to_bool("on").unwrap());

        assert!(!string_to_bool("false").unwrap());
        assert!(!string_to_bool("FALSE").unwrap());
        assert!(!string_to_bool("no").unwrap());
        assert!(!string_to_bool("0").unwrap());
        assert!(!string_to_bool("off").unwrap());

        assert!(string_to_bool("invalid").is_err());
    }

    #[test]
    fn test_string_to_bytes_and_back() {
        assert_eq!(string_to_bytes("1024").unwrap(), 1024);
        assert_eq!(string_to_bytes("1024B").unwrap(), 1024);
        assert_eq!(string_to_bytes("10KB").unwrap(), 10240);
        assert_eq!(string_to_bytes("10K").unwrap(), 10240);
        assert_eq!(string_to_bytes("20MB").unwrap(), 20 * 1024 * 1024);
        assert_eq!(string_to_bytes("20M").unwrap(), 20 * 1024 * 1024);
        assert_eq!(string_to_bytes("1GB").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(
            string_to_bytes("1.5MB").unwrap(),
            (1.5 * 1024.0 * 1024.0) as usize
        );
        assert!(string_to_bytes("").is_err());
        assert!(string_to_bytes("invalid").is_err());

        assert_eq!(bytes_to_string(50 * 1024 * 1024), "50MB");
        assert_eq!(bytes_to_string(10240), "10KB");
        assert_eq!(bytes_to_string(1024), "1KB");
        assert_eq!(bytes_to_string(25), "25B");
        assert_eq!(bytes_to_string(0), "0B");
        assert_eq!(bytes_to_string(1500), "1500B");

        for sz in [0, 25, 1024, 10240, 52428800, 1073741824] {
            assert_eq!(string_to_bytes(&bytes_to_string(sz)).unwrap(), sz);
        }
    }

    #[test]
    fn test_string_to_chmod() {
        assert_eq!(string_to_chmod("0700").unwrap(), 0o700);
        assert_eq!(string_to_chmod("700").unwrap(), 0o700);
        assert_eq!(string_to_chmod("0o700").unwrap(), 0o700);
        assert_eq!(string_to_chmod("0O700").unwrap(), 0o700);
        assert_eq!(string_to_chmod(" 0755 ").unwrap(), 0o755);
        // Mask to 0o7777 (permission + setuid/setgid/sticky)
        assert_eq!(string_to_chmod("7777").unwrap(), 0o7777);
        assert!(string_to_chmod("").is_err());
        assert!(string_to_chmod("xyz").is_err());
        assert!(string_to_chmod("8").is_err());

        assert_eq!(chmod_to_string(0o750), "0750");
        assert_eq!(chmod_to_string(0o700), "0700");
    }

    #[test]
    fn test_string_to_umask() {
        assert_eq!(string_to_umask("022").unwrap(), 0o22);
        assert_eq!(string_to_umask("0o22").unwrap(), 0o22);
        assert_eq!(string_to_umask("18").unwrap(), 18);
    }

    #[test]
    fn test_string_to_i32_list() {
        assert_eq!(string_to_i32_list("0,2").unwrap(), vec![0, 2]);
        assert_eq!(string_to_i32_list("0, 2").unwrap(), vec![0, 2]);
        assert_eq!(string_to_i32_list("0 2 3").unwrap(), vec![0, 2, 3]);
        assert_eq!(string_to_i32_list("0").unwrap(), vec![0]);
        assert!(string_to_i32_list("").unwrap().is_empty());
        assert!(string_to_i32_list("abc").is_err());
    }

    #[test]
    fn test_string_to_str_list() {
        assert_eq!(string_to_str_list("ticker,catx"), vec!["ticker", "catx"]);
        assert_eq!(
            string_to_str_list("ticker  catx \n worker"),
            vec!["ticker", "catx", "worker"]
        );
        assert_eq!(
            string_to_str_list("conf.d/*.ini\nother/*.conf"),
            vec!["conf.d/*.ini", "other/*.conf"]
        );
    }

    #[test]
    fn test_string_to_duration() {
        assert_eq!(string_to_duration("10").unwrap(), Duration::from_secs(10));
        assert!(string_to_duration("").is_err());
        assert!(string_to_duration("abc").is_err());
    }

    #[test]
    fn test_duration_secs_to_string() {
        assert_eq!(duration_secs_to_string(45), "45s");
        assert_eq!(duration_secs_to_string(90), "1m 30s");
        assert_eq!(duration_secs_to_string(3600), "1h 0m");
        assert_eq!(duration_secs_to_string(6300), "1h 45m");
    }

    #[test]
    fn test_normalize_http_bind() {
        assert_eq!(normalize_http_bind(":9001"), "0.0.0.0:9001");
        assert_eq!(normalize_http_bind("*:9001"), "0.0.0.0:9001");
        assert_eq!(normalize_http_bind("9001"), "0.0.0.0:9001");
        assert_eq!(normalize_http_bind("127.0.0.1:9001"), "127.0.0.1:9001");
        assert_eq!(normalize_http_bind("localhost:9001"), "localhost:9001");
    }

    #[test]
    fn test_percent_decode_string() {
        assert_eq!(percent_decode_string("hello"), "hello");
        assert_eq!(percent_decode_string("hello%20world"), "hello world");
        assert_eq!(percent_decode_string("a%2Bb%3Dc"), "a+b=c");
        assert_eq!(percent_decode_string("a+b"), "a b");
        assert_eq!(percent_decode_string("invalid%2"), "invalid%2");
    }

    #[test]
    fn test_u64_to_i32() {
        assert_eq!(u64_to_i32(0), 0);
        assert_eq!(u64_to_i32(12345), 12345);
        assert_eq!(u64_to_i32(i32::MAX as u64), i32::MAX);
        assert_eq!(u64_to_i32(i32::MAX as u64 + 1), i32::MAX);
        assert_eq!(u64_to_i32(u64::MAX), i32::MAX);
    }
}
