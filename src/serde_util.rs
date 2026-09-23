// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

//! Boundary serde helpers: wire-compatible conversions so config/JSON value
//! types stay unchanged while runtime structs hold parsed forms.

use crate::error::ProgramError;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Duration ↔ integer seconds (wire: plain u64)
// ---------------------------------------------------------------------------

/// Serializes/deserializes `Duration` as whole seconds on the wire.
pub mod duration_secs {
    use super::*;
    use serde::{Deserialize, Serializer};

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
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

/// Serde helpers for `Option<Duration>` fields: absent stays `None`,
/// present integers become `Duration`. Wire stays integer-or-null.
pub mod option_duration_secs {
    use super::*;
    use serde::{Deserialize, Serializer};

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
        let secs = Option::<u64>::deserialize(deserializer)?;
        Ok(secs.map(Duration::from_secs))
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
// ChmodMode: octal string on the wire, u32 bits at runtime
// ---------------------------------------------------------------------------

/// File mode parsed from an octal string (`"0700"`, `"0o700"`, `"700"`).
/// Serializes as a zero-padded 4-digit octal string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChmodMode(u32);

impl ChmodMode {
    pub fn new(mode: u32) -> Self {
        Self(mode & crate::consts::CHMOD_MASK)
    }

    /// Parses via the shared octal-mode rules (accepts `0700`/`0o700`/`700`).
    pub fn parse(s: &str) -> Result<Self, ProgramError> {
        crate::config::schema::parse_chmod(s).map(Self)
    }

    #[inline]
    pub fn mode(self) -> u32 {
        self.0
    }
}

impl From<u32> for ChmodMode {
    fn from(mode: u32) -> Self {
        Self::new(mode)
    }
}

impl fmt::Display for ChmodMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04o}", self.0)
    }
}

impl Serialize for ChmodMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{:04o}", self.0))
    }
}

impl<'de> Deserialize<'de> for ChmodMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        // Empty string means "use platform default" and is stored as None by
        // the Option wrapper; reject here so bad tokens fail at the boundary.
        ChmodMode::parse(&s).map_err(de::Error::custom)
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

/// Deserializes `Option<ChmodMode>`; empty/whitespace strings become `None`
/// (meaning "use platform default"), matching pre-migration semantics.
pub fn optional_chmod<'de, D>(deserializer: D) -> Result<Option<ChmodMode>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Ok(None),
        Some(s) => ChmodMode::parse(&s).map(Some).map_err(de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct DummyConfig {
        #[serde(default, with = "option_byte_size")]
        max_bytes: Option<usize>,
    }

    #[test]
    fn test_option_byte_size_serde_round_trip() {
        // String deserialization
        let json_input = r#"{"max_bytes":"50MB"}"#;
        let parsed: DummyConfig = serde_json::from_str(json_input).unwrap();
        assert_eq!(parsed.max_bytes, Some(50 * 1024 * 1024));

        // Serialization perfectly restores canonical string
        let reserialized = serde_json::to_string(&parsed).unwrap();
        assert_eq!(reserialized, r#"{"max_bytes":"50MB"}"#);

        // Numeric deserialization also works
        let num_input = r#"{"max_bytes":52428800}"#;
        let parsed_num: DummyConfig = serde_json::from_str(num_input).unwrap();
        assert_eq!(parsed_num.max_bytes, Some(50 * 1024 * 1024));
        assert_eq!(
            serde_json::to_string(&parsed_num).unwrap(),
            r#"{"max_bytes":"50MB"}"#
        );

        // Small byte sizes
        let small_input = r#"{"max_bytes":"25B"}"#;
        let parsed_small: DummyConfig = serde_json::from_str(small_input).unwrap();
        assert_eq!(parsed_small.max_bytes, Some(25));
        assert_eq!(
            serde_json::to_string(&parsed_small).unwrap(),
            r#"{"max_bytes":"25B"}"#
        );

        // None / null handling
        let null_input = r#"{"max_bytes":null}"#;
        let parsed_null: DummyConfig = serde_json::from_str(null_input).unwrap();
        assert_eq!(parsed_null.max_bytes, None);
        assert_eq!(
            serde_json::to_string(&parsed_null).unwrap(),
            r#"{"max_bytes":null}"#
        );

        // Absent field
        let empty_input = r#"{}"#;
        let parsed_empty: DummyConfig = serde_json::from_str(empty_input).unwrap();
        assert_eq!(parsed_empty.max_bytes, None);
    }
}
