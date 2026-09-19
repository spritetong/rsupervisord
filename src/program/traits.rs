use crate::error::ProgramError;
use crate::program::state::ProgramStatus;
use async_trait::async_trait;
use std::time::Duration;

/// Trait defining the lifecycle management of a supervised program.
#[async_trait]
pub trait Program: Send + Sync {
    /// Unique program name.
    fn name(&self) -> &str;

    /// Priority within range [0, 99]; lower numbers indicate higher startup priority.
    fn priority(&self) -> u8;

    /// List of program names this program strongly depends on.
    fn dependencies(&self) -> &[String];

    /// Retrieves current state snapshot instantly without channel or lock contention.
    fn status(&self) -> ProgramStatus;

    /// Asynchronously triggers program startup.
    async fn start(&mut self) -> Result<(), ProgramError>;

    /// Asynchronously triggers graceful shutdown within grace_period.
    async fn stop(&mut self, grace_period: Duration) -> Result<(), ProgramError>;

    /// Asynchronously restarts the program.
    async fn restart(&mut self, grace_period: Duration) -> Result<(), ProgramError>;

    /// Cooperatively shuts down and waits for the internal actor task to terminate cleanly.
    async fn shutdown(&mut self) -> Result<(), ProgramError>;
}
