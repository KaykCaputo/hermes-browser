use std::cell::RefCell;
use std::rc::Rc;

use servo::LoadStatus;
use talaria_core::TabId;

use crate::page_feedback::PageFeedback;
use crate::page_requests::{DocumentStartTracker, Navigation, arms_document_start};
use crate::page_tabs::DocumentStartedListener;

/// The browser's listener for document starts, shared by every tab of the runtime.
pub(crate) type SharedDocumentStartedListener = Rc<RefCell<Option<DocumentStartedListener>>>;

/// One "document started" signal per tab, fired when Servo reports a start or when the
/// dialogs lane's `DocumentStartTracker` infers one that Servo swallowed (a reload, or a
/// navigation while the status was still `Started`). It resets the tab's title and icon
/// bookkeeping (`PageFeedback::document_started`) and tells the browser's listener, which
/// forgets the tab's user activation.
///
/// The listener is also told when Mercury itself sends a load, reload or traversal, at the
/// moment it does: the tracker can only infer a swallowed start at `HeadParsed`, after the head's
/// scripts ran.
pub(crate) struct DocumentStarts {
    tab_id: TabId,
    tracker: RefCell<DocumentStartTracker>,
    page_feedback: Rc<PageFeedback>,
    listener: SharedDocumentStartedListener,
}

impl DocumentStarts {
    pub(crate) fn new(
        tab_id: TabId,
        page_feedback: Rc<PageFeedback>,
        listener: SharedDocumentStartedListener,
    ) -> Rc<Self> {
        Rc::new(Self {
            tab_id,
            tracker: RefCell::new(DocumentStartTracker::default()),
            page_feedback,
            listener,
        })
    }

    /// Mercury is about to make the web view navigate; `status` is its load status now.
    pub(crate) fn navigation_sent(&self, navigation: Navigation, status: LoadStatus) {
        if arms_document_start(navigation, status) {
            self.tracker.borrow_mut().arm();
        }
        self.notify_listener();
    }

    /// The navigation will not happen (the user chose to stay on the page).
    pub(crate) fn navigation_cancelled(&self) {
        self.tracker.borrow_mut().disarm();
    }

    /// Servo reported a load status. Returns whether a new document started, reported or
    /// inferred.
    pub(crate) fn load_status_changed(&self, status: LoadStatus) -> bool {
        if !self.tracker.borrow_mut().document_started(status) {
            return false;
        }
        self.page_feedback.document_started();
        self.notify_listener();
        true
    }

    fn notify_listener(&self) {
        let listener = self.listener.borrow().clone();
        if let Some(listener) = listener {
            listener(self.tab_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver};

    use servo::{JSValue, JavaScriptEvaluationError};
    use talaria_core::{BrowserEvent, TabEvent};

    use super::*;
    use crate::page_feedback::TitleCallback;

    const TAB: TabId = TabId::new(5);

    struct Fixture {
        starts: Rc<DocumentStarts>,
        feedback: Rc<PageFeedback>,
        events: Receiver<TabEvent>,
        forgotten: Rc<RefCell<Vec<TabId>>>,
        evaluations: Rc<RefCell<Vec<TitleCallback>>>,
    }

    impl Fixture {
        fn new() -> Self {
            let (sender, events) = mpsc::channel();
            let feedback = PageFeedback::new(TAB, sender);
            let forgotten = Rc::new(RefCell::new(Vec::new()));
            let record = Rc::clone(&forgotten);
            let listener: DocumentStartedListener =
                Rc::new(move |tab_id| record.borrow_mut().push(tab_id));
            let starts = DocumentStarts::new(
                TAB,
                Rc::clone(&feedback),
                Rc::new(RefCell::new(Some(listener))),
            );
            Self {
                starts,
                feedback,
                events,
                forgotten,
                evaluations: Rc::default(),
            }
        }

        fn request_title(&self) {
            let evaluations = Rc::clone(&self.evaluations);
            self.feedback
                .request_title(move |callback| evaluations.borrow_mut().push(callback));
        }

        fn titles(&self) -> Vec<BrowserEvent> {
            self.events
                .try_iter()
                .map(TabEvent::into_event)
                .filter(|event| matches!(event, BrowserEvent::PageTitleChanged { .. }))
                .collect()
        }
    }

    fn answer(callback: TitleCallback, title: &str) {
        let result: Result<JSValue, JavaScriptEvaluationError> =
            Ok(JSValue::String(title.to_owned()));
        callback(result);
    }

    #[test]
    fn a_reload_forgets_the_activation_and_the_stuck_title_read() {
        let fixture = Fixture::new();
        // The page is loaded; its title read hangs (a stuck script thread).
        fixture.request_title();

        // The user reloads: the activation goes at once.
        fixture
            .starts
            .navigation_sent(Navigation::Reload, LoadStatus::Complete);
        assert_eq!(*fixture.forgotten.borrow(), vec![TAB]);

        // Servo swallows the reload's `Started`; the tracker infers the start at `HeadParsed`.
        assert!(fixture.starts.load_status_changed(LoadStatus::HeadParsed));
        assert_eq!(*fixture.forgotten.borrow(), vec![TAB, TAB]);

        // The new document's title is read at once, not after the stuck read.
        fixture.request_title();
        assert_eq!(fixture.evaluations.borrow().len(), 2);
        let new_read = fixture.evaluations.borrow_mut().remove(1);
        answer(new_read, "Reloaded");
        assert_eq!(
            fixture.titles(),
            vec![BrowserEvent::PageTitleChanged {
                title: Some("Reloaded".to_owned())
            }]
        );
    }

    #[test]
    fn a_reported_start_is_one_signal_and_later_statuses_are_not() {
        let fixture = Fixture::new();

        assert!(fixture.starts.load_status_changed(LoadStatus::Started));
        assert!(!fixture.starts.load_status_changed(LoadStatus::HeadParsed));
        assert!(!fixture.starts.load_status_changed(LoadStatus::Complete));

        assert_eq!(*fixture.forgotten.borrow(), vec![TAB]);
    }

    #[test]
    fn a_page_inserting_a_body_does_not_count_as_a_document_start() {
        let fixture = Fixture::new();
        let _ = fixture.starts.load_status_changed(LoadStatus::Started);
        let _ = fixture.starts.load_status_changed(LoadStatus::Complete);
        fixture.forgotten.borrow_mut().clear();

        // Servo sends `HeadParsed` when a page inserts a `<body>` into its document.
        assert!(!fixture.starts.load_status_changed(LoadStatus::HeadParsed));
        assert!(fixture.forgotten.borrow().is_empty());
    }

    #[test]
    fn a_cancelled_navigation_infers_no_start() {
        let fixture = Fixture::new();
        fixture
            .starts
            .navigation_sent(Navigation::Reload, LoadStatus::Complete);
        fixture.starts.navigation_cancelled();

        assert!(!fixture.starts.load_status_changed(LoadStatus::HeadParsed));
    }
}
