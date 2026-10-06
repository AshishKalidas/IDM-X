//! The engine: owns every job, schedules the queue, watches the networks, persists state and
//! broadcasts events. The desktop app, the CLI and the HTTP API are all thin skins over this.

use crate::batch;
use crate::blocks::SavedBlocks;
use crate::error::{Error, Result};
use crate::http::parse_url;
use crate::limiter::Limiter;
use crate::net;
use crate::os;
use crate::run::Run;
use crate::store::{State, StoredJob, UsageRec};
use crate::types::*;
use crate::util::now_ts;
use chrono::Timelike;
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch, Notify};
use tokio_util::sync::CancellationToken;

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Want {
    #[default]
    None,
    Pause,
    /// Put back in the queue (the schedule window closed).
    Requeue,
    Cancel { delete: bool },
    Remove { delete: bool },
}

#[derive(Default)]
pub(crate) struct Ctl {
    pub run: Option<Arc<Run>>,
    pub saved: Option<SavedBlocks>,
    pub want: Want,
    pub running: bool,
    pub token: CancellationToken,
    /// The torrent, while this job is one (kept, paused, between runs).
    pub torrent: Option<crate::torrent::Handle>,
}

pub(crate) struct JobEntry {
    pub job: Mutex<Job>,
    pub ctl: Mutex<Ctl>,
    pub limiter: Arc<Limiter>,
}

/// What was found the last time we looked for yt-dlp, ffmpeg and a JavaScript runtime.
#[derive(Default)]
pub(crate) struct ToolCache {
    pub ytdlp: Option<crate::media::Tool>,
    pub ffmpeg: Option<crate::media::Tool>,
    pub js: Option<(String, crate::media::Tool)>,
    pub at: Option<Instant>,
}

pub(crate) struct Inner {
    pub data_dir: PathBuf,
    pub jobs: RwLock<Vec<Arc<JobEntry>>>,
    pub settings: RwLock<Settings>,
    pub events: broadcast::Sender<Event>,
    pub ifaces: RwLock<Vec<NetInterface>>,
    pub iface_tx: watch::Sender<Arc<Vec<NetInterface>>>,
    pub global_limiter: Arc<Limiter>,
    pub net_limiters: Mutex<HashMap<String, Arc<Limiter>>>,
    pub usage: Mutex<HashMap<String, (String, Arc<AtomicU64>)>>,
    pub live: Mutex<HashMap<String, NetStat>>,
    pub total_bytes: AtomicU64,
    pub session_bytes: AtomicU64,
    pub dirty: AtomicBool,
    pub kick: Notify,
    pub reserved: Mutex<HashSet<PathBuf>>,
    pub http: reqwest::Client,
    pub shutdown: CancellationToken,
    pub had_work: AtomicBool,
    pub awake: Mutex<Option<os::AwakeGuard>>,
    pub speedtest_busy: AtomicBool,
    pub endurance_stop: AtomicBool,
    pub endurance_status: Mutex<Option<EnduranceStatus>>,
    /// Force sockets onto their device (off for tests that talk to loopback).
    pub bind_device: bool,
    /// Interfaces were supplied by the caller; don't poll the OS.
    pub fixed_ifaces: bool,
    /// Latest ping per network id (`None` = nothing got through).
    pub ping: Mutex<HashMap<String, Option<f64>>>,
    pub ping_kick: Notify,
    pub ping_target: Option<std::net::SocketAddr>,
    pub update: Mutex<Option<UpdateInfo>>,
    pub extension: Mutex<Option<ExtensionInfo>>,
    pub update_url: Option<String>,
    pub torrents: crate::torrent::TorrentHub,
    /// Links the OS handed to the app (a magnet link, a double-clicked .torrent) waiting for the window to show them.
    pub pending_add: Mutex<Vec<String>>,
    pub tool_cache: Mutex<ToolCache>,
    pub installing: Mutex<Option<InstallProgress>>,
    /// Where release files live, for every tool (a mirror, or a test server). `None` = the real ones.
    pub tool_base: Option<String>,
    /// Tests: torrents on loopback only, and these peers to start from.
    pub torrent_isolated: bool,
    pub torrent_peers: Vec<std::net::SocketAddr>,
    _lock: std::fs::File,
}

#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

/// Knobs for embedding and testing; the defaults are what the apps use.
#[derive(Default)]
pub struct EngineOptions {
    /// Use exactly these networks instead of discovering them.
    pub fixed_interfaces: Option<Vec<NetInterface>>,
    /// Skip forcing sockets onto their device.
    pub no_device_binding: bool,
    /// Ping this address instead of the public internet (for tests). Also turns pinging on when
    /// the interface list is fixed.
    pub ping_target: Option<std::net::SocketAddr>,
    /// Ping the public internet even though the interface list is fixed (simulated networks).
    pub force_ping: bool,
    /// Where to look for releases, overriding the settings (for tests).
    pub update_url: Option<String>,
    /// Torrents on loopback only, starting from these peers (for tests).
    pub torrent_isolated: bool,
    pub torrent_peers: Vec<std::net::SocketAddr>,
    /// Where yt-dlp's release files live (a mirror, or a test server).
    pub ytdlp_base: Option<String>,
}

impl Engine {
    /// Start an engine over `data_dir`. Fails if another IDMX process already owns it.
    pub async fn start(data_dir: PathBuf) -> Result<Engine> {
        Self::start_with(data_dir, EngineOptions::default()).await
    }

    pub async fn start_with(data_dir: PathBuf, opts: EngineOptions) -> Result<Engine> {
        std::fs::create_dir_all(&data_dir)?;
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(data_dir.join("engine.lock"))?;
        if lock.try_lock().is_err() {
            return Err(Error::msg("IDMX is already running. Open the existing window (check the menu bar or tray) instead of starting a second copy."));
        }

        let state = State::load(&data_dir.join("state.json"));
        let mut settings = state.settings;
        if settings.remote.token.is_empty() {
            settings.remote.token = new_token();
        }
        let (events, _) = broadcast::channel(2048);
        let fixed_ifaces = opts.fixed_interfaces.is_some();
        let nets = match opts.fixed_interfaces {
            Some(n) => n,
            None => tokio::task::spawn_blocking(net::discover).await.map_err(|e| Error::msg(e.to_string()))?,
        };
        let (iface_tx, _) = watch::channel(Arc::new(nets.clone()));

        let mut reserved = HashSet::new();
        let jobs: Vec<Arc<JobEntry>> = state
            .jobs
            .into_iter()
            .map(|sj| {
                let mut j = sj.job;
                // A job that was mid-flight when we last exited comes back paused.
                if j.status == JobStatus::Seeding {
                    // Sharing doesn't survive a restart; the files do.
                    j.status = JobStatus::Completed;
                } else if j.status.is_running() {
                    j.status = JobStatus::Paused;
                    j.speed = 0.0;
                    j.eta = None;
                }
                for n in &mut j.networks {
                    n.speed = 0.0;
                    n.active = 0;
                }
                if !j.status.is_finished() && j.size.is_some() {
                    reserved.insert(PathBuf::from(&j.dest_dir).join(&j.file_name));
                }
                let limiter = Arc::new(Limiter::new(j.options.speed_limit));
                Arc::new(JobEntry {
                    job: Mutex::new(j),
                    ctl: Mutex::new(Ctl { saved: sj.saved, ..Default::default() }),
                    limiter,
                })
            })
            .collect();

        let usage = state
            .usage
            .into_iter()
            .map(|(k, v)| (k, (v.period, Arc::new(AtomicU64::new(v.bytes)))))
            .collect();

        let http = crate::http::build_client(&Default::default())?;
        let inner = Arc::new(Inner {
            data_dir,
            jobs: RwLock::new(jobs),
            global_limiter: Arc::new(Limiter::new(settings.global_speed_limit)),
            settings: RwLock::new(settings),
            events,
            ifaces: RwLock::new(nets),
            iface_tx,
            net_limiters: Mutex::new(HashMap::new()),
            usage: Mutex::new(usage),
            live: Mutex::new(HashMap::new()),
            total_bytes: AtomicU64::new(state.total_bytes),
            session_bytes: AtomicU64::new(0),
            dirty: AtomicBool::new(true),
            kick: Notify::new(),
            reserved: Mutex::new(reserved),
            http,
            shutdown: CancellationToken::new(),
            had_work: AtomicBool::new(false),
            awake: Mutex::new(None),
            speedtest_busy: AtomicBool::new(false),
            endurance_stop: AtomicBool::new(false),
            endurance_status: Mutex::new(None),
            bind_device: !opts.no_device_binding,
            fixed_ifaces,
            ping: Mutex::new(HashMap::new()),
            ping_kick: Notify::new(),
            ping_target: opts.ping_target,
            update: Mutex::new(None),
            extension: Mutex::new(None),
            update_url: opts.update_url,
            torrents: crate::torrent::TorrentHub::default(),
            pending_add: Mutex::new(Vec::new()),
            tool_cache: Mutex::new(ToolCache::default()),
            installing: Mutex::new(None),
            tool_base: opts.ytdlp_base,
            torrent_isolated: opts.torrent_isolated,
            torrent_peers: opts.torrent_peers,
            _lock: lock,
        });
        inner.apply_limits();
        tokio::spawn(inner.clone().monitor());
        tokio::spawn(inner.clone().manager());
        tokio::spawn(inner.clone().ticker());
        tokio::spawn(inner.clone().saver());
        if !fixed_ifaces || inner.ping_target.is_some() || opts.force_ping {
            tokio::spawn(inner.clone().pinger());
        }
        tokio::spawn(inner.clone().updater());
        Ok(Engine { inner })
    }

