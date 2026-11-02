//! Formatting, filename, and path helpers (clean-room ports of the originals).

use std::path::PathBuf;

/// Browser-like UA for maximum server compatibility; override per download.
pub const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/124.0 Safari/537.36 SDM/2.0";

/// Default request headers. `Accept-Encoding: identity` is essential — a gzip'd
/// range would break byte offsets, and we need raw bytes at known positions.
pub fn default_headers() -> Vec<(String, String)> {
    vec![
        ("User-Agent".into(), DEFAULT_UA.into()),
        ("Accept".into(), "*/*".into()),
        ("Accept-Encoding".into(), "identity".into()),
    ]
}

fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `~/.sdm` (or `$SDM_CONFIG_DIR`), created if missing. Holds the queue, the
/// native-host manifest and the IPC endpoint files. The override keeps dev and
/// test runs off a real profile.
pub fn config_dir() -> PathBuf {
    let d = std::env::var_os("SDM_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".sdm"));
    let _ = std::fs::create_dir_all(&d);
    d
}

/// `~/Downloads` (or `$SDM_DOWNLOAD_DIR`, for dev and test runs).
pub fn default_download_dir() -> PathBuf {
    std::env::var_os("SDM_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join("Downloads"))
}

/// Human-readable byte count: `123B`, `1.5KB`, `2.0GB`. `None` renders as `?`.
pub fn human_bytes(n: Option<u64>) -> String {
    let n = match n {
        None => return "?".into(),
        Some(n) => n,
    };
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    for (i, u) in units.iter().enumerate() {
        if v < 1024.0 || i == units.len() - 1 {
            if *u == "B" {
                return format!("{}B", n);
            }
            return format!("{:.1}{}", v, u);
        }
        v /= 1024.0;
    }
    format!("{:.1}TB", v)
}

/// `H:MM:SS` or `MM:SS`; `None` renders as `--:--`.
pub fn human_time(secs: Option<f64>) -> String {
    let s = match secs {
        None => return "--:--".into(),
        Some(s) => s as i64,
    };
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, sec)
    } else {
        format!("{:02}:{:02}", m, sec)
    }
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Decode `%XX` escapes; invalid sequences pass through untouched.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hexval(bytes[i + 1]), hexval(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Room for the longest name a filesystem takes (255 UTF-16 units) minus what
/// SDM adds while working: ".sdmdownload.sdm.tmp", or " (99)" for a duplicate.
const MAX_NAME: usize = 230;

/// Strip path separators / illegal filename chars; never returns empty.
pub fn sanitize_filename(name: &str) -> String {
    let decoded = percent_decode(name);
    let trimmed = decoded.trim().trim_matches('.');
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        match ch {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => out.push('_'),
            c if (c as u32) < 0x20 => out.push('_'),
            c => out.push(c),
        }
    }
    let out = out.trim().trim_matches('.').to_string();
    if out.is_empty() {
        return "download".into();
    }
    shorten(&not_a_device(out))
}

/// Windows reserves these names, with any extension, for devices.
fn not_a_device(name: String) -> String {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        format!("_{name}")
    } else {
        name
    }
}

/// Cut an over-long name down to fit, keeping its extension.
fn shorten(name: &str) -> String {
    if name.encode_utf16().count() <= MAX_NAME {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if ext.len() <= 16 => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    let mut budget = MAX_NAME - ext.encode_utf16().count();
    let mut out = String::new();
    for c in stem.chars() {
        if c.len_utf16() > budget {
            break;
        }
        budget -= c.len_utf16();
        out.push(c);
    }
    out.push_str(&ext);
    out
}

fn has_extension(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && (1..=8).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

/// The extension for a server-declared type, for names that came without one.
fn extension_for_type(content_type: &str) -> Option<&'static str> {
    let essence = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    Some(match essence.as_str() {
        "application/zip" | "application/x-zip-compressed" => "zip",
        "application/x-7z-compressed" => "7z",
        "application/vnd.rar" | "application/x-rar-compressed" => "rar",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-tar" => "tar",
        "application/x-msdownload" | "application/x-msdos-program" | "application/x-dosexec"
        | "application/vnd.microsoft.portable-executable" => "exe",
        "application/x-msi" | "application/x-ms-installer" => "msi",
        "application/vnd.android.package-archive" => "apk",
        "application/x-iso9660-image" => "iso",
        "application/pdf" => "pdf",
        "video/mp4" => "mp4",
        "video/x-matroska" => "mkv",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "audio/mpeg" => "mp3",
        "audio/flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "text/html" => "html",
        "text/plain" => "txt",
        _ => return None,
    })
}

/// A filename the link itself carries in its query. Signed storage links (S3,
/// Azure — GitHub releases among them) end in a random id and name the file here.
fn filename_hint(url: &reqwest::Url) -> Option<String> {
    url.query_pairs().find_map(|(key, value)| {
        match key.to_ascii_lowercase().as_str() {
            "rscd" | "response-content-disposition" => parse_content_disposition(&value),
            _ => None,
        }
    })
}

fn last_segment(url: &reqwest::Url) -> Option<String> {
    url.path_segments()?
        .last()
        .filter(|s| !s.is_empty())
        .map(sanitize_filename)
}

/// What to call a download once the server has answered: its stated name first,
/// then what the links say. A redirect's target is often a random blob id, so
/// whichever link's last segment looks like a file wins over one that doesn't.
pub(crate) fn resolve_filename(
    link: &str,
    final_url: &str,
    disposition: Option<&str>,
    content_type: Option<&str>,
) -> String {
    let link = reqwest::Url::parse(link).ok();
    let last = reqwest::Url::parse(final_url).ok();
    let name = disposition
        .and_then(parse_content_disposition)
        .or_else(|| last.as_ref().and_then(filename_hint))
        .or_else(|| link.as_ref().and_then(filename_hint))
        .unwrap_or_else(|| {
            let from_last = last.as_ref().and_then(last_segment);
            let from_link = link.as_ref().and_then(last_segment);
            match (from_last, from_link) {
                (Some(a), _) if has_extension(&a) => a,
                (_, Some(b)) if has_extension(&b) => b,
                (Some(a), _) => a,
                (_, Some(b)) => b,
                _ => "download".to_string(),
            }
        });
    if has_extension(&name) {
        return name;
    }
    match content_type.and_then(extension_for_type) {
        Some(ext) => format!("{name}.{ext}"),
        None => name,
    }
}

/// Pull a filename out of a `Content-Disposition` header value.
pub fn parse_content_disposition(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    // RFC 5987: filename*=UTF-8''<percent-encoded>
    if let Some(p) = lower.find("filename*") {
        if let Some(eq) = value[p..].find('=') {
            let after = &value[p + eq + 1..];
            if let Some(q) = after.find("''") {
                let v = after[q + 2..].split(';').next().unwrap_or("").trim();
                if !v.is_empty() {
                    return Some(sanitize_filename(v));
                }
            }
        }
    }
    // filename="quoted" or filename=bare
    if let Some(p) = lower.find("filename") {
        let after = value[p + "filename".len()..].trim_start();
        if let Some(after) = after.strip_prefix('=') {
            let after = after.trim_start();
            if let Some(rest) = after.strip_prefix('"') {
                if let Some(end) = rest.find('"') {
                    return Some(sanitize_filename(&rest[..end]));
                }
            }
            let v = after.split(';').next().unwrap_or("").trim();
            if !v.is_empty() {
                return Some(sanitize_filename(v));
            }
        }
    }
    None
}

/// The best name a URL alone gives: the one its query promises, or its last path
/// segment, sanitized; falls back to `download`.
pub fn filename_from_url(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| filename_hint(&u).or_else(|| last_segment(&u)))
        .unwrap_or_else(|| "download".into())
}

