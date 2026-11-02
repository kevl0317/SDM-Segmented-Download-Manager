//! The segmented download engine.

use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::types::{describe_http, describe_io, describe_status, Status, CONNECTION_LOST, TOO_SLOW};
use crate::util::{default_headers, filename_from_url, resolve_filename, sanitize_filename};

const MIN_SEGMENT: u64 = 1 << 20; // don't parallelize below ~1 MiB/segment
const SPLIT_MIN: u64 = 2 << 20; // a finished worker only steals a remainder this big
const MAX_RETRIES: u32 = 5; // attempts in a row that bring no bytes

// Cut short under test, so each failure path takes moments rather than minutes.
/// How long a connection may stay silent before it counts as dead.
const STALL_TIMEOUT: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 30 });
const CONNECT_TIMEOUT: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 15 });
/// How often a running job saves its progress, so a crash costs little.
const CHECKPOINT_EVERY: Duration = Duration::from_millis(if cfg!(test) { 100 } else { 3000 });
const SAMPLE_EVERY: Duration = Duration::from_millis(if cfg!(test) { 50 } else { 500 });

fn retry_wait(failures: u32) -> Duration {
    if cfg!(test) {
        Duration::from_millis(20)
    } else {
        Duration::from_secs((1u64 << failures).min(10))
    }
}

pub const WORKING_SUFFIX: &str = ".sdmdownload";
pub const STATE_SUFFIX: &str = ".sdm";

/// Who SDM says it is when claiming to be a browser got it refused.
const PLAIN_AGENT: &str = concat!("SDM/", env!("CARGO_PKG_VERSION"));

/// Names unfinished jobs have taken, across all jobs in this process: they share
/// a working file otherwise, and would write over each other.
static CLAIMED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn claimed() -> MutexGuard<'static, Vec<String>> {
    CLAIMED.lock().unwrap_or_else(|e| e.into_inner())
}

fn claim_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase() // Windows names ignore case
}

/// `name (i).ext`, the name for the i-th file called `name`.
fn numbered(name: &str, i: u32) -> String {
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    match p.extension() {
        Some(e) => format!("{stem} ({i}).{}", e.to_string_lossy()),
        None => format!("{stem} ({i})"),
    }
}

/// One contiguous byte range of the file. `end` is inclusive.
#[derive(Debug, Clone)]
pub struct Segment {
    pub index: usize,
    pub start: u64,
    pub end: u64,
    pub downloaded: u64,
    pub active: bool,
}

impl Segment {
    pub fn length(&self) -> u64 {
        self.end - self.start + 1
    }
    pub fn done(&self) -> bool {
        self.downloaded >= self.length()
    }
}

/// All mutable state, guarded by one short-lived mutex (never held across await).
struct State {
    /// The link as given. It stays: the signed links it may redirect to expire.
    url: String,
    /// Where the link last led; requests go straight there until it stops working.
    resolved: Option<String>,
    filename: Option<String>,
    size: Option<u64>,
    supports_ranges: bool,
    etag: Option<String>,
    last_modified: Option<String>,
    segments: Vec<Segment>,
    stream_downloaded: u64,
    status: Status,
    error: Option<String>,
    speed: u64, // bytes/sec
    completed: Option<PathBuf>,
    samples: VecDeque<(Instant, u64)>,
}

struct Inner {
    dest_dir: PathBuf,
    connections: usize,
    client: reqwest::Client,
    /// Sent to the link's own host only, never to wherever it redirects.
    cookie: Option<String>,
    plain_agent: AtomicBool,
    stop: AtomicBool,
    /// Connections are being stopped to change approach (not a pause).
    halted: AtomicBool,
    wake: Notify,
    probed: AtomicBool,
    running: AtomicBool,
    retrying: AtomicUsize,
    claim: Mutex<Option<String>>,
    state: Mutex<State>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.release_name();
    }
}

/// A single download job. Cheap to clone (shared `Arc` state).
#[derive(Clone)]
pub struct Download {
    inner: Arc<Inner>,
}

/// The JSON sidecar format (the Python build's, plus optional fields it ignores).
#[derive(Serialize, Deserialize)]
struct StateFile {
    url: String,
    dest_dir: String,
    filename: String,
    connections: usize,
    size: Option<u64>,
    supports_ranges: bool,
    segments: Vec<[u64; 3]>, // [start, end, downloaded]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
}

/// Do these [start, end, downloaded] entries cover a file of this size exactly?
fn segments_tile(segments: &[[u64; 3]], size: Option<u64>) -> bool {
    if segments.is_empty() {
        return true;
    }
    let mut sorted = segments.to_vec();
    sorted.sort_by_key(|s| s[0]);
    let mut expect = 0;
    for [start, end, done] in sorted {
        if start != expect || end < start || done > end - start + 1 {
            return false;
        }
        expect = end + 1;
    }
    size == Some(expect)
}

fn build_client(headers: &[(String, String)]) -> (reqwest::Client, Option<String>) {
    let mut map = reqwest::header::HeaderMap::new();
    let mut cookie = None;
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("cookie") {
            cookie = Some(v.clone());
        } else if let (Ok(name), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(v),
        ) {
            map.insert(name, val);
        }
    }
    let client = reqwest::Client::builder()
        .default_headers(map)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    (client, cookie)
}

fn same_host(a: &str, b: &str) -> bool {
    match (reqwest::Url::parse(a), reqwest::Url::parse(b)) {
        (Ok(a), Ok(b)) => {
            a.host_str() == b.host_str() && a.port_or_known_default() == b.port_or_known_default()
        }
        _ => false,
    }
}

/// Is this something SDM can fetch at all? Err says why not, in words.
fn check_link(url: &str) -> Result<(), String> {
    match reqwest::Url::parse(url) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => Ok(()),
        Ok(_) => Err("SDM can only download http and https links".to_string()),
        Err(_) => Err("That doesn't look like a link".to_string()),
    }
}

