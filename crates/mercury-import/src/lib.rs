//! Reads bookmarks and history that other browsers keep, and reads and writes the Netscape
//! bookmark file every browser can exchange.
//!
//! Everything here is a pure reader or writer that returns neutral values ([`ImportedNode`],
//! [`ImportedVisit`]); nothing writes to Mercury's own storage.

#![forbid(unsafe_code)]

mod model;
mod netscape;

pub use model::{BrowserProfile, ImportError, ImportedNode, ImportedVisit, MAX_IMPORTED_VISITS};
pub use netscape::{parse_netscape_bookmarks, write_netscape_bookmarks};
