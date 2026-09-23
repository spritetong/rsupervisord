// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

//! Boundary serde helpers: wire-compatible conversions so config/JSON value
//! types stay unchanged while runtime structs hold parsed forms.

use serde::{Deserialize, Deserializer};
use std::fmt;
use std::time::Duration;

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
        serializer.serialize_str(&crate::logging::format_byte_size(*value))
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
                crate::logging::parse_byte_size(v).map_err(de::Error::custom)
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
            Some(bytes) => serializer.serialize_str(&crate::logging::format_byte_size(*bytes)),
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
                crate::logging::parse_byte_size(trimmed)
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
pub fn format_chmod(mode: u32) -> String {
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
        serializer.serialize_str(&format_chmod(*value))
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
                crate::config::schema::parse_chmod(v).map_err(de::Error::custom)
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
            Some(mode) => serializer.serialize_str(&format_chmod(*mode)),
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
                crate::config::schema::parse_chmod(trimmed)
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
    Ok(value.map(|s| crate::config::schema::normalize_http_bind(&s)))
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
}
