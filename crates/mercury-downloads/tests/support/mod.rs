//! A small HTTP/1.1 server on `127.0.0.1:0`, a temporary download folder, and helpers for
//! collecting a download's events. Nothing here touches the network beyond loopback.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mercury_downloads::{
    DownloadEvent, DownloadId, DownloadManager, DownloadRequest, ResumeFrom, Timeouts,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use url::Url;

/// The body served by `/file.bin` and friends: 64 KiB of a recognisable pattern.
pub const FILE_LEN: usize = 64 * 1024;
pub const ETAG: &str = "\"v1\"";
pub const LAST_MODIFIED: &str = "Wed, 21 Oct 2026 07:28:00 GMT";
/// `/cut*` promises this many bytes but sends only `CUT_SENT`.
pub const CUT_PROMISED: usize = 10_000;
pub const CUT_SENT: usize = 4_000;
/// `/slow` sends `SLOW_CHUNKS` chunks of `SLOW_CHUNK` bytes, one every `SLOW_DELAY`.
pub const SLOW_CHUNKS: usize = 40;
pub const SLOW_CHUNK: usize = 1024;
pub const SLOW_DELAY: Duration = Duration::from_millis(50);

pub fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 251) as u8).collect()
}

/// One request as the server saw it. Header names are lower-cased.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub path: String,
    pub headers: HashMap<String, String>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

