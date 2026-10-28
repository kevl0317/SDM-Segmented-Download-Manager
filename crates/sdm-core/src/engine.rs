//! The segmented download engine.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::types::{SdmError, Status};
use crate::util::{default_headers, filename_from_url, parse_content_disposition, sanitize_filename};

const MIN_SEGMENT: u64 = 1 << 20; // don't parallelize below ~1 MiB/segment
const SPLIT_MIN: u64 = 2 << 20; // a finished worker only steals a remainder this big
const MAX_RETRIES: u32 = 5;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

pub const WORKING_SUFFIX: &str = ".sdmdownload";
pub const STATE_SUFFIX: &str = ".sdm";

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
    url: String,
    filename: Option<String>,
    size: Option<u64>,
    supports_ranges: bool,
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
    stop: AtomicBool,
    probed: AtomicBool,
    state: Mutex<State>,
}

/// A single download job. Cheap to clone (shared `Arc` state).
#[derive(Clone)]
pub struct Download {
    inner: Arc<Inner>,
}

/// The JSON sidecar format (byte-for-byte compatible with the Python build).
#[derive(Serialize, Deserialize)]
struct StateFile {
    url: String,
    dest_dir: String,
    filename: String,
    connections: usize,
    size: Option<u64>,
    supports_ranges: bool,
    segments: Vec<[u64; 3]>, // [start, end, downloaded]
}