fn header<'a>(headers: &'a reqwest::header::HeaderMap, name: reqwest::header::HeaderName) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// `bytes 100-199/1000` as (100, Some(1000)).
fn content_range(headers: &reqwest::header::HeaderMap) -> Option<(u64, Option<u64>)> {
    let value = header(headers, reqwest::header::CONTENT_RANGE)?;
    let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let start = range.split('-').next()?.trim().parse().ok()?;
    Some((start, total.trim().parse().ok()))
}

fn folder_error(e: &std::io::Error) -> String {
    if e.raw_os_error() == Some(183) || e.kind() == std::io::ErrorKind::AlreadyExists {
        return "Can't use the download folder: a file is in the way".to_string();
    }
    format!("Can't use the download folder: {}", describe_io(e))
}

/// What came of waiting for something a pause should interrupt.
enum Waited<T> {
    Got(T),
    TimedOut,
    Stopped,
}

/// How a job's run ended.
enum Outcome {
    Finished,
    Paused,
    Failed(String),
}

/// How the transfer itself ended.
enum Transfer {
    Done,
    Stopped,
    Failed(String),
    /// The server doesn't really do ranges: stream the file instead.
    NoRanges,
    /// The file on the server isn't the one this job started on.
    Changed,
}

/// How a connection's work ended.
enum WorkerEnd {
    /// Nothing more for it to do, or it was asked to stop.
    Finished,
    /// It can't go on, but others might: per-client connection limits look like this.
    GaveUp(String),
    /// No connection can go on: the disk is full, say.
    Fatal(String),
    NoRanges,
    Changed,
}

/// What came of one request for part of the file.
enum Attempt {
    Progress,
    /// Nothing usable came back; wait a little and try again.
    Retry(String),
    /// The link a redirect led to stopped working; ask the original again.
    AskLink(String),
    End(WorkerEnd),
}

/// Counts a connection as reconnecting for as long as it's held.
struct Retrying<'a>(&'a AtomicUsize);

impl<'a> Retrying<'a> {
    fn new(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Retrying(counter)
    }
}

impl Drop for Retrying<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Marks the job as not running when its run ends, however it ends.
struct RunGuard<'a>(&'a AtomicBool);

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// The speed sampler, gone with the run even if the run is dropped midway.
struct Sampler(tokio::task::JoinHandle<()>);

