//! The events of `mercury-downloads`, waiting for the UI thread, and stopping every transfer
//! when the browser quits.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use mercury_downloads::{DownloadEvent, DownloadManager};

use super::model::{DownloadList, RecordOp};

/// Events delivered on the manager's threads, applied on the UI thread.
#[derive(Default)]
pub(crate) struct Inbox {
    events: Mutex<Vec<DownloadEvent>>,
    wake_pending: AtomicBool,
}

impl Inbox {
    /// Queues an event; true if the UI thread must be woken (no wake-up is outstanding).
    pub(crate) fn push(&self, event: DownloadEvent) -> bool {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
        !self.wake_pending.swap(true, Ordering::AcqRel)
    }

    pub(crate) fn take(&self) -> Vec<DownloadEvent> {
        self.wake_pending.store(false, Ordering::Release);
        std::mem::take(&mut *self.events.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Applies the queued events to the list; returns the database writes they need.
pub(crate) fn apply_events(list: &RefCell<DownloadList>, inbox: &Inbox) -> Vec<RecordOp> {
    let events = inbox.take();
    let now = Instant::now();
    let mut list = list.borrow_mut();
    events
        .into_iter()
        .flat_map(|event| list.apply(event, now))
        .collect()
}

/// Interrupts every running transfer (keeping what can be resumed), waits for them to end,
/// then shuts the manager down, all by `deadline`. Returns the database writes for how each
/// ended.
pub(crate) fn finish_transfers(
    manager: DownloadManager,
    list: &RefCell<DownloadList>,
    inbox: &Inbox,
    deadline: Instant,
) -> Vec<RecordOp> {
    for id in list.borrow().running_transfers() {
        manager.interrupt(id);
    }
    let mut ops = Vec::new();
    loop {
        ops.extend(apply_events(list, inbox));
        if list.borrow().active_count() == 0 || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let left = list.borrow().active_count();
    if left > 0 {
        log::warn!("{left} download(s) did not stop in time; their partial files are removed");
    }
    // Cancels whatever is left, waiting at most until the deadline.
    manager.shut_down(deadline);
    ops.extend(apply_events(list, inbox));
    ops
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    use mercury_downloads::{DownloadId, DownloadRequest};
    use mercury_store::DownloadState;

    use super::*;
    use crate::downloads::model::{ItemState, NewItem};

    /// Serves one response: headers for 100,000 bytes with a validator, 1,000 bytes, then
    /// nothing more until the connection is closed.
    fn stalling_server() -> (std::thread::JoinHandle<()>, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nETag: \"v1\"\r\n\
                  Accept-Ranges: bytes\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.write_all(&[7_u8; 1000]);
            let _ = stream.flush();
            // Hold the connection until the client goes away.
            let _ = stream.read(&mut request);
        });
        (thread, port)
    }

    #[test]
    fn quitting_interrupts_and_records_resumable_downloads_by_the_deadline() {
        let (server, port) = stalling_server();
        let folder = tempfile::tempdir().unwrap();
        let inbox = Arc::new(Inbox::default());
        let sink = Arc::clone(&inbox);
        let manager = DownloadManager::new(move |event| {
            sink.push(event);
        })
        .unwrap();
        let url = url::Url::parse(&format!("http://127.0.0.1:{port}/big.bin")).unwrap();
        let id = manager.start(DownloadRequest {
            url: url.clone(),
            referrer: None,
            user_agent: String::new(),
            cookie_header: None,
            destination_dir: folder.path().to_path_buf(),
            suggested_name: None,
            resume: None,
        });
        let list = RefCell::new(DownloadList::default());
        list.borrow_mut().begin(NewItem {
            guid: "g".into(),
            url: url.to_string(),
            referrer: None,
            name_guess: "big.bin".into(),
            transfer: id,
            started_at_ms: 1,
            suggested_name: None,
            refetched: false,
        });
        // Wait until the server has answered and some bytes arrived.
        let waited = Instant::now();
        while list.borrow().item("g").unwrap().state != ItemState::InProgress {
            apply_events(&list, &inbox);
            assert!(waited.elapsed() < Duration::from_secs(20), "never started");
            std::thread::sleep(Duration::from_millis(10));
        }

        let asked = Instant::now();
        let ops = finish_transfers(manager, &list, &inbox, asked + Duration::from_secs(5));
        assert!(
            asked.elapsed() < Duration::from_secs(5),
            "{:?}",
            asked.elapsed()
        );
        assert_eq!(
            list.borrow().item("g").unwrap().state,
            ItemState::Interrupted { resumable: true }
        );
        assert!(ops.iter().any(|op| matches!(
            op,
            RecordOp::Finish {
                state: DownloadState::Interrupted,
                ..
            }
        )));
        assert!(folder.path().join("big.bin.part").exists());
        let _ = DownloadId(0);
        server.join().unwrap();
    }

    #[test]
    fn quitting_with_nothing_running_returns_at_once() {
        let inbox = Arc::new(Inbox::default());
        let sink = Arc::clone(&inbox);
        let manager = DownloadManager::new(move |event| {
            sink.push(event);
        })
        .unwrap();
        let list = RefCell::new(DownloadList::default());
        let asked = Instant::now();
        // A deadline already past: nothing to wait for, and nothing waits.
        let ops = finish_transfers(manager, &list, &inbox, asked);
        assert!(ops.is_empty());
        assert!(
            asked.elapsed() < Duration::from_secs(2),
            "{:?}",
            asked.elapsed()
        );
    }
}