    // ───── observation ─────

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }
    pub fn data_dir(&self) -> PathBuf {
        self.inner.data_dir.clone()
    }
    pub fn list(&self) -> Vec<Job> {
        self.inner.jobs.read().iter().map(|e| e.job.lock().clone()).filter(|j| j.parent.is_none()).collect()
    }
    pub fn get(&self, id: &str) -> Option<Job> {
        self.inner.find(id).map(|e| e.job.lock().clone())
    }
    pub fn settings(&self) -> Settings {
        self.inner.settings.read().clone()
    }
    pub fn stats(&self) -> GlobalStats {
        self.inner.stats()
    }
    pub fn interfaces(&self) -> Vec<InterfaceInfo> {
        self.inner.interface_infos()
    }

    /// Every connection of a running download, with what it is doing.
    pub fn streams(&self, id: &str) -> Result<Vec<StreamStat>> {
        let e = self.inner.find(id).ok_or(Error::NotFound)?;
        let run = e.ctl.lock().run.clone();
        Ok(run.map(|r| r.streams_snapshot()).unwrap_or_default())
    }

    /// Switch one network on or off for one download, even while it runs. Switching the last
    /// network off pauses the download (there is nothing left to carry it).
    pub fn set_job_network(&self, id: &str, net: &str, enabled: bool) -> Result<()> {
        let e = self.inner.find(id).ok_or(Error::NotFound)?;
        let run = e.ctl.lock().run.clone();
        // What is on right now: the running download's own switches, else the defaults.
        let mut on: Vec<String> = match (&run, &e.job.lock().options.interfaces) {
            (Some(r), _) => r.snapshot().nets.into_iter().filter(|n| n.enabled).map(|n| n.id).collect(),
            (None, Some(list)) => list.clone(),
            (None, None) => self.inner.default_enabled_ids(),
        };
        on.retain(|x| x != net);
        if enabled {
            on.push(net.to_string());
        }
        self.inner.update(&e, |j| j.options.interfaces = Some(on.clone()));
        if let Some(r) = run {
            if !r.set_network_enabled(net, enabled) {
                return Err(Error::msg("That network isn't part of this download."));
            }
            if !r.any_enabled() {
                self.inner.emit(Event::Notice { level: "info".into(), message: "Every network is off — download paused".into() });
                self.pause(id)?;
            }
        }
        Ok(())
    }

    /// Replace the set of networks as if the OS had just reported it — plugging a phone in, or
    /// pulling the cable. For tests and demos with a fixed interface list.
    #[doc(hidden)]
    pub fn simulate_networks(&self, nets: Vec<NetInterface>) {
        self.inner.apply_networks(nets, false);
    }

    /// Ask the window to open its Add sheet for something the OS handed over, so the user can pick
    /// files and a folder first. Held until a window collects it, in case it isn't up yet.
    pub fn request_add(&self, text: String) {
        self.inner.pending_add.lock().push(text);
        self.inner.emit(Event::Action { action: "add".into(), path: None, job_id: None });
    }

    pub fn take_pending_add(&self) -> Vec<String> {
        std::mem::take(&mut *self.inner.pending_add.lock())
    }

    /// What is inside a torrent (magnet link or `.torrent`), before downloading anything.
    pub async fn torrent_inspect(&self, link: &str) -> Result<TorrentMeta> {
        let source = crate::torrent::Source::parse(link).ok_or_else(|| Error::msg("That isn't a magnet link or a .torrent file."))?;
        let cfg = self.inner.torrent_cfg();
        let m = self.inner.torrents.inspect(&cfg, &source, &self.inner.http, &self.inner.torrent_peers, Duration::from_secs(90)).await?;
        Ok(TorrentMeta { name: m.name, info_hash: m.info_hash, total: m.total, files: m.files.into_iter().map(|f| TorrentMetaFile { index: f.index, path: f.path, size: f.size }).collect() })
    }

    /// Choose which files of a torrent to fetch, while it runs or is paused.
    pub async fn set_torrent_files(&self, id: &str, files: Vec<u32>) -> Result<()> {
        let e = self.inner.find(id).ok_or(Error::NotFound)?;
        self.inner.update(&e, |j| j.options.torrent_files = Some(files.clone()));
        let h = e.ctl.lock().torrent.clone();
        if let Some(h) = h {
            self.inner.torrents.set_only_files(&h, files.into_iter().map(|f| f as usize).collect::<std::collections::HashSet<_>>()).await?;
        }
        Ok(())
    }

    /// Is yt-dlp (and ffmpeg) available, and is something being installed?
    pub async fn tools_status(&self, refresh: bool) -> ToolsStatus {
        self.inner.tools(refresh).await;
        self.inner.tools_status()
    }

    /// What a video page offers, before downloading anything.
    pub async fn media_probe(&self, url: &str, options: &JobOptions) -> Result<MediaProbe> {
        self.inner.media_probe(url, options).await
    }

    /// Download and verify a helper into IDMX's own folder: `yt-dlp`, `deno` (the JavaScript
    /// runtime) or `ffmpeg` — or `all` for the lot. Progress arrives as `Event::Tools`.
    pub fn install_tool(&self, which: &str) -> Result<()> {
        use crate::media::ToolKind;
        let kinds: Vec<ToolKind> = match which {
            "all" | "video" => vec![ToolKind::YtDlp, ToolKind::Deno, ToolKind::Ffmpeg],
            other => vec![ToolKind::parse(other).ok_or_else(|| Error::msg("Unknown tool; choose yt-dlp, deno, ffmpeg or all."))?],
        };
        if self.inner.installing.lock().as_ref().is_some_and(|i| i.error.is_none()) {
            return Err(Error::msg("Something is already being installed. Wait for it to finish."));
        }
        *self.inner.installing.lock() = Some(InstallProgress { tool: kinds[0].name().into(), ..Default::default() });
        let inner = self.inner.clone();
        tokio::spawn(async move {
            for kind in kinds {
                let last = std::sync::Mutex::new(Instant::now() - Duration::from_secs(1));
                let me = inner.clone();
                let name = kind.name().to_string();
                *inner.installing.lock() = Some(InstallProgress { tool: name.clone(), ..Default::default() });
                inner.emit(Event::Tools(inner.tools_status()));
                let res = crate::media::install(kind, &inner.http, &inner.data_dir, inner.tool_base.as_deref(), move |d, t| {
                    let mut l = last.lock().unwrap();
                    if l.elapsed() >= Duration::from_millis(250) {
                        *l = Instant::now();
                        *me.installing.lock() = Some(InstallProgress { tool: name.clone(), downloaded: d, total: t, error: None });
                        me.emit(Event::Tools(me.tools_status()));
                    }
                })
                .await;
                if let Err(e) = res {
                    *inner.installing.lock() = Some(InstallProgress { tool: kind.name().into(), error: Some(e.to_string()), ..Default::default() });
                    inner.tools(true).await;
                    inner.emit(Event::Tools(inner.tools_status()));
                    return;
                }
                inner.tools(true).await;
            }
            *inner.installing.lock() = None;
            inner.emit(Event::Tools(inner.tools_status()));
            inner.kick.notify_one();
        });
        Ok(())
    }

    pub fn install_ytdlp(&self) -> Result<()> {
        self.install_tool("yt-dlp")
    }

    /// The browser extension said hello. Remembered, and announced so the UI can show it's connected.
    pub fn extension_hello(&self, version: String, browser: String) {
        let info = ExtensionInfo { version, browser, seen_at: now_ts() };
        let changed = self.inner.extension.lock().as_ref().is_none_or(|e| e.version != info.version || e.browser != info.browser || now_ts() - e.seen_at > 60);
        *self.inner.extension.lock() = Some(info.clone());
        if changed {
            self.inner.emit(Event::Extension(Some(info)));
        }
    }

    pub fn extension_info(&self) -> Option<ExtensionInfo> {
        self.inner.extension.lock().clone()
    }

    /// Is any network usable right now?
    pub fn is_online(&self) -> bool {
        self.inner.ifaces.read().iter().any(|i| i.usable)
    }

    /// Look at the networks again right now ("Check again"), and return what's there.
    pub async fn refresh_networks(&self) -> Result<Vec<InterfaceInfo>> {
        if !self.inner.fixed_ifaces {
            let nets = tokio::task::spawn_blocking(net::discover).await.map_err(|e| Error::msg(e.to_string()))?;
            self.inner.apply_networks(nets, false);
        }
        self.inner.ping_round().await;
        Ok(self.inner.interface_infos())
    }

    /// Measure every network now instead of waiting for the next round.
    pub async fn ping_now(&self) -> Vec<InterfaceInfo> {
        self.inner.ping_round().await;
        self.inner.interface_infos()
    }

    pub fn update_info(&self) -> Option<UpdateInfo> {
        self.inner.update.lock().clone()
    }

    /// Look for a newer release now. Returns what was found (also broadcast as an event).
    pub async fn check_update(&self) -> Result<Option<UpdateInfo>> {
        self.inner.check_update().await
    }

    /// Hide the banner for the release currently on offer.
    pub fn dismiss_update(&self) {
        let v = self.inner.update.lock().as_ref().map(|u| u.version.clone());
        if let Some(v) = v {
            self.inner.settings.write().dismissed_update = Some(v.clone());
            self.inner.dirty.store(true, Relaxed);
            let mut u = self.inner.update.lock();
            if let Some(info) = u.as_mut() {
                info.dismissed = true;
            }
            let cur = u.clone();
            drop(u);
            self.inner.emit(Event::Update(cur));
        }
    }

    pub fn block_map(&self, id: &str) -> Option<BlockMap> {
        let e = self.inner.find(id)?;
        let (run, saved) = {
            let c = e.ctl.lock();
            (c.run.clone(), c.saved.clone())
        };
        if let Some(r) = run {
            return Some(r.block_map());
        }
        let s = saved?;
        let size = e.job.lock().size?;
        let mut state = Vec::new();
        let mut fill = Vec::new();
        for (i, d) in s.done.iter().enumerate() {
            let len = s.block_size.min(size.saturating_sub(i as u64 * s.block_size)).max(1);
            let f = (*d as f32 / len as f32).min(1.0);
            state.push(if f >= 1.0 { 2 } else if f > 0.0 { 1 } else { 0 });
            fill.push(f);
        }
        let split = s
            .contrib
            .iter()
            .enumerate()
            .filter(|(_, c)| c.len() > 1)
            .map(|(i, c)| {
                let total: u64 = c.iter().map(|x| x.1).sum::<u64>().max(1);
                (i as u32, c.iter().map(|(n, v)| (*n, ((*v as f64 / total as f64) * 100.0).round() as u8)).collect::<Vec<_>>())
            })
            .collect();
        Some(BlockMap { block_size: s.block_size, networks: s.nets, state, owner: s.owner, fill, split })
    }

    // ───── commands ─────

    pub async fn probe(&self, url: &str, options: &JobOptions) -> Result<ProbeInfo> {
        let settings = self.settings();
        let headers = crate::http::base_headers(options, settings.user_agent.as_deref());
        let client = self.inner.probe_client(&options.proxy.clone().or(settings.proxy))?;
        let (info, _resp) = crate::http::probe(&client, url, &headers).await?;
        Ok(info)
    }

    /// Queue downloads. Each entry may be a plain URL, a pattern (`f[01-10].zip`), or a whole
    /// pasted list / Metalink document.
    pub fn add(&self, req: AddRequest) -> Result<Vec<Job>> {
        let settings = self.settings();
        let dest = req.dest_dir.clone().filter(|d| !d.trim().is_empty()).unwrap_or(settings.download_dir.clone());
        let mut items = Vec::new();
        for u in &req.urls {
            items.extend(batch::expand_items(u));
        }
        if items.is_empty() {
            return Err(Error::msg("Paste at least one link to download."));
        }
        let mut added = Vec::new();
        let mut errors = Vec::new();
        for it in items {
            let is_torrent = crate::torrent::Source::parse(&it.url).is_some();
            if !is_torrent {
                if let Err(e) = parse_url(&it.url) {
                    errors.push(e.to_string());
                    continue;
                }
            }
            // Don't queue the same thing twice.
            let dup = self.inner.jobs.read().iter().any(|e| {
                let j = e.job.lock();
                j.url == it.url && j.dest_dir == dest && !j.status.is_finished()
            });
            if dup {
                continue;
            }
            let mut options = req.options.clone();
            if options.checksum.is_none() {
                options.checksum = it.checksum.clone();
            }
            if options.file_name.is_none() && req.urls.len() == 1 && it.name.is_some() {
                options.file_name = it.name.clone();
            }
            let id = loop {
                let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
                if self.inner.find(&id).is_none() {
                    break id;
                }
            };
            let kind = if crate::torrent::Source::parse(&it.url).is_some() {
                JobKind::Torrent
            } else if !options.force_http && (options.media == Some(true) || (settings.media.enabled && crate::media::looks_like_media_page(&it.url))) {
                JobKind::Media
            } else {
                JobKind::Http
            };
            let name = options
                .file_name
                .clone()
                .or(it.name.clone())
                .unwrap_or_else(|| match kind {
                    JobKind::Torrent => torrent_display_name(&it.url),
                    JobKind::Media => media_display_name(&it.url),
                    _ => parse_url(&it.url).map(|u| crate::http::filename_from_url(&u)).unwrap_or_default(),
                });
            let job = Job {
                id,
                url: it.url,
                final_url: None,
                file_name: crate::util::sanitize_filename(&name),
                dest_dir: dest.clone(),
                path: None,
                status: if req.paused { JobStatus::Paused } else { JobStatus::Queued },
                size: None,
                downloaded: 0,
                speed: 0.0,
                eta: None,
                error: None,
                resumable: true,
                priority: req.priority,
                start_at: req.start_at,
                created_at: now_ts(),
                started_at: None,
                completed_at: None,
                options: options.clone(),
                networks: Vec::new(),
                blocks_total: 0,
                blocks_done: 0,
                checksum: None,
                etag: None,
                last_modified: None,
                log: Vec::new(),
                mime: None,
                kind,
                torrent: None,
                media: None,
                parent: None,
                peak_streams: 0,
                speed_history: Vec::new(),
            };
            let entry = Arc::new(JobEntry {
                limiter: Arc::new(Limiter::new(options.speed_limit)),
                job: Mutex::new(job.clone()),
                ctl: Mutex::new(Ctl::default()),
            });
            self.inner.jobs.write().push(entry);
            self.inner.emit(Event::JobAdded(Box::new(job.clone())));
            added.push(job);
        }
        if added.is_empty() && !errors.is_empty() {
            return Err(Error::msg(errors.join("; ")));
        }
        self.inner.dirty.store(true, Relaxed);
        self.inner.kick.notify_one();
        Ok(added)
    }

    pub fn pause(&self, id: &str) -> Result<()> {
        let e = self.inner.find(id).ok_or(Error::NotFound)?;
        self.inner.stop_entry(&e, Want::Pause);
        Ok(())
    }

    pub fn resume(&self, id: &str) -> Result<()> {
        let e = self.inner.find(id).ok_or(Error::NotFound)?;
        let st = e.job.lock().status;
        if matches!(st, JobStatus::Paused | JobStatus::Failed | JobStatus::Cancelled) {
            self.inner.update(&e, |j| {
                j.status = JobStatus::Queued;
                j.error = None;
                // A manual resume overrides any "start later".
                j.start_at = None;
            });
            self.inner.kick.notify_one();
        }
        Ok(())
    }

    pub fn pause_all(&self) {
        for e in self.inner.entries() {
            let st = e.job.lock().status;
            if st.is_running() || st == JobStatus::Queued {
                self.inner.stop_entry(&e, Want::Pause);
            }
        }
    }

    pub fn resume_all(&self) {
        for e in self.inner.entries() {
            // Copy what we need out first: `resume` locks the job again.
            let (id, paused) = {
                let j = e.job.lock();
                (j.id.clone(), j.status == JobStatus::Paused)
            };
            if paused {
                let _ = self.resume(&id);
            }
        }
    }

    /// Stop and forget the partial data; the job stays in the list as cancelled.
    pub fn cancel(&self, id: &str) -> Result<()> {
        let e = self.inner.find(id).ok_or(Error::NotFound)?;
        self.inner.stop_entry(&e, Want::Cancel { delete: true });
        Ok(())
    }

    /// Remove from the list. `delete_file` also deletes a finished download from disk.
    pub fn remove(&self, id: &str, delete_file: bool) -> Result<()> {
        let e = self.inner.find(id).ok_or(Error::NotFound)?;
        self.inner.stop_entry(&e, Want::Remove { delete: delete_file });
        Ok(())
    }

    pub fn clear_finished(&self) {
        let ids: Vec<String> = self
            .inner
            .entries()
            .iter()
            .filter(|e| matches!(e.job.lock().status, JobStatus::Completed | JobStatus::Cancelled))
            .map(|e| e.job.lock().id.clone())
            .collect();
        for id in ids {
            let _ = self.remove(&id, false);
        }
    }

    /// Start over from nothing, even if there is saved progress.
    pub fn restart(&self, id: &str) -> Result<()> {
        let e = self.inner.find(id).ok_or(Error::NotFound)?;
        if e.job.lock().status.is_running() {
            return Err(Error::msg("Pause the download first, then start it over."));
        }
        self.inner.discard_partial(&e);
        self.inner.update(&e, |j| {
            j.status = JobStatus::Queued;
            j.error = None;
            j.downloaded = 0;
            j.size = None;
            j.checksum = None;
            j.log.clear();
            j.completed_at = None;
            j.path = None;
        });
        self.inner.kick.notify_one();
        Ok(())
    }

    pub fn set_priority(&self, id: &str, priority: i32) -> Result<()> {
        let e = self.inner.find(id).ok_or(Error::NotFound)?;
        self.inner.update(&e, |j| j.priority = priority);
        self.inner.kick.notify_one();
        Ok(())
    }

    /// Put at the front of the queue.
    pub fn move_to_top(&self, id: &str) -> Result<()> {
        let top = self.list().iter().map(|j| j.priority).max().unwrap_or(0);
        self.set_priority(id, top + 1)
    }

    pub fn update_job(&self, id: &str, patch: JobPatch) -> Result<Job> {
        let e = self.inner.find(id).ok_or(Error::NotFound)?;
        let job = self.inner.update(&e, |j| {
            if let Some(p) = patch.priority {
                j.priority = p;
            }
            if let Some(s) = patch.start_at {
                j.start_at = s;
            }
            if let Some(o) = &patch.options {
                j.options = o.clone();
            }
        });
        if let Some(o) = &patch.options {
            e.limiter.set_rate(o.speed_limit);
        }
        self.inner.kick.notify_one();
        Ok(job)
    }

    pub fn update_settings(&self, patch: serde_json::Value) -> Result<Settings> {
        let cur = serde_json::to_value(self.settings())?;
        let merged = merge(cur, patch);
        let mut next: Settings = serde_json::from_value(merged).map_err(|e| Error::msg(format!("Those settings aren't valid: {e}")))?;
        next.max_concurrent_jobs = next.max_concurrent_jobs.clamp(1, 16);
        next.default_streams = next.default_streams.clamp(1, 64);
        next.max_streams = next.max_streams.clamp(2, 64);
        if next.remote.token.len() < 16 {
            next.remote.token = new_token();
        }
        let repo_changed = self.settings().update_repo != next.update_repo;
        *self.inner.settings.write() = next.clone();
        self.inner.apply_limits();
        if repo_changed {
            // Look straight away so the user sees whether the source they typed works.
            let inner = self.inner.clone();
            tokio::spawn(async move {
                let _ = inner.check_update().await;
            });
        }
        self.inner.dirty.store(true, Relaxed);
        self.inner.emit(Event::Settings(Box::new(next.clone())));
        self.inner.kick.notify_one();
        Ok(next)
    }

    /// Replace (or with `None`, clear) the rule for one network.
    pub fn set_interface_rule(&self, id: &str, rule: Option<InterfaceRule>) -> Settings {
        let next = {
            let mut s = self.inner.settings.write();
            match rule {
                Some(r) => {
                    s.interfaces.insert(id.to_string(), r);
                }
                None => {
                    s.interfaces.remove(id);
                }
            }
            s.clone()
        };
        self.inner.apply_limits();
        self.inner.dirty.store(true, Relaxed);
        self.inner.emit(Event::Settings(Box::new(next.clone())));
        self.inner.emit(Event::Interfaces(self.inner.interface_infos()));
        self.inner.kick.notify_one();
        next
    }

    pub fn regenerate_token(&self) -> Result<Settings> {
        self.update_settings(serde_json::json!({ "remote": { "token": new_token() } }))
    }

    /// Stop everything cleanly and write state. Call before exiting.
    pub async fn shutdown(&self) {
        for e in self.inner.entries() {
            if e.job.lock().status.is_running() {
                self.inner.stop_entry(&e, Want::Pause);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline && self.inner.entries().iter().any(|e| e.ctl.lock().running) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.inner.torrents.shutdown().await;
        self.inner.shutdown.cancel();
        self.inner.persist();
    }
}

#[derive(Default, Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobPatch {
    pub priority: Option<i32>,
    /// `Some(None)` clears the start time.
    #[serde(default, deserialize_with = "double_option")]
    pub start_at: Option<Option<i64>>,
    pub options: Option<JobOptions>,
}

