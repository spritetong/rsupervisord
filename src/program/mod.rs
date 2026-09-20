// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

pub mod config;
pub mod process;
pub mod state;
pub mod traits;

pub use config::{AutoRestartPolicy, ProgramConfig, StopSignal};
pub use process::ProcessProgram;
pub use state::{ProgramState, ProgramStatus};
pub use traits::Program;
