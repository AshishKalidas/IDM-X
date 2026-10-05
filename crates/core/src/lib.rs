pub mod batch;
pub mod blocks;
pub mod engine;
pub mod error;
pub mod http;
pub mod limiter;
pub mod media;
pub mod net;
pub mod ping;
pub mod os;
mod pipeline;
mod pipeline_media;
mod pipeline_torrent;
pub mod post;
pub mod run;
pub mod torrent;
mod speedtest;
pub mod store;
pub mod types;
pub mod update;
pub mod util;
pub mod verify;

pub use engine::{Engine, EngineOptions, JobPatch};

/// Where IDMX keeps its state: `$IDMX_HOME`, else the OS's per-user application data folder.
pub fn default_data_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("IDMX_HOME") {
        if !p.is_empty() {
            return p.into();
        }
    }
    directories::ProjectDirs::from("app", "idmx", "IDMX")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from(".idmx"))
}