fn double_option<'de, D, T>(d: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    use serde::Deserialize;
    Ok(Some(Option::deserialize(d)?))
}

pub fn new_token() -> String {
    use rand::Rng;
    let mut b = [0u8; 24];
    rand::rng().fill(&mut b);
    hex::encode(b)
}

fn merge(base: serde_json::Value, patch: serde_json::Value) -> serde_json::Value {
    use serde_json::Value::*;
    match (base, patch) {
        (Object(mut a), Object(b)) => {
            for (k, v) in b {
                let cur = a.remove(&k).unwrap_or(Null);
                // `interfaces` is a map the caller edits entry by entry: a null deletes one.
                if v.is_null() && k != "interfaces" && !cur.is_null() && !matches!(cur, Object(_)) {
                    a.insert(k, Null);
                } else {
                    a.insert(k, merge(cur, v));
                }
            }
            Object(a)
        }
        (_, p) => p,
    }
}

// ───────────────────────────── internals ─────────────────────────────

impl Inner {
    pub fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    pub fn entries(&self) -> Vec<Arc<JobEntry>> {
        self.jobs.read().clone()
    }

    pub fn find(&self, id: &str) -> Option<Arc<JobEntry>> {
        self.jobs.read().iter().find(|e| e.job.lock().id == id).cloned()
    }

