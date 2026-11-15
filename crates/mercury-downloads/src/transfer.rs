// The streaming loop is adapted from Verso (https://github.com/versotile-org/verso), commit
// eb719bdd6c7b4a22b640d33ea55c6d440a5a850d, `src/download.rs` (`download_body`), licensed
// Apache-2.0 OR MIT (used here under MIT).
// Copyright (c) 2020-2023 Ngo Iok Ui & Tauri Programme within The Commons Conservancy
//
// Changes from Verso: the partial file is named `<final name>.part` and created exclusively in
// the destination folder, and the finished file is published without replacing anything (Verso
// renamed over an existing file); a body error, a stall or a short body is a failure (Verso's
// `while let Ok(chunk)` ended silently on an error); no space is pre-allocated, so a short body
// cannot leave a zero-filled tail that looks complete; cancelling interrupts a pending read
// instead of being noticed only when the next chunk arrives; redirects are followed by hand with
// scheme and cookie checks; and resuming with `Range`/`If-Range` is new.

//! One download, from the first request to the published file.

use std::collections::VecDeque;
use std::io;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Response, StatusCode};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};
use tokio::sync::watch;
use url::Url;

use crate::error::TransferError;
use crate::manager::{DownloadEvent, DownloadId, DownloadRequest, ResumeFrom, Timeouts};
use crate::naming::{self, PART_SUFFIX};
use crate::quarantine;

const MAX_REDIRECTS: usize = 20;
/// Progress is reported at most this often (plus once at the end).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// `bytes_per_second` averages over roughly this much recent time.
const SPEED_WINDOW: Duration = Duration::from_secs(2);
const WRITE_BUFFER_BYTES: usize = 256 * 1024;
/// Attempts at claiming a free name before giving up.
const MAX_CLAIM_ATTEMPTS: usize = 100;

/// The error text of a transfer whose task panicked.
const INTERNAL_FAILURE: &str = "the transfer failed internally";

pub(crate) enum Outcome {
    Finished {
        target_path: PathBuf,
        received_bytes: u64,
    },
    Failed {
        error: String,
        resumable: bool,
    },
    Cancelled,
}

impl Outcome {
    pub(crate) fn internal_failure() -> Self {
        Self::Failed {
            error: INTERNAL_FAILURE.to_owned(),
            resumable: false,
        }
    }
}

pub(crate) struct Transfer {
    pub(crate) id: DownloadId,
    pub(crate) request: DownloadRequest,
    pub(crate) client: reqwest::Client,
    pub(crate) timeouts: Timeouts,
    pub(crate) emit: Arc<dyn Fn(DownloadEvent) + Send + Sync>,
}

/// What is on disk so far. Lives outside the cancellable part so cleanup can always see it.
#[derive(Default)]
struct OnDisk {
    /// The partial file this transfer owns, once it exists.
    part: Option<PathBuf>,
    /// The server gave a validator and accepts byte ranges, so the partial file can be resumed.
    resumable: bool,
}

/// A body written completely to the partial file.
struct Complete {
    dir: PathBuf,
    final_url: Url,
    part: PathBuf,
    target: PathBuf,
    received: u64,
}

/// A checked `ResumeFrom`.
struct ResumePlan {
    dir: PathBuf,
    part: PathBuf,
    target: PathBuf,
    /// Bytes already in the partial file, if a validator allows asking for the rest.
    offset: Option<u64>,
    if_range: Option<String>,
    etag: Option<String>,
    last_modified: Option<String>,
}