/// File types worth catching when a link is copied, so ordinary page links
/// don't trigger a prompt. Mirrors the original clipboard filter.
pub const DOWNLOAD_EXTENSIONS: &[&str] = &[
    "zip", "rar", "7z", "gz", "tgz", "tar", "bz2", "xz", "z", //
    "exe", "msi", "dmg", "pkg", "deb", "rpm", "appimage", "apk", "jar", "bin", //
    "iso", "img", //
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts", //
    "mp3", "flac", "wav", "aac", "ogg", "m4a", "opus", "wma", //
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "epub", "mobi", //
    "torrent",
];

/// A clean download URL from clipboard text, or `None`: one http(s)/ftp URL
/// with no whitespace whose filename has a known download extension — or any
/// such link when `monitor_all` is set.
pub fn extract_download_url(text: &str, monitor_all: bool) -> Option<String> {
    let t = text
        .trim()
        .trim_matches(|c| matches!(c, '<' | '>' | '"' | '\''));
    if t.is_empty() || t.len() > 4096 || t.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if !["http://", "https://", "ftp://"]
        .iter()
        .any(|s| lower.starts_with(s))
    {
        return None;
    }
    if monitor_all {
        return Some(t.to_string());
    }
    let name = filename_from_url(t);
    let (_, ext) = name.rsplit_once('.')?;
    DOWNLOAD_EXTENSIONS
        .contains(&ext.to_ascii_lowercase().as_str())
        .then(|| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catches_download_links() {
        assert_eq!(
            extract_download_url("https://x.com/a/file.zip", false).as_deref(),
            Some("https://x.com/a/file.zip")
        );
        assert_eq!(
            extract_download_url("  <https://x.com/Setup.EXE?sig=1>  ", false).as_deref(),
            Some("https://x.com/Setup.EXE?sig=1")
        );
        assert!(extract_download_url("ftp://host/pub/disc.iso", false).is_some());
    }

    #[test]
    fn ignores_pages_text_and_bare_hosts() {
        assert!(extract_download_url("https://x.com/page.html", false).is_none());
        assert!(extract_download_url("https://x.com/", false).is_none());
        assert!(extract_download_url("see https://x.com/a.zip", false).is_none());
        assert!(extract_download_url("x.com/a.zip", false).is_none());
        assert!(extract_download_url("", false).is_none());
    }

    #[test]
    fn monitor_all_takes_any_link_but_not_text() {
        assert!(extract_download_url("https://x.com/page.html", true).is_some());
        assert!(extract_download_url("not a link", true).is_none());
    }

    #[test]
    fn formats_sizes_and_times() {
        assert_eq!(human_bytes(Some(512)), "512B");
        assert_eq!(human_bytes(Some(1536)), "1.5KB");
        assert_eq!(human_bytes(None), "?");
        assert_eq!(human_time(Some(65.0)), "01:05");
        assert_eq!(human_time(Some(3725.0)), "1:02:05");
        assert_eq!(human_time(None), "--:--");
    }

    #[test]
    fn derives_filenames() {
        assert_eq!(
            parse_content_disposition("attachment; filename=\"a b.zip\"").as_deref(),
            Some("a b.zip")
        );
        assert_eq!(
            parse_content_disposition("attachment; filename*=UTF-8''%E7%8E%8B.rar").as_deref(),
            Some("王.rar")
        );
        assert_eq!(filename_from_url("https://x.com/dl/%E7%8E%8B.rar?x=1"), "王.rar");
        assert_eq!(filename_from_url("https://x.com/"), "download");
        assert_eq!(sanitize_filename("a/b:c*?.txt"), "a_b_c__.txt");
    }
}
