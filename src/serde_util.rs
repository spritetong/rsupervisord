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
// ByteSize: human string on the wire, parsed bytes at runtime
// ---------------------------------------------------------------------------

/// Log-size threshold parsed from a human string (`"50MB"`) and round-tripped
/// back to the original spelling so config JSON stays a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteSize {
    bytes: usize,
    raw: String,
}

impl ByteSize {
    pub fn new(bytes: usize, raw: impl Into<String>) -> Self {
        Self {
            bytes,
            raw: raw.into(),
        }
    }

    /// Parses a human-readable size (`"50MB"`, `"10KB"`, `"1024"`).
    pub fn parse(s: &str) -> Result<Self, ProgramError> {
        let bytes = crate::logging::parse_byte_size(s)?;
        Ok(Self::new(bytes, s.trim().to_string()))
    }

    #[inline]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    #[inline]
    pub fn raw(&self) -> &str {
        &self.raw
    }
}

impl From<usize> for ByteSize {
    fn from(bytes: usize) -> Self {
        Self {
            bytes,
            raw: bytes.to_string(),
        }
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl Serialize for ByteSize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.raw)
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        ByteSize::parse(&s).map_err(de::Error::custom)
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
        Some(s) => ChmodMode::parse(&s)
            .map(Some)
            .map_err(de::Error::custom),
    }
}
