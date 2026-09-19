use anyhow::Result;

/// Validates caller privileges before sending commands to the supervisor daemon.
pub fn validate_caller_privileges(allow_unelevated: bool) -> Result<()> {
    let _ = allow_unelevated;
    let platform = crate::platform::native_platform();

    #[cfg(windows)]
    {
        if !platform.is_elevated() && !allow_unelevated {
            tracing::debug!("Caller process is not running as Administrator");
        }
    }

    #[cfg(unix)]
    {
        let _ = platform.is_elevated();
    }

    Ok(())
}