    pub fn update(&self, e: &Arc<JobEntry>, f: impl FnOnce(&mut Job)) -> Job {
        let job = {
            let mut j = e.job.lock();
            f(&mut j);
            j.clone()
        };
        self.dirty.store(true, Relaxed);
        // The parts of a video are an implementation detail; the video is what is shown.
        if job.parent.is_none() {
            self.emit(Event::JobUpdated(Box::new(job.clone())));
        }
        job
    }

    /// For updates that arrive every half second: the ticker broadcasts the numbers, so only
    /// announce the job when its status changes.
    pub fn update_quiet(&self, e: &Arc<JobEntry>, f: impl FnOnce(&mut Job)) {
        let (job, changed) = {
            let mut j = e.job.lock();
            let before = j.status;
            f(&mut j);
            (j.clone(), j.status != before)
        };
        self.dirty.store(true, Relaxed);
        if changed {
            self.emit(Event::JobUpdated(Box::new(job)));
        }
    }

    /// A hidden download that is one stream of a video.
    pub fn spawn_child(&self, parent: &str, url: &str, options: JobOptions, dir: &std::path::Path, priority: i32) -> Arc<JobEntry> {
        let id = loop {
            let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
            if self.find(&id).is_none() {
                break id;
            }
        };
        let name = options.file_name.clone().unwrap_or_else(|| "part".into());
        let job = Job {
            id,
            url: url.to_string(),
            final_url: None,
            file_name: name,
            dest_dir: dir.to_string_lossy().into_owned(),
            path: None,
            status: JobStatus::Queued,
            size: None,
            downloaded: 0,
            speed: 0.0,
            eta: None,
            error: None,
            resumable: true,
            priority,
            start_at: None,
            created_at: now_ts(),
            started_at: None,
            completed_at: None,
            options: options.clone(),
            networks: Vec::new(),
            blocks_total: 0,
            blocks_done: 0,
            checksum: None,
            etag: None,
            last_modified: None,
            log: Vec::new(),
            mime: None,
            kind: JobKind::Http,
            torrent: None,
            media: None,
            parent: Some(parent.to_string()),
            peak_streams: 0,
            speed_history: Vec::new(),
        };
        let entry = Arc::new(JobEntry { limiter: Arc::new(Limiter::new(options.speed_limit)), job: Mutex::new(job), ctl: Mutex::new(Ctl::default()) });
        self.jobs.write().push(entry.clone());
        self.dirty.store(true, Relaxed);
        entry
    }

