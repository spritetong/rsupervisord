pub mod config;
pub mod error;
pub mod manager;
pub mod platform;
pub mod program;

pub use config::{ConfigDiff, SupervisorConfig};
pub use error::ProgramError;
pub use manager::{DependencyGraph, ManagerHandle, ReloadSummary, SupervisorManager};
pub use platform::{PlatformBackend, PlatformProcessGuard, native_platform};
pub use program::{
    AutoRestartPolicy, ProcessProgram, Program, ProgramConfig, ProgramState, ProgramStatus,
    StopSignal,
};
