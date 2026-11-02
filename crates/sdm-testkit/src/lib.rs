//! A local HTTP server that misbehaves on request, for testing SDM against what
//! real servers and networks do: drop or stall connections, refuse, lie about
//! ranges, redirect to expiring links, swap the file mid-download.
//!
//! What a URL does is written in its query, so a test or a person can ask for
//! any failure from any client:
//!
//! ```text
//! /v2rayN.zip?size=5000000&drop_after=100000&drop_times=2
//! ```
//!
//! | option | effect |
//! |---|---|
//! | `size=N` | body length (default 1 MiB) |
//! | `seed=N` | which bytes (see [`fill`]) |
//! | `ranges=0` | ignore `Range`, answer 200 |
//! | `accept=1` | send `Accept-Ranges: bytes` on a 200 |
//! | `len=0`, `chunked=1` | no `Content-Length`: close-delimited, or chunked |
//! | `cd=V`, `ctype=V` | `Content-Disposition` / `Content-Type` value |
//! | `etag=1` | send a validator, which [`FaultServer::replace_files`] changes |
//! | `status=N` | always answer N |
//! | `fail_first=K&fail_status=N` | answer N to the first K requests |
//! | `late_after=K&late_status=N` | answer N after the first K requests |
//! | `limit=K&limit_status=N` | answer N beyond K simultaneous requests |
//! | `delay_ms=N`, `hang=1` | wait before answering, or never answer |
//! | `drop_after=N&drop_times=K` | close after N body bytes, the first K times |
//! | `rst=1` | that close is a connection reset |
//! | `stall_after=N&stall_times=K` | go silent after N body bytes, connection open |
//! | `rate=N` | throttle to N bytes/s |
//! | `slow_start=O&slow_rate=N` | throttle only responses starting at offset O |
//! | `plain_after=K` | ignore `Range` (answer 200) after the first K requests |
//! | `shift_range=N` | claim a `Content-Range` N bytes later than asked |
//! | `need_cookie=V`, `need_referer=V`, `need_ua=V` | 401 unless the header has V |
//! | `block_browsers=1` | 403 to any agent claiming to be a browser, as Cloudflare does |
//! | `to=URL`, `code=N` | on `/redirect`: redirect there (default 302) |
//! | `ttl_ms=N`, `name=V` | on `/release`: redirect to a `/signed/...` link that expires |
//!
//! Faults skip the engine's `bytes=0-0` size probe unless `fault_probe=1`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::watch;

/// One request as the server saw it.
#[derive(Debug, Clone)]
pub struct Logged {
    pub path: String,
    pub range: Option<String>,
    pub cookie: Option<String>,
    pub referer: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Net {
    Up,
    /// Connections stay open but nothing moves: a dead link TCP can't see.
    Silent,
    /// Connections reset, new ones refused.
    Down,
}

struct Shared {
    log: Mutex<Vec<Logged>>,
    counts: Mutex<HashMap<String, u64>>,
    active: Mutex<HashMap<String, usize>>,
    version: AtomicU64,
    opened: AtomicUsize,
    net: watch::Sender<Net>,
}

/// A running server. Drop it to stop it.
pub struct FaultServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    stop: Option<watch::Sender<bool>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FaultServer {
    /// Start on a free loopback port.
    pub fn start() -> FaultServer {
        FaultServer::start_on(0)
    }