impl Sampler {
    /// Stop it and wait, so it can't save progress over what comes next.
    async fn finish(mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Download {
    pub fn new(
        url: &str,
        dest_dir: impl Into<PathBuf>,
        filename: Option<&str>,
        connections: usize,
        headers: Option<Vec<(String, String)>>,
    ) -> Download {
        let headers = headers.unwrap_or_else(default_headers);
        let (client, cookie) = build_client(&headers);
        let dd: PathBuf = dest_dir.into();
        let dd = std::path::absolute(&dd).unwrap_or(dd);
        let state = State {
            url: url.to_string(),
            resolved: None,
            filename: filename.map(sanitize_filename),
            size: None,
            supports_ranges: false,
            etag: None,
            last_modified: None,
            segments: Vec::new(),
            stream_downloaded: 0,
            status: Status::Queued,
            error: None,
            speed: 0,
            completed: None,
            samples: VecDeque::with_capacity(8),
        };
        Download {
            inner: Arc::new(Inner {
                dest_dir: dd,
                connections: connections.max(1),
                client,
                cookie,
                plain_agent: AtomicBool::new(false),
                stop: AtomicBool::new(false),
                halted: AtomicBool::new(false),
                wake: Notify::new(),
                probed: AtomicBool::new(false),
                running: AtomicBool::new(false),
                retrying: AtomicUsize::new(0),
                claim: Mutex::new(None),
                state: Mutex::new(state),
            }),
        }
    }

    /// Rebuild a paused download from its `.sdm` sidecar.
    pub fn from_state(state_path: &Path) -> std::io::Result<Download> {
        let invalid = |e: String| std::io::Error::new(std::io::ErrorKind::InvalidData, e);
        let text = std::fs::read_to_string(state_path)?;
        let data: StateFile = serde_json::from_str(&text).map_err(|e| invalid(e.to_string()))?;
        if !segments_tile(&data.segments, data.size) {
            return Err(invalid("segments don't cover the file".to_string()));
        }
        let d = Download::new(
            &data.url,
            data.dest_dir.as_str(),
            Some(&data.filename),
            data.connections,
            None,
        );
        {
            let mut st = d.inner.lock();
            st.size = data.size;
            st.supports_ranges = data.supports_ranges;
            st.resolved = data.resolved_url;
            st.etag = data.etag;
            st.last_modified = data.last_modified;
            st.segments = data
                .segments
                .iter()
                .enumerate()
                .map(|(i, s)| Segment {
                    index: i,
                    start: s[0],
                    end: s[1],
                    downloaded: s[2],
                    active: false,
                })
                .collect();
            st.status = Status::Paused; // resume re-runs; it finalizes if already complete
        }
        d.inner.probed.store(true, Ordering::SeqCst);
        // Its partial file already has this name.
        let key = claim_key(&d.inner.final_path());
        claimed().push(key.clone());
        *d.inner.claim_slot() = Some(key);
        Ok(d)
    }

    /// Restore a finished job from the queue file (its sidecar is gone).
    pub(crate) fn restore_complete(&self, size: Option<u64>, path: PathBuf) {
        {
            let mut st = self.inner.lock();
            st.size = size;
            st.stream_downloaded = size.unwrap_or(0);
            st.segments.clear();
            st.completed = Some(path);
            st.status = Status::Complete;
        }
        self.inner.probed.store(true, Ordering::SeqCst);
    }

    /// Restore a job with nothing on disk to resume: it starts over when resumed.
    pub(crate) fn restore_unstarted(&self, size: Option<u64>) {
        let mut st = self.inner.lock();
        st.size = size;
        st.status = Status::Paused;
    }

    /// Restore a job that had failed, with its reason.
    pub(crate) fn restore_failed(&self, why: String) {
        let mut st = self.inner.lock();
        st.error = Some(why);
        st.status = Status::Error;
    }

    /// The name a link's file would be saved under, asking the server: for showing
    /// it before the download starts. `None` if the server doesn't answer in time.
    pub async fn suggested_name(url: &str, headers: Option<Vec<(String, String)>>) -> Option<String> {
        check_link(url).ok()?;
        let d = Download::new(url, ".", None, 1, headers);
        let Waited::Got(Ok(resp)) = d.inner.first_contact(url).await else {
            return None;
        };
        resp.status().is_success().then(|| {
            resolve_filename(
                url,
                resp.url().as_str(),
                header(resp.headers(), reqwest::header::CONTENT_DISPOSITION),
                header(resp.headers(), reqwest::header::CONTENT_TYPE),
            )
        })
    }

    // ---- accessors (each takes the lock briefly) ------------------------
    pub fn url(&self) -> String {
        self.inner.lock().url.clone()
    }
    pub fn filename(&self) -> String {
        self.inner.resolved_name()
    }
    pub fn size(&self) -> Option<u64> {
        self.inner.lock().size
    }
    pub fn status(&self) -> Status {
        self.inner.lock().status
    }
    pub fn error(&self) -> Option<String> {
        self.inner.lock().error.clone()
    }
    pub fn speed(&self) -> u64 {
        self.inner.lock().speed
    }
    pub fn supports_ranges(&self) -> bool {
        self.inner.lock().supports_ranges
    }
    /// A connection was lost and is being re-established.
    pub fn reconnecting(&self) -> bool {
        self.inner.retrying.load(Ordering::SeqCst) > 0
    }
    pub fn connections(&self) -> usize {
        self.inner.connections
    }
    pub fn is_active(&self) -> bool {
        self.inner.lock().status.is_active()
    }
    pub fn segment_count(&self) -> usize {
        self.inner.lock().segments.len()
    }
    pub fn segments_snapshot(&self) -> Vec<Segment> {
        self.inner.lock().segments.clone()
    }
    pub fn dest_dir(&self) -> &Path {
        &self.inner.dest_dir
    }
    pub fn working_path(&self) -> PathBuf {
        self.inner.working_path()
    }
    pub fn state_path(&self) -> PathBuf {
        self.inner.state_path()
    }
    pub fn output_path(&self) -> PathBuf {
        // Drop the guard before final_path() locks again — std's Mutex isn't
        // reentrant, and holding it here deadlocked Manager::save_queue().
        let completed = self.inner.lock().completed.clone();
        completed.unwrap_or_else(|| self.inner.final_path())
    }
    /// Stable identity across clones (same underlying job).
    pub fn id(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }

    pub fn downloaded(&self) -> u64 {
        self.inner.downloaded()
    }
    pub fn progress(&self) -> Option<f64> {
        let st = self.inner.lock();
        let size = st.size?;
        if size == 0 {
            return Some(1.0);
        }
        Some((Inner::downloaded_locked(&st) as f64 / size as f64).min(1.0))
    }
    pub fn eta(&self) -> Option<f64> {
        let st = self.inner.lock();
        let size = st.size?;
        if st.speed == 0 {
            return None;
        }
        let dl = Inner::downloaded_locked(&st);
        if dl >= size {
            return Some(0.0);
        }
        Some((size - dl) as f64 / st.speed as f64)
    }

    // ---- control --------------------------------------------------------
    /// Stop the transfer now, whatever its connections are doing.
    pub fn pause(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        self.inner.wake.notify_waiters();
    }

    /// Run the download to completion (or until paused/failed). Honors pause.
    pub async fn run(&self) {
        Inner::run(self.inner.clone()).await;
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn claim_slot(&self) -> MutexGuard<'_, Option<String>> {
        self.claim.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn downloaded_locked(st: &State) -> u64 {
        if st.segments.is_empty() {
            st.stream_downloaded
        } else {
            st.segments.iter().map(|s| s.downloaded).sum()
        }
    }

    fn downloaded(&self) -> u64 {
        Self::downloaded_locked(&self.lock())
    }

    fn resolved_name(&self) -> String {
        let st = self.lock();
        st.filename
            .clone()
            .unwrap_or_else(|| filename_from_url(&st.url))
    }

    fn final_path(&self) -> PathBuf {
        self.dest_dir.join(self.resolved_name())
    }

    fn working_path(&self) -> PathBuf {
        let mut p = self.final_path().into_os_string();
        p.push(WORKING_SUFFIX);
        PathBuf::from(p)
    }

    fn state_path(&self) -> PathBuf {
        let mut p = self.working_path().into_os_string();
        p.push(STATE_SUFFIX);
        PathBuf::from(p)
    }

    fn set_status(&self, s: Status) {
        self.lock().status = s;
    }

    fn all_done_locked(st: &State) -> bool {
        if !st.segments.is_empty() {
            st.segments.iter().all(|s| s.done())
        } else {
            st.size.map_or(false, |sz| st.stream_downloaded >= sz)
        }
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst) || self.halted.load(Ordering::SeqCst)
    }

    /// Stop every connection, to change approach.
    fn halt(&self) {
        self.halted.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
    }

    async fn stopped(&self) {
        loop {
            let woken = self.wake.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            if self.stopping() {
                return;
            }
            woken.await;
        }
    }

    /// Wait for `work`, for at most `limit` and only until the job is stopped.
    async fn wait_for<F: Future>(&self, limit: Duration, work: F) -> Waited<F::Output> {
        tokio::select! {
            r = tokio::time::timeout(limit, work) => match r {
                Ok(v) => Waited::Got(v),
                Err(_) => Waited::TimedOut,
            },
            _ = self.stopped() => Waited::Stopped,
        }
    }

    // ---- names ----------------------------------------------------------
    /// The first name for this job free on disk and among other unfinished jobs.
    fn free_name(&self, others: &[String]) -> String {
        let base = self.resolved_name();
        let free = |name: &str| {
            let path = self.dest_dir.join(name);
            !others.contains(&claim_key(&path)) && !path.exists()
        };
        if free(&base) {
            return base;
        }
        (1..).map(|i| numbered(&base, i)).find(|n| free(n)).unwrap_or(base)
    }

    /// Take this job's name for itself, numbering it if it's in use.
    fn claim_name(&self) {
        if self.claim_slot().is_some() {
            return;
        }
        let mut others = claimed();
        let name = self.free_name(&others);
        let key = claim_key(&self.dest_dir.join(&name));
        others.push(key.clone());
        *self.claim_slot() = Some(key);
        self.lock().filename = Some(name);
    }

    fn release_name(&self) {
        if let Some(key) = self.claim_slot().take() {
            claimed().retain(|c| *c != key);
        }
    }

    // ---- orchestration --------------------------------------------------
    async fn run(self: Arc<Inner>) {
        // One run at a time: a second Resume while this one winds down does nothing.
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let guard = RunGuard(&self.running);
        if self.lock().status == Status::Complete {
            return; // already finished — nothing to do
        }
        self.stop.store(false, Ordering::SeqCst);
        self.halted.store(false, Ordering::SeqCst);
        {
            let mut st = self.lock();
            st.error = None;
            st.status = Status::Queued;
        }

        let outcome = self.attempt().await;

        // Progress goes on disk first, so whoever sees the new status can trust it.
        if !matches!(outcome, Outcome::Finished) {
            self.save_state();
        }
        drop(guard);
        let mut st = self.lock();
        st.speed = 0;
        match outcome {
            Outcome::Finished => st.status = Status::Complete,
            Outcome::Paused => st.status = Status::Paused,
            Outcome::Failed(why) => {
                st.error = Some(why);
                st.status = Status::Error;
            }
        }
    }

    async fn attempt(self: &Arc<Inner>) -> Outcome {
        let mut started_over = false;
        loop {
            if !self.probed.load(Ordering::SeqCst) {
                self.set_status(Status::Probing);
                let link = self.lock().url.clone();
                if let Err(why) = check_link(&link) {
                    return Outcome::Failed(why);
                }
                match self.first_contact(&link).await {
                    Waited::Stopped => return Outcome::Paused,
                    Waited::TimedOut => return Outcome::Failed(TOO_SLOW.to_string()),
                    Waited::Got(Err(e)) => return Outcome::Failed(describe_http(&e)),
                    Waited::Got(Ok(resp)) => self.learn_from_probe(&link, &resp),
                }
                self.plan_segments();
                self.probed.store(true, Ordering::SeqCst);
            }
            self.claim_name();

            if self.lock().size == Some(0) {
                if let Err(why) = self.preallocate().await {
                    return Outcome::Failed(why);
                }
                return self.finished().await;
            }

            self.set_status(Status::Running);
            let sampler = Sampler(tokio::spawn(Inner::sample_speed(self.clone())));
            let ranged = {
                let st = self.lock();
                st.supports_ranges && st.size.unwrap_or(0) > 0 && !st.segments.is_empty()
            };
            let transfer = if ranged {
                self.transfer_ranges().await
            } else {
                self.transfer_stream().await
            };
            sampler.finish().await;

            match transfer {
                Transfer::Done => return self.finished().await,
                Transfer::Stopped => return Outcome::Paused,
                Transfer::Failed(why) => {
                    // Nothing on disk: ask the server afresh next time.
                    if !self.working_path().exists() {
                        self.probed.store(false, Ordering::SeqCst);
                    }
                    return Outcome::Failed(why);
                }
                Transfer::NoRanges => {
                    let mut st = self.lock();
                    st.supports_ranges = false;
                    st.segments.clear();
                }
                Transfer::Changed if started_over => {
                    return Outcome::Failed("The file on the server keeps changing".to_string());
                }
                Transfer::Changed => {
                    started_over = true;
                    let mut st = self.lock();
                    st.segments.clear();
                    st.size = None;
                    st.supports_ranges = false;
                    st.resolved = None;
                    st.etag = None;
                    st.last_modified = None;
                    st.stream_downloaded = 0;
                    self.probed.store(false, Ordering::SeqCst);
                }
            }
            self.halted.store(false, Ordering::SeqCst);
        }
    }

    async fn finished(self: &Arc<Inner>) -> Outcome {
        match self.finalize().await {
            Ok(()) => Outcome::Finished,
            Err(why) => Outcome::Failed(why),
        }
    }

    // ---- network --------------------------------------------------------
    fn request(&self, target: &str, link: &str) -> reqwest::RequestBuilder {
        let mut req = self.client.get(target);
        if self.plain_agent.load(Ordering::SeqCst) {
            req = req.header(reqwest::header::USER_AGENT, PLAIN_AGENT);
        }
        match &self.cookie {
            // A cross-host redirect drops it too; going straight there mustn't add it.
            Some(cookie) if same_host(target, link) => req.header(reqwest::header::COOKIE, cookie),
            _ => req,
        }
    }

    /// Should a refusal get one more try, without claiming to be a browser? Most
    /// servers like a browser's agent, but Cloudflare answers with a 403 when a client
    /// claims to be one and doesn't shake hands like one, and lets plain tools through.
    fn try_plain_agent(&self, status: reqwest::StatusCode) -> bool {
        status == reqwest::StatusCode::FORBIDDEN && !self.plain_agent.swap(true, Ordering::SeqCst)
    }

    /// Where to send requests: where the link last led, else the link. Also the link.
    fn target(&self) -> (String, String) {
        let st = self.lock();
        (st.resolved.clone().unwrap_or_else(|| st.url.clone()), st.url.clone())
    }

    fn note_resolved(&self, link: &str, resp: &reqwest::Response) {
        let mut st = self.lock();
        if resp.url().as_str() != link && st.resolved.as_deref() != Some(resp.url().as_str()) {
            st.resolved = Some(resp.url().to_string());
        }
    }

    /// The first contact: one byte, which tells the size and whether ranges work.
    fn ask(&self, link: &str) -> impl Future<Output = reqwest::Result<reqwest::Response>> {
        self.request(link, link)
            .header(reqwest::header::RANGE, "bytes=0-0")
            .send()
    }

    async fn first_contact(&self, link: &str) -> Waited<reqwest::Result<reqwest::Response>> {
        match self.wait_for(STALL_TIMEOUT, self.ask(link)).await {
            Waited::Got(Ok(resp)) if self.try_plain_agent(resp.status()) => {
                self.wait_for(STALL_TIMEOUT, self.ask(link)).await
            }
            other => other,
        }
    }

    fn learn_from_probe(&self, link: &str, resp: &reqwest::Response) {
        let status = resp.status();
        let headers = resp.headers();
        // A refusal here isn't final: some servers only refuse the range, and the
        // plain request that follows gives the real answer. It names nothing, though.
        let answered = status.is_success() || status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE;
        if answered {
            self.note_resolved(link, resp);
        }
        let mut st = self.lock();
        if answered && st.filename.is_none() {
            st.filename = Some(resolve_filename(
                link,
                resp.url().as_str(),
                header(headers, reqwest::header::CONTENT_DISPOSITION),
                header(headers, reqwest::header::CONTENT_TYPE),
            ));
        }
        st.etag = header(headers, reqwest::header::ETAG).map(str::to_string);
        st.last_modified = header(headers, reqwest::header::LAST_MODIFIED).map(str::to_string);
        let range = content_range(headers);
        if status == reqwest::StatusCode::PARTIAL_CONTENT && range.is_some() {
            st.supports_ranges = true;
            st.size = range.and_then(|(_, total)| total);
        } else {
            st.supports_ranges = header(headers, reqwest::header::ACCEPT_RANGES)
                .is_some_and(|v| v.eq_ignore_ascii_case("bytes"));
            if status == reqwest::StatusCode::OK {
                st.size = header(headers, reqwest::header::CONTENT_LENGTH)
                    .and_then(|v| v.parse().ok());
            }
        }
    }

    /// Did this answer come from a different file than the job started on? Both
    /// validators have to disagree: load-balanced servers can differ in one.
    fn file_changed(&self, headers: &reqwest::header::HeaderMap) -> bool {
        let st = self.lock();
        let strong = |tag: &str| !tag.starts_with("W/");
        let etag = match (st.etag.as_deref(), header(headers, reqwest::header::ETAG)) {
            (Some(a), Some(b)) if strong(a) && strong(b) => Some(a != b),
            _ => None,
        };
        let date = match (st.last_modified.as_deref(), header(headers, reqwest::header::LAST_MODIFIED)) {
            (Some(a), Some(b)) => Some(a != b),
            _ => None,
        };
        match (etag, date) {
            (Some(a), Some(b)) => a && b,
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => false,
        }
    }

    fn plan_segments(&self) {
        let mut st = self.lock();
        let size = match st.size {
            Some(s) if s > 0 && st.supports_ranges => s,
            _ => {
                st.segments.clear();
                return;
            }
        };
        let n = ((size + MIN_SEGMENT - 1) / MIN_SEGMENT)
            .min(self.connections as u64)
            .max(1) as usize;
        let base = size / n as u64;
        let mut segments = Vec::with_capacity(n);
        let mut offset = 0u64;
        for i in 0..n {
            let start = offset;
            let end = if i == n - 1 { size - 1 } else { start + base - 1 };
            segments.push(Segment {
                index: i,
                start,
                end,
                downloaded: 0,
                active: false,
            });
            offset = end + 1;
        }
        st.segments = segments;
    }

    async fn preallocate(self: &Arc<Inner>) -> Result<(), String> {
        tokio::fs::create_dir_all(&self.dest_dir)
            .await
            .map_err(|e| folder_error(&e))?;
        let p = self.working_path();
        let size = self.lock().size.unwrap_or(0);
        if tokio::fs::metadata(&p).await.is_ok_and(|m| m.len() == size) {
            return Ok(()); // resume: keep the bytes already on disk
        }
        // Whatever progress the job remembers isn't on disk: start clean.
        if self.downloaded() > 0 {
            self.plan_segments();
        }
        let f = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&p)
            .await
            .map_err(|e| describe_io(&e))?;
        if let Err(e) = f.set_len(size).await {
            drop(f);
            let _ = tokio::fs::remove_file(&p).await;
            return Err(describe_io(&e));
        }
        Ok(())
    }

    /// All the connections, until the file is whole or they can't go on.
    async fn transfer_ranges(self: &Arc<Inner>) -> Transfer {
        if let Err(why) = self.preallocate().await {
            return Transfer::Failed(why);
        }
        {
            let mut st = self.lock();
            for s in st.segments.iter_mut() {
                s.active = false; // clear stale flags before (re)starting
            }
        }
        let mut gave_up = None;
        let mut verdict = None;
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..self.connections {
            set.spawn(Inner::run_worker(self.clone()));
        }
        while let Some(res) = set.join_next().await {
            let decided = match res {
                Ok(WorkerEnd::Finished) => None,
                Ok(WorkerEnd::GaveUp(why)) => {
                    gave_up = Some(why);
                    None
                }
                Ok(WorkerEnd::Fatal(why)) => Some(Transfer::Failed(why)),
                Ok(WorkerEnd::NoRanges) => Some(Transfer::NoRanges),
                Ok(WorkerEnd::Changed) => Some(Transfer::Changed),
                Err(e) => Some(Transfer::Failed(format!("worker task failed: {e}"))),
            };
            if let Some(decided) = decided {
                self.halt();
                verdict.get_or_insert(decided);
            }
        }
        if self.stop.load(Ordering::SeqCst) {
            return Transfer::Stopped;
        }
        if let Some(verdict) = verdict {
            return verdict;
        }
        if Self::all_done_locked(&self.lock()) {
            Transfer::Done
        } else {
            Transfer::Failed(gave_up.unwrap_or_else(|| CONNECTION_LOST.to_string()))
        }
    }

    /// Hand a worker its next unit of work: an unclaimed segment, or the back
    /// half of the largest in-progress segment (so idle connections help the
    /// tail). Returns the segment index, or `None` when nothing's left.
    fn claim_segment(self: &Arc<Inner>) -> Option<usize> {
        let mut st = self.lock();
        for i in 0..st.segments.len() {
            if !st.segments[i].active && !st.segments[i].done() {
                st.segments[i].active = true;
                return Some(i);
            }
        }
        let mut victim: Option<usize> = None;
        let mut best: u64 = 0;
        for i in 0..st.segments.len() {
            let s = &st.segments[i];
            if s.active && !s.done() {
                let remaining = s.end - (s.start + s.downloaded) + 1;
                if remaining > best {
                    best = remaining;
                    victim = Some(i);
                }
            }
        }
        if let Some(vi) = victim {
            if best >= SPLIT_MIN {
                let (cursor, vend) = {
                    let v = &st.segments[vi];
                    (v.start + v.downloaded, v.end)
                };
                let split_at = cursor + (vend - cursor + 1) / 2;
                let new_index = st.segments.len();
                st.segments[vi].end = split_at - 1; // shrink the victim
                st.segments.push(Segment {
                    index: new_index,
                    start: split_at,
                    end: vend,
                    downloaded: 0,
                    active: true,
                });
                return Some(new_index);
            }
        }
        None
    }

    async fn run_worker(self: Arc<Inner>) -> WorkerEnd {
        let path = self.working_path();
        let mut file = match tokio::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .open(&path)
            .await
        {
            Ok(f) => f,
            Err(e) => return WorkerEnd::Fatal(describe_io(&e)),
        };
        while !self.stopping() {
            let idx = match self.claim_segment() {
                Some(i) => i,
                None => return WorkerEnd::Finished,
            };
            let end = self.download_segment(&mut file, idx).await;
            self.lock().segments[idx].active = false;
            if !matches!(end, WorkerEnd::Finished) {
                return end;
            }
        }
        WorkerEnd::Finished
    }

    /// Keep one segment going through whatever the connection does, until it's
    /// whole or several attempts in a row bring nothing.
    async fn download_segment(
        self: &Arc<Inner>,
        file: &mut tokio::fs::File,
        idx: usize,
    ) -> WorkerEnd {
        let mut failures: u32 = 0;
        let mut waiting = None;
        loop {
            if self.stopping() {
                return WorkerEnd::Finished;
            }
            let (pos, end) = {
                let st = self.lock();
                let s = &st.segments[idx];
                (s.start + s.downloaded, s.end)
            };
            if pos > end {
                return WorkerEnd::Finished; // complete (possibly after a steal shrank it)
            }
            match self.fetch_range(file, idx, pos, end, &mut waiting).await {
                Attempt::Progress => failures = 0,
                Attempt::End(end) => return end,
                Attempt::AskLink(why) => {
                    failures += 1;
                    if failures > MAX_RETRIES {
                        return WorkerEnd::GaveUp(why);
                    }
                }
                Attempt::Retry(why) => {
                    failures += 1;
                    if failures > MAX_RETRIES {
                        return WorkerEnd::GaveUp(why);
                    }
                    waiting.get_or_insert_with(|| Retrying::new(&self.retrying));
                    let pause = std::future::pending::<()>();
                    if let Waited::Stopped = self.wait_for(retry_wait(failures), pause).await {
                        return WorkerEnd::Finished;
                    }
                }
            }
        }
    }

    /// One request for bytes `pos..=end`, written to the file as they arrive.
    async fn fetch_range<'a>(
        self: &'a Arc<Inner>,
        file: &mut tokio::fs::File,
        idx: usize,
        pos: u64,
        end: u64,
        waiting: &mut Option<Retrying<'a>>,
    ) -> Attempt {
        use futures_util::StreamExt;
        use tokio::io::{AsyncSeekExt, AsyncWriteExt};

        let (target, link) = self.target();
        let send = self
            .request(&target, &link)
            .header(reqwest::header::RANGE, format!("bytes={pos}-{end}"))
            .send();
        let resp = match self.wait_for(STALL_TIMEOUT, send).await {
            Waited::Stopped => return Attempt::End(WorkerEnd::Finished),
            Waited::TimedOut => return Attempt::Retry(TOO_SLOW.to_string()),
            Waited::Got(Err(e)) => return Attempt::Retry(describe_http(&e)),
            Waited::Got(Ok(r)) => r,
        };
        if let Err(attempt) = self.check_answer(&resp, pos, &target, &link) {
            return attempt;
        }
        self.note_resolved(&link, &resp);

        if let Err(e) = file.seek(std::io::SeekFrom::Start(pos)).await {
            return Attempt::End(WorkerEnd::Fatal(describe_io(&e)));
        }
        let mut stream = resp.bytes_stream();
        let mut got_bytes = false;
        let why = loop {
            let chunk = match self.wait_for(STALL_TIMEOUT, stream.next()).await {
                Waited::Stopped => return Attempt::End(WorkerEnd::Finished),
                Waited::TimedOut => break TOO_SLOW,
                Waited::Got(Some(Ok(c))) => c,
                Waited::Got(_) => break CONNECTION_LOST, // reset, or ended early
            };
            let remaining = {
                let st = self.lock();
                let s = &st.segments[idx];
                (s.end as i64) - (s.start as i64 + s.downloaded as i64) + 1
            };
            if remaining <= 0 {
                return Attempt::Progress; // a steal shrank this segment past here
            }
            let mut slice: &[u8] = &chunk;
            if slice.len() as i64 > remaining {
                slice = &slice[..remaining as usize];
            }
            // Flushed before it's counted: a saved checkpoint never claims bytes
            // that were still in memory when the process died.
            let written = match file.write_all(slice).await {
                Ok(()) => file.flush().await,
                Err(e) => Err(e),
            };
            if let Err(e) = written {
                return Attempt::End(WorkerEnd::Fatal(describe_io(&e)));
            }
            *waiting = None;
            got_bytes = true;
            let past = {
                let mut st = self.lock();
                let s = &mut st.segments[idx];
                s.downloaded += slice.len() as u64;
                s.start + s.downloaded > s.end
            };
            if past {
                return Attempt::Progress;
            }
        };
        if got_bytes {
            Attempt::Progress // only the connection was lost; the next one continues
        } else {
            Attempt::Retry(why.to_string())
        }
    }

