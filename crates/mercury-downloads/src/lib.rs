//! The download engine of Mercury Browser: naming, sanitising, streaming to disk, progress,
//! cancel and resume.
//!
//! Servo has no download support, so the browser fetches files itself. Every name, path and URL
//! handled here comes from a web server, which makes this crate security-sensitive: a downloaded
//! file must never land outside the folder chosen for it, never replace an existing file, and
//! never carry a name that lies about what it is.

mod content_disposition;
mod error;
mod manager;
pub mod naming;
mod transfer;

pub use manager::{
    DownloadEvent, DownloadId, DownloadManager, DownloadRequest, ResumeFrom, Timeouts,
};
pub use naming::{file_name_for, format_bytes, sanitize_file_name, unique_path};
