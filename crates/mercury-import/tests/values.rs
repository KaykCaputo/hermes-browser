use mercury_import::{BrowserProfile, ImportError, ImportedNode, ImportedVisit};

fn movable_between_threads<T: Send + Sync + 'static>() {}

#[test]
fn results_and_errors_can_be_sent_to_another_thread() {
    movable_between_threads::<ImportError>();
    movable_between_threads::<ImportedNode>();
    movable_between_threads::<ImportedVisit>();
    movable_between_threads::<BrowserProfile>();
}

#[test]
fn errors_describe_themselves() {
    let error = ImportError::Malformed("no roots".into());

    assert_eq!(error.to_string(), "unexpected format: no roots");
    let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(error);
    assert!(boxed.source().is_none());
}
