use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use talaria_core::{TabEvent, TabId};

use crate::ServoWebView;
use crate::adblocker::ContentBlocker;
use crate::document_starts::SharedDocumentStartedListener;
use crate::embedder_controls::ControlIds;
use crate::screen::ScreenMetrics;
use crate::session_data::AfterEngineCleanup;

/// Called synchronously when a page asks for a new web view. Returns the id the new tab
/// should use, or None to refuse.
pub type NewTabAllocator = Rc<dyn Fn(TabId) -> Option<TabId>>;

/// Called synchronously inside Servo's event loop when a tab starts loading a new document.
pub type DocumentStartedListener = Rc<dyn Fn(TabId)>;

/// A web view a page opened, with the channel its events arrive on.
pub type AdoptedWebView = (TabId, ServoWebView, Receiver<TabEvent>);

/// What a web view opened by a page needs: the allocator that gives it a tab, the queue it
/// waits in until the browser takes it, and the parts every web view's delegate shares.
///
/// Delegates reach this through a `Weak`, so the queued web views (whose delegates point back
/// here) never keep it alive. Field order is drop order: the queued web views first, then the
/// user content manager (a handle to Servo), then the clean-up that must outlive both.
pub(crate) struct PageTabs {
    pub(crate) adopted: RefCell<Vec<AdoptedWebView>>,
    pub(crate) allocator: RefCell<Option<NewTabAllocator>>,
    /// Shared with every tab's `DocumentStarts`.
    pub(crate) document_started_listener: SharedDocumentStartedListener,
    pub(crate) control_ids: ControlIds,
    pub(crate) content_blocker: Rc<ContentBlocker>,
    pub(crate) screen_metrics: Rc<Cell<Option<ScreenMetrics>>>,
    pub(crate) user_content: Rc<servo::UserContentManager>,
    pub(crate) after_engine_cleanup: Option<Rc<AfterEngineCleanup>>,
}

impl PageTabs {
    /// The allocator, cloned so that it is not borrowed while it runs.
    pub(crate) fn allocator(&self) -> Option<NewTabAllocator> {
        self.allocator.borrow().clone()
    }
}
