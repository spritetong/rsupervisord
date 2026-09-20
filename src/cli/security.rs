// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use anyhow::Result;

/// Validates caller privileges before sending commands to the supervisor daemon.
pub fn validate_caller_privileges(allow_unelevated: bool) -> Result<()> {
    crate::platform::native_platform()
        .validate_caller_privileges(allow_unelevated)
        .map_err(|e| anyhow::anyhow!(e))
}