impl Transfer {
    /// Runs the transfer to its outcome. A panic anywhere in it (including in `on_event` while
    /// `Started` or `Progress` is delivered) becomes an internal failure, after the partial file
    /// is removed.
    pub(crate) async fn run(self, mut cancelled: watch::Receiver<bool>) -> Outcome {
        let mut on_disk = OnDisk::default();
        let fetched = AssertUnwindSafe(async {
            tokio::select! {
                biased;
                () = wait_for_cancel(&mut cancelled) => None,
                result = self.fetch(&mut on_disk) => Some(result),
            }
        })
        .catch_unwind()
        .await;
        match fetched {
            Err(_) => {
                remove_partial(on_disk.part.as_deref());
                Outcome::internal_failure()
            }
            Ok(None) => {
                remove_partial(on_disk.part.as_deref());
                Outcome::Cancelled
            }
            Ok(Some(Err(error))) => {
                let keep = on_disk.resumable
                    && error.is_transient()
                    && on_disk.part.as_deref().is_some_and(is_regular_file);
                if !keep {
                    remove_partial(on_disk.part.as_deref());
                }
                Outcome::Failed {
                    error: error.to_string(),
                    resumable: keep,
                }
            }
            // From here the download is complete: a late cancel no longer applies.
            Ok(Some(Ok(complete))) => {
                let finished = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    publish(&complete).inspect(|target_path| {
                        quarantine::mark_downloaded(
                            target_path,
                            &complete.final_url,
                            self.request.referrer.as_ref(),
                        );
                    })
                }));
                match finished {
                    Ok(Ok(target_path)) => Outcome::Finished {
                        target_path,
                        received_bytes: complete.received,
                    },
                    Ok(Err(error)) => {
                        remove_partial(Some(&complete.part));
                        Outcome::Failed {
                            error: TransferError::Io(error).to_string(),
                            resumable: false,
                        }
                    }
                    Err(_) => {
                        remove_partial(Some(&complete.part));
                        Outcome::internal_failure()
                    }
                }
            }
        }
    }

    async fn fetch(&self, on_disk: &mut OnDisk) -> Result<Complete, TransferError> {
        let request = &self.request;
        // Before any I/O.
        check_scheme(&request.url).map_err(TransferError::UnsupportedScheme)?;

        let plan = match &request.resume {
            Some(resume) => {
                let plan = check_resume(&request.destination_dir, resume)?;
                if plan.offset.is_some() {
                    on_disk.part = Some(plan.part.clone());
                    on_disk.resumable = true;
                }
                Some(plan)
            }
            None => None,
        };
        let range = plan
            .as_ref()
            .and_then(|plan| plan.offset.zip(plan.if_range.as_deref()));

        let mut response = self.send(range).await?;
        let status = response.status();
        if !status.is_success() {
            return Err(TransferError::Status(status));
        }

        let headers = response.headers();
        let appending = match (status, range) {
            (StatusCode::PARTIAL_CONTENT, Some((offset, _))) => {
                if content_range_start(headers) != Some(offset) {
                    return Err(TransferError::RangeMismatch);
                }
                true
            }
            (StatusCode::PARTIAL_CONTENT, None) => return Err(TransferError::UnexpectedPartial),
            _ => false,
        };
        let offset = if appending {
            range.map_or(0, |(offset, _)| offset)
        } else {
            0
        };
        let mut etag = header_text(headers, header::ETAG);
        let mut last_modified = header_text(headers, header::LAST_MODIFIED);
        if appending
            && etag.is_none()
            && last_modified.is_none()
            && let Some(plan) = &plan
        {
            etag.clone_from(&plan.etag);
            last_modified.clone_from(&plan.last_modified);
        }
        let accepts_ranges = appending
            || header_text(headers, header::ACCEPT_RANGES)
                .is_some_and(|value| value.eq_ignore_ascii_case("bytes"));
        let mime_type = header_text(headers, header::CONTENT_TYPE)
            .and_then(|value| value.split(';').next().map(str::trim).map(str::to_owned))
            .filter(|essence| !essence.is_empty())
            .map(|essence| essence.to_ascii_lowercase());
        let total = if appending {
            content_range_total(headers)
                .or_else(|| response.content_length().map(|rest| offset + rest))
        } else {
            response.content_length()
        };
        let final_url = response.url().clone();

        let (dir, target, part, file) = match plan {
            Some(plan) => {
                let file = open_partial(&plan, appending).await?;
                (plan.dir, plan.target, plan.part, file)
            }
            None => {
                let content_disposition = headers
                    .get(header::CONTENT_DISPOSITION)
                    // Many servers send raw UTF-8 here, which `to_str` would reject.
                    .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
                let name = naming::file_name_for(
                    content_disposition.as_deref(),
                    &final_url,
                    mime_type.as_deref(),
                    request.suggested_name.as_deref(),
                );
                claim_partial(&request.destination_dir, &name).await?
            }
        };
        on_disk.part = Some(part.clone());
        on_disk.resumable = (etag.is_some() || last_modified.is_some()) && accepts_ranges;

        let file_name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        (self.emit)(DownloadEvent::Started {
            id: self.id,
            file_name,
            target_path: target.clone(),
            partial_path: part.clone(),
            total_bytes: total,
            mime_type,
            final_url: final_url.clone(),
            etag,
            last_modified,
        });

        let received = self.write_body(&mut response, file, offset, total).await?;
        Ok(Complete {
            dir,
            final_url,
            part,
            target,
            received,
        })
    }

    /// Sends the request, following up to `MAX_REDIRECTS` redirects by hand.
    async fn send(&self, range: Option<(u64, &str)>) -> Result<Response, TransferError> {
        let mut url = self.request.url.clone();
        for _ in 0..=MAX_REDIRECTS {
            let headers = self.headers_for(&url, range)?;
            let pending = self.client.get(url.clone()).headers(headers).send();
            let response = tokio::time::timeout(self.timeouts.idle, pending)
                .await
                .map_err(|_| TransferError::Stalled(self.timeouts.idle))?
                .map_err(TransferError::network)?;
            if !is_redirect(response.status()) {
                return Ok(response);
            }
            let location = response
                .headers()
                .get(header::LOCATION)
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
                .ok_or(TransferError::BadRedirect)?;
            let next = url
                .join(&location)
                .map_err(|_| TransferError::BadRedirect)?;
            check_scheme(&next).map_err(TransferError::UnsupportedRedirect)?;
            url = next;
        }
        Err(TransferError::TooManyRedirects)
    }

    fn headers_for(
        &self,
        url: &Url,
        range: Option<(u64, &str)>,
    ) -> Result<HeaderMap, TransferError> {
        let request = &self.request;
        let mut headers = HeaderMap::new();
        if !request.user_agent.is_empty() {
            insert(
                &mut headers,
                header::USER_AGENT,
                &request.user_agent,
                "User-Agent",
            )?;
        }
        if let Some(referrer) = referrer_for(request.referrer.as_ref(), url) {
            insert(&mut headers, header::REFERER, &referrer, "Referer")?;
        }
        if let Some(cookie) = &request.cookie_header
            && cookie_allowed(&request.url, url)
        {
            insert(&mut headers, header::COOKIE, cookie, "Cookie")?;
        }
        if let Some((offset, validator)) = range {
            insert(
                &mut headers,
                header::RANGE,
                &format!("bytes={offset}-"),
                "Range",
            )?;
            insert(&mut headers, header::IF_RANGE, validator, "If-Range")?;
        }
        Ok(headers)
    }

    /// Streams the body into `file`, reporting progress. Returns the size of the complete file.
    async fn write_body(
        &self,
        response: &mut Response,
        file: tokio::fs::File,
        offset: u64,
        total: Option<u64>,
    ) -> Result<u64, TransferError> {
        let mut writer = BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);
        let mut received = offset;
        let mut meter = SpeedMeter::new(Instant::now(), received);
        let ended = loop {
            let chunk = match tokio::time::timeout(self.timeouts.idle, response.chunk()).await {
                Err(_) => break Err(TransferError::Stalled(self.timeouts.idle)),
                Ok(Err(error)) => break Err(TransferError::network(error)),
                Ok(Ok(None)) => break Ok(()),
                Ok(Ok(Some(chunk))) => chunk,
            };
            if let Err(error) = writer.write_all(&chunk).await {
                break Err(TransferError::Io(error));
            }
            received += chunk.len() as u64;
            if let Some(bytes_per_second) = meter.tick(Instant::now(), received) {
                self.progress(received, total, bytes_per_second);
            }
        };
        if let Err(error) = ended {
            // Keep what did arrive, in case the partial file is resumable.
            let _ = writer.flush().await;
            return Err(error);
        }
        writer.flush().await?;
        writer.get_ref().sync_all().await?;
        if let Some(expected) = total
            && received != expected
        {
            return Err(TransferError::Truncated { received, expected });
        }
        self.progress(received, total, meter.finish(Instant::now(), received));
        Ok(received)
    }

    fn progress(&self, received_bytes: u64, total_bytes: Option<u64>, bytes_per_second: u64) {
        (self.emit)(DownloadEvent::Progress {
            id: self.id,
            received_bytes,
            total_bytes,
            bytes_per_second,
        });
    }
}

