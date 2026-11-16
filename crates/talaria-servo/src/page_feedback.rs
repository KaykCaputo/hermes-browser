use std::cell::{Cell, RefCell};

use servo::{JSValue, JavaScriptEvaluationError, LoadStatus};
use talaria_core::{BrowserEvent, PageIcon};

/// The script that reads the current document's title.
pub(crate) const DOCUMENT_TITLE_SCRIPT: &str = "document.title";

/// Keeps the tab's title and icon right where Servo's notifications are not enough.
///
/// Servo notifies a title only when its cached title changes, and never resets that cache
/// between documents, so a new document with the same title, or with none, sends nothing.
/// Its cached favicon is never cleared either. The title is therefore read from the document
/// (`document.title`) when a document's head has been parsed, when it has loaded, and after a
/// same-document navigation; the icon is remembered per document here.
#[derive(Debug, Default)]
pub(crate) struct PageFeedback {
    /// Changes whenever the document or its address changes, so that an answer about an
    /// earlier one is dropped.
    generation: Cell<u64>,
    icon: RefCell<Option<PageIcon>>,
}

/// Identifies the document state a title request was made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TitleTicket(u64);

impl PageFeedback {
    /// A new document is loading: it has no icon yet.
    pub(crate) fn document_started(&self) {
        self.bump();
        self.icon.replace(None);
    }

    pub(crate) fn icon_changed(&self, icon: Option<PageIcon>) {
        self.icon.replace(icon);
    }

    /// The address changed. Returns the events to send after `AddressChanged`: for a
    /// same-document navigation, the current icon again (the address change cleared it).
    pub(crate) fn address_changed(&self, status: LoadStatus) -> Vec<BrowserEvent> {
        self.bump();
        if is_same_document_navigation(status) {
            vec![BrowserEvent::PageIconChanged {
                icon: self.icon.borrow().clone(),
            }]
        } else {
            Vec::new()
        }
    }

    pub(crate) fn title_ticket(&self) -> TitleTicket {
        TitleTicket(self.generation.get())
    }

    /// The event for a title answer, or None if the document or address changed since the
    /// request, or the script failed.
    pub(crate) fn title_answer(
        &self,
        ticket: TitleTicket,
        result: Result<JSValue, JavaScriptEvaluationError>,
    ) -> Option<BrowserEvent> {
        if ticket != self.title_ticket() {
            return None;
        }
        match result {
            Ok(JSValue::String(title)) => {
                Some(BrowserEvent::PageTitleChanged { title: Some(title) })
            }
            _ => None,
        }
    }

    fn bump(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
    }
}

/// An address change while no load is in progress is a same-document navigation (fragment,
/// `pushState`, `replaceState`); during a load it is the new document committing.
pub(crate) fn is_same_document_navigation(status: LoadStatus) -> bool {
    status == LoadStatus::Complete
}

/// Whether a load status means the document's title can now be read.
pub(crate) fn reads_title_at(status: LoadStatus) -> bool {
    matches!(status, LoadStatus::HeadParsed | LoadStatus::Complete)
}

/// Whether the address change should be followed by reading the title.
pub(crate) fn reads_title_after_address_change(status: LoadStatus) -> bool {
    is_same_document_navigation(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon() -> PageIcon {
        PageIcon::from_rgba(1, 1, vec![1, 2, 3, 255]).expect("the test icon is valid")
    }

    #[test]
    fn a_title_answer_for_the_current_document_becomes_a_title_event() {
        let feedback = PageFeedback::default();
        let ticket = feedback.title_ticket();

        assert_eq!(
            feedback.title_answer(ticket, Ok(JSValue::String("Same title".to_owned()))),
            Some(BrowserEvent::PageTitleChanged {
                title: Some("Same title".to_owned())
            })
        );
    }

    #[test]
    fn an_empty_document_title_is_sent_so_the_placeholder_shows() {
        let feedback = PageFeedback::default();
        let ticket = feedback.title_ticket();

        assert_eq!(
            feedback.title_answer(ticket, Ok(JSValue::String(String::new()))),
            Some(BrowserEvent::PageTitleChanged {
                title: Some(String::new())
            })
        );
    }

    #[test]
    fn a_title_answer_about_an_earlier_document_or_address_is_dropped() {
        let feedback = PageFeedback::default();
        let ticket = feedback.title_ticket();
        feedback.document_started();
        assert_eq!(
            feedback.title_answer(ticket, Ok(JSValue::String("Old".to_owned()))),
            None
        );

        let ticket = feedback.title_ticket();
        let _events = feedback.address_changed(LoadStatus::Complete);
        assert_eq!(
            feedback.title_answer(ticket, Ok(JSValue::String("Old".to_owned()))),
            None
        );
    }

    #[test]
    fn a_failed_title_script_sends_nothing() {
        let feedback = PageFeedback::default();
        let ticket = feedback.title_ticket();

        assert_eq!(
            feedback.title_answer(ticket, Err(JavaScriptEvaluationError::CompilationFailure)),
            None
        );
        assert_eq!(feedback.title_answer(ticket, Ok(JSValue::Null)), None);
    }

    #[test]
    fn a_same_document_navigation_resends_the_documents_icon() {
        let feedback = PageFeedback::default();
        feedback.document_started();
        feedback.icon_changed(Some(icon()));

        assert_eq!(
            feedback.address_changed(LoadStatus::Complete),
            vec![BrowserEvent::PageIconChanged { icon: Some(icon()) }]
        );
    }

    #[test]
    fn a_new_document_forgets_the_previous_documents_icon() {
        let feedback = PageFeedback::default();
        feedback.icon_changed(Some(icon()));
        feedback.document_started();

        assert_eq!(feedback.address_changed(LoadStatus::Started), Vec::new());
        assert_eq!(
            feedback.address_changed(LoadStatus::Complete),
            vec![BrowserEvent::PageIconChanged { icon: None }]
        );
    }

    #[test]
    fn titles_are_read_when_the_head_is_parsed_on_load_and_after_same_document_navigation() {
        assert!(!reads_title_at(LoadStatus::Started));
        assert!(reads_title_at(LoadStatus::HeadParsed));
        assert!(reads_title_at(LoadStatus::Complete));
        assert!(!reads_title_at(LoadStatus::Stopped));
        assert!(reads_title_after_address_change(LoadStatus::Complete));
        assert!(!reads_title_after_address_change(LoadStatus::HeadParsed));
    }
}
