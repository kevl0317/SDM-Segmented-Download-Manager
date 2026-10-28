//! Token-guarded loopback IPC: the browser native-messaging host hands caught
//! downloads to the already-running GUI over 127.0.0.1, authenticated by a
//! per-run token; later launches of the GUI ask the first to show itself (or,
//! for the uninstaller, to quit).
//! No traffic interception — just a local hand-off socket.

use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::util::config_dir;

const PORT_FILE: &str = "ipc-port";
const TOKEN_FILE: &str = "ipc-token";
const PID_FILE: &str = "ipc-pid";

/// A download the browser asked SDM to take over, with the context needed to
/// fetch it as the browser would (cookies / User-Agent / referrer).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatchRequest {
    pub url: String,
    #[serde(default)]
    pub referrer: String,
    #[serde(default)]
    pub cookies: String,
    #[serde(default, rename = "userAgent")]
    pub user_agent: String,
    #[serde(default)]
    pub filename: String,
}

/// What the running GUI is asked to do.
#[derive(Debug, Clone)]
pub enum Request {
    /// Offer a download the browser caught.
    Catch(CatchRequest),
    /// SDM was launched again: show the window instead of starting a second copy.
    Show,
    /// Quit (the uninstaller needs SDM.exe free before it can remove it).
    Quit,
}

/// A request that isn't a catch, from another launch of SDM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Op {
    Show,
    Quit,
}

#[derive(Serialize, Deserialize)]
struct Wire {
    token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    op: Option<Op>,
    #[serde(flatten)]
    req: CatchRequest,
}

fn port_path() -> PathBuf {
    config_dir().join(PORT_FILE)
}
fn token_path() -> PathBuf {
    config_dir().join(TOKEN_FILE)
}

fn gen_token() -> String {
    // Two independently-seeded hasher states give ~128 bits from the OS entropy
    // that seeds RandomState — a local guard with no extra dependency.
    let a = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let b = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    format!("{:016x}{:016x}", a, b)
}

/// Read the endpoint (port, token) a running GUI published.
pub fn read_endpoint() -> Option<(u16, String)> {
    let port = std::fs::read_to_string(port_path())
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let token = std::fs::read_to_string(token_path()).ok()?.trim().to_string();
    if token.is_empty() {
        None
    } else {
        Some((port, token))
    }
}

/// The process id of the GUI that published the endpoint, so the browser host
/// can let it take the foreground for a catch (Windows' foreground lock).
pub fn read_pid() -> Option<u32> {
    std::fs::read_to_string(config_dir().join(PID_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Serve requests to `tx` forever. Binds loopback only and publishes the
/// port + token for the host to read.
pub async fn serve(tx: tokio::sync::mpsc::Sender<Request>) -> io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let token = gen_token();
    // The pid goes first, so whenever the endpoint is readable it's current.
    let _ = std::fs::write(config_dir().join(PID_FILE), std::process::id().to_string());
    let _ = std::fs::write(port_path(), port.to_string());
    let _ = std::fs::write(token_path(), &token);
    loop {
        let (sock, _) = match listener.accept().await {
            Ok(x) => x,
            Err(_) => continue,
        };
        let token = token.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            let (r, mut w) = sock.into_split();
            let mut reader = BufReader::new(r);
            let mut line = String::new();
            if reader.read_line(&mut line).await.is_err() {
                return;
            }
            let Ok(wire) = serde_json::from_str::<Wire>(line.trim()) else {
                return;
            };
            if wire.token != token {
                return;
            }
            let request = match wire.op {
                Some(Op::Show) => Request::Show,
                Some(Op::Quit) => Request::Quit,
                None if !wire.req.url.is_empty() => Request::Catch(wire.req),
                None => return,
            };
            let _ = tx.send(request).await;
            let _ = w.write_all(b"{\"ok\":true}\n").await;
        });
    }
}

/// Client side: hand a caught download to the running GUI. Blocking; used by
/// the host.
pub fn forward(req: &CatchRequest) -> io::Result<()> {
    send(None, req.clone())
}

/// Ask an already-running GUI to show its window. True when one answered — the
/// caller is a second launch and should step aside.
pub fn show_running() -> bool {
    send(Some(Op::Show), CatchRequest::default()).is_ok()
}

/// Ask an already-running GUI to quit. True when one answered.
pub fn quit_running() -> bool {
    send(Some(Op::Quit), CatchRequest::default()).is_ok()
}

/// One request to the GUI that published the endpoint; Ok once it acknowledged
/// (a stale endpoint, or another program now on that port, doesn't count).
fn send(op: Option<Op>, req: CatchRequest) -> io::Result<()> {
    let (port, token) = read_endpoint()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "SDM endpoint not published"))?;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut sock = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(1))?;
    sock.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut line = serde_json::to_string(&Wire { token, op, req })
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    sock.write_all(line.as_bytes())?;
    let mut buf = [0u8; 64];
    let n = sock.read(&mut buf)?;
    if std::str::from_utf8(&buf[..n]).is_ok_and(|ack| ack.contains("\"ok\":true")) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SDM did not acknowledge",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_marks_launch_requests_and_leaves_catches_unchanged() {
        let catch = Wire {
            token: "t".into(),
            op: None,
            req: CatchRequest {
                url: "https://example.com/a.zip".into(),
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&catch).unwrap();
        assert!(!json.contains("\"op\""), "catches keep the original shape: {json}");
        let back: Wire = serde_json::from_str(&json).unwrap();
        assert_eq!(back.op, None);
        assert_eq!(back.req.url, "https://example.com/a.zip");

        for op in [Op::Show, Op::Quit] {
            let wire = Wire {
                token: "t".into(),
                op: Some(op),
                req: CatchRequest::default(),
            };
            let back: Wire = serde_json::from_str(&serde_json::to_string(&wire).unwrap()).unwrap();
            assert_eq!(back.op, Some(op));
            assert!(back.req.url.is_empty());
        }
    }
}