/// Resolves when cancel is requested or the manager is gone.
async fn wait_for_cancel(cancelled: &mut watch::Receiver<bool>) {
    loop {
        if *cancelled.borrow_and_update() {
            return;
        }
        if cancelled.changed().await.is_err() {
            return;
        }
    }
}

/// `Err(scheme)` for anything but http and https.
fn check_scheme(url: &Url) -> Result<(), String> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        other => Err(other.to_owned()),
    }
}

fn is_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn insert(
    headers: &mut HeaderMap,
    name: HeaderName,
    value: &str,
    label: &'static str,
) -> Result<(), TransferError> {
    // `HeaderValue` refuses CR, LF and NUL, so a value cannot smuggle in another header.
    let mut value =
        HeaderValue::from_str(value).map_err(|_| TransferError::InvalidHeader(label))?;
    if name == header::COOKIE {
        value.set_sensitive(true);
    }
    headers.insert(name, value);
    Ok(())
}

/// The `Referer` to send to `target`: no fragment or credentials, nothing from a non-web URL,
/// and nothing from `https` to `http` (the default referrer policy).
fn referrer_for(referrer: Option<&Url>, target: &Url) -> Option<String> {
    let mut referrer = referrer?.clone();
    if check_scheme(&referrer).is_err()
        || (referrer.scheme() == "https" && target.scheme() == "http")
    {
        return None;
    }
    referrer.set_fragment(None);
    let _ = referrer.set_username("");
    let _ = referrer.set_password(None);
    Some(referrer.into())
}