pub struct TestServer {
    pub addr: SocketAddr,
    requests: Arc<Mutex<Vec<Recorded>>>,
    gate: Arc<AtomicBool>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl TestServer {
    pub fn start() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        let addr = listener.local_addr().expect("local address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (shutdown, mut stop) = oneshot::channel();
        let recorded = Arc::clone(&requests);
        let gate = Arc::new(AtomicBool::new(false));
        let server_gate = Arc::clone(&gate);
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            runtime.block_on(async move {
                let listener = TcpListener::from_std(listener).expect("tokio listener");
                loop {
                    tokio::select! {
                        _ = &mut stop => break,
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { continue };
                            let recorded = Arc::clone(&recorded);
                            let gate = Arc::clone(&server_gate);
                            tokio::spawn(serve(stream, addr.port(), recorded, gate));
                        }
                    }
                }
            });
        });
        Self {
            addr,
            requests,
            gate,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    /// `http://127.0.0.1:<port><path>`
    pub fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://127.0.0.1:{}{path}", self.addr.port())).expect("test URL")
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    /// Lets `/gated/…` routes answer. Until then they hold their response.
    pub fn open_gate(&self) {
        self.gate.store(true, Ordering::SeqCst);
    }

    /// Waits until the server has received `count` requests.
    pub fn wait_for_requests(&self, count: usize) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        while self.requests().len() < count {
            assert!(Instant::now() < deadline, "only {:?}", self.requests());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn serve(
    mut stream: TcpStream,
    port: u16,
    recorded: Arc<Mutex<Vec<Recorded>>>,
    gate: Arc<AtomicBool>,
) {
    let mut head = Vec::new();
    let mut buffer = [0_u8; 4096];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => head.extend_from_slice(&buffer[..read]),
        }
        if head.len() > 64 * 1024 {
            return;
        }
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let mut lines = text.split("\r\n");
    let path = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or("/")
        .to_owned();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let request = Recorded { path, headers };
    recorded.lock().unwrap().push(request.clone());
    if request.path.starts_with("/gated/") {
        while !gate.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    let _ = respond(&mut stream, port, &request).await;
}

async fn respond(stream: &mut TcpStream, port: u16, request: &Recorded) -> std::io::Result<()> {
    let file = pattern(FILE_LEN);
    match request.path.as_str() {
        "/file.bin" | "/named/file.bin" | "/gated/file.bin" => {
            ranged(stream, request, &file, Some(ETAG), None).await
        }
        "/lm.bin" => ranged(stream, request, &file, None, Some(LAST_MODIFIED)).await,
        "/plain.bin" => send(stream, "200 OK", &[], &file).await,
        "/no-length" => {
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n";
            stream.write_all(head.as_bytes()).await?;
            stream.write_all(&file).await?;
            stream.shutdown().await
        }
        "/attachment" => {
            let headers = [
                ("Content-Disposition", "attachment; filename=\"report.pdf\""),
                ("Content-Type", "application/pdf"),
            ];
            send(stream, "200 OK", &headers, b"%PDF-1.7 test").await
        }
        "/escape" => {
            let headers = [(
                "Content-Disposition",
                "attachment; filename=\"../../escape.txt\"",
            )];
            send(stream, "200 OK", &headers, b"escaped?").await
        }
        "/long-extension" => {
            let disposition = format!("attachment; filename=\"a.{}\"", "x".repeat(300));
            let headers = [("Content-Disposition", disposition.as_str())];
            send(stream, "200 OK", &headers, b"long").await
        }
        "/typed" => {
            let headers = [("Content-Type", "text/plain; charset=utf-8")];
            send(stream, "200 OK", &headers, b"hello").await
        }
        "/redirect" => redirect(stream, "/file.bin").await,
        "/redirect-same-host" => {
            redirect(stream, &format!("http://127.0.0.1:{port}/file.bin")).await
        }
        "/redirect-other-host" => {
            redirect(stream, &format!("http://localhost:{port}/file.bin")).await
        }
        "/redirect-file-scheme" => redirect(stream, "file:///etc/passwd").await,
        "/redirect-loop" => redirect(stream, "/redirect-loop").await,
        "/slow-headers" => {
            tokio::time::sleep(Duration::from_secs(2)).await;
            ranged(stream, request, &file, Some(ETAG), None).await
        }
        "/bad-range" => {
            // Answers a range request with the whole file, labelled as a range from 0.
            let headers = [("ETag", ETAG), ("Accept-Ranges", "bytes")];
            if request.header("range").is_some() {
                let content_range = format!("bytes 0-{}/{}", file.len() - 1, file.len());
                let mut headers = headers.to_vec();
                headers.push(("Content-Range", &content_range));
                send(stream, "206 Partial Content", &headers, &file).await
            } else {
                send(stream, "200 OK", &headers, &file).await
            }
        }
        "/always-206" => {
            let content_range = format!("bytes 0-{}/{}", file.len() - 1, file.len());
            let headers = [("ETag", ETAG), ("Content-Range", content_range.as_str())];
            send(stream, "206 Partial Content", &headers, &file).await
        }
        "/short-range" => {
            // Honours `Range: bytes=N-` with only the next 1000 bytes.
            let start = request
                .header("range")
                .and_then(|range| range.strip_prefix("bytes="))
                .and_then(|range| range.strip_suffix('-'))
                .and_then(|start| start.parse::<usize>().ok());
            let mut headers = vec![("ETag", ETAG), ("Accept-Ranges", "bytes")];
            match start {
                Some(start) if start + 1000 <= file.len() => {
                    let content_range = format!("bytes {start}-{}/{}", start + 999, file.len());
                    headers.push(("Content-Range", &content_range));
                    send(
                        stream,
                        "206 Partial Content",
                        &headers,
                        &file[start..start + 1000],
                    )
                    .await
                }
                _ => send(stream, "200 OK", &headers, &file).await,
            }
        }
        "/unavailable" => send(stream, "503 Service Unavailable", &[], b"later").await,
        "/missing" => send(stream, "404 Not Found", &[], b"no such file").await,
        "/broken" => send(stream, "500 Internal Server Error", &[], b"oops").await,
        "/cut" => cut(stream, true).await,
        "/cut-plain" => cut(stream, false).await,
        "/slow" => slow(stream, SLOW_CHUNKS).await,
        "/slow-long" => slow(stream, 100_000).await,
        "/slow-plain" => slow_plain(stream, 100_000).await,
        "/stall" => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {FILE_LEN}\r\nETag: {ETAG}\r\n\
                 Accept-Ranges: bytes\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(head.as_bytes()).await?;
            stream.write_all(&file[..1000]).await?;
            stream.flush().await?;
            tokio::time::sleep(Duration::from_secs(600)).await;
            Ok(())
        }
        _ => send(stream, "404 Not Found", &[], b"unknown route").await,
    }
}

async fn send(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}

async fn redirect(stream: &mut TcpStream, location: &str) -> std::io::Result<()> {
    send(stream, "302 Found", &[("Location", location)], b"").await
}

/// Serves `body` with a validator and byte ranges: `Range: bytes=N-` gives `206` from `N`
/// unless `If-Range` names a different validator, in which case the whole body comes back.
async fn ranged(
    stream: &mut TcpStream,
    request: &Recorded,
    body: &[u8],
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> std::io::Result<()> {
    let mut headers = vec![
        ("Accept-Ranges", "bytes"),
        ("Content-Type", "application/octet-stream"),
    ];
    if let Some(etag) = etag {
        headers.push(("ETag", etag));
    }
    if let Some(last_modified) = last_modified {
        headers.push(("Last-Modified", last_modified));
    }
    let validator_matches = request
        .header("if-range")
        .is_none_or(|value| Some(value) == etag || Some(value) == last_modified);
    let start = request
        .header("range")
        .and_then(|range| range.strip_prefix("bytes="))
        .and_then(|range| range.strip_suffix('-'))
        .and_then(|start| start.parse::<usize>().ok())
        .filter(|_| validator_matches);
    match start {
        Some(start) if start >= body.len() => {
            let content_range = format!("bytes */{}", body.len());
            headers.push(("Content-Range", &content_range));
            send(stream, "416 Range Not Satisfiable", &headers, b"").await
        }
        Some(start) => {
            let content_range = format!("bytes {start}-{}/{}", body.len() - 1, body.len());
            headers.push(("Content-Range", &content_range));
            send(stream, "206 Partial Content", &headers, &body[start..]).await
        }
        None => send(stream, "200 OK", &headers, body).await,
    }
}

async fn cut(stream: &mut TcpStream, validators: bool) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 200 OK\r\nContent-Length: {CUT_PROMISED}\r\n");
    if validators {
        head.push_str(&format!("ETag: {ETAG}\r\nAccept-Ranges: bytes\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&pattern(CUT_SENT)).await?;
    stream.flush().await?;
    // Close without sending the rest.
    stream.shutdown().await
}

/// Like `slow`, without a validator or byte ranges, so it cannot be resumed.
async fn slow_plain(stream: &mut TcpStream, chunks: usize) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        chunks * SLOW_CHUNK
    );
    stream.write_all(head.as_bytes()).await?;
    let chunk = vec![b's'; SLOW_CHUNK];
    for _ in 0..chunks {
        stream.write_all(&chunk).await?;
        stream.flush().await?;
        tokio::time::sleep(SLOW_DELAY).await;
    }
    Ok(())
}

async fn slow(stream: &mut TcpStream, chunks: usize) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: {ETAG}\r\nAccept-Ranges: bytes\r\n\
         Connection: close\r\n\r\n",
        chunks * SLOW_CHUNK
    );
    stream.write_all(head.as_bytes()).await?;
    let chunk = vec![b's'; SLOW_CHUNK];
    for _ in 0..chunks {
        stream.write_all(&chunk).await?;
        stream.flush().await?;
        tokio::time::sleep(SLOW_DELAY).await;
    }
    Ok(())
}

