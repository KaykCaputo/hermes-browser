//! Mercury Browser's settings and saved session.
//!
//! A library only: typed models with safe defaults, validation of whatever is on disk, private
//! files replaced atomically, search engines and their keywords, and the snapshot that restores
//! tabs after a restart or a crash. It never reads the clock, never generates random values and
//! starts no threads; methods that need the time take `now_ms` (milliseconds since the Unix
//! epoch) from the caller.

#![deny(missing_docs)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod atomic;

pub use atomic::write_private_atomic;