/// The cookie header belongs to the first request's host: it is sent while requests stay on
/// that host, and never from `https` down to `http`, where `Secure` cookies would leak.
fn cookie_allowed(first: &Url, current: &Url) -> bool {
    first.host_str() == current.host_str()
        && !(first.scheme() == "https" && current.scheme() == "http")
}

fn header_text(headers: &HeaderMap, name: HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// `N` from `Content-Range: bytes N-M/T`.
fn content_range_start(headers: &HeaderMap) -> Option<u64> {
    let value = header_text(headers, header::CONTENT_RANGE)?;
    let range = value.strip_prefix("bytes ")?;
    range.split('-').next()?.trim().parse().ok()
}

/// `T` from `Content-Range: bytes N-M/T`, unless it is `*`.
fn content_range_total(headers: &HeaderMap) -> Option<u64> {
    let value = header_text(headers, header::CONTENT_RANGE)?;
    value.rsplit('/').next()?.trim().parse().ok()
}

fn exists_without_following(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

fn is_regular_file(path: &Path) -> bool {
    path.symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_file())
}

fn partial_path_for(target: &Path) -> PathBuf {
    let mut path = target.as_os_str().to_owned();
    path.push(PART_SUFFIX);
    PathBuf::from(path)
}

/// Creates the destination folder and exclusively creates `<unique name>.part` in it.
///
/// `create_new` fails if anything at all is at the path, a symlink included, so this neither
/// overwrites nor writes through a link; on a lost race the next free name is tried.
async fn claim_partial(
    destination_dir: &Path,
    name: &str,
) -> Result<(PathBuf, PathBuf, PathBuf, tokio::fs::File), TransferError> {
    tokio::fs::create_dir_all(destination_dir).await?;
    let dir = tokio::fs::canonicalize(destination_dir).await?;
    for _ in 0..MAX_CLAIM_ATTEMPTS {
        let target = naming::unique_path(&dir, name, exists_without_following);
        let part = partial_path_for(&target);
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .await
        {
            Ok(file) => return Ok((dir, target, part, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free file name").into())
}

/// Checks a `ResumeFrom` without writing anything: both paths directly inside the destination
/// folder, the partial name exactly the target name plus `.part`, the target name already
/// sanitised, and the partial file (if present) a regular file rather than a link.
fn check_resume(destination_dir: &Path, resume: &ResumeFrom) -> Result<ResumePlan, TransferError> {
    use TransferError::InvalidResume;

    let dir = std::fs::canonicalize(destination_dir)
        .map_err(|_| InvalidResume("the destination folder does not exist"))?;
    let target_name = resume
        .target_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(InvalidResume("the target path has no usable file name"))?;
    if naming::sanitize_file_name(target_name) != target_name {
        return Err(InvalidResume("the target file name is not a safe name"));
    }
    let part_name = resume
        .partial_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(InvalidResume("the partial path has no usable file name"))?;
    if part_name != format!("{target_name}{PART_SUFFIX}") {
        return Err(InvalidResume(
            "the partial file does not belong to the target",
        ));
    }
    for path in [&resume.target_path, &resume.partial_path] {
        let parent = path
            .parent()
            .and_then(|parent| std::fs::canonicalize(parent).ok());
        if parent.as_deref() != Some(dir.as_path()) {
            return Err(InvalidResume("the files are not in the destination folder"));
        }
    }
    let part = dir.join(part_name);
    let target = dir.join(target_name);

    let length = match part.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_file() => metadata.len(),
        Ok(_) => return Err(InvalidResume("the partial file is not a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    // `If-Range` needs a strong validator: a weak ETag cannot be used, a date can.
    let if_range = resume
        .etag
        .clone()
        .filter(|etag| !etag.starts_with("W/"))
        .or_else(|| resume.last_modified.clone());
    let offset = (length > 0 && if_range.is_some()).then_some(length);
    Ok(ResumePlan {
        dir,
        part,
        target,
        offset,
        if_range,
        etag: resume.etag.clone(),
        last_modified: resume.last_modified.clone(),
    })
}

/// Opens the partial file of a resumed download, appending to it or starting it over.
async fn open_partial(
    plan: &ResumePlan,
    appending: bool,
) -> Result<tokio::fs::File, TransferError> {
    if !exists_without_following(&plan.part) {
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&plan.part)
            .await?;
        return Ok(file);
    }
    let file = std::fs::OpenOptions::new().write(true).open(&plan.part)?;
    // The path was a regular file when checked; make sure it still is the file now open, so a
    // link swapped in meanwhile is never written through.
    let opened = file.metadata()?;
    let at_path = plan.part.symlink_metadata()?;
    if !at_path.file_type().is_file() || !opened.is_file() || !same_file(&opened, &at_path) {
        return Err(TransferError::InvalidResume("the partial file changed"));
    }
    let mut file = tokio::fs::File::from_std(file);
    if appending {
        file.seek(io::SeekFrom::End(0)).await?;
    } else {
        file.set_len(0).await?;
    }
    Ok(file)
}

#[cfg(unix)]
fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

/// Gives the complete partial file its final name without replacing anything.
///
/// A hard link fails if the name exists (a symlink included), which `rename` would silently
/// replace; if the name was taken while downloading, the next free one is used. On file systems
/// without hard links, `rename` is used after checking the name is free.
fn publish(complete: &Complete) -> io::Result<PathBuf> {
    let name = complete
        .target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut target = complete.target.clone();
    for _ in 0..MAX_CLAIM_ATTEMPTS {
        match std::fs::hard_link(&complete.part, &target) {
            Ok(()) => {
                if let Err(error) = std::fs::remove_file(&complete.part) {
                    log::warn!("could not remove {}: {error}", complete.part.display());
                }
                return Ok(target);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) if !exists_without_following(&target) => {
                std::fs::rename(&complete.part, &target)?;
                return Ok(target);
            }
            Err(_) => {}
        }
        target = naming::unique_path(&complete.dir, &name, exists_without_following);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free file name",
    ))
}

fn remove_partial(part: Option<&Path>) {
    let Some(part) = part else { return };
    match std::fs::remove_file(part) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => log::warn!("could not remove {}: {error}", part.display()),
    }
}

/// Throttles progress reports and averages the speed over the last couple of seconds.
struct SpeedMeter {
    last_report: Instant,
    /// `(time, bytes received)` at each report, oldest first, spanning about `SPEED_WINDOW`.
    samples: VecDeque<(Instant, u64)>,
}

impl SpeedMeter {
    fn new(now: Instant, received: u64) -> Self {
        Self {
            last_report: now,
            samples: VecDeque::from([(now, received)]),
        }
    }

    /// `Some(bytes per second)` when a report is due.
    fn tick(&mut self, now: Instant, received: u64) -> Option<u64> {
        if now.duration_since(self.last_report) < PROGRESS_INTERVAL {
            return None;
        }
        self.last_report = now;
        Some(self.record(now, received))
    }

    /// The speed for the final report, which is always sent.
    fn finish(&mut self, now: Instant, received: u64) -> u64 {
        self.record(now, received)
    }

    fn record(&mut self, now: Instant, received: u64) -> u64 {
        self.samples.push_back((now, received));
        while self.samples.len() > 2
            && self
                .samples
                .get(1)
                .is_some_and(|(time, _)| now.duration_since(*time) >= SPEED_WINDOW)
        {
            self.samples.pop_front();
        }
        let (first_time, first_bytes) = self.samples.front().copied().unwrap_or((now, received));
        let seconds = now.duration_since(first_time).as_secs_f64();
        if seconds <= 0.0 {
            return 0;
        }
        (received.saturating_sub(first_bytes) as f64 / seconds) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_reported_at_most_every_quarter_second() {
        let start = Instant::now();
        let mut meter = SpeedMeter::new(start, 0);
        assert_eq!(meter.tick(start + Duration::from_millis(100), 100), None);
        assert!(
            meter
                .tick(start + Duration::from_millis(250), 250)
                .is_some()
        );
        assert_eq!(meter.tick(start + Duration::from_millis(400), 400), None);
        assert!(
            meter
                .tick(start + Duration::from_millis(500), 500)
                .is_some()
        );
    }

    #[test]
    fn speed_is_a_moving_average_over_the_recent_window() {
        let start = Instant::now();
        let mut meter = SpeedMeter::new(start, 0);
        // 1,000 bytes a second for four seconds…
        for step in 1..=16 {
            let at = start + Duration::from_millis(250 * step);
            let speed = meter.tick(at, 250 * step).unwrap();
            assert!((900..=1100).contains(&speed), "{speed} at step {step}");
        }
        // …then 10,000 a second: the average catches up within the window, not the whole run.
        let mut received = 4_000;
        let mut speed = 0;
        for step in 17..=28 {
            received += 2_500;
            speed = meter
                .tick(start + Duration::from_millis(250 * step), received)
                .unwrap();
        }
        assert!((9_000..=10_100).contains(&speed), "{speed}");
    }

    #[test]
    fn a_referrer_is_trimmed_and_not_sent_on_a_downgrade() {
        let page = Url::parse("https://user:pw@example.com/page?q=1#frag").unwrap();
        let https = Url::parse("https://files.example.com/a.zip").unwrap();
        let http = Url::parse("http://files.example.com/a.zip").unwrap();
        assert_eq!(
            referrer_for(Some(&page), &https).as_deref(),
            Some("https://example.com/page?q=1")
        );
        assert_eq!(referrer_for(Some(&page), &http), None);
        let about = Url::parse("about:blank").unwrap();
        assert_eq!(referrer_for(Some(&about), &https), None);
        assert_eq!(referrer_for(None, &https), None);
    }

    #[test]
    fn cookies_stay_on_the_first_host_and_off_plain_http() {
        let first = Url::parse("https://example.com/download").unwrap();
        let same = Url::parse("https://example.com:8443/elsewhere").unwrap();
        let other = Url::parse("https://cdn.example.net/file").unwrap();
        let sub = Url::parse("https://cdn.example.com/file").unwrap();
        let downgrade = Url::parse("http://example.com/file").unwrap();
        assert!(cookie_allowed(&first, &first));
        assert!(cookie_allowed(&first, &same));
        assert!(!cookie_allowed(&first, &other));
        assert!(!cookie_allowed(&first, &sub));
        assert!(!cookie_allowed(&first, &downgrade));
    }

    #[test]
    fn content_range_is_read() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 100-999/1000"),
        );
        assert_eq!(content_range_start(&headers), Some(100));
        assert_eq!(content_range_total(&headers), Some(1000));
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 100-999/*"),
        );
        assert_eq!(content_range_total(&headers), None);
    }
}
