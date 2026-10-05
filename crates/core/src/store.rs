//! Everything that survives a restart, in one atomically-written JSON file.

use crate::blocks::SavedBlocks;
use crate::types::{Job, Settings};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredJob {
    pub job: Job,
    pub saved: Option<SavedBlocks>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct UsageRec {
    /// `2026-10-04` for daily caps, `2026-10` for monthly.
    pub period: String,
    pub bytes: u64,
}

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(default)]
pub struct State {
    pub version: u32,
    pub jobs: Vec<StoredJob>,
    pub settings: Settings,
    pub usage: HashMap<String, UsageRec>,
    /// Lifetime bytes downloaded.
    pub total_bytes: u64,
}

impl State {
    pub fn load(path: &Path) -> State {
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<State>(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    // Keep the broken file for inspection rather than silently discarding it.
                    let bad = path.with_extension(format!("corrupt-{}", crate::util::now_ts()));
                    let _ = std::fs::rename(path, &bad);
                    tracing::error!("state file unreadable ({e}); moved to {}", bad.display());
                    State::default()
                }
            },
            Err(_) => State::default(),
        }
    }

    /// Write to a temp file, then rename over the real one: a crash can't leave half a file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(io::Error::other)?)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_survives_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        let mut s = State::default();
        s.total_bytes = 42;
        s.settings.max_concurrent_jobs = 7;
        s.save(&p).unwrap();
        let back = State::load(&p);
        assert_eq!((back.total_bytes, back.settings.max_concurrent_jobs), (42, 7));

        std::fs::write(&p, b"{ not json").unwrap();
        let fresh = State::load(&p);
        assert_eq!(fresh.total_bytes, 0);
        // The bad file was set aside, not deleted.
        assert!(std::fs::read_dir(dir.path()).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains("corrupt")));
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        std::fs::write(&p, br#"{"totalBytes": 1, "settings": {"maxConcurrentJobs": 9, "somethingNew": true}}"#).unwrap();
        // `State` is snake_case on disk but settings are camelCase; a mismatch must not panic.
        let s = State::load(&p);
        assert_eq!(s.settings.max_concurrent_jobs, 9);
    }
}
