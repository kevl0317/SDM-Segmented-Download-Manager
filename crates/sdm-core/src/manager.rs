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

    pub fn remove(&mut self, d: &Download, delete_files: bool) {
        d.pause();
        let id = d.id();
        if delete_files {
            let _ = std::fs::remove_file(d.working_path());
            let _ = std::fs::remove_file(d.state_path());
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
                d = Some(dl);
            }
            let dl = d.unwrap_or_else(|| {
                Download::new(
                    &e.url,
                    e.dest_dir.as_str(),
                    Some(&e.filename),
                    e.connections,
                    None,
                )
            });
            self.downloads.push(dl);
        }
    }
}