    pub fn start_on(port: u16) -> FaultServer {
        let (net, _) = watch::channel(Net::Up);
        let shared = Arc::new(Shared {
            log: Mutex::new(Vec::new()),
            counts: Mutex::new(HashMap::new()),
            active: Mutex::new(HashMap::new()),
            version: AtomicU64::new(0),
            opened: AtomicUsize::new(0),
            net,
        });
        let (addr_tx, addr_rx) = std::sync::mpsc::channel();
        let (stop, stop_rx) = watch::channel(false);
        let worker = shared.clone();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("test server runtime");
            rt.block_on(serve(port, worker, addr_tx, stop_rx));
            rt.shutdown_background();
        });
        let addr = addr_rx.recv().expect("test server failed to start");
        FaultServer {
            addr,
            shared,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// A URL on this server, e.g. `url("/a.zip?size=1000")`.
    pub fn url(&self, path_and_query: &str) -> String {
        format!("http://{}{}", self.addr, path_and_query)
    }

    /// Requests so far, oldest first.
    pub fn log(&self) -> Vec<Logged> {
        self.shared.log.lock().unwrap().clone()
    }

    pub fn request_count(&self) -> usize {
        self.shared.log.lock().unwrap().len()
    }

    /// Connections accepted so far (a spinning client shows up here).
    pub fn connections(&self) -> usize {
        self.shared.opened.load(Ordering::SeqCst)
    }

    /// The network dies: open connections reset, new ones are refused.
    pub fn go_down(&self) {
        let _ = self.shared.net.send(Net::Down);
    }

    /// The network dies silently: connections stay open, but nothing moves.
    pub fn go_silent(&self) {
        let _ = self.shared.net.send(Net::Silent);
    }

    /// The network is back.
    pub fn come_back(&self) {
        let _ = self.shared.net.send(Net::Up);
    }

    /// Every `etag=1` file is replaced by a new version with different bytes.
    pub fn replace_files(&self) {
        self.shared.version.fetch_add(1, Ordering::SeqCst);
    }
}

impl Drop for FaultServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn bind(port: u16) -> std::io::Result<TcpListener> {
    let socket = TcpSocket::new_v4()?;
    socket.set_reuseaddr(true)?;
    socket.bind(SocketAddr::from(([127, 0, 0, 1], port)))?;
    socket.listen(1024)
}

async fn serve(
    port: u16,
    shared: Arc<Shared>,
    addr_tx: std::sync::mpsc::Sender<SocketAddr>,
    mut stop: watch::Receiver<bool>,
) {
    let mut listener = bind(port).expect("bind test server");
    let addr = listener.local_addr().expect("test server address");
    let _ = addr_tx.send(addr);
    let mut net = shared.net.subscribe();
    loop {
        tokio::select! {
            _ = stop.changed() => return,
            changed = net.changed() => {
                if changed.is_err() {
                    return;
                }
                if *net.borrow() == Net::Down {
                    // Closing the listener is what makes new connections fail.
                    drop(std::mem::replace(&mut listener, placeholder()));
                    loop {
                        tokio::select! {
                            _ = stop.changed() => return,
                            _ = net.changed() => {}
                        }
                        if *net.borrow() != Net::Down {
                            break;
                        }
                    }
                    listener = bind(addr.port()).expect("rebind test server");
                }
            }
            accepted = listener.accept() => {
                let Ok((sock, _)) = accepted else { continue };
                shared.opened.fetch_add(1, Ordering::SeqCst);
                let shared = shared.clone();
                tokio::spawn(async move {
                    let _ = connection(sock, shared).await;
                });
            }
        }
    }
}

/// Stands in for the closed listener while the network is down.
fn placeholder() -> TcpListener {
    bind(0).expect("placeholder listener")
}

struct Req {
    method: String,
    path: String,
    opts: HashMap<String, String>,
    headers: HashMap<String, String>,
}

impl Req {
    fn num(&self, key: &str) -> Option<u64> {
        self.opts.get(key).and_then(|v| v.parse().ok())
    }
    fn flag(&self, key: &str) -> bool {
        self.opts.get(key).is_some_and(|v| v == "1")
    }
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_head(text: &str) -> Option<Req> {
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let path = path.to_string();
    let opts = query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k.to_string(), percent_decode(v))
        })
        .collect();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Some(Req {
        method,
        path,
        opts,
        headers,
    })
}

async fn read_head(sock: &mut TcpStream) -> std::io::Result<Option<String>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
        }
        if buf.len() > 256 * 1024 {
            return Ok(None);
        }
    }
}

