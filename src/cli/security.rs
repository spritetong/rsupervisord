// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use anyhow::Result;

/// Validates caller privileges before sending commands to the supervisor daemon.
pub fn validate_caller_privileges(allow_unelevated: bool) -> Result<()> {
    crate::platform::native_platform()
        .validate_caller_privileges(allow_unelevated)
        .map_err(|e| anyhow::anyhow!(e))
}