    /// Is this the part of the file we asked for? Err says what to do instead.
    fn check_answer(
        &self,
        resp: &reqwest::Response,
        pos: u64,
        target: &str,
        link: &str,
    ) -> Result<(), Attempt> {
        use reqwest::StatusCode;
        let status = resp.status();
        let headers = resp.headers();
        let size = self.lock().size;
        match status {
            StatusCode::PARTIAL_CONTENT => {
                let Some((start, total)) = content_range(headers) else {
                    return Err(Attempt::End(WorkerEnd::NoRanges));
                };
                if start != pos {
                    return Err(Attempt::End(WorkerEnd::NoRanges));
                }
                if total.is_some() && total != size {
                    return Err(Attempt::End(WorkerEnd::Changed));
                }
            }
            // The whole file: usable from its first byte, but nowhere else.
            StatusCode::OK => {
                if pos != 0 {
                    return Err(Attempt::End(WorkerEnd::NoRanges));
                }
                let length = header(headers, reqwest::header::CONTENT_LENGTH).and_then(|v| v.parse().ok());
                if length.is_some() && length != size {
                    return Err(Attempt::End(WorkerEnd::Changed));
                }
            }
            StatusCode::RANGE_NOT_SATISFIABLE => return Err(Attempt::End(WorkerEnd::Changed)),
            _ => {
                let why = describe_status(status);
                // A refusal won't change on a retry; a busy server's answer might.
                if status.is_client_error() && !matches!(status.as_u16(), 408 | 429) {
                    if self.try_plain_agent(status) {
                        return Err(Attempt::AskLink(why));
                    }
                    if target != link {
                        self.lock().resolved = None;
                        return Err(Attempt::AskLink(why));
                    }
                    return Err(Attempt::End(WorkerEnd::GaveUp(why)));
                }
                return Err(Attempt::Retry(why));
            }
        }
        if self.file_changed(headers) {
            return Err(Attempt::End(WorkerEnd::Changed));
        }
        Ok(())
    }

