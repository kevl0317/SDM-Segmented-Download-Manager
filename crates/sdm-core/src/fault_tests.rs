//! The engine against everything a server or network can do to it: each test
//! asks the fault server for one misbehavior and checks what SDM ends up with,
//! byte for byte. A test that hangs fails at `DEADLINE` rather than blocking.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sdm_testkit::{verify, FaultServer};

use crate::engine::{Download, STATE_SUFFIX, WORKING_SUFFIX};
use crate::manager::Manager;
use crate::types::Status;
use crate::util::{extract_download_url, filename_from_url, sanitize_filename};

const MIB: u64 = 1 << 20;
const DEADLINE: Duration = Duration::from_secs(60);
/// How soon Pause must take effect, whatever the connection is doing.
const PROMPT: Duration = Duration::from_secs(3);

struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("sdm-fault-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
    fn assert_no_temp_files(&self) {
        let left: Vec<String> = self
            .names()
            .into_iter()
            .filter(|n| n.ends_with(WORKING_SUFFIX) || n.ends_with(STATE_SUFFIX) || n.ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "left behind: {left:?}");
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn job(url: &str, dir: &Dir) -> Download {
    Download::new(url, dir.path(), None, 8, None)
}

async fn finish(d: &Download) {
    if tokio::time::timeout(DEADLINE, d.run()).await.is_err() {
        panic!("hung in {} for {DEADLINE:?}", d.status().as_str());
    }
}

fn start(d: &Download) -> tokio::task::JoinHandle<()> {
    let d = d.clone();
    tokio::spawn(async move { d.run().await })
}

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    let begun = Instant::now();
    while !cond() {
        assert!(begun.elapsed() < DEADLINE, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn joined(h: tokio::task::JoinHandle<()>, within: Duration, what: &str) {
    tokio::time::timeout(within, h)
        .await
        .unwrap_or_else(|_| panic!("{what}: still going after {within:?}"))
        .unwrap();
}

fn assert_done(d: &Download) {
    assert_eq!(d.status(), Status::Complete, "error: {:?}", d.error());
}

fn assert_failed(d: &Download) -> String {
    assert_eq!(d.status(), Status::Error, "expected a failure");
    let e = d.error().expect("a failure has a message");
    assert!(!e.is_empty());
    assert!(!e.contains("http://") && !e.contains("https://"), "message shows a URL: {e}");
    e
}

// ---- succeeds -----------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ok_ranged_download_is_byte_exact() {
    let (s, t) = (FaultServer::start(), Dir::new("ok-ranged"));
    let size = 8 * MIB + 12_345;
    let d = job(&s.url(&format!("/a.zip?size={size}&seed=7")), &t);
    finish(&d).await;
    assert_done(&d);
    assert_eq!(d.size(), Some(size));
    assert!(d.supports_ranges());
    assert_eq!(d.segment_count(), 8);
    assert_eq!(d.downloaded(), size);
    assert_eq!(d.speed(), 0);
    verify(&t.file("a.zip"), 7, size).unwrap();
    t.assert_no_temp_files();
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_small_file_uses_one_segment() {
    let (s, t) = (FaultServer::start(), Dir::new("ok-small"));
    let d = job(&s.url("/s.bin?size=1000"), &t);
    finish(&d).await;
    assert_done(&d);
    assert_eq!(d.segment_count(), 1);
    verify(&t.file("s.bin"), 1, 1000).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_without_ranges_streams_the_whole_file() {
    let (s, t) = (FaultServer::start(), Dir::new("ok-stream"));
    let d = job(&s.url(&format!("/s.bin?size={}&ranges=0", 3 * MIB)), &t);
    finish(&d).await;
    assert_done(&d);
    assert!(!d.supports_ranges());
    assert_eq!(d.progress(), Some(1.0));
    verify(&t.file("s.bin"), 1, 3 * MIB).unwrap();
    t.assert_no_temp_files();
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_unknown_size_chunked_learns_the_size() {
    let (s, t) = (FaultServer::start(), Dir::new("ok-chunked"));
    let d = job(&s.url(&format!("/c.bin?size={}&ranges=0&chunked=1", 2 * MIB)), &t);
    finish(&d).await;
    assert_done(&d);
    assert_eq!(d.size(), Some(2 * MIB));
    verify(&t.file("c.bin"), 1, 2 * MIB).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_unknown_size_close_delimited() {
    let (s, t) = (FaultServer::start(), Dir::new("ok-close"));
    let d = job(&s.url(&format!("/c.bin?size={}&ranges=0&len=0", MIB)), &t);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("c.bin"), 1, MIB).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_empty_file() {
    // Many servers answer a range on an empty file with 416; others with a 200.
    for (i, query) in ["size=0", "size=0&ranges=0", "size=0&ranges=0&chunked=1"].iter().enumerate() {
        let (s, t) = (FaultServer::start(), Dir::new(&format!("ok-empty{i}")));
        let d = job(&s.url(&format!("/empty.txt?{query}")), &t);
        finish(&d).await;
        assert_done(&d);
        verify(&t.file("empty.txt"), 1, 0).unwrap();
        t.assert_no_temp_files();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_sizes_around_the_segment_boundaries() {
    let s = FaultServer::start();
    for (i, size) in [1, 2, MIB - 1, MIB, MIB + 1, 2 * MIB, 8 * MIB - 1, 8 * MIB, 8 * MIB + 1]
        .into_iter()
        .enumerate()
    {
        let t = Dir::new(&format!("ok-size{i}"));
        let d = job(&s.url(&format!("/b.bin?size={size}&seed={i}")), &t);
        finish(&d).await;
        assert_done(&d);
        verify(&t.file("b.bin"), i as u64, size).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_any_number_of_connections() {
    let s = FaultServer::start();
    for conns in [1, 2, 3, 16, 64] {
        let t = Dir::new(&format!("ok-conns{conns}"));
        let url = s.url(&format!("/n.bin?size={}&seed={conns}", 5 * MIB));
        let d = Download::new(&url, t.path(), None, conns, None);
        finish(&d).await;
        assert_done(&d);
        verify(&t.file("n.bin"), conns as u64, 5 * MIB).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_redirects_are_followed() {
    let s = FaultServer::start();
    for code in [301, 302, 307] {
        let t = Dir::new(&format!("ok-redirect{code}"));
        let to = format!("/real.zip%3Fsize%3D{}", 2 * MIB);
        let d = job(&s.url(&format!("/redirect?code={code}&to={to}")), &t);
        finish(&d).await;
        assert_done(&d);
        verify(&t.file("real.zip"), 1, 2 * MIB).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_idle_connections_help_a_slow_one() {
    let (s, t) = (FaultServer::start(), Dir::new("ok-steal"));
    let size = 16 * MIB;
    // The connection serving the start of the file crawls; the other is fast.
    let url = s.url(&format!("/w.bin?size={size}&slow_start=0&slow_rate=1000000"));
    let d = Download::new(&url, t.path(), None, 2, None);
    finish(&d).await;
    assert_done(&d);
    assert!(d.segment_count() > 2, "the fast connection should have split the slow one's work");
    verify(&t.file("w.bin"), 1, size).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_headers_reach_every_request() {
    let (s, t) = (FaultServer::start(), Dir::new("ok-headers"));
    let headers = vec![
        ("User-Agent".to_string(), "BrowserUA".to_string()),
        ("Cookie".to_string(), "session=abc".to_string()),
        ("Referer".to_string(), "https://site.example/page".to_string()),
    ];
    let url = s.url(&format!(
        "/h.bin?size={}&need_cookie=session%3Dabc&need_referer=site.example&need_ua=BrowserUA",
        4 * MIB
    ));
    let d = Download::new(&url, t.path(), None, 8, Some(headers));
    finish(&d).await;
    assert_done(&d);
    assert!(s.request_count() > 2);
    verify(&t.file("h.bin"), 1, 4 * MIB).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_cookies_stay_with_their_own_host() {
    let (a, b, t) = (FaultServer::start(), FaultServer::start(), Dir::new("ok-cookie-host"));
    let headers = vec![("Cookie".to_string(), "session=secret".to_string())];
    let to = b.url("/x.bin").replace(':', "%3A").replace('/', "%2F");
    let url = a.url(&format!("/redirect?need_cookie=session&to={to}"));
    let d = Download::new(&url, t.path(), None, 2, Some(headers));
    finish(&d).await;
    assert_done(&d);
    assert!(a.log().iter().any(|r| r.cookie.is_some()));
    assert!(b.log().iter().all(|r| r.cookie.is_none()), "cookie leaked to another host");
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_sites_that_refuse_fake_browsers_get_a_plain_client() {
    // Cloudflare's bot check, as met on SourceForge: claiming to be Chrome without
    // shaking hands like Chrome gets a 403, while plain tools pass.
    let (s, t) = (FaultServer::start(), Dir::new("ok-agent"));
    let size = 8 * MIB;
    let url = s.url(&format!("/a.zip?size={size}&block_browsers=1&rate=2000000"));
    let d = job(&url, &t);
    let h = start(&d);
    until("some bytes", || d.downloaded() > MIB).await;
    d.pause();
    joined(h, PROMPT, "pause").await;
    let name = Download::suggested_name(&url, None).await;
    assert_eq!(name.as_deref(), Some("a.zip"), "the popup's question gets through too");

    // A restart forgets the agent; asking again finds it again.
    let again = Download::from_state(&d.state_path()).unwrap();
    finish(&again).await;
    assert_done(&again);
    verify(&t.file("a.zip"), 1, size).unwrap();
    assert!(s.log().last().unwrap().user_agent.as_deref().unwrap().starts_with("SDM/"));
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_a_stream_from_a_site_that_refuses_fake_browsers() {
    let (s, t) = (FaultServer::start(), Dir::new("ok-agent-stream"));
    let d = job(&s.url(&format!("/a.zip?size={}&block_browsers=1&ranges=0", MIB)), &t);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("a.zip"), 1, MIB).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_a_real_refusal_still_fails_quickly() {
    // Trying again as a plain client is one extra request, not a loop.
    let (s, t) = (FaultServer::start(), Dir::new("fail-403-once"));
    let d = job(&s.url("/a.zip?status=403"), &t);
    finish(&d).await;
    assert!(assert_failed(&d).contains("403"));
    assert!(s.request_count() <= 3, "{} requests", s.request_count());
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_running_a_finished_job_changes_nothing() {
    let (s, t) = (FaultServer::start(), Dir::new("ok-rerun"));
    let d = job(&s.url("/r.bin?size=5000"), &t);
    finish(&d).await;
    let requests = s.request_count();
    finish(&d).await;
    assert_done(&d);
    assert_eq!(s.request_count(), requests);
    assert_eq!(t.names(), vec!["r.bin"]);
}

// ---- names --------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn name_github_release_link_gets_the_real_name() {
    // What the user hit: the release link redirects to a signed link whose last
    // segment is random; only Content-Disposition (and the query) have the name.
    let (s, t) = (FaultServer::start(), Dir::new("name-github"));
    let name = "v2rayN-windows-64.zip";
    let cd = format!("attachment%3B%20filename%3D{name}");
    let d = job(&s.url(&format!("/release/{name}?size={}&cd={cd}", 2 * MIB)), &t);
    finish(&d).await;
    assert_done(&d);
    assert_eq!(d.filename(), name);
    verify(&t.file(name), 1, 2 * MIB).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn name_signed_link_handed_over_directly() {
    // The extension passes the link after redirects, with no name.
    let (s, t) = (FaultServer::start(), Dir::new("name-signed"));
    let far = 4_000_000_000_000u64;
    let url = s.url(&format!(
        "/signed/{far}/f804704f-4e35-4e59-840d-ac5f65c70c79?rscd=attachment%3B+filename%3Dreal.exe&size=5000"
    ));
    let d = job(&url, &t);
    finish(&d).await;
    assert_done(&d);
    assert_eq!(d.filename(), "real.exe");
    verify(&t.file("real.exe"), 1, 5000).unwrap();
}

#[test]
fn name_from_url_reads_the_query_hints() {
    assert_eq!(
        filename_from_url(
            "https://release-assets.githubusercontent.com/github-production-release-asset/1/\
             f804704f-4e35?sp=r&rscd=attachment%3B+filename%3Dv2rayN.zip&rsct=application%2Foctet-stream"
        ),
        "v2rayN.zip"
    );
    assert_eq!(
        filename_from_url(
            "https://bucket.s3.amazonaws.com/abc?response-content-disposition=attachment%3B%20filename%3D%22a%20b.exe%22&X=1"
        ),
        "a b.exe"
    );
    assert_eq!(filename_from_url("https://x.com/dl/plain.zip?id=3"), "plain.zip");
}

#[tokio::test(flavor = "multi_thread")]
async fn name_from_content_disposition() {
    let s = FaultServer::start();
    for (i, (header, want)) in [
        ("attachment; filename=\"real name.zip\"", "real name.zip"),
        ("attachment; filename=plain.exe; size=3", "plain.exe"),
        ("attachment; filename*=UTF-8''%E7%8E%8B%E6%A7%98.rar", "\u{738B}\u{69D8}.rar"),
        ("attachment; filename=\"..\\..\\evil.exe\"", "_.._evil.exe"),
        ("attachment; filename=\"../../evil.exe\"", "_.._evil.exe"),
    ]
    .iter()
    .enumerate()
    {
        let t = Dir::new(&format!("name-cd{i}"));
        let cd: String = header
            .bytes()
            .map(|b| match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'_' | b'-' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect();
        let d = job(&s.url(&format!("/dl?size=2000&seed={i}&cd={cd}")), &t);
        finish(&d).await;
        assert_done(&d);
        assert_eq!(d.filename(), *want, "for {header}");
        verify(&t.file(want), i as u64, 2000).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn name_gets_an_extension_from_the_content_type() {
    let (s, t) = (FaultServer::start(), Dir::new("name-ctype"));
    let d = job(&s.url("/download/12345?size=2000&ctype=application%2Fzip"), &t);
    finish(&d).await;
    assert_done(&d);
    assert_eq!(d.filename(), "12345.zip");
    verify(&t.file("12345.zip"), 1, 2000).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn name_collision_keeps_both_and_reports_the_real_name() {
    let (s, t) = (FaultServer::start(), Dir::new("name-collision"));
    std::fs::write(t.file("a.zip"), b"mine").unwrap();
    let d = job(&s.url("/a.zip?size=3000"), &t);
    finish(&d).await;
    assert_done(&d);
    assert_eq!(d.filename(), "a (1).zip");
    assert_eq!(d.output_path(), t.file("a (1).zip"));
    assert_eq!(std::fs::read(t.file("a.zip")).unwrap(), b"mine");
    verify(&t.file("a (1).zip"), 1, 3000).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn name_windows_reserved_names_still_save() {
    let s = FaultServer::start();
    for (i, name) in ["CON", "nul.txt", "COM1.zip", "aux"].iter().enumerate() {
        let t = Dir::new(&format!("name-reserved{i}"));
        let d = job(&s.url(&format!("/{name}?size=1000")), &t);
        finish(&d).await;
        assert_done(&d);
        verify(&d.output_path(), 1, 1000).unwrap();
        assert!(t.names().len() == 1, "{:?}", t.names());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn name_too_long_for_the_filesystem_still_saves() {
    let (s, t) = (FaultServer::start(), Dir::new("name-long"));
    let long = format!("{}.zip", "a".repeat(400));
    let d = job(&s.url(&format!("/{long}?size=1000")), &t);
    finish(&d).await;
    assert_done(&d);
    assert!(d.filename().ends_with(".zip"), "keeps its type: {}", d.filename());
    verify(&d.output_path(), 1, 1000).unwrap();
}

#[test]
fn name_sanitizing() {
    assert_eq!(sanitize_filename("a/b:c*?.txt"), "a_b_c__.txt");
    assert_eq!(sanitize_filename("  spaced.zip  "), "spaced.zip");
    assert_eq!(sanitize_filename("trailing.dots..."), "trailing.dots");
    assert_eq!(sanitize_filename("..."), "download");
    assert_eq!(sanitize_filename(""), "download");
    assert_eq!(sanitize_filename("%2e%2e%2fetc"), "_etc");
    assert_eq!(sanitize_filename("tab\there"), "tab_here");
    assert_eq!(sanitize_filename("\u{738B}\u{69D8}.rar"), "\u{738B}\u{69D8}.rar");
}

#[test]
fn catch_filter() {
    assert!(extract_download_url("https://x.com/a.zip?sig=1", false).is_some());
    assert!(extract_download_url("https://x.com/A.ZIP", false).is_some());
    assert!(extract_download_url("https://x.com/page.html", false).is_none());
    assert!(extract_download_url("https://x.com/page.html", true).is_some());
    assert!(extract_download_url("mailto:a@b.c", true).is_none());
    assert!(extract_download_url(&format!("https://x.com/{}.zip", "a".repeat(5000)), false).is_none());
    // Signed links name the file in the query, not the path.
    assert!(extract_download_url("https://x.com/f8047?rscd=attachment%3B+filename%3Dv.zip", false).is_some());
}

// ---- fails, and says why ------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn fail_refusals_are_fast_and_explained() {
    let s = FaultServer::start();
    for (status, words) in [
        (401, "sign-in"),
        (403, "sign-in"),
        (404, "no longer there"),
        (410, "no longer there"),
        (500, "500"),
    ] {
        let t = Dir::new(&format!("fail-refused{status}"));
        let d = job(&s.url(&format!("/x{status}.zip?status={status}")), &t);
        let begun = Instant::now();
        finish(&d).await;
        let e = assert_failed(&d);
        assert!(e.contains(&status.to_string()), "{e}");
        assert!(e.contains(words), "{e}");
        assert!(begun.elapsed() < Duration::from_secs(5), "took {:?}", begun.elapsed());
        assert!(t.names().is_empty(), "a refused download leaves nothing: {:?}", t.names());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_refused_segments_stop_quickly() {
    let (s, t) = (FaultServer::start(), Dir::new("fail-seg-refused"));
    // The probe works; every download request is refused.
    let d = job(&s.url(&format!("/f.zip?size={}&late_after=1&late_status=403", 8 * MIB)), &t);
    let begun = Instant::now();
    finish(&d).await;
    let e = assert_failed(&d);
    assert!(e.contains("403"), "{e}");
    assert!(begun.elapsed() < Duration::from_secs(5), "took {:?}", begun.elapsed());
    // The first contact, one try per connection, and one more as a plain client.
    assert!(s.request_count() <= 1 + 8 + 1, "retried a refusal: {} requests", s.request_count());
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_server_errors_retry_then_give_up() {
    let (s, t) = (FaultServer::start(), Dir::new("fail-503"));
    let d = job(&s.url(&format!("/f.zip?size={}&late_after=1&late_status=503", 8 * MIB)), &t);
    finish(&d).await;
    let e = assert_failed(&d);
    assert!(e.contains("503"), "{e}");
    assert!(s.request_count() > 8, "should have retried");
    assert!(s.request_count() <= 1 + 8 * 6, "too many requests: {}", s.request_count());
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_busy_server_then_success() {
    for (status, query) in [(429, "fail_status=429"), (503, "fail_status=503"), (408, "fail_status=408")] {
        let (s, t) = (FaultServer::start(), Dir::new(&format!("fail-busy{status}")));
        let d = job(&s.url(&format!("/b.zip?size={}&fail_first=3&{query}", 4 * MIB)), &t);
        finish(&d).await;
        assert_done(&d);
        verify(&t.file("b.zip"), 1, 4 * MIB).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_nothing_listening() {
    let port = FaultServer::start().port(); // gone again at the end of this line
    let t = Dir::new("fail-refused-conn");
    let d = job(&format!("http://127.0.0.1:{port}/a.zip"), &t);
    finish(&d).await;
    assert!(assert_failed(&d).contains("Couldn't reach"));
    assert!(t.names().is_empty(), "{:?}", t.names());
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_unknown_host() {
    let t = Dir::new("fail-dns");
    let d = job("http://nonexistent.invalid/a.zip", &t);
    finish(&d).await;
    assert!(assert_failed(&d).contains("Couldn't reach"));
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_links_sdm_cannot_fetch_are_worded_plainly() {
    for (i, link) in ["ftp://host/pub/a.zip", "file:///C:/a.zip", "not a link", "http://", "javascript:alert(1)"]
        .iter()
        .enumerate()
    {
        let t = Dir::new(&format!("fail-link{i}"));
        let d = job(link, &t);
        finish(&d).await;
        assert_eq!(d.status(), Status::Error, "{link}");
        let e = d.error().unwrap();
        assert!(!e.contains("builder"), "{link}: {e}");
        assert!(e.to_lowercase().contains("link"), "{link}: {e}");
        assert!(t.names().is_empty(), "{link}: {:?}", t.names());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_redirect_loop() {
    let (s, t) = (FaultServer::start(), Dir::new("fail-loop"));
    let url = s.url("/redirect?to=/redirect%3Fto%3D/redirect");
    let d = job(&url, &t);
    finish(&d).await;
    let e = assert_failed(&d);
    assert!(e.to_lowercase().contains("redirect"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_https_to_a_plain_server() {
    let (s, t) = (FaultServer::start(), Dir::new("fail-tls"));
    let d = job(&format!("https://127.0.0.1:{}/a.zip", s.port()), &t);
    finish(&d).await;
    assert_failed(&d);
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_server_never_answers() {
    let (s, t) = (FaultServer::start(), Dir::new("fail-hang"));
    let d = job(&s.url("/h.zip?hang=1&fault_probe=1"), &t);
    finish(&d).await;
    assert!(assert_failed(&d).contains("too long"));
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_download_folder_is_not_a_folder() {
    let (s, t) = (FaultServer::start(), Dir::new("fail-dest"));
    std::fs::write(t.file("file"), b"x").unwrap();
    let url = s.url(&format!("/a.zip?size={}", 2 * MIB));
    let d = Download::new(&url, t.file("file").join("sub"), None, 4, None);
    finish(&d).await;
    let e = assert_failed(&d);
    assert!(e.to_lowercase().contains("folder") || e.to_lowercase().contains("save"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_file_bigger_than_the_drive_can_hold() {
    let (s, t) = (FaultServer::start(), Dir::new("fail-toobig"));
    let d = job(&s.url("/huge.iso?size=9000000000000000"), &t);
    finish(&d).await;
    let e = assert_failed(&d);
    assert!(e.contains("too big") || e.contains("disk space"), "{e}");
    t.assert_no_temp_files();
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_not_enough_disk_space() {
    // Space is reserved up front, so a full disk shows at the start, not 90% in.
    let (s, t) = (FaultServer::start(), Dir::new("fail-space"));
    let d = job(&s.url("/huge.iso?size=10000000000000"), &t); // 10 TB
    finish(&d).await;
    assert!(assert_failed(&d).contains("disk space"));
    t.assert_no_temp_files();
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_body_shorter_than_promised() {
    let (s, t) = (FaultServer::start(), Dir::new("fail-short"));
    let d = job(&s.url("/s.zip?size=5000&ranges=0&drop_after=1000"), &t);
    finish(&d).await;
    assert_failed(&d);
    assert!(!t.file("s.zip").exists(), "a truncated file must not look finished");
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_range_ignored_midway_never_corrupts() {
    // The size probe gets a range; later requests get the whole file from byte 0.
    let (s, t) = (FaultServer::start(), Dir::new("fail-plain"));
    let d = job(&s.url(&format!("/p.zip?size={}&plain_after=1", 4 * MIB)), &t);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("p.zip"), 1, 4 * MIB).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_advertised_ranges_that_do_not_work_never_corrupt() {
    let (s, t) = (FaultServer::start(), Dir::new("fail-advert"));
    let d = job(&s.url(&format!("/p.zip?size={}&ranges=0&accept=1", 4 * MIB)), &t);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("p.zip"), 1, 4 * MIB).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_wrong_content_range_never_corrupts() {
    let (s, t) = (FaultServer::start(), Dir::new("fail-shift"));
    let d = job(&s.url(&format!("/p.zip?size={}&shift_range=1000", 4 * MIB)), &t);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("p.zip"), 1, 4 * MIB).unwrap();
}

// ---- disconnects --------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn drop_recovers_from_a_closed_connection() {
    let (s, t) = (FaultServer::start(), Dir::new("drop-once"));
    let d = job(&s.url(&format!("/d.zip?size={}&drop_after=100000&drop_times=3", 4 * MIB)), &t);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("d.zip"), 1, 4 * MIB).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_recovers_from_a_reset_connection() {
    let (s, t) = (FaultServer::start(), Dir::new("drop-rst"));
    let d = job(&s.url(&format!("/d.zip?size={}&drop_after=100000&drop_times=3&rst=1", 4 * MIB)), &t);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("d.zip"), 1, 4 * MIB).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_every_connection_still_finishes_while_bytes_arrive() {
    let (s, t) = (FaultServer::start(), Dir::new("drop-always"));
    let d = job(&s.url(&format!("/d.zip?size={}&drop_after=200000", 4 * MIB)), &t);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("d.zip"), 1, 4 * MIB).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_with_no_bytes_at_all_fails_instead_of_spinning() {
    let (s, t) = (FaultServer::start(), Dir::new("drop-spin"));
    let d = job(&s.url(&format!("/d.zip?size={}&drop_after=0", 4 * MIB)), &t);
    finish(&d).await;
    assert_failed(&d);
    assert!(s.connections() < 100, "spun through {} connections", s.connections());
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_network_gone_fails_then_resume_continues() {
    let (s, t) = (FaultServer::start(), Dir::new("drop-down"));
    let size = 16 * MIB;
    let d = job(&s.url(&format!("/d.zip?size={size}&rate=2000000")), &t);
    let h = start(&d);
    until("some bytes", || d.downloaded() > MIB).await;
    s.go_down();
    joined(h, DEADLINE, "after the network went down").await;
    assert!(assert_failed(&d).contains("Couldn't reach"));
    let kept = d.downloaded();
    assert!(kept > MIB, "progress must survive a failure");
    assert!(d.state_path().exists(), "a failed download can resume");

    s.come_back();
    let d2 = Download::from_state(&d.state_path()).unwrap();
    assert_eq!(d2.downloaded(), kept);
    finish(&d2).await;
    assert_done(&d2);
    verify(&t.file("d.zip"), 1, size).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_stalled_connection_is_replaced() {
    let (s, t) = (FaultServer::start(), Dir::new("drop-stall"));
    let d = job(&s.url(&format!("/d.zip?size={}&stall_after=100000&stall_times=2", 4 * MIB)), &t);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("d.zip"), 1, 4 * MIB).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_silent_network_fails_with_a_reason() {
    let (s, t) = (FaultServer::start(), Dir::new("drop-silent"));
    let d = job(&s.url(&format!("/d.zip?size={}&rate=2000000", 16 * MIB)), &t);
    let h = start(&d);
    until("some bytes", || d.downloaded() > MIB).await;
    s.go_silent();
    joined(h, DEADLINE, "after the network went silent").await;
    assert_failed(&d);
    assert!(d.downloaded() > MIB);
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_stream_without_ranges_fails_then_resume_restarts() {
    let (s, t) = (FaultServer::start(), Dir::new("drop-stream"));
    let d = job(&s.url(&format!("/s.zip?size={}&ranges=0&drop_after=100000&drop_times=1", 2 * MIB)), &t);
    finish(&d).await;
    assert_failed(&d);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("s.zip"), 1, 2 * MIB).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_stream_stall_times_out() {
    let (s, t) = (FaultServer::start(), Dir::new("drop-stream-stall"));
    let d = job(&s.url(&format!("/s.zip?size={}&ranges=0&stall_after=1000", 2 * MIB)), &t);
    finish(&d).await;
    assert_failed(&d);
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_connection_limit_still_finishes() {
    // Servers that allow only a few connections per client refuse the rest.
    for (i, status) in [503, 403, 429].into_iter().enumerate() {
        let (s, t) = (FaultServer::start(), Dir::new(&format!("drop-limit{i}")));
        let size = 16 * MIB;
        let url = s.url(&format!("/l.zip?size={size}&limit=2&limit_status={status}&rate=8000000"));
        let d = job(&url, &t);
        finish(&d).await;
        assert_done(&d);
        verify(&t.file("l.zip"), 1, size).unwrap();
    }
}

// ---- pause ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn pause_and_resume_is_byte_exact() {
    let (s, t) = (FaultServer::start(), Dir::new("pause-basic"));
    let size = 16 * MIB;
    let d = job(&s.url(&format!("/p.zip?size={size}&rate=2000000")), &t);
    let h = start(&d);
    until("some bytes", || d.downloaded() > MIB).await;
    d.pause();
    joined(h, PROMPT, "pause").await;
    assert_eq!(d.status(), Status::Paused);
    assert_eq!(d.speed(), 0);
    assert!(d.state_path().exists());
    assert!(d.working_path().exists());
    let at = d.downloaded();
    assert!(at > MIB && at < size);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("p.zip"), 1, size).unwrap();
    t.assert_no_temp_files();
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_many_times_is_byte_exact() {
    let (s, t) = (FaultServer::start(), Dir::new("pause-many"));
    let size = 8 * MIB;
    let d = job(&s.url(&format!("/p.zip?size={size}&rate=4000000")), &t);
    for _ in 0..6 {
        let h = start(&d);
        tokio::time::sleep(Duration::from_millis(120)).await;
        d.pause();
        joined(h, PROMPT, "pause").await;
        if d.status() == Status::Complete {
            break;
        }
        assert_eq!(d.status(), Status::Paused);
    }
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("p.zip"), 1, size).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_then_restart_resumes_from_the_sidecar() {
    let (s, t) = (FaultServer::start(), Dir::new("pause-restart"));
    let size = 16 * MIB;
    let d = job(&s.url(&format!("/p.zip?size={size}&rate=2000000")), &t);
    let h = start(&d);
    until("some bytes", || d.downloaded() > 2 * MIB).await;
    d.pause();
    joined(h, PROMPT, "pause").await;
    let at = d.downloaded();

    let again = Download::from_state(&d.state_path()).unwrap();
    assert_eq!(again.status(), Status::Paused);
    assert_eq!(again.downloaded(), at);
    assert_eq!(again.size(), Some(size));
    let before = s.log().len();
    finish(&again).await;
    assert_done(&again);
    verify(&t.file("p.zip"), 1, size).unwrap();
    // It really resumed: no request re-fetched the file from the start.
    let starts_over = s.log()[before..]
        .iter()
        .filter(|r| r.range.as_deref().is_some_and(|v| v.starts_with("bytes=0-")))
        .count();
    assert_eq!(starts_over, 0, "resume went back to byte 0");
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_after_connections_split_work() {
    let (s, t) = (FaultServer::start(), Dir::new("pause-split"));
    let size = 16 * MIB;
    let url = s.url(&format!("/w.bin?size={size}&slow_start=0&slow_rate=500000"));
    let d = Download::new(&url, t.path(), None, 2, None);
    let h = start(&d);
    until("a split", || d.segment_count() > 2).await;
    d.pause();
    joined(h, PROMPT, "pause").await;
    let again = Download::from_state(&d.state_path()).unwrap();
    assert_eq!(again.segment_count(), d.segment_count());
    finish(&again).await;
    assert_done(&again);
    verify(&t.file("w.bin"), 1, size).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_takes_effect_during_the_first_contact() {
    let (s, t) = (FaultServer::start(), Dir::new("pause-probe"));
    let d = job(&s.url("/h.zip?hang=1&fault_probe=1"), &t);
    let h = start(&d);
    until("the probe", || d.status() == Status::Probing).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    d.pause();
    joined(h, PROMPT, "pause while connecting").await;
    assert_eq!(d.status(), Status::Paused);
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_takes_effect_on_a_stalled_connection() {
    let (s, t) = (FaultServer::start(), Dir::new("pause-stall"));
    let d = job(&s.url(&format!("/d.zip?size={}&stall_after=100000", 4 * MIB)), &t);
    let h = start(&d);
    until("the stall", || d.downloaded() >= 100_000 * 4).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    d.pause();
    joined(h, PROMPT, "pause on a stalled connection").await;
    assert_eq!(d.status(), Status::Paused);
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_takes_effect_while_waiting_to_retry() {
    let (s, t) = (FaultServer::start(), Dir::new("pause-backoff"));
    let d = job(&s.url(&format!("/d.zip?size={}&late_after=1&late_status=503", 8 * MIB)), &t);
    let h = start(&d);
    until("the retries", || s.request_count() > 8).await;
    d.pause();
    joined(h, PROMPT, "pause during a retry wait").await;
    assert_eq!(d.status(), Status::Paused);
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_a_stream_then_resume_restarts_it() {
    let (s, t) = (FaultServer::start(), Dir::new("pause-stream"));
    let size = 4 * MIB;
    let d = job(&s.url(&format!("/s.zip?size={size}&ranges=0&rate=4000000")), &t);
    let h = start(&d);
    until("some bytes", || d.downloaded() > 200_000).await;
    d.pause();
    joined(h, PROMPT, "pause").await;
    assert_eq!(d.status(), Status::Paused);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("s.zip"), 1, size).unwrap();
}

// ---- resume against what changed meanwhile --------------------------------

async fn paused_partway(s: &FaultServer, t: &Dir, query: &str) -> Download {
    let d = job(&s.url(&format!("/p.zip?{query}")), t);
    let h = start(&d);
    until("some bytes", || d.downloaded() > 2 * MIB).await;
    d.pause();
    joined(h, PROMPT, "pause").await;
    d
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_after_the_partial_file_was_deleted_starts_clean() {
    let (s, t) = (FaultServer::start(), Dir::new("resume-deleted"));
    let size = 16 * MIB;
    let d = paused_partway(&s, &t, &format!("size={size}&rate=2000000")).await;
    std::fs::remove_file(d.working_path()).unwrap();
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("p.zip"), 1, size).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_after_the_partial_file_was_truncated_starts_clean() {
    let (s, t) = (FaultServer::start(), Dir::new("resume-truncated"));
    let size = 16 * MIB;
    let d = paused_partway(&s, &t, &format!("size={size}&rate=2000000")).await;
    let f = std::fs::OpenOptions::new().write(true).open(d.working_path()).unwrap();
    f.set_len(size / 2).unwrap();
    drop(f);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("p.zip"), 1, size).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_after_the_server_replaced_the_file_starts_clean() {
    let (s, t) = (FaultServer::start(), Dir::new("resume-replaced"));
    let size = 16 * MIB;
    let d = paused_partway(&s, &t, &format!("size={size}&rate=4000000&etag=1")).await;
    s.replace_files();
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("p.zip"), 2, size).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_after_the_file_changed_size_starts_clean() {
    let (s, t) = (FaultServer::start(), Dir::new("resume-resized"));
    let size = 16 * MIB;
    let new = 5 * MIB + 7;
    let d = paused_partway(&s, &t, &format!("size={size}&new_size={new}&rate=4000000&etag=1")).await;
    s.replace_files();
    finish(&d).await;
    assert_done(&d);
    assert_eq!(d.size(), Some(new));
    verify(&t.file("p.zip"), 2, new).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_after_a_signed_link_expired_gets_a_new_one() {
    // GitHub's signed links last an hour; a paused download must outlive them.
    let (s, t) = (FaultServer::start(), Dir::new("resume-expired"));
    let size = 8 * MIB;
    let url = s.url(&format!("/release/x.zip?size={size}&ttl_ms=1500&rate=1000000"));
    let d = job(&url, &t);
    let h = start(&d);
    until("some bytes", || d.downloaded() > 200_000).await;
    d.pause();
    joined(h, PROMPT, "pause").await;
    assert_eq!(d.url(), url, "the job must remember the link it was given");
    tokio::time::sleep(Duration::from_millis(1600)).await;

    let again = Download::from_state(&d.state_path()).unwrap();
    finish(&again).await;
    assert_done(&again);
    verify(&t.file("x.zip"), 1, size).unwrap();
}

#[test]
fn resume_rejects_a_damaged_sidecar() {
    let t = Dir::new("resume-damaged");
    for (i, text) in [
        "not json",
        "{}",
        r#"{"url":"http://x/a","dest_dir":"d","filename":"a","connections":1,"size":10,"supports_ranges":true,"segments":[[5,2,0]]}"#,
        r#"{"url":"http://x/a","dest_dir":"d","filename":"a","connections":1,"size":10,"supports_ranges":true,"segments":[[0,4,99]]}"#,
        r#"{"url":"http://x/a","dest_dir":"d","filename":"a","connections":1,"size":10,"supports_ranges":true,"segments":[[0,4,0]]}"#,
        r#"{"url":"http://x/a","dest_dir":"d","filename":"a","connections":1,"size":10,"supports_ranges":true,"segments":[[0,5,0],[3,9,0]]}"#,
    ]
    .iter()
    .enumerate()
    {
        let p = t.file(&format!("s{i}.sdm"));
        std::fs::write(&p, text).unwrap();
        assert!(Download::from_state(&p).is_err(), "accepted: {text}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn crash_mid_download_can_resume_from_a_checkpoint() {
    let (s, t) = (FaultServer::start(), Dir::new("crash"));
    let size = 16 * MIB;
    let d = job(&s.url(&format!("/c.zip?size={size}&rate=2000000")), &t);
    let h = start(&d);
    until("a checkpoint", || d.state_path().exists()).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    // The process dies here: nothing gets to save.
    h.abort();
    let _ = h.await;
    let again = Download::from_state(&d.state_path()).unwrap();
    finish(&again).await;
    assert_done(&again);
    verify(&t.file("c.zip"), 1, size).unwrap();
}

// ---- two jobs, one name ----------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn same_name_downloads_do_not_overwrite_each_other() {
    let (s, t) = (FaultServer::start(), Dir::new("same-name"));
    let size = 8 * MIB;
    let a = job(&s.url(&format!("/one/setup.exe?size={size}&seed=11&rate=8000000")), &t);
    let b = job(&s.url(&format!("/two/setup.exe?size={size}&seed=22&rate=8000000")), &t);
    let (ha, hb) = (start(&a), start(&b));
    joined(ha, DEADLINE, "first").await;
    joined(hb, DEADLINE, "second").await;
    assert_done(&a);
    assert_done(&b);
    assert_ne!(a.output_path(), b.output_path());
    verify(&a.output_path(), 11, size).unwrap();
    verify(&b.output_path(), 22, size).unwrap();
    t.assert_no_temp_files();
}

#[tokio::test(flavor = "multi_thread")]
async fn same_link_twice_gives_two_good_files() {
    let (s, t) = (FaultServer::start(), Dir::new("same-link"));
    let size = 4 * MIB;
    let url = s.url(&format!("/same.zip?size={size}&rate=4000000"));
    let (a, b) = (job(&url, &t), job(&url, &t));
    let (ha, hb) = (start(&a), start(&b));
    joined(ha, DEADLINE, "first").await;
    joined(hb, DEADLINE, "second").await;
    verify(&a.output_path(), 1, size).unwrap();
    verify(&b.output_path(), 1, size).unwrap();
    assert_ne!(a.output_path(), b.output_path());
}

#[tokio::test(flavor = "multi_thread")]
async fn same_name_as_a_paused_job_leaves_its_partial_alone() {
    let (s, t) = (FaultServer::start(), Dir::new("same-paused"));
    let size = 16 * MIB;
    let a = paused_partway(&s, &t, &format!("size={size}&seed=11&rate=2000000")).await;
    let b = job(&s.url(&format!("/p.zip?size={size}&seed=22")), &t);
    finish(&b).await;
    assert_done(&b);
    finish(&a).await;
    assert_done(&a);
    assert_ne!(a.output_path(), b.output_path());
    verify(&a.output_path(), 11, size).unwrap();
    verify(&b.output_path(), 22, size).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn running_one_job_twice_at_once_is_safe() {
    let (s, t) = (FaultServer::start(), Dir::new("double-run"));
    let size = 8 * MIB;
    let d = job(&s.url(&format!("/d.zip?size={size}&rate=4000000")), &t);
    let (h1, h2) = (start(&d), start(&d));
    joined(h1, DEADLINE, "first run").await;
    joined(h2, DEADLINE, "second run").await;
    until("the job to end", || !d.is_active()).await;
    assert_done(&d);
    verify(&t.file("d.zip"), 1, size).unwrap();
    assert_eq!(d.downloaded(), size, "counted bytes twice");
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_right_after_a_failure_never_doubles_the_work() {
    // Status says Failed while slower connections may still be winding down.
    let (s, t) = (FaultServer::start(), Dir::new("resume-race"));
    let size = 16 * MIB;
    let url = s.url(&format!("/r.zip?size={size}&rate=2000000&late_after=3&late_status=403"));
    let d = job(&url, &t);
    let h = start(&d);
    until("a failure", || d.status() == Status::Error).await;
    let second = start(&d); // the user clicks Resume at once
    joined(h, DEADLINE, "first run").await;
    joined(second, DEADLINE, "resume").await;
    until("the job to end", || !d.is_active()).await;
    assert!(d.downloaded() <= size, "counted {} of {size} bytes", d.downloaded());
}

// ---- the queue --------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn queue_keeps_every_state_across_a_restart() {
    let (s, t) = (FaultServer::start(), Dir::new("queue-states"));
    let size = 16 * MIB;
    let mut m = Manager::scratch(t.path());

    let done = m.add(&s.url("/done.zip?size=1000"), None, None, 4, None);
    finish(&done).await;
    assert_done(&done);

    let failed = m.add(&s.url("/failed.zip?status=404"), None, None, 4, None);
    finish(&failed).await;
    assert_failed(&failed);

    let paused = m.add(&s.url(&format!("/paused.zip?size={size}&rate=2000000")), None, None, 4, None);
    let h = start(&paused);
    until("some bytes", || paused.downloaded() > MIB).await;
    paused.pause();
    joined(h, PROMPT, "pause").await;
    let at = paused.downloaded();

    m.add(&s.url("/never-started.zip?size=2000"), None, None, 4, None);
    m.save_queue();

    let mut back = Manager::scratch(t.path());
    back.load_queue();
    assert_eq!(back.downloads.len(), 4);
    let by_name = |name: &str| back.downloads.iter().find(|d| d.filename() == name).unwrap().clone();

    assert_eq!(by_name("done.zip").status(), Status::Complete);
    assert_eq!(by_name("done.zip").size(), Some(1000));
    assert_eq!(by_name("failed.zip").status(), Status::Error);
    assert!(by_name("failed.zip").error().unwrap().contains("404"));
    assert_eq!(by_name("paused.zip").status(), Status::Paused);
    assert_eq!(by_name("paused.zip").downloaded(), at);

    // Anything unfinished can be started from where it stands.
    let waiting = by_name("never-started.zip");
    assert_eq!(waiting.status(), Status::Paused, "the app can resume Paused, not Queued");
    finish(&waiting).await;
    assert_done(&waiting);
    finish(&by_name("paused.zip")).await;
    assert_done(&by_name("paused.zip"));
    verify(&t.file("paused.zip"), 1, size).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_finished_file_moved_away_stays_finished() {
    let (s, t) = (FaultServer::start(), Dir::new("queue-moved"));
    let mut m = Manager::scratch(t.path());
    let d = m.add(&s.url("/moved.zip?size=1000"), None, None, 4, None);
    finish(&d).await;
    m.save_queue();
    std::fs::remove_file(t.file("moved.zip")).unwrap();

    let mut back = Manager::scratch(t.path());
    back.load_queue();
    assert_eq!(back.downloads[0].status(), Status::Complete);
    assert_eq!(s.request_count(), 2, "nothing downloads again by itself");
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_after_a_crash_can_still_start() {
    let (s, t) = (FaultServer::start(), Dir::new("queue-crash"));
    let mut m = Manager::scratch(t.path());
    let url = s.url(&format!("/crash.zip?size={}", 2 * MIB));
    let d = m.add(&url, None, None, 4, None);
    m.save_queue(); // listed, but the process died before any sidecar existed
    drop(d);
    drop(m);

    let mut back = Manager::scratch(t.path());
    back.load_queue();
    let d = back.downloads[0].clone();
    assert_eq!(d.status(), Status::Paused);
    finish(&d).await;
    assert_done(&d);
    verify(&t.file("crash.zip"), 1, 2 * MIB).unwrap();
}

#[test]
fn queue_damaged_file_is_survived() {
    let t = Dir::new("queue-damaged");
    for text in ["not json", "[{\"url\":3}]", "{}", ""] {
        std::fs::write(t.file("queue.json"), text).unwrap();
        let mut m = Manager::scratch(t.path());
        m.load_queue();
        assert!(m.downloads.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_running_with_delete_cleans_up() {
    let (s, t) = (FaultServer::start(), Dir::new("remove-running"));
    let mut m = Manager::scratch(t.path());
    let d = m.add(&s.url(&format!("/r.zip?size={}&rate=2000000", 16 * MIB)), None, None, 8, None);
    let h = start(&d);
    until("some bytes", || d.downloaded() > MIB).await;
    m.remove(&d, true);
    assert!(m.downloads.is_empty());
    joined(h, PROMPT, "stop after remove").await;
    until("the data to go", || !d.working_path().exists() && !d.state_path().exists()).await;
    assert!(!t.file("r.zip").exists());
    let left: Vec<String> = t.names().into_iter().filter(|n| n != "queue.json").collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_without_delete_keeps_a_finished_file() {
    let (s, t) = (FaultServer::start(), Dir::new("remove-keep"));
    let mut m = Manager::scratch(t.path());
    let d = m.add(&s.url("/k.zip?size=1000"), None, None, 4, None);
    finish(&d).await;
    m.remove(&d, false);
    verify(&t.file("k.zip"), 1, 1000).unwrap();
}