    /// Forget the parts of a video (and what they fetched).
    pub fn drop_children(self: &Arc<Self>, parent: &str) {
        let kids: Vec<Arc<JobEntry>> = self.entries().into_iter().filter(|c| c.job.lock().parent.as_deref() == Some(parent)).collect();
        for c in kids {
            self.stop_entry(&c, Want::Remove { delete: true });
        }
    }

    /// Add downloads from inside the engine (a playlist fanning out into its videos).
    pub fn add_internal(self: &Arc<Self>, req: AddRequest) -> Result<Vec<Job>> {
        Engine { inner: self.clone() }.add(req)
    }

    pub fn probe_client(&self, proxy: &Option<String>) -> Result<reqwest::Client> {
        match proxy.as_deref().filter(|p| !p.is_empty()) {
            None => Ok(self.http.clone()),
            Some(p) => crate::http::build_client(&crate::http::ClientSpec { proxy: Some(p.into()), ..Default::default() }),
        }
    }

    pub fn net_limiter(&self, id: &str) -> Arc<Limiter> {
        let rate = self.settings.read().interfaces.get(id).and_then(|r| r.speed_limit);
        let mut m = self.net_limiters.lock();
        let l = m.entry(id.to_string()).or_insert_with(|| Arc::new(Limiter::new(rate)));
        l.clone()
    }

    pub fn usage_counter(&self, id: &str, period: CapPeriod) -> Arc<AtomicU64> {
        let now = chrono::Local::now();
        let key = match period {
            CapPeriod::Daily => now.format("%Y-%m-%d").to_string(),
            CapPeriod::Monthly => now.format("%Y-%m").to_string(),
        };
        let mut m = self.usage.lock();
        let entry = m.entry(id.to_string()).or_insert_with(|| (key.clone(), Arc::new(AtomicU64::new(0))));
        if entry.0 != key {
            // New period: start counting from zero.
            *entry = (key, Arc::new(AtomicU64::new(0)));
        }
        entry.1.clone()
    }

    pub fn apply_limits(&self) {
        let s = self.settings.read();
        self.global_limiter.set_rate(s.global_speed_limit);
        for (id, l) in self.net_limiters.lock().iter() {
            l.set_rate(s.interfaces.get(id).and_then(|r| r.speed_limit));
        }
    }

    pub fn stop_entry(self: &Arc<Self>, e: &Arc<JobEntry>, want: Want) {
        let (running, run, token) = {
            let mut c = e.ctl.lock();
            if c.running {
                c.want = want;
            }
            (c.running, c.run.clone(), c.token.clone())
        };
        if running {
            token.cancel();
            if let Some(r) = run {
                r.stop();
            }
            return;
        }
        // Not running: apply immediately.
        self.apply_stopped(e, want);
        self.kick.notify_one();
    }

    /// What to do with a job that is not (or no longer) running, per what was asked of it.
    pub fn apply_stopped(self: &Arc<Self>, e: &Arc<JobEntry>, want: Want) {
        match want {
            Want::None | Want::Pause => {
                let st = e.job.lock().status;
                if !st.is_finished() {
                    self.update(e, |j| {
                        j.status = JobStatus::Paused;
                        j.speed = 0.0;
                        j.eta = None;
                        for n in &mut j.networks {
                            n.speed = 0.0;
                            n.active = 0;
                        }
                    });
                }
            }
            Want::Requeue => {
                self.update(e, |j| {
                    j.status = JobStatus::Queued;
                    j.speed = 0.0;
                    j.eta = None;
                });
            }
            Want::Cancel { delete } => {
                if delete {
                    self.discard_partial(e);
                }
                self.update(e, |j| {
                    j.status = JobStatus::Cancelled;
                    j.speed = 0.0;
                    j.eta = None;
                    j.downloaded = 0;
                });
            }
            Want::Remove { delete } => {
                let (id, path, finished) = {
                    let j = e.job.lock();
                    (j.id.clone(), j.path.clone(), j.status == JobStatus::Completed)
                };
                self.discard_partial(e);
                if delete && finished {
                    if let Some(p) = path {
                        let _ = std::fs::remove_file(p);
                    }
                }
                self.jobs.write().retain(|x| !Arc::ptr_eq(x, e));
                self.dirty.store(true, Relaxed);
                self.emit(Event::JobRemoved(id));
            }
        }
    }