    /// Fallback when the server won't do ranges or won't reveal size.
    async fn transfer_stream(self: &Arc<Inner>) -> Transfer {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;

        if let Err(e) = tokio::fs::create_dir_all(&self.dest_dir).await {
            return Transfer::Failed(folder_error(&e));
        }
        self.lock().stream_downloaded = 0;
        let mut asked_link = false;
        let (resp, link) = loop {
            let (target, link) = self.target();
            let send = self.request(&target, &link).send();
            match self.wait_for(STALL_TIMEOUT, send).await {
                Waited::Stopped => return Transfer::Stopped,
                Waited::TimedOut => return Transfer::Failed(TOO_SLOW.to_string()),
                Waited::Got(Err(e)) => return Transfer::Failed(describe_http(&e)),
                Waited::Got(Ok(r)) if r.status().is_success() => break (r, link),
                Waited::Got(Ok(r)) if self.try_plain_agent(r.status()) => {}
                Waited::Got(Ok(r)) => {
                    if target == link || asked_link {
                        return Transfer::Failed(describe_status(r.status()));
                    }
                    asked_link = true;
                    self.lock().resolved = None; // it may just have expired
                }
            }
        };
        self.note_resolved(&link, &resp);
        let mut file = match tokio::fs::File::create(self.working_path()).await {
            Ok(f) => f,
            Err(e) => return Transfer::Failed(describe_io(&e)),
        };
        let mut stream = resp.bytes_stream();
        loop {
            let chunk = match self.wait_for(STALL_TIMEOUT, stream.next()).await {
                Waited::Stopped => return Transfer::Stopped,
                Waited::TimedOut => return Transfer::Failed(TOO_SLOW.to_string()),
                Waited::Got(None) => break,
                Waited::Got(Some(Err(_))) => return Transfer::Failed(CONNECTION_LOST.to_string()),
                Waited::Got(Some(Ok(c))) => c,
            };
            let written = match file.write_all(&chunk).await {
                Ok(()) => file.flush().await,
                Err(e) => Err(e),
            };
            if let Err(e) = written {
                return Transfer::Failed(describe_io(&e));
            }
            self.lock().stream_downloaded += chunk.len() as u64;
        }
        let mut st = self.lock();
        match st.size {
            Some(size) if st.stream_downloaded < size => Transfer::Failed(CONNECTION_LOST.to_string()),
            _ => {
                st.size = Some(st.stream_downloaded);
                Transfer::Done
            }
        }
    }