/// A unique folder under the system temporary directory, removed on drop.
pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "mercury-downloads-test-{label}-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create temporary folder");
        let path = path.canonicalize().expect("canonical temporary folder");
        Self { path }
    }

    /// File names in `dir`, sorted.
    pub fn list(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub const TEST_TIMEOUT: Duration = Duration::from_secs(20);

/// A manager whose events arrive on a channel.
pub fn manager_with(timeouts: Timeouts) -> (DownloadManager, Receiver<DownloadEvent>) {
    let (sender, receiver) = mpsc::channel();
    let manager = DownloadManager::with_timeouts(timeouts, move |event| {
        let _ = sender.send(event);
    })
    .expect("start the download manager");
    (manager, receiver)
}

pub fn manager() -> (DownloadManager, Receiver<DownloadEvent>) {
    manager_with(Timeouts::default())
}

pub fn request(url: Url, destination_dir: &Path) -> DownloadRequest {
    DownloadRequest {
        url,
        referrer: None,
        user_agent: "MercuryTest/1.0".to_owned(),
        cookie_header: None,
        destination_dir: destination_dir.to_path_buf(),
        suggested_name: None,
        resume: None,
    }
}

pub fn resume_from(
    partial_path: PathBuf,
    target_path: PathBuf,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> ResumeFrom {
    ResumeFrom {
        partial_path,
        target_path,
        etag: etag.map(str::to_owned),
        last_modified: last_modified.map(str::to_owned),
    }
}

pub fn event_id(event: &DownloadEvent) -> DownloadId {
    match event {
        DownloadEvent::Started { id, .. }
        | DownloadEvent::Progress { id, .. }
        | DownloadEvent::Finished { id, .. }
        | DownloadEvent::Failed { id, .. }
        | DownloadEvent::Cancelled { id } => *id,
    }
}

pub fn is_terminal(event: &DownloadEvent) -> bool {
    matches!(
        event,
        DownloadEvent::Finished { .. }
            | DownloadEvent::Failed { .. }
            | DownloadEvent::Cancelled { .. }
    )
}

/// Every event for `id` up to and including its terminal event.
pub fn events_until_done(receiver: &Receiver<DownloadEvent>, id: DownloadId) -> Vec<DownloadEvent> {
    let deadline = Instant::now() + TEST_TIMEOUT;
    let mut events = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(left) {
            Ok(event) if event_id(&event) == id => {
                let done = is_terminal(&event);
                events.push(event);
                if done {
                    return events;
                }
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => panic!("download {id:?} did not finish: {events:#?}"),
            Err(RecvTimeoutError::Disconnected) => panic!("event channel closed: {events:#?}"),
        }
    }
}

/// Waits for the `Started` event of `id`, returning it.
pub fn wait_started(receiver: &Receiver<DownloadEvent>, id: DownloadId) -> DownloadEvent {
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = receiver
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("download {id:?} never started"));
        if event_id(&event) != id {
            continue;
        }
        assert!(
            !is_terminal(&event),
            "download {id:?} ended before it started: {event:?}"
        );
        if matches!(event, DownloadEvent::Started { .. }) {
            return event;
        }
    }
}

/// The terminal event of a list produced by `events_until_done`.
pub fn terminal(events: &[DownloadEvent]) -> &DownloadEvent {
    events.last().expect("at least one event")
}