    pub fn discard_partial(self: &Arc<Self>, e: &Arc<JobEntry>) {
        let (dir, name, finished, torrent_path) = {
            let j = e.job.lock();
            (PathBuf::from(&j.dest_dir), j.file_name.clone(), j.status == JobStatus::Completed, (j.kind == JobKind::Torrent).then(|| j.path.clone()).flatten())
        };
        // A video's parts live in a hidden folder next to where it will end up.
        let is_media = e.job.lock().kind == JobKind::Media;
        if is_media && !finished {
            let id = e.job.lock().id.clone();
            self.drop_children(&id);
            let _ = std::fs::remove_dir_all(dir.join(format!(".idmx-{id}")));
        }
        // A torrent's files are in a folder (or a single file) that IDMX created for it.
        if let (false, Some(p)) = (finished, torrent_path) {
            let p = PathBuf::from(p);
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
            e.ctl.lock().torrent = None;
        }
        if !finished {
            let _ = std::fs::remove_file(crate::util::staging_path(&dir, &name));
            self.reserved.lock().remove(&dir.join(&name));
        }
        e.ctl.lock().saved = None;
    }

    pub fn stats(&self) -> GlobalStats {
        let mut s = GlobalStats::default();
        for e in self.jobs.read().iter() {
            let j = e.job.lock();
            if j.parent.is_some() {
                continue;
            }
            match j.status {
                JobStatus::Downloading | JobStatus::Probing => {
                    s.active += 1;
                    s.speed += j.speed;
                }
                JobStatus::Verifying | JobStatus::Processing => s.active += 1,
                JobStatus::Seeding => s.seeding += 1,
                JobStatus::Queued => s.queued += 1,
                JobStatus::Completed => s.completed += 1,
                JobStatus::Failed => s.failed += 1,
                _ => {}
            }
        }
        s.total_bytes = self.total_bytes.load(Relaxed);
        s.session_bytes = self.session_bytes.load(Relaxed);
        s
    }

    /// Networks that would be used by default (usable, and not switched off by their rule).
    pub fn default_enabled_ids(&self) -> Vec<String> {
        let settings = self.settings.read();
        self.ifaces
            .read()
            .iter()
            .filter(|i| i.usable && settings.interfaces.get(&i.id).and_then(|r| r.enabled).unwrap_or(i.kind.default_enabled()))
            .map(|i| i.id.clone())
            .collect()
    }

    pub fn interface_infos(&self) -> Vec<InterfaceInfo> {
        let settings = self.settings.read();
        let live = self.live.lock();
        let ifaces = self.ifaces.read();
        let ping = self.ping.lock();
        ifaces
            .iter()
            .map(|i| {
                let rule = settings.interfaces.get(&i.id).cloned().unwrap_or_default();
                let enabled = rule.enabled.unwrap_or(i.kind.default_enabled());
                let l = live.get(&i.id);
                let used = self.usage.lock().get(&i.id).map(|(_, c)| c.load(Relaxed)).unwrap_or(0);
                let capped = rule.data_cap.is_some_and(|c| used >= c);
                let pinged = ping.get(&i.id).copied();
                let reachable = pinged.map(|p| p.is_some());
                let state = if !i.usable {
                    NetState::Offline
                } else if !enabled {
                    NetState::Disabled
                } else if capped {
                    NetState::Capped
                } else if let Some(s) = l.map(|s| s.state).filter(|s| matches!(s, NetState::Unreachable | NetState::Failed)) {
                    s
                } else if l.is_some_and(|s| s.speed > 1.0) {
                    NetState::Active
                } else if reachable == Some(false) {
                    // Connected to a network, but nothing gets through it to the internet.
                    NetState::Unreachable
                } else {
                    NetState::Idle
                };
                let sharing = |same: &dyn Fn(&NetInterface) -> bool| -> Vec<String> {
                    if !i.usable {
                        return vec![];
                    }
                    ifaces.iter().filter(|o| o.id != i.id && o.usable && same(o)).map(|o| o.id.clone()).collect()
                };
                let shares = sharing(&|o| i.gateway.is_some() && o.gateway == i.gateway);
                let shares_subnet = sharing(&|o| i.subnets.iter().any(|s| o.subnets.contains(s)));
                InterfaceInfo {
                    iface: i.clone(),
                    enabled,
                    health: l.map(|s| s.health).unwrap_or(100),
                    latency_ms: l.map(|s| s.latency_ms).filter(|v| *v > 0.0).or(pinged.flatten()),
                    used_bytes: used,
                    speed: l.map(|s| s.speed).unwrap_or(0.0),
                    shares_gateway_with: shares,
                    shares_subnet_with: shares_subnet,
                    reachable,
                    state,
                    rule,
                }
            })
            .collect()
    }

    // ───── ping and update checks ─────

    pub(crate) async fn ping_round(self: &Arc<Self>) {
        let nets: Vec<NetInterface> = self.ifaces.read().iter().filter(|i| i.usable).cloned().collect();
        let bind = self.bind_device;
        let target = self.ping_target;
        let results = futures_util::future::join_all(nets.into_iter().map(|n| async move {
            let ms = match target {
                Some(t) => crate::ping::ping_targets(&n, bind, &[t]).await,
                None => crate::ping::ping(&n, bind).await,
            };
            (n.id, ms)
        }))
        .await;
        let mut map = self.ping.lock();
        map.retain(|k, _| self.ifaces.read().iter().any(|i| &i.id == k));
        for (id, ms) in results {
            map.insert(id, ms);
        }
    }