    // ---- finish / persistence ------------------------------------------
    /// Put the finished file in place under its name. Only now is it finished: an
    /// antivirus can hold or remove a new file, and that's a failure to report.
    async fn finalize(self: &Arc<Inner>) -> Result<(), String> {
        let working = self.working_path();
        let state = self.state_path();
        let name = {
            let others = claimed();
            let mine = self.claim_slot().clone();
            let others: Vec<String> = others.iter().filter(|c| Some(*c) != mine.as_ref()).cloned().collect();
            self.free_name(&others)
        };
        let final_path = self.dest_dir.join(&name);
        let mut tries = 0;
        while let Err(e) = std::fs::rename(&working, &final_path) {
            let busy = e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(32);
            tries += 1;
            if !busy || tries >= 25 {
                return Err(format!("Couldn't save the file: {}", describe_io(&e)));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let _ = std::fs::remove_file(&state);
        {
            let mut st = self.lock();
            st.filename = Some(name);
            st.completed = Some(final_path);
        }
        self.release_name();
        Ok(())
    }

    /// Save progress beside the working file. Without a working file there is
    /// nothing to resume, and nothing to leave in the user's folder.
    fn save_state(&self) {
        if !self.working_path().exists() {
            return;
        }
        let data = {
            let st = self.lock();
            StateFile {
                url: st.url.clone(),
                dest_dir: self.dest_dir.to_string_lossy().into_owned(),
                filename: st
                    .filename
                    .clone()
                    .unwrap_or_else(|| filename_from_url(&st.url)),
                connections: self.connections,
                size: st.size,
                supports_ranges: st.supports_ranges,
                segments: st
                    .segments
                    .iter()
                    .map(|s| [s.start, s.end, s.downloaded])
                    .collect(),
                resolved_url: st.resolved.clone(),
                etag: st.etag.clone(),
                last_modified: st.last_modified.clone(),
            }
        };
        let sp = self.state_path();
        let tmp = {
            let mut p = sp.clone().into_os_string();
            p.push(".tmp");
            PathBuf::from(p)
        };
        if let Ok(json) = serde_json::to_string(&data) {
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &sp);
            }
        }
    }