/// Removes this request from the simultaneous count when it ends.
struct ActiveGuard {
    shared: Arc<Shared>,
    key: String,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        if let Some(n) = self.shared.active.lock().unwrap().get_mut(&self.key) {
            *n = n.saturating_sub(1);
        }
    }
}

async fn write_head(
    sock: &mut TcpStream,
    status: u16,
    headers: &[(String, String)],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        307 => "Temporary Redirect",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        410 => "Gone",
        416 => "Range Not Satisfiable",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    sock.write_all(head.as_bytes()).await
}

async fn answer(sock: &mut TcpStream, status: u16) -> std::io::Result<()> {
    let body = format!("<html><body>{status}</body></html>");
    let headers = [
        ("Content-Type".to_string(), "text/html".to_string()),
        ("Content-Length".to_string(), body.len().to_string()),
    ];
    write_head(sock, status, &headers).await?;
    sock.write_all(body.as_bytes()).await
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn wait_until_gone(shared: &Shared) {
    let mut net = shared.net.subscribe();
    let _ = net.wait_for(|n| *n == Net::Down).await;
}

async fn connection(mut sock: TcpStream, shared: Arc<Shared>) -> std::io::Result<()> {
    let Some(head) = read_head(&mut sock).await? else {
        return Ok(());
    };
    let Some(req) = parse_head(&head) else {
        return Ok(());
    };
    let key = req
        .opts
        .get("key")
        .cloned()
        .unwrap_or_else(|| head.lines().next().unwrap_or("").split(' ').nth(1).unwrap_or("").to_string());
    let range = req.header("range").map(str::to_string);
    let is_probe = range.as_deref() == Some("bytes=0-0") && !req.flag("fault_probe");
    shared.log.lock().unwrap().push(Logged {
        path: req.path.clone(),
        range: range.clone(),
        cookie: req.header("cookie").map(str::to_string),
        referer: req.header("referer").map(str::to_string),
        user_agent: req.header("user-agent").map(str::to_string),
    });
    let n = {
        let mut counts = shared.counts.lock().unwrap();
        let c = counts.entry(key.clone()).or_insert(0);
        *c += 1;
        *c
    };
    let concurrent = {
        let mut active = shared.active.lock().unwrap();
        let a = active.entry(key.clone()).or_insert(0);
        *a += 1;
        *a
    };
    let _guard = ActiveGuard {
        shared: shared.clone(),
        key: key.clone(),
    };

    if let Some(ms) = req.num("delay_ms") {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
    if req.flag("hang") && !is_probe {
        wait_until_gone(&shared).await;
        return reset(sock);
    }
    if *shared.net.borrow() == Net::Silent {
        wait_until_gone(&shared).await;
        return reset(sock);
    }

    if req.flag("block_browsers") && req.header("user-agent").is_some_and(|ua| ua.contains("Mozilla")) {
        return answer(&mut sock, 403).await;
    }

    // Redirects: plain, and GitHub's release-asset shape.
    if req.path == "/redirect" {
        let code = req.num("code").unwrap_or(302) as u16;
        let to = req.opts.get("to").cloned().unwrap_or_default();
        write_head(
            &mut sock,
            code,
            &[
                ("Location".to_string(), to),
                ("Content-Length".to_string(), "0".to_string()),
            ],
        )
        .await?;
        return Ok(());
    }
    if req.path.starts_with("/release/") {
        let name = req.path.rsplit('/').next().unwrap_or("file").to_string();
        let expires = now_ms() + req.num("ttl_ms").unwrap_or(3_600_000);
        let rest = head.lines().next().unwrap_or("").split(' ').nth(1).unwrap_or("");
        let rest = rest.split_once('?').map_or("", |(_, q)| q);
        let to = format!(
            "/signed/{expires}/f804704f-4e35-4e59-840d-ac5f65c70c79?rscd=attachment%3B+filename%3D{name}&{rest}"
        );
        write_head(
            &mut sock,
            302,
            &[
                ("Location".to_string(), to),
                ("Content-Length".to_string(), "0".to_string()),
            ],
        )
        .await?;
        return Ok(());
    }
    if let Some(rest) = req.path.strip_prefix("/signed/") {
        let expires: u64 = rest.split('/').next().and_then(|v| v.parse().ok()).unwrap_or(0);
        if now_ms() > expires {
            return answer(&mut sock, 403).await;
        }
    }

    // Who may have the file.
    for (opt, header) in [
        ("need_cookie", "cookie"),
        ("need_referer", "referer"),
        ("need_ua", "user-agent"),
    ] {
        if let Some(want) = req.opts.get(opt) {
            if !req.header(header).is_some_and(|h| h.contains(want.as_str())) {
                return answer(&mut sock, 401).await;
            }
        }
    }

    // Refusals.
    if let Some(s) = req.num("status") {
        return answer(&mut sock, s as u16).await;
    }
    if !is_probe {
        if let Some(k) = req.num("fail_first") {
            if n <= k {
                return answer(&mut sock, req.num("fail_status").unwrap_or(503) as u16).await;
            }
        }
    }
    if let Some(k) = req.num("late_after") {
        if n > k {
            return answer(&mut sock, req.num("late_status").unwrap_or(503) as u16).await;
        }
    }
    if let Some(k) = req.num("limit") {
        if !is_probe && concurrent as u64 > k {
            return answer(&mut sock, req.num("limit_status").unwrap_or(503) as u16).await;
        }
    }

    // The file itself.
    let version = if req.flag("etag") {
        shared.version.load(Ordering::SeqCst)
    } else {
        0
    };
    let seed = req.num("seed").unwrap_or(1) + version;
    let size = match (version, req.num("new_size")) {
        (v, Some(s)) if v > 0 => s,
        _ => req.num("size").unwrap_or(1 << 20),
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(cd) = req.opts.get("cd") {
        headers.push(("Content-Disposition".to_string(), cd.clone()));
    }
    if let Some(ct) = req.opts.get("ctype") {
        headers.push(("Content-Type".to_string(), ct.clone()));
    }
    if req.flag("etag") {
        headers.push(("ETag".to_string(), format!("\"v{version}\"")));
        headers.push((
            "Last-Modified".to_string(),
            format!("Sun, 11 Oct 2026 06:0{}:40 GMT", version.min(9)),
        ));
    }

    let wants_range = range.is_some()
        && !req.opts.get("ranges").is_some_and(|v| v == "0")
        && !req.num("plain_after").is_some_and(|k| n > k);
    let parsed = range.as_deref().filter(|_| wants_range).and_then(parse_range);
    let (status, start, len) = match parsed {
        Some((a, b)) => {
            if a >= size {
                headers.push(("Content-Range".to_string(), format!("bytes */{size}")));
                headers.push(("Content-Length".to_string(), "0".to_string()));
                write_head(&mut sock, 416, &headers).await?;
                return Ok(());
            }
            let end = b.min(size - 1);
            let shift = req.num("shift_range").unwrap_or(0);
            headers.push((
                "Content-Range".to_string(),
                format!("bytes {}-{}/{size}", a + shift, end + shift),
            ));
            headers.push(("Accept-Ranges".to_string(), "bytes".to_string()));
            (206, a, end - a + 1)
        }
        None => {
            if req.flag("accept") {
                headers.push(("Accept-Ranges".to_string(), "bytes".to_string()));
            }
            (200, 0, size)
        }
    };
    let chunked = req.flag("chunked");
    if chunked {
        headers.push(("Transfer-Encoding".to_string(), "chunked".to_string()));
    } else if !req.opts.get("len").is_some_and(|v| v == "0") {
        headers.push(("Content-Length".to_string(), len.to_string()));
    }
    write_head(&mut sock, status, &headers).await?;
    if req.method == "HEAD" {
        return Ok(());
    }

    // Faults that apply to this response.
    let times_key = |name: &str| format!("{key}#{name}");
    let armed = |limit_opt: &str, name: &str| -> bool {
        if is_probe {
            return false;
        }
        let Some(limit) = req.num(limit_opt) else {
            return true;
        };
        let mut counts = shared.counts.lock().unwrap();
        let c = counts.entry(times_key(name)).or_insert(0);
        *c += 1;
        *c <= limit
    };
    let drop_at = req.num("drop_after").filter(|_| armed("drop_times", "drop"));
    let stall_at = req.num("stall_after").filter(|_| armed("stall_times", "stall"));
    let rate = if req.num("slow_start").is_some_and(|o| o == start) {
        req.num("slow_rate")
    } else {
        req.num("rate")
    };

    let mut net = shared.net.subscribe();
    let mut sent: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if drop_at == Some(sent) {
            return if req.flag("rst") { reset(sock) } else { Ok(()) };
        }
        if stall_at == Some(sent) {
            wait_until_gone(&shared).await;
            return reset(sock);
        }
        if sent >= len {
            break;
        }
        let state = *net.borrow_and_update();
        match state {
            Net::Down => return reset(sock),
            Net::Silent => {
                wait_until_gone(&shared).await;
                return reset(sock);
            }
            Net::Up => {}
        }
        let mut want = (len - sent).min(buf.len() as u64);
        if let Some(limit) = drop_at.filter(|d| *d > sent) {
            want = want.min(limit - sent);
        }
        if let Some(limit) = stall_at.filter(|d| *d > sent) {
            want = want.min(limit - sent);
        }
        if let Some(r) = rate {
            want = want.min((r / 20).max(1));
        }
        let piece = &mut buf[..want as usize];
        fill(seed, start + sent, piece);
        if chunked {
            sock.write_all(format!("{want:x}\r\n").as_bytes()).await?;
            sock.write_all(piece).await?;
            sock.write_all(b"\r\n").await?;
        } else {
            sock.write_all(piece).await?;
        }
        sent += want;
        if rate.is_some() {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                _ = net.changed() => {}
            }
        }
    }
    if chunked {
        sock.write_all(b"0\r\n\r\n").await?;
    }
    sock.shutdown().await
}

/// End a connection with a reset, as a crashing server or a dead route would.
#[allow(deprecated)] // a linger of zero is what resets it, and doesn't block
fn reset(sock: TcpStream) -> std::io::Result<()> {
    let _ = sock.set_linger(Some(Duration::ZERO));
    drop(sock);
    Ok(())
}

fn parse_range(v: &str) -> Option<(u64, u64)> {
    let (a, b) = v.strip_prefix("bytes=")?.split_once('-')?;
    let a = a.parse().ok()?;
    let b = if b.is_empty() { u64::MAX } else { b.parse().ok()? };
    Some((a, b))
}

/// The file's bytes: a function of `seed` and the offset, so the server stores
/// nothing and any mix-up of offsets shows.
pub fn fill(seed: u64, offset: u64, out: &mut [u8]) {
    let mut block = offset / 8;
    let mut value = mix(seed, block);
    for (i, byte) in out.iter_mut().enumerate() {
        let at = offset + i as u64;
        if at / 8 != block {
            block = at / 8;
            value = mix(seed, block);
        }
        *byte = (value >> (8 * (at % 8))) as u8;
    }
}

fn mix(seed: u64, block: u64) -> u64 {
    let mut z = block.wrapping_add(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Check that a file is exactly what the server serves for `seed` and `size`.
pub fn verify(path: &Path, seed: u64, size: u64) -> Result<(), String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let actual = f.metadata().map_err(|e| e.to_string())?.len();
    if actual != size {
        return Err(format!("{} is {actual} bytes, expected {size}", path.display()));
    }
    let (mut offset, mut got, mut want) = (0u64, vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let n = f.read(&mut got).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(());
        }
        fill(seed, offset, &mut want[..n]);
        if got[..n] != want[..n] {
            let i = got[..n].iter().zip(&want[..n]).position(|(a, b)| a != b).unwrap_or(0);
            return Err(format!(
                "{} differs at byte {}",
                path.display(),
                offset + i as u64
            ));
        }
        offset += n as u64;
    }
}