    /// Every 20 s, and as soon as the set of networks changes.
    async fn pinger(self: Arc<Self>) {
        loop {
            self.ping_round().await;
            tokio::select! {
                _ = self.shutdown.cancelled() => return,
                _ = self.ping_kick.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(20)) => {}
            }
        }
    }

    pub(crate) async fn check_update(self: &Arc<Self>) -> Result<Option<UpdateInfo>> {
        let (repo, dismissed) = {
            let s = self.settings.read();
            (s.update_repo.clone(), s.dismissed_update.clone())
        };
        let source = self.update_url.clone().unwrap_or(repo);
        if source.trim().is_empty() {
            *self.update.lock() = None;
            return Ok(None);
        }
        let found = crate::update::check(&self.http, &source, env!("CARGO_PKG_VERSION")).await?;
        let info = found.map(|mut u| {
            u.dismissed = dismissed.as_deref() == Some(u.version.as_str());
            u
        });
        let changed = *self.update.lock() != info;
        *self.update.lock() = info.clone();
        if changed {
            self.emit(Event::Update(info.clone()));
        }
        Ok(info)
    }

    /// Soon after launch, then every 12 hours. A failure (offline, rate-limited) is not worth
    /// telling anyone about.
    async fn updater(self: Arc<Self>) {
        tokio::select! {
            _ = self.shutdown.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(4)) => {}
        }
        loop {
            if let Err(e) = self.check_update().await {
                tracing::debug!("update check failed: {e}");
            }
            tokio::select! {
                _ = self.shutdown.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(12 * 3600)) => {}
            }
        }
    }

    pub fn persist(&self) {
        let jobs: Vec<StoredJob> = self
            .jobs
            .read()
            .iter()
            .map(|e| {
                let saved = {
                    let c = e.ctl.lock();
                    c.run.as_ref().map(|r| r.saved()).or_else(|| c.saved.clone())
                };
                StoredJob { job: e.job.lock().clone(), saved }
            })
            .collect();
        let usage = self.usage.lock().iter().map(|(k, (p, c))| (k.clone(), UsageRec { period: p.clone(), bytes: c.load(Relaxed) })).collect();
        let state = State {
            version: 1,
            jobs,
            settings: self.settings.read().clone(),
            usage,
            total_bytes: self.total_bytes.load(Relaxed),
        };
        if let Err(e) = state.save(&self.data_dir.join("state.json")) {
            tracing::error!("couldn't save state: {e}");
        }
    }

    // ───── background tasks ─────

    /// Every 2 s: look at the networks. A long gap between looks means the machine slept.
    async fn monitor(self: Arc<Self>) {
        if self.fixed_ifaces {
            return;
        }
        let mut last = Instant::now();
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
            let gap = last.elapsed();
            last = Instant::now();
            let Ok(nets) = tokio::task::spawn_blocking(net::discover).await else { continue };
            self.apply_networks(nets, gap > Duration::from_secs(8));
        }
    }

    /// A fresh look at the networks. Updates what we know and tells every running download:
    /// rebind what moved, park what vanished, and offer anything new.
    pub(crate) fn apply_networks(self: &Arc<Self>, nets: Vec<NetInterface>, woke: bool) -> bool {
        let before: Vec<String> = self.ifaces.read().iter().map(net::fingerprint).collect();
        let now: Vec<String> = nets.iter().map(net::fingerprint).collect();
        let changed = before != now;
        let was_online = self.ifaces.read().iter().any(|i| i.usable);
        if changed {
            *self.ifaces.write() = nets.clone();
            let _ = self.iface_tx.send(Arc::new(nets.clone()));
            self.ping_kick.notify_one();
        }
        if !(changed || woke) {
            return false;
        }
        for e in self.entries() {
            let run = e.ctl.lock().run.clone();
            let Some(r) = run else { continue };
            if changed {
                r.networks_changed(&nets);
                for i in nets.iter().filter(|i| i.usable && !r.has_network(&i.id)) {
                    self.offer_network(&e, &r, i);
                }
            }
            if woke {
                r.wake_up();
            }
        }
        if woke {
            self.emit(Event::Notice { level: "info".into(), message: "Welcome back — reconnecting".into() });
        }
        // Back online: downloads that stopped for lack of a network carry on by themselves.
        if changed && !was_online && nets.iter().any(|i| i.usable) {
            let waiting: Vec<Arc<JobEntry>> = self.entries().into_iter().filter(|e| {
                let j = e.job.lock();
                j.status == JobStatus::Failed && j.error.as_deref() == Some(crate::error::NO_NETWORK_MSG)
            }).collect();
            if !waiting.is_empty() {
                for e in &waiting {
                    self.update(e, |j| {
                        j.status = JobStatus::Queued;
                        j.error = None;
                    });
                }
                self.emit(Event::Notice { level: "info".into(), message: format!("You're back online — resuming {} download{}", waiting.len(), if waiting.len() == 1 { "" } else { "s" }) });
            }
            self.kick.notify_one();
        }
        self.emit(Event::Interfaces(self.interface_infos()));
        true
    }

    async fn manager(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => return,
                _ = self.kick.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
            self.schedule();
        }
    }

    pub(crate) fn schedule(self: &Arc<Self>) {
        let s = self.settings.read().clone();
        let now = now_ts();
        let local = chrono::Local::now();
        let minute = local.hour() * 60 + local.minute();
        let in_window = !s.schedule.enabled || window_contains(&s.schedule, minute);

        let mut used = 0u32;
        let mut busy = false;
        let mut eligible: Vec<(i32, i64, Arc<JobEntry>)> = Vec::new();
        for e in self.entries() {
            let (st, win, start_at, prio, created) = {
                let j = e.job.lock();
                (j.status, j.options.scheduled_window, j.start_at, j.priority, j.created_at)
            };
            match st {
                JobStatus::Probing | JobStatus::Downloading => {
                    // A video fetched as separate parts holds no slot of its own: its parts do.
                    let holds_no_slot = {
                        let j = e.job.lock();
                        j.kind == JobKind::Media && j.media.as_ref().is_none_or(|m| m.strategy != "ytdlp")
                    };
                    if !holds_no_slot {
                        used += 1;
                    }
                    busy = true;
                    if win && !in_window {
                        self.stop_entry(&e, Want::Requeue);
                    }
                }
                JobStatus::Verifying | JobStatus::Processing => busy = true,
                JobStatus::Queued => {
                    let ready = start_at.is_none_or(|t| t <= now) && (!win || in_window);
                    if ready {
                        busy = true;
                        eligible.push((-prio, created, e));
                    }
                }
                _ => {}
            }
        }
        eligible.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        // With no network at all there is nothing to start; they wait in the queue instead of
        // failing one by one, and go as soon as a network is back.
        let online = self.ifaces.read().iter().any(|i| i.usable);
        if online {
            for (_, _, e) in eligible.into_iter().take(s.max_concurrent_jobs.saturating_sub(used) as usize) {
                self.spawn_job(e);
            }
        }

        // Keep the machine awake while there is work.
        {
            let mut awake = self.awake.lock();
            if used > 0 && awake.is_none() {
                *awake = Some(os::keep_awake());
            } else if used == 0 && !busy {
                *awake = None;
            }
        }

        if busy {
            self.had_work.store(true, Relaxed);
        } else if self.had_work.swap(false, Relaxed) {
            if s.notifications {
                self.emit(Event::Notice { level: "queue-done".into(), message: "All downloads finished".into() });
            }
            match s.on_queue_empty {
                QueueEmpty::Sleep => os::sleep_system(),
                QueueEmpty::Quit => self.emit(Event::Action { action: "quit".into(), path: None, job_id: None }),
                QueueEmpty::Nothing => {}
            }
        }
    }

    /// Twice a second: fold live numbers from running downloads into jobs and broadcast.
    async fn ticker(self: Arc<Self>) {
        let mut last: HashMap<String, u64> = HashMap::new();
        let mut n = 0u64;
        let mut last_ifaces = String::new();
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
            n += 1;
            let mut ticks = Vec::new();
            let mut live: HashMap<String, NetStat> = HashMap::new();
            for e in self.entries() {
                let run = e.ctl.lock().run.clone();
                let Some(run) = run else {
                    // Torrents aren't a Run: their task keeps the job's numbers current, and we
                    // just broadcast them.
                    let j = e.job.lock();
                    let media_parent = j.kind == JobKind::Media && matches!(j.status, JobStatus::Downloading | JobStatus::Probing | JobStatus::Processing);
                    if media_parent {
                        ticks.push(JobTick { id: j.id.clone(), status: Some(j.status), downloaded: j.downloaded, speed: j.speed, eta: j.eta, blocks_done: 0, networks: j.networks.clone(), torrent: None });
                        // Parts that run through the engine are already counted; yt-dlp's own isn't.
                        if j.media.as_ref().is_some_and(|m| m.strategy == "ytdlp") {
                            let prev = last.insert(j.id.clone(), j.downloaded).unwrap_or(j.downloaded);
                            let delta = j.downloaded.saturating_sub(prev);
                            self.session_bytes.fetch_add(delta, Relaxed);
                            self.total_bytes.fetch_add(delta, Relaxed);
                        }
                    }
                    if j.kind == JobKind::Torrent && matches!(j.status, JobStatus::Downloading | JobStatus::Probing | JobStatus::Seeding) {
                        let mut live = j.torrent.clone();
                        if let Some(t) = live.as_mut() {
                            t.files.clear();
                        }
                        ticks.push(JobTick { id: j.id.clone(), status: Some(j.status), downloaded: j.downloaded, speed: j.speed, eta: j.eta, blocks_done: 0, networks: j.networks.clone(), torrent: live });
                        let prev = last.insert(j.id.clone(), j.downloaded).unwrap_or(j.downloaded);
                        let delta = j.downloaded.saturating_sub(prev);
                        self.session_bytes.fetch_add(delta, Relaxed);
                        self.total_bytes.fetch_add(delta, Relaxed);
                    }
                    continue;
                };
                let snap = run.snapshot();
                let (id, status) = {
                    let mut j = e.job.lock();
                    j.peak_streams = j.peak_streams.max(snap.nets.iter().map(|n| n.active).sum());
                    // One sample every 2 s; when the record gets long, thin it out by half so a
                    // multi-day download still fits in a few hundred points.
                    if n % 4 == 0 && snap.speed > 0.0 {
                        j.speed_history.push(snap.speed as f32);
                        if j.speed_history.len() > 600 {
                            let thinned: Vec<f32> = j.speed_history.chunks(2).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
                            j.speed_history = thinned;
                        }
                    }
                    j.downloaded = snap.downloaded;
                    j.speed = snap.speed;
                    j.eta = snap.eta;
                    j.blocks_total = snap.blocks_total;
                    j.blocks_done = snap.blocks_done;
                    j.networks = snap.nets.clone();
                    (j.id.clone(), j.status)
                };
                let prev = last.insert(id.clone(), snap.downloaded).unwrap_or(snap.downloaded);
                let delta = snap.downloaded.saturating_sub(prev);
                self.session_bytes.fetch_add(delta, Relaxed);
                self.total_bytes.fetch_add(delta, Relaxed);
                // A network can serve several downloads at once: add them up.
                for ns in &snap.nets {
                    match live.entry(ns.id.clone()) {
                        std::collections::hash_map::Entry::Vacant(v) => {
                            v.insert(ns.clone());
                        }
                        std::collections::hash_map::Entry::Occupied(mut o) => {
                            let s = o.get_mut();
                            s.speed += ns.speed;
                            s.bytes += ns.bytes;
                            s.health = s.health.min(ns.health);
                            if matches!(ns.state, NetState::Unreachable | NetState::Failed) {
                                s.state = ns.state;
                            }
                        }
                    }
                }
                ticks.push(JobTick {
                    id,
                    status: Some(status),
                    downloaded: snap.downloaded,
                    speed: snap.speed,
                    eta: snap.eta,
                    blocks_done: snap.blocks_done,
                    networks: snap.nets,
                    torrent: None,
                });
            }
            *self.live.lock() = live;
            self.emit(Event::Tick { stats: self.stats(), jobs: ticks });
            if n % 2 == 0 {
                let infos = self.interface_infos();
                let fp = serde_json::to_string(&infos).unwrap_or_default();
                if fp != last_ifaces {
                    last_ifaces = fp;
                    self.emit(Event::Interfaces(infos));
                }
            }
        }
    }

    async fn saver(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
            // Running downloads change constantly; always snapshot those.
            let any_running = self.entries().iter().any(|e| e.ctl.lock().run.is_some());
            if self.dirty.swap(false, Relaxed) || any_running {
                let me = self.clone();
                let _ = tokio::task::spawn_blocking(move || me.persist()).await;
            }
        }
    }
}