    async fn sample_speed(self: Arc<Inner>) {
        self.lock().samples.clear();
        let mut saved = Instant::now();
        loop {
            {
                let mut st = self.lock();
                if st.status != Status::Running {
                    break;
                }
                let dl = Self::downloaded_locked(&st);
                let now = Instant::now();
                if st.samples.len() == 8 {
                    st.samples.pop_front();
                }
                st.samples.push_back((now, dl));
                if st.samples.len() >= 2 {
                    let (t0, b0) = *st.samples.front().unwrap();
                    let (t1, b1) = *st.samples.back().unwrap();
                    let dt = (t1 - t0).as_secs_f64();
                    st.speed = if dt > 0.0 {
                        (b1.saturating_sub(b0) as f64 / dt) as u64
                    } else {
                        0
                    };
                }
            }
            if saved.elapsed() >= CHECKPOINT_EVERY {
                saved = Instant::now();
                self.save_state();
            }
            tokio::time::sleep(SAMPLE_EVERY).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dl(size: u64, conns: usize) -> Download {
        let d = Download::new(
            "https://example.com/file.bin",
            std::env::temp_dir(),
            None,
            conns,
            None,
        );
        {
            let mut st = d.inner.lock();
            st.size = Some(size);
            st.supports_ranges = true;
        }
        d
    }

    fn assert_tiles(segs: &[Segment], size: u64) {
        let mut sorted = segs.to_vec();
        sorted.sort_by_key(|s| s.start);
        let mut expect = 0;
        for s in &sorted {
            assert_eq!(s.start, expect, "gap or overlap at byte {}", s.start);
            assert!(s.end >= s.start);
            expect = s.end + 1;
        }
        assert_eq!(expect, size, "segments must cover the whole file");
    }

    #[test]
    fn output_path_does_not_deadlock_before_completion() {
        // Regression: output_path() re-locked the state mutex it already held,
        // which froze the GUI on its first add. Fail fast instead of hanging.
        let d = dl(1, 1);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(d.output_path());
        });
        let p = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("output_path() deadlocked");
        assert!(p.ends_with("file.bin"));
    }