fn build_client(headers: &[(String, String)]) -> reqwest::Client {
    let mut map = reqwest::header::HeaderMap::new();
    for (k, v) in headers {
        if let (Ok(name), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(v),
        ) {
            map.insert(name, val);
        }
    }
    reqwest::Client::builder()
        .default_headers(map)
        .connect_timeout(PROBE_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
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
        let client = build_client(&headers);
        let dd: PathBuf = dest_dir.into();
        let dd = std::path::absolute(&dd).unwrap_or(dd);
        let state = State {
            url: url.to_string(),
            filename: filename.map(sanitize_filename),
            size: None,
            supports_ranges: false,
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
                stop: AtomicBool::new(false),
                probed: AtomicBool::new(false),
                state: Mutex::new(state),
            }),
        }
    }

    /// Rebuild a paused download from its `.sdm` sidecar.
    pub fn from_state(state_path: &Path) -> std::io::Result<Download> {
        let text = std::fs::read_to_string(state_path)?;
        let data: StateFile = serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
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

    /// Remember a queued job's last-known size (nothing on disk to resume).
    pub(crate) fn restore_size(&self, size: Option<u64>) {
        self.inner.lock().size = size;
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
    /// Signal the orchestrator/workers to stop at the next chunk boundary.
    pub fn pause(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
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

    fn fail(&self, msg: String) {
        {
            let mut st = self.lock();
            if st.status != Status::Error {
                st.error = Some(msg);
                st.status = Status::Error;
            }
        }
        self.stop.store(true, Ordering::SeqCst);
    }

    fn all_done_locked(st: &State) -> bool {
        if !st.segments.is_empty() {
            st.segments.iter().all(|s| s.done())
        } else {
            st.size.map_or(false, |sz| st.stream_downloaded >= sz)
        }
    }

    // ---- orchestration --------------------------------------------------
    async fn run(self: Arc<Inner>) {
        if self.lock().status == Status::Complete {
            return; // already finished — nothing to do
        }
        self.stop.store(false, Ordering::SeqCst);
        {
            let mut st = self.lock();
            st.error = None;
            st.status = Status::Queued;
        }

        if !self.probed.load(Ordering::SeqCst) {
            self.set_status(Status::Probing);
            if let Err(e) = self.probe().await {
                self.fail(format!("{}", e));
                self.save_state();
                self.lock().speed = 0;
                return;
            }
            self.plan_segments();
            self.probed.store(true, Ordering::SeqCst);
        }

        if self.lock().size == Some(0) {
            if let Err(e) = self.preallocate().await {
                self.fail(e.to_string());
                self.save_state();
            } else {
                self.finalize();
            }
            return;
        }

        self.set_status(Status::Running);
        let sampler = tokio::spawn(Inner::sample_speed(self.clone()));

        let (ranges, has_size, has_segs) = {
            let st = self.lock();
            (
                st.supports_ranges,
                st.size.unwrap_or(0) > 0,
                !st.segments.is_empty(),
            )
        };

        if ranges && has_size && has_segs {
            if let Err(e) = self.preallocate().await {
                self.fail(e.to_string());
            } else {
                {
                    let mut st = self.lock();
                    for s in st.segments.iter_mut() {
                        s.active = false; // clear stale flags before (re)starting
                    }
                }
                let any_todo = self.lock().segments.iter().any(|s| !s.done());
                if any_todo {
                    let mut set = tokio::task::JoinSet::new();
                    for _ in 0..self.connections {
                        set.spawn(Inner::run_worker(self.clone()));
                    }
                    while let Some(res) = set.join_next().await {
                        if let Err(join_err) = res {
                            self.fail(format!("worker task failed: {join_err}"));
                        }
                    }
                }
            }
        } else {
            self.single_stream().await;
        }

        // Decide the final state.
        let (status_now, all_done) = {
            let st = self.lock();
            (st.status, Self::all_done_locked(&st))
        };
        if status_now == Status::Error {
            self.save_state();
        } else if self.stop.load(Ordering::SeqCst) {
            self.set_status(Status::Paused);
            self.save_state();
        } else if all_done {
            self.finalize();
        } else {
            self.set_status(Status::Paused);
            self.save_state();
        }
        self.lock().speed = 0;
        sampler.abort();
    }

    // ---- network --------------------------------------------------------
    async fn probe(self: &Arc<Inner>) -> Result<(), SdmError> {
        let url = self.lock().url.clone();
        let resp = self
            .client
            .get(url.as_str())
            .header(reqwest::header::RANGE, "bytes=0-0")
            .timeout(PROBE_TIMEOUT)
            .send()
            .await?;
        let final_url = resp.url().to_string();
        let status = resp.status();
        let headers = resp.headers().clone();

        let mut st = self.lock();
        st.url = final_url.clone();
        if st.filename.is_none() {
            let cd = headers
                .get(reqwest::header::CONTENT_DISPOSITION)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_content_disposition);
            st.filename = Some(cd.unwrap_or_else(|| filename_from_url(&final_url)));
        }
        let content_range = headers
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok());
        if status == reqwest::StatusCode::PARTIAL_CONTENT && content_range.is_some() {
            st.supports_ranges = true;
            if let Some(total) = content_range
                .and_then(|cr| cr.rsplit('/').next())
                .and_then(|t| t.trim().parse::<u64>().ok())
            {
                st.size = Some(total);
            }
        } else {
            st.supports_ranges = headers
                .get(reqwest::header::ACCEPT_RANGES)
                .and_then(|v| v.to_str().ok())
                .map_or(false, |v| v.eq_ignore_ascii_case("bytes"));
            if status == reqwest::StatusCode::OK {
                if let Some(cl) = headers
                    .get(reqwest::header::CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                {
                    st.size = Some(cl);
                }
            }
        }
        Ok(())
    }

    fn plan_segments(self: &Arc<Inner>) {
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

    async fn preallocate(self: &Arc<Inner>) -> std::io::Result<()> {
        tokio::fs::create_dir_all(&self.dest_dir).await?;
        let p = self.working_path();
        let size = self.lock().size;
        if let (Ok(meta), Some(sz)) = (tokio::fs::metadata(&p).await, size) {
            if meta.len() == sz {
                return Ok(()); // resume: keep the bytes already on disk
            }
        }
        let f = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .open(&p)
            .await?;
        if let Some(sz) = size {
            f.set_len(sz).await?;
        }
        Ok(())
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

    async fn run_worker(self: Arc<Inner>) {
        let path = self.working_path();
        let mut file = match tokio::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .open(&path)
            .await
        {
            Ok(f) => f,
            Err(e) => {
                self.fail(e.to_string());
                return;
            }
        };
        while !self.stop.load(Ordering::SeqCst) {
            let idx = match self.claim_segment() {
                Some(i) => i,
                None => return,
            };
            let res = self.download_segment(&mut file, idx).await;
            self.lock().segments[idx].active = false;
            if let Err(e) = res {
                self.fail(e.to_string());
                return;
            }
        }
    }

    async fn download_segment(
        self: &Arc<Inner>,
        file: &mut tokio::fs::File,
        idx: usize,
    ) -> Result<(), SdmError> {
        use futures_util::StreamExt;
        use tokio::io::{AsyncSeekExt, AsyncWriteExt};

        let mut retries: u32 = 0;
        while !self.stop.load(Ordering::SeqCst) {
            let (pos, end, url) = {
                let st = self.lock();
                let s = &st.segments[idx];
                (s.start + s.downloaded, s.end, st.url.clone())
            };
            if pos > end {
                return Ok(()); // complete (possibly after a steal shrank it)
            }

            let resp = match self
                .client
                .get(url.as_str())
                .header(reqwest::header::RANGE, format!("bytes={}-{}", pos, end))
                .send()
                .await
                .and_then(|r| r.error_for_status())
            {
                Ok(r) => r,
                Err(_) => {
                    retries += 1;
                    if retries > MAX_RETRIES {
                        return Err(SdmError::Other(format!("segment {idx}: too many retries")));
                    }
                    tokio::time::sleep(Duration::from_secs((1u64 << retries).min(10))).await;
                    continue;
                }
            };

            file.seek(std::io::SeekFrom::Start(pos)).await?;
            let mut stream = resp.bytes_stream();
            while !self.stop.load(Ordering::SeqCst) {
                let chunk = match stream.next().await {
                    Some(Ok(c)) => c,
                    Some(Err(_)) => break, // hiccup: outer loop re-opens from the new offset
                    None => break,         // stream ended
                };
                let remaining = {
                    let st = self.lock();
                    let s = &st.segments[idx];
                    (s.end as i64) - (s.start as i64 + s.downloaded as i64) + 1
                };
                if remaining <= 0 {
                    break; // a steal shrank this segment past here
                }
                let mut slice: &[u8] = &chunk;
                if slice.len() as i64 > remaining {
                    slice = &slice[..remaining as usize];
                }
                file.write_all(slice).await?;
                {
                    let mut st = self.lock();
                    st.segments[idx].downloaded += slice.len() as u64;
                }
                let past = {
                    let st = self.lock();
                    let s = &st.segments[idx];
                    s.start + s.downloaded > s.end
                };
                if past {
                    break;
                }
            }
            retries = 0;
        }
        Ok(())
    }

    /// Fallback when the server won't do ranges or won't reveal size.
    async fn single_stream(self: &Arc<Inner>) {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;

        if let Err(e) = tokio::fs::create_dir_all(&self.dest_dir).await {
            self.fail(e.to_string());
            return;
        }
        self.lock().stream_downloaded = 0;
        let url = self.lock().url.clone();
        let resp = match self
            .client
            .get(url.as_str())
            .send()
            .await
            .and_then(|r| r.error_for_status())
        {
            Ok(r) => r,
            Err(e) => {
                self.fail(e.to_string());
                return;
            }
        };
        let mut file = match tokio::fs::File::create(self.working_path()).await {
            Ok(f) => f,
            Err(e) => {
                self.fail(e.to_string());
                return;
            }
        };
        let mut stream = resp.bytes_stream();
        while !self.stop.load(Ordering::SeqCst) {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    if let Err(e) = file.write_all(&chunk).await {
                        self.fail(e.to_string());
                        return;
                    }
                    self.lock().stream_downloaded += chunk.len() as u64;
                }
                Some(Err(e)) => {
                    self.fail(e.to_string());
                    return;
                }
                None => break,
            }
        }
        if !self.stop.load(Ordering::SeqCst) {
            let mut st = self.lock();
            if st.size.is_none() {
                st.size = Some(st.stream_downloaded);
            }
        }
    }

    // ---- finish / persistence ------------------------------------------
    fn finalize(self: &Arc<Inner>) {
        let working = self.working_path();
        let mut final_path = self.final_path();
        if final_path.exists() {
            let parent = final_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let stem = final_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ext = final_path
                .extension()
                .map(|s| s.to_string_lossy().into_owned());
            let mut i = 1;
            loop {
                let name = match &ext {
                    Some(e) => format!("{} ({}).{}", stem, i, e),
                    None => format!("{} ({})", stem, i),
                };
                let cand = parent.join(name);
                if !cand.exists() {
                    final_path = cand;
                    break;
                }
                i += 1;
            }
        }
        let _ = std::fs::rename(&working, &final_path);
        let _ = std::fs::remove_file(self.state_path());
        let mut st = self.lock();
        st.completed = Some(final_path);
        st.status = Status::Complete;
    }

    fn save_state(self: &Arc<Inner>) {
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
            tokio::time::sleep(Duration::from_millis(500)).await;
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
