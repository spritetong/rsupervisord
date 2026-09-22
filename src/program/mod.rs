// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

pub mod config;
pub mod process;
pub mod state;
pub mod traits;

pub use config::{AutoRestartPolicy, ProgramConfig, StopSignal};
pub use process::ProcessProgram;
pub use state::{ProgramState, ProgramStatus};
pub use traits::Program;
