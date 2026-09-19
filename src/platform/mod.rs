pub mod traits;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

pub use traits::{PlatformBackend, PlatformProcessGuard};

/// Returns the native platform backend singleton.
pub fn native_platform() -> &'static dyn PlatformBackend {
    #[cfg(unix)]
    {
        &unix::UnixPlatformBackend
    }
    #[cfg(windows)]
    {
        &windows::WindowsPlatformBackend
    }
}
