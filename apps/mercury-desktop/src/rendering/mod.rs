#[cfg(not(target_os = "linux"))]
mod software;
#[cfg(target_os = "linux")]
mod surfman;
#[cfg(target_os = "linux")]
mod vulkan;

#[cfg(target_os = "linux")]
pub use self::surfman::BrowserRenderingContext;
#[cfg(not(target_os = "linux"))]
pub use software::BrowserRenderingContext;