pub fn window_contains(w: &ScheduleWindow, minute: u32) -> bool {
    if w.start_minute == w.end_minute {
        return true;
    }
    if w.start_minute < w.end_minute {
        minute >= w.start_minute && minute < w.end_minute
    } else {
        // Overnight: 22:00 → 06:00.
        minute >= w.start_minute || minute < w.end_minute
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_windows() {
        let day = ScheduleWindow { enabled: true, start_minute: 9 * 60, end_minute: 17 * 60 };
        assert!(window_contains(&day, 12 * 60));
        assert!(!window_contains(&day, 18 * 60));
        let night = ScheduleWindow { enabled: true, start_minute: 22 * 60, end_minute: 6 * 60 };
        assert!(window_contains(&night, 23 * 60));
        assert!(window_contains(&night, 2 * 60));
        assert!(!window_contains(&night, 12 * 60));
    }

    #[test]
    fn merge_is_deep_and_keeps_siblings() {
        let base = serde_json::json!({"a": {"x": 1, "y": 2}, "b": 3});
        let out = merge(base, serde_json::json!({"a": {"y": 9}}));
        assert_eq!(out, serde_json::json!({"a": {"x": 1, "y": 9}, "b": 3}));
    }

    #[test]
    fn tokens_are_long_and_unique() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 48);
        assert_ne!(a, b);
    }
}

#[cfg(test)]
mod offline_tests {
    use super::*;

    #[tokio::test]
    async fn a_job_that_failed_for_lack_of_network_is_requeued_when_one_returns() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::start_with(dir.path().join("data"), EngineOptions { fixed_interfaces: Some(vec![]), no_device_binding: true, ..Default::default() }).await.unwrap();
        let job = e.add(AddRequest { urls: vec!["http://127.0.0.1:9/a.bin".into()], paused: true, ..Default::default() }).unwrap().remove(0);
        let entry = e.inner.find(&job.id).unwrap();
        // The state an older run (or a lost race) would have left behind.
        e.inner.update(&entry, |j| {
            j.status = JobStatus::Failed;
            j.error = Some(crate::error::NO_NETWORK_MSG.to_string());
        });
        // An unrelated failure must not be touched.
        let other = e.add(AddRequest { urls: vec!["http://127.0.0.1:9/b.bin".into()], paused: true, ..Default::default() }).unwrap().remove(0);
        let other_entry = e.inner.find(&other.id).unwrap();
        e.inner.update(&other_entry, |j| {
            j.status = JobStatus::Failed;
            j.error = Some("The file isn't there (404).".into());
        });
        let lo = NetInterface { id: "lo".into(), label: "lo".into(), kind: IfKind::Ethernet, ipv4: vec!["127.0.0.1".into()], ipv6: vec![], gateway: None, subnets: vec![], mac: None, up: true, link_mbps: None, usable: true };
        let mut ev = e.subscribe();
        e.simulate_networks(vec![lo]);
        assert_ne!(e.get(&job.id).unwrap().status, JobStatus::Failed, "should have been put back in the queue");
        assert_eq!(e.get(&other.id).unwrap().status, JobStatus::Failed);
        let mut told = false;
        while let Ok(x) = ev.try_recv() {
            told |= matches!(x, Event::Notice { ref message, .. } if message.contains("back online"));
        }
        assert!(told, "the user should be told");
        e.shutdown().await;
    }
}

/// A readable name for a torrent that hasn't told us its real one yet: the magnet's `dn=`, the
/// `.torrent`'s file name, or the info hash.
pub fn torrent_display_name(link: &str) -> String {
    if link.to_ascii_lowercase().starts_with("magnet:") {
        if let Ok(u) = url::Url::parse(link) {
            if let Some((_, v)) = u.query_pairs().find(|(k, _)| k == "dn") {
                if !v.trim().is_empty() {
                    return crate::util::sanitize_filename(&v);
                }
            }
            if let Some((_, v)) = u.query_pairs().find(|(k, _)| k == "xt") {
                let h = v.rsplit(':').next().unwrap_or("");
                return format!("torrent {}", &h[..h.len().min(8)]);
            }
        }
        return "torrent".into();
    }
    let path = link.split(['?', '#']).next().unwrap_or(link);
    let n = path.rsplit(['/', '\\']).next().unwrap_or("torrent");
    crate::util::sanitize_filename(n.trim_end_matches(".torrent"))
}

/// A readable name for a video page before yt-dlp has told us its title.
pub fn media_display_name(link: &str) -> String {
    let Ok(u) = url::Url::parse(link) else { return "video".into() };
    let host = u.host_str().unwrap_or("video").trim_start_matches("www.");
    let id = u.query_pairs().find(|(k, _)| k == "v").map(|(_, v)| v.into_owned()).or_else(|| u.path_segments().and_then(|mut p| p.next_back().map(str::to_string))).filter(|s| !s.is_empty());
    match id {
        Some(id) => format!("{host} · {id}"),
        None => host.to_string(),
    }
}