    #[test]
    fn plan_tiles_the_file() {
        let size = 10 * MIN_SEGMENT + 12_345;
        let d = dl(size, 4);
        d.inner.plan_segments();
        let segs = d.segments_snapshot();
        assert_eq!(segs.len(), 4);
        assert_tiles(&segs, size);
    }

    #[test]
    fn small_files_use_a_single_segment() {
        let d = dl(MIN_SEGMENT / 2, 8);
        d.inner.plan_segments();
        assert_eq!(d.segment_count(), 1);
    }

    #[test]
    fn steal_splits_the_largest_remainder_and_keeps_coverage() {
        let size = 10 * MIN_SEGMENT;
        let d = dl(size, 4);
        d.inner.plan_segments();
        for _ in 0..4 {
            assert!(d.inner.claim_segment().is_some());
        }
        let stolen = d.inner.claim_segment().expect("an idle worker should steal");
        assert_eq!(stolen, 4);
        let segs = d.segments_snapshot();
        assert_eq!(segs.len(), 5);
        assert_tiles(&segs, size);
    }

    #[test]
    fn no_steal_below_the_split_threshold() {
        let d = dl(4 * MIN_SEGMENT, 4); // four 1 MiB segments, each < SPLIT_MIN
        d.inner.plan_segments();
        for _ in 0..4 {
            d.inner.claim_segment();
        }
        assert!(d.inner.claim_segment().is_none());
    }
}
