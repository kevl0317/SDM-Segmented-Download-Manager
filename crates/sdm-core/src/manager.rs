//! Download manager: owns a list of jobs and persists the queue to `~/.sdm`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::Download;
use crate::types::Status;
use crate::util::{config_dir, default_download_dir};

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
            Err(_) => return,
        };
        for e in entries {
            let mut d: Option<Download> = None;
            if !e.state_path.is_empty() && std::path::Path::new(&e.state_path).exists() {
                d = Download::from_state(std::path::Path::new(&e.state_path)).ok();
            }
            if d.is_none()
                && e.status == Status::Complete.as_str()
                && !e.final_path.is_empty()
                && std::path::Path::new(&e.final_path).exists()
            {
                let dl = Download::new(
                    &e.url,
                    e.dest_dir.as_str(),
                    Some(&e.filename),
                    e.connections,
                    None,
                );
                let size = e
                    .size
                    .or_else(|| std::fs::metadata(&e.final_path).ok().map(|m| m.len()));
                dl.restore_complete(size, PathBuf::from(&e.final_path));
                d = Some(dl);
            }
            let dl = d.unwrap_or_else(|| {
                // Nothing on disk to resume; keep it as a fresh queued job.
                let dl = Download::new(
                    &e.url,
                    e.dest_dir.as_str(),
                    Some(&e.filename),
                    e.connections,
                    None,
                );
                dl.restore_size(e.size);
                dl
            });
            self.downloads.push(dl);
        }
    }
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
        let mut m = Manager {
            default_dir: dir.clone(),
            queue_path: dir.join("queue.json"),
            downloads: Vec::new(),
        };
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
