//! Download manager: owns a list of jobs and persists the queue to `~/.sdm`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::Download;
use crate::types::Status;
use crate::util::{config_dir, default_download_dir, filename_from_url};

#[derive(Serialize, Deserialize)]
struct QueueEntry {
    url: String,
    dest_dir: String,
    filename: String,
    connections: usize,
    status: String,
    state_path: String,
    final_path: String,
    size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub struct Manager {
    pub default_dir: PathBuf,
    queue_path: PathBuf,
    pub downloads: Vec<Download>,
}

impl Manager {
    pub fn new(default_dir: Option<PathBuf>) -> Self {
        Manager {
            default_dir: default_dir.unwrap_or_else(default_download_dir),
            queue_path: config_dir().join("queue.json"),
            downloads: Vec::new(),
        }
    }

    /// A manager on a private folder: never the real profile's queue.
    #[cfg(test)]
    pub(crate) fn scratch(dir: &Path) -> Self {
        Manager {
            default_dir: dir.to_path_buf(),
            queue_path: dir.join("queue.json"),
            downloads: Vec::new(),
        }
    }

    pub fn add(
        &mut self,
        url: &str,
        dest_dir: Option<PathBuf>,
        filename: Option<&str>,
        connections: usize,
        headers: Option<Vec<(String, String)>>,
    ) -> Download {
        let dir = dest_dir.unwrap_or_else(|| self.default_dir.clone());
        let d = Download::new(url, dir, filename, connections, headers);
        self.downloads.push(d.clone());
        self.save_queue();
        d
    }

    /// Drop a job from the list. With `delete_files`, its data goes too: the
    /// finished file, or a partial download's working file and sidecar.
    pub fn remove(&mut self, d: &Download, delete_files: bool) {
        d.pause();
        let id = d.id();
        if delete_files {
            let running = d.is_active();
            let d = d.clone();
            let delete = move || {
                // A running job checkpoints its sidecar as it stops; let it.
                for _ in 0..100 {
                    if !d.is_active() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                if d.status() == Status::Complete {
                    let _ = std::fs::remove_file(d.output_path());
                }
                let _ = std::fs::remove_file(d.working_path());
                let _ = std::fs::remove_file(d.state_path());
            };
            if running {
                std::thread::spawn(delete);
            } else {
                delete();
            }
        }
        self.downloads.retain(|x| x.id() != id);
        self.save_queue();
    }

    pub fn pause_all(&self) {
        for d in &self.downloads {
            d.pause();
        }
    }

    pub fn save_queue(&self) {
        let entries: Vec<QueueEntry> = self
            .downloads
            .iter()
            .map(|d| QueueEntry {
                url: d.url(),
                dest_dir: d.dest_dir().to_string_lossy().into_owned(),
                filename: d.filename(),
                connections: d.connections(),
                status: d.status().as_str().to_string(),
                state_path: d.state_path().to_string_lossy().into_owned(),
                final_path: d.output_path().to_string_lossy().into_owned(),
                size: d.size(),
                error: d.error(),
            })
            .collect();
        let tmp = {
            let mut p = self.queue_path.clone().into_os_string();
            p.push(".tmp");
            PathBuf::from(p)
        };
        if let Ok(json) = serde_json::to_string_pretty(&entries) {
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.queue_path);
            }
        }
    }

    pub fn load_queue(&mut self) {
        let text = match std::fs::read_to_string(&self.queue_path) {
            Ok(t) => t,
            Err(_) => return,
        };
        let entries: Vec<QueueEntry> = match serde_json::from_str(&text) {
            Ok(e) => e,
            Err(_) => {
                // The next save would overwrite it: keep what's there for a look.
                let _ = std::fs::rename(&self.queue_path, self.queue_path.with_extension("json.bad"));
                return;
            }
        };
        self.downloads.extend(entries.into_iter().map(restore));
    }
}

/// A queue entry back as a job, in a state the app can act on.
fn restore(e: QueueEntry) -> Download {
    let failed = e.status == Status::Error.as_str();
    let fresh = |filename: Option<&str>| {
        Download::new(&e.url, e.dest_dir.as_str(), filename, e.connections, None)
    };
    // A file moved or deleted since doesn't undo the download: the list is history.
    if e.status == Status::Complete.as_str() {
        let dl = fresh(Some(&e.filename));
        let size = e.size.or_else(|| std::fs::metadata(&e.final_path).ok().map(|m| m.len()));
        dl.restore_complete(size, PathBuf::from(&e.final_path));
        return dl;
    }
    let resumable = (!e.state_path.is_empty())
        .then(|| Download::from_state(Path::new(&e.state_path)).ok())
        .flatten();
    let dl = resumable.unwrap_or_else(|| {
        // Nothing on disk to resume, so it starts over when resumed. A name that only
        // came from the link leaves the server free to give the real one.
        let derived = e.filename == filename_from_url(&e.url);
        let dl = fresh((!derived).then_some(e.filename.as_str()));
        dl.restore_unstarted(e.size);
        dl
    });
    if failed {
        dl.restore_failed(e.error.clone().unwrap_or_else(|| "Download failed".to_string()));
    }
    dl
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_keeps_the_file_unless_asked_to_delete_it() {
        // A private scratch profile: never the real queue.json.
        let dir = std::env::temp_dir().join(format!("sdm-remove-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = Manager::scratch(&dir);
        let mut finished = |name: &str| {
            let file = dir.join(name);
            std::fs::write(&file, b"data").unwrap();
            let d = Download::new("https://example.com/f", &dir, Some(name), 1, None);
            d.restore_complete(Some(4), file.clone());
            m.downloads.push(d.clone());
            (d, file)
        };
        let (kept, kept_file) = finished("kept.bin");
        let (gone, gone_file) = finished("gone.bin");

        m.remove(&kept, false);
        m.remove(&gone, true);

        assert!(m.downloads.is_empty());
        assert!(kept_file.exists(), "removing from the list leaves the file");
        assert!(!gone_file.exists(), "removing with delete takes the file too");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
