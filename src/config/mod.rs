// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

pub mod diff;
pub mod expand;
pub mod paths;
pub mod schema;
pub mod transform;

pub use diff::ConfigDiff;
pub use expand::{MacroExpander, StringExpression, expand_env_vars};
pub use paths::PathResolver;
pub use schema::{
    CtlConfig, LoggingConfig, MetricsConfig, ProgramConfigRaw, ProgramDefaults, ServerConfig,
    SupervisorConfig,
};
