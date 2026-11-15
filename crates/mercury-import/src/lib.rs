//! Reads bookmarks and history that other browsers keep, and reads and writes the Netscape
//! bookmark file every browser can exchange.
//!
//! Everything here is a pure reader or writer that returns neutral values ([`ImportedNode`],
//! [`ImportedVisit`]); nothing writes to Mercury's own storage. Databases that another browser
//! may hold open are copied first and the copy is read, so a running browser's files are never
//! locked or changed.
//!
//! Profile discovery takes the directories to look in from the caller: the platform's
//! application-support directory on macOS (`~/Library/Application Support`), the config
//! directory and the home directory on Linux (`~/.config`, `~`), and `%LOCALAPPDATA%` and
//! `%APPDATA%` on Windows. Tests pass a directory of their own.

#![forbid(unsafe_code)]

mod chromium;
mod firefox;
mod model;
mod netscape;
mod snapshot;

pub use chromium::{find_chromium_profiles, read_chromium_bookmarks, read_chromium_history};
pub use firefox::{find_firefox_profiles, read_firefox_bookmarks, read_firefox_history};
pub use model::{BrowserProfile, ImportError, ImportedNode, ImportedVisit, MAX_IMPORTED_VISITS};
pub use netscape::{parse_netscape_bookmarks, write_netscape_bookmarks};
