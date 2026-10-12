//! BitTorrent, on top of librqbit: magnet links and `.torrent` files, with the same pause,
//! resume, limits and "bind to one network" controls as everything else in IDMX.

use crate::error::{Error, Result};
use librqbit::api::TorrentIdOrHash;
use librqbit::limits::LimitsConfig;
use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, ListenerMode, ListenerOptions, ManagedTorrent, Session, SessionOptions};
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// One session's share of a torrent.
pub struct Part {
    /// The network this part's session is bound to (`None` = the system's default route).
    pub net: Option<String>,
    session: Arc<Session>,
    t: Arc<ManagedTorrent>,
    /// The files this part is responsible for (`None` = all of them).
    assigned: std::sync::Mutex<Option<std::collections::HashSet<usize>>>,
}

/// A running torrent. Usually one session; with several networks and several files, one session
/// per network, each fetching its own share of the files.
pub struct Group {
    pub parts: Vec<Part>,
}

pub type Handle = Arc<Group>;

/// Split `files` (index, size) across `n` buckets so each carries about the same bytes.
/// Whether the torrent engine can pin its traffic to one network device. It can't on Windows (devices
/// there are named by GUID, and the engine has no way to bind to one), so torrents use the system's
/// choice of network instead of failing.
pub const CAN_BIND_DEVICE: bool = cfg!(not(windows));

pub fn partition(files: &[(usize, u64)], n: usize) -> Vec<Vec<usize>> {
    let n = n.max(1);
    let mut order: Vec<&(usize, u64)> = files.iter().collect();
    order.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut load = vec![0u64; n];
    let mut out = vec![Vec::new(); n];
    for (i, sz) in order {
        let k = (0..n).min_by_key(|k| load[*k]).unwrap_or(0);
        load[k] += (*sz).max(1);
        out[k].push(*i);
    }
    out.iter_mut().for_each(|v| v.sort_unstable());
    out
}

/// Where a torrent comes from.
#[derive(Clone, Debug)]
pub enum Source {
    Magnet(String),
    /// An `http(s)` link to a `.torrent` file.
    Url(String),
    File(PathBuf),
    Bytes(Vec<u8>),
}

