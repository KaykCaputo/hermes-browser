mod address;
mod browser;
mod controller;
mod page_icon;
mod web_view;

pub use address::{AddressResolutionError, NavigationTarget, classify_address, resolve_address};
pub use browser::{
    BrowserCommand, BrowserEvent, BrowserState, LoadState, PointerButton, PointerButtonAction,
};
pub use controller::BrowserController;
pub use page_icon::PageIcon;
pub use web_view::{WebView, dispatch_command};
