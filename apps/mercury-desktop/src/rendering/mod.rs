#[cfg(target_os = "macos")]
mod metal;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod software;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod surfman;
#[cfg(target_os = "linux")]
mod vulkan;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use self::surfman::BrowserRenderingContext;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub use software::BrowserRenderingContext;