impl Source {
    /// Work out what a user pasted: a magnet link, a link to a `.torrent`, or a file path.
    pub fn parse(s: &str) -> Option<Source> {
        let t = s.trim();
        if t.to_ascii_lowercase().starts_with("magnet:?") {
            return Some(Source::Magnet(t.to_string()));
        }
        if t.starts_with("http://") || t.starts_with("https://") {
            let path = t.split(['?', '#']).next().unwrap_or(t).to_ascii_lowercase();
            return path.ends_with(".torrent").then(|| Source::Url(t.to_string()));
        }
        let p = t.strip_prefix("file://").unwrap_or(t);
        (p.to_ascii_lowercase().ends_with(".torrent") && std::path::Path::new(p).exists()).then(|| Source::File(PathBuf::from(p)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionCfg {
    /// Bind torrent traffic (peers, DHT, trackers) to this network device.
    pub bind_device: Option<String>,
    pub dht: bool,
    pub listen_port: Option<u16>,
    pub upload_bps: Option<u32>,
    pub download_bps: Option<u32>,
    pub peer_limit: Option<usize>,
    /// Listen on loopback only and never touch the public DHT/trackers — for tests.
    pub isolated: bool,
}

#[derive(Clone, Debug)]
pub struct MetaFile {
    pub index: u32,
    pub path: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct Meta {
    pub name: String,
    pub info_hash: String,
    pub total: u64,
    pub files: Vec<MetaFile>,
}

#[derive(Default)]
pub struct AddOpts {
    pub output: PathBuf,
    pub only_files: Option<Vec<usize>>,
    pub initial_peers: Vec<SocketAddr>,
    pub paused: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub state: String,
    pub name: String,
    pub info_hash: String,
    pub total: u64,
    pub downloaded: u64,
    pub uploaded: u64,
    pub finished: bool,
    pub down_bps: f64,
    pub up_bps: f64,
    pub peers: u32,
    pub known_peers: u32,
    pub eta: Option<u64>,
    pub error: Option<String>,
    /// `(path, size, downloaded, selected)`
    pub files: Vec<(String, u64, u64, bool)>,
    pub have_metadata: bool,
    /// One entry per session (network).
    pub parts: Vec<PartSnap>,
}

#[derive(Clone, Debug, Default)]
pub struct PartSnap {
    pub net: Option<String>,
    pub downloaded: u64,
    pub down_bps: f64,
    pub peers: u32,
}

pub struct TorrentHub {
    /// One session per network (keyed by the device it is bound to).
    inner: Mutex<std::collections::HashMap<Option<String>, (SessionCfg, Arc<Session>)>>,
}

impl Default for TorrentHub {
    fn default() -> Self {
        Self { inner: Mutex::new(Default::default()) }
    }
}

impl TorrentHub {
    /// The session for this configuration, creating it on first use. A session is a process-wide
    /// thing (one listen port, one DHT node); changing its network or port needs a new one, which
    /// we only do when nothing is using the old.
    pub async fn session(&self, cfg: &SessionCfg) -> Result<Arc<Session>> {
        let mut g = self.inner.lock().await;
        if let Some((have, s)) = g.get(&cfg.bind_device) {
            let busy = s.with_torrents(|it| it.count()) > 0;
            if have == cfg || busy {
                return Ok(s.clone());
            }
        }
        if let Some((_, old)) = g.remove(&cfg.bind_device) {
            old.stop().await;
        }
        let dir = std::env::temp_dir().join("idmx-torrent-default").join(cfg.bind_device.as_deref().unwrap_or("default"));
        let _ = std::fs::create_dir_all(&dir);
        let listen_addr: SocketAddr = if cfg.isolated { ([127, 0, 0, 1], cfg.listen_port.unwrap_or(0)).into() } else { ([0, 0, 0, 0], cfg.listen_port.unwrap_or(0)).into() };
        let opts = SessionOptions {
            // No shared on-disk DHT state: a second IDMX (or the CLI beside the app) would fight over that file.
            dht: (cfg.dht && !cfg.isolated).then(|| librqbit::DhtSessionConfig { persistence: None, ..Default::default() }),
            disable_trackers: cfg.isolated,
            disable_local_service_discovery: cfg.isolated,
            bind_device_name: if CAN_BIND_DEVICE { cfg.bind_device.clone() } else { None },
            ipv4_only: true,
            fastresume: false,
            persistence: None,
            peer_limit: cfg.peer_limit,
            listen: Some(ListenerOptions { mode: ListenerMode::TcpOnly, listen_addr, ..Default::default() }),
            ratelimits: LimitsConfig { upload_bps: cfg.upload_bps.and_then(NonZeroU32::new), download_bps: cfg.download_bps.and_then(NonZeroU32::new) },
            client_name_and_version: Some(concat!("IDMX/", env!("CARGO_PKG_VERSION")).into()),
            ..Default::default()
        };
        // Bounded, because the hub lock is held meanwhile and every other torrent would queue behind a stuck start.
        let s = tokio::time::timeout(Duration::from_secs(30), Session::new_with_opts(dir, opts))
            .await
            .map_err(|_| Error::msg("Couldn't start the torrent engine: it took too long. Try again."))?
            .map_err(|e| Error::msg(format!("Couldn't start the torrent engine: {e}")))?;
        g.insert(cfg.bind_device.clone(), (cfg.clone(), s.clone()));
        Ok(s)
    }

    pub async fn listen_addr(&self) -> Option<SocketAddr> {
        self.inner.lock().await.values().find_map(|(_, s)| s.listen_addr())
    }

    pub async fn shutdown(&self) {
        let all: Vec<_> = self.inner.lock().await.drain().collect();
        for (_, (_, s)) in all {
            s.stop().await;
        }
    }

    async fn to_add(source: &Source, http: &reqwest::Client) -> Result<AddTorrent<'static>> {
        Ok(match source {
            Source::Magnet(m) => AddTorrent::from_url(m.clone()),
            Source::Bytes(b) => AddTorrent::from_bytes(b.clone()),
            Source::File(p) => AddTorrent::from_bytes(std::fs::read(p).map_err(|e| Error::msg(format!("Couldn't read that .torrent file: {}", crate::error::io_message(&e))))?),
            Source::Url(u) => {
                let resp = http.get(u).timeout(Duration::from_secs(30)).send().await?;
                if !resp.status().is_success() {
                    return Err(Error::Http(resp.status().as_u16()));
                }
                if resp.content_length().is_some_and(|l| l > 20 << 20) {
                    return Err(Error::msg("That .torrent file is suspiciously large (over 20 MB)."));
                }
                let bytes = resp.bytes().await?;
                AddTorrent::from_bytes(bytes)
            }
        })
    }

    /// What's inside, without downloading anything. For a magnet link this has to ask the swarm
    /// for the metadata first, which can take a while (or never finish if nobody has it).
    pub async fn inspect(&self, cfg: &SessionCfg, source: &Source, http: &reqwest::Client, peers: &[SocketAddr], wait: Duration) -> Result<Meta> {
        let session = self.session(cfg).await?;
        let add = Self::to_add(source, http).await?;
        let opts = AddTorrentOptions { list_only: true, initial_peers: (!peers.is_empty()).then(|| peers.to_vec()), ..Default::default() };
        let resp = tokio::time::timeout(wait, session.add_torrent(add, Some(opts)))
            .await
            .map_err(|_| Error::msg("Couldn't get this torrent's details: no peer answered in time. Check the magnet link, or try again in a bit."))?
            .map_err(|e| friendly(&e.to_string()))?;
        let AddTorrentResponse::ListOnly(l) = resp else { return Err(Error::msg("Unexpected reply from the torrent engine.")) };
        let files: Vec<MetaFile> = l
            .info
            .iter_file_details()
            .enumerate()
            .map(|(i, f)| MetaFile { index: i as u32, path: f.filename.to_string(), size: f.len })
            .collect();
        let total = files.iter().map(|f| f.size).sum();
        let name = l.info.name().map(|n| n.into_owned()).unwrap_or_default();
        Ok(Meta { name, info_hash: l.info_hash.as_string(), total, files })
    }

    /// Start (or resume) a torrent on one session. Existing data in the folder is checked and kept,
    /// so this is also how a paused or interrupted one carries on.
    pub async fn add(&self, cfg: &SessionCfg, source: &Source, http: &reqwest::Client, o: AddOpts) -> Result<Handle> {
        let part = self.add_part(cfg, source, http, o.output, o.only_files.map(|v| v.into_iter().collect()), o.initial_peers, o.paused).await?;
        Ok(Arc::new(Group { parts: vec![part] }))
    }

    /// Start a torrent across several networks: each gets its own session and its share of the
    /// files. `files` is `(index, size)` of the files to fetch. A torrent with one file (or one
    /// network) can't be divided, and runs on the first network only.
    pub async fn add_multi(&self, cfgs: &[SessionCfg], source: &Source, http: &reqwest::Client, o: AddOpts, files: &[(usize, u64)]) -> Result<Handle> {
        let n = cfgs.len().min(files.len());
        if n < 2 {
            return self.add(cfgs.first().ok_or_else(|| Error::msg("No network is available for torrents."))?, source, http, o).await;
        }
        let mut parts = Vec::new();
        for (cfg, share) in cfgs.iter().zip(partition(files, n)) {
            if share.is_empty() {
                continue;
            }
            parts.push(self.add_part(cfg, source, http, o.output.clone(), Some(share.into_iter().collect()), o.initial_peers.clone(), o.paused).await?);
        }
        Ok(Arc::new(Group { parts }))
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_part(&self, cfg: &SessionCfg, source: &Source, http: &reqwest::Client, output: PathBuf, only: Option<std::collections::HashSet<usize>>, peers: Vec<SocketAddr>, paused: bool) -> Result<Part> {
        let session = self.session(cfg).await?;
        let add = Self::to_add(source, http).await?;
        let opts = AddTorrentOptions {
            paused,
            overwrite: true,
            output_folder: Some(output.to_string_lossy().into_owned()),
            only_files: only.as_ref().map(|s| s.iter().copied().collect()),
            initial_peers: (!peers.is_empty()).then_some(peers),
            ..Default::default()
        };
        let resp = session.add_torrent(add, Some(opts)).await.map_err(|e| friendly(&e.to_string()))?;
        let t = resp.into_handle().ok_or_else(|| Error::msg("Unexpected reply from the torrent engine."))?;
        Ok(Part { net: cfg.bind_device.clone(), session, t, assigned: std::sync::Mutex::new(only) })
    }

    pub async fn pause(&self, h: &Handle) -> Result<()> {
        for p in &h.parts {
            p.session.pause(&p.t).await.map_err(|e| friendly(&e.to_string()))?;
        }
        Ok(())
    }

    pub async fn unpause(&self, h: &Handle) -> Result<()> {
        for p in &h.parts {
            p.session.unpause(&p.t).await.map_err(|e| friendly(&e.to_string()))?;
        }
        Ok(())
    }

    /// Change which files are wanted. Files nobody was assigned yet go to the first part.
    pub async fn set_only_files(&self, h: &Handle, files: std::collections::HashSet<usize>) -> Result<()> {
        let multi = h.parts.len() > 1;
        for (k, p) in h.parts.iter().enumerate() {
            let mine: std::collections::HashSet<usize> = if multi {
                let owned_elsewhere: std::collections::HashSet<usize> = h.parts.iter().enumerate().filter(|(o, _)| *o != k).flat_map(|(_, q)| q.assigned.lock().unwrap().clone().unwrap_or_default()).collect();
                files.iter().copied().filter(|f| p.assigned.lock().unwrap().as_ref().is_some_and(|a| a.contains(f)) || (k == 0 && !owned_elsewhere.contains(f))).collect()
            } else {
                files.clone()
            };
            if multi {
                let mut a = p.assigned.lock().unwrap();
                let mut next = a.clone().unwrap_or_default();
                next.extend(mine.iter().copied());
                *a = Some(next);
            }
            p.session.update_only_files(&p.t, &mine).await.map_err(|e| friendly(&e.to_string()))?;
        }
        Ok(())
    }

    /// Forget the torrent; `delete_files` also removes what was downloaded.
    pub async fn remove(&self, h: &Handle, delete_files: bool) -> Result<()> {
        for p in &h.parts {
            p.session.delete(TorrentIdOrHash::Id(p.t.id()), delete_files).await.map_err(|e| friendly(&e.to_string()))?;
        }
        Ok(())
    }
}

fn part_snapshot(h: &ManagedTorrent) -> Snapshot {
    let st = h.stats();
    let (have_metadata, files, name) = h
        .with_metadata(|m| {
            let only = h.only_files();
            let files: Vec<(String, u64, u64, bool)> = m
                .file_infos
                .iter()
                .enumerate()
                .map(|(i, f)| (f.relative_filename.to_string_lossy().into_owned(), f.len, st.file_progress.get(i).copied().unwrap_or(0), only.as_ref().is_none_or(|o| o.contains(&i))))
                .collect();
            (true, files, m.info.name().map(|n| n.into_owned()))
        })
        .unwrap_or((false, vec![], None));
    let name = name.or_else(|| h.name()).unwrap_or_default();
    let live = st.live.as_ref();
    let selected_total: u64 = files.iter().filter(|f| f.3).map(|f| f.1).sum();
    let peers = live.map(|l| l.snapshot.peer_stats.live as u32).unwrap_or(0);
    let known = live.map(|l| (l.snapshot.peer_stats.queued + l.snapshot.peer_stats.connecting + l.snapshot.peer_stats.seen) as u32).unwrap_or(0);
    // librqbit reports MiB/s.
    let mib = 1024.0 * 1024.0;
    Snapshot {
        state: st.state.to_string(),
        name,
        info_hash: h.info_hash().as_string(),
        total: if selected_total > 0 { selected_total } else { st.total_bytes },
        downloaded: st.progress_bytes,
        uploaded: st.uploaded_bytes,
        finished: st.finished,
        down_bps: live.map(|l| l.download_speed.mbps * mib).unwrap_or(0.0),
        up_bps: live.map(|l| l.upload_speed.mbps * mib).unwrap_or(0.0),
        peers,
        known_peers: known,
        eta: live.and_then(|l| {
            let bps = l.download_speed.mbps * mib;
            let left = st.total_bytes.saturating_sub(st.progress_bytes);
            (bps > 1.0 && left > 0).then(|| (left as f64 / bps).ceil() as u64)
        }),
        error: st.error.clone(),
        files,
        have_metadata,
        parts: vec![],
    }
}

/// Everything about a torrent, adding up its parts.
pub fn snapshot(h: &Handle) -> Snapshot {
    let snaps: Vec<Snapshot> = h.parts.iter().map(|p| part_snapshot(&p.t)).collect();
    let mut a = snaps.first().cloned().unwrap_or_default();
    if snaps.len() > 1 {
        a.total = snaps.iter().map(|s| s.total).sum();
        a.downloaded = snaps.iter().map(|s| s.downloaded).sum();
        a.uploaded = snaps.iter().map(|s| s.uploaded).sum();
        a.down_bps = snaps.iter().map(|s| s.down_bps).sum();
        a.up_bps = snaps.iter().map(|s| s.up_bps).sum();
        a.peers = snaps.iter().map(|s| s.peers).sum();
        a.known_peers = snaps.iter().map(|s| s.known_peers).sum();
        a.finished = snaps.iter().all(|s| s.finished);
        a.have_metadata = snaps.iter().all(|s| s.have_metadata);
        a.error = snaps.iter().find_map(|s| s.error.clone());
        a.state = if snaps.iter().all(|s| s.state == "paused") {
            "paused".into()
        } else if snaps.iter().any(|s| s.state == "initializing") {
            "initializing".into()
        } else {
            "live".into()
        };
        for (i, f) in a.files.iter_mut().enumerate() {
            f.2 = snaps.iter().filter_map(|s| s.files.get(i)).map(|f| f.2).max().unwrap_or(0);
            f.3 = snaps.iter().filter_map(|s| s.files.get(i)).any(|f| f.3);
        }
        let left = a.total.saturating_sub(a.downloaded);
        a.eta = (a.down_bps > 1.0 && left > 0).then(|| (left as f64 / a.down_bps).ceil() as u64);
    }
    a.parts = snaps.iter().zip(&h.parts).map(|(s, p)| PartSnap { net: p.net.clone(), downloaded: s.downloaded, down_bps: s.down_bps, peers: s.peers }).collect();
    a
}

/// Turn the torrent engine's errors into something a person can act on.
pub fn friendly(e: &str) -> Error {
    let l = e.to_lowercase();
    let m = if l.contains("already exist") {
        "A file with that name already exists in the destination folder."
    } else if l.contains("permission denied") {
        "IDMX isn't allowed to write to that folder. Pick a different one."
    } else if l.contains("no space") {
        "The disk is full. Free up some space and resume."
    } else if l.contains("invalid magnet") || l.contains("magnet") && l.contains("parse") {
        "That magnet link isn't valid — it may have been cut off when copied."
    } else if l.contains("bencode") || l.contains("deserial") || l.contains("not a valid torrent") {
        "That isn't a valid .torrent file."
    } else {
        return Error::msg(format!("The torrent engine reported a problem: {e}"));
    };
    Error::msg(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_split_by_size() {
        let files = [(0, 100), (1, 90), (2, 10), (3, 10), (4, 5)];
        let p = partition(&files, 2);
        let sum = |v: &Vec<usize>| v.iter().map(|i| files[*i].1).sum::<u64>();
        assert_eq!(p.iter().map(|v| v.len()).sum::<usize>(), 5);
        assert!((sum(&p[0]) as i64 - sum(&p[1]) as i64).abs() <= 15, "{p:?}");
        assert_eq!(partition(&files, 1)[0].len(), 5);
    }

    #[test]
    fn recognises_what_was_pasted() {
        assert!(matches!(Source::parse("magnet:?xt=urn:btih:abc"), Some(Source::Magnet(_))));
        assert!(matches!(Source::parse("  MAGNET:?xt=urn:btih:abc  "), Some(Source::Magnet(_))));
        assert!(matches!(Source::parse("https://x.org/a/b.torrent"), Some(Source::Url(_))));
        assert!(matches!(Source::parse("https://x.org/a/b.torrent?token=1"), Some(Source::Url(_))));
        assert!(Source::parse("https://x.org/a/b.zip").is_none());
        assert!(Source::parse("https://x.org/torrent").is_none());
        assert!(Source::parse("/definitely/not/here.torrent").is_none());
    }

    #[test]
    fn errors_get_plain_words() {
        assert!(friendly("Permission denied (os error 13)").to_string().contains("isn't allowed"));
        assert!(friendly("something exotic").to_string().contains("something exotic"));
    }
}
