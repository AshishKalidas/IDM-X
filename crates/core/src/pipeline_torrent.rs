//! A torrent job from "added" to "seeded": find it, download it, share it back.

use crate::engine::{Inner, JobEntry, Want};
use crate::error::{Error, Result};
use crate::torrent::{self, AddOpts, Handle, Source};
use crate::types::*;
use crate::util::{now_ts, sanitize_filename};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum TDone {
    Completed,
    Stopped,
}

impl Inner {
    /// Torrent session settings, from the user's preferences.
    pub(crate) fn torrent_cfg(&self) -> torrent::SessionCfg {
        let s = self.settings.read();
        let t = &s.torrent;
        // Only bind to a network that's actually up; otherwise the system chooses.
        let bind = t.network.clone().filter(|id| self.ifaces.read().iter().any(|i| &i.id == id && i.usable));
        torrent::SessionCfg {
            bind_device: if self.bind_device { bind } else { None },
            dht: t.dht,
            listen_port: t.listen_port,
            upload_bps: t.upload_limit.map(|v| v.min(u32::MAX as u64) as u32),
            download_bps: s.global_speed_limit.map(|v| v.min(u32::MAX as u64) as u32),
            peer_limit: t.peer_limit.map(|p| p as usize),
            isolated: self.torrent_isolated,
        }
    }

    /// The sessions to run: one per enabled network when several are up (and the user hasn't pinned
    /// torrents to a single one), else just the usual one.
    pub(crate) fn torrent_cfgs(&self) -> Vec<torrent::SessionCfg> {
        let base = self.torrent_cfg();
        let s = self.settings.read();
        if !s.torrent.multi_network || s.torrent.network.is_some() || !self.bind_device || self.torrent_isolated {
            return vec![base];
        }
        let nets: Vec<String> = self.ifaces.read().iter().filter(|i| i.usable && s.interfaces.get(&i.id).and_then(|r| r.enabled).unwrap_or(i.kind.default_enabled())).map(|i| i.id.clone()).collect();
        if nets.len() < 2 {
            return vec![base];
        }
        nets.into_iter().enumerate().map(|(k, id)| torrent::SessionCfg { bind_device: Some(id), listen_port: if k == 0 { base.listen_port } else { None }, ..base.clone() }).collect()
    }

    pub(crate) async fn torrent_pipeline(self: &Arc<Self>, entry: &Arc<JobEntry>) -> Result<TDone> {
        let token = entry.ctl.lock().token.clone();
        let settings = self.settings.read().clone();
        if !settings.torrent.enabled {
            return Err(Error::msg("Torrents are turned off. Switch them on in Settings → Torrents."));
        }
        let (url, opts, dest_dir, resumed_path) = {
            let j = entry.job.lock();
            (j.url.clone(), j.options.clone(), j.dest_dir.clone(), j.path.clone())
        };
        let source = Source::parse(&url).ok_or_else(|| Error::msg("That isn't a magnet link or a .torrent file."))?;
        let cfg = self.torrent_cfg();
        let mut ever_seeded = false;
        let _ = &mut ever_seeded;

        self.update(entry, |j| {
            j.status = JobStatus::Probing;
            j.error = None;
            j.started_at.get_or_insert_with(now_ts);
            j.torrent.get_or_insert_with(TorrentStatus::default).fetching_metadata = matches!(source, Source::Magnet(_));
        });
        if !self.ifaces.read().iter().any(|i| i.usable) && !self.fixed_ifaces {
            return Err(Error::NoNetwork);
        }

        // ── 1. what is in it? (a magnet link has to ask the swarm for this first) ──
        let existing = entry.ctl.lock().torrent.clone();
        let handle: Handle = match existing {
            Some(h) => {
                // Paused earlier in this run of the app: just carry on.
                self.torrents.unpause(&h).await?;
                h
            }
            None => {
                let meta = tokio::select! {
                    _ = token.cancelled() => return Ok(TDone::Stopped),
                    m = self.torrents.inspect(&cfg, &source, &self.http, &self.torrent_peers, Duration::from_secs(240)) => m?,
                };
                let name = sanitize_filename(&meta.name);
                let multi = meta.files.len() > 1;
                let dest = PathBuf::from(&dest_dir);
                std::fs::create_dir_all(&dest).map_err(|e| Error::msg(format!("Can't save to “{}”: {}", dest.display(), crate::error::io_message(&e))))?;
                // Where the files will live. A multi-file torrent gets a folder of its own (a fresh
                // one, never an existing folder we might write over); a single file goes straight in
                // the destination, unless something already has that name.
                let output = match (&resumed_path, multi) {
                    (Some(p), _) => {
                        let p = PathBuf::from(p);
                        if multi { p } else { p.parent().map(Path::to_path_buf).unwrap_or(dest.clone()) }
                    }
                    (None, true) => dest.join(crate::util::unique_name(&dest, &name, |_| false)),
                    (None, false) => {
                        let first = meta.files.first().map(|f| f.path.clone()).unwrap_or_else(|| name.clone());
                        if dest.join(&first).exists() {
                            return Err(Error::msg(format!("A file named “{first}” is already in that folder. Move it away or choose another folder, then try again.")));
                        }
                        dest.clone()
                    }
                };
                let only: Option<Vec<usize>> = opts.torrent_files.as_ref().map(|v| v.iter().map(|i| *i as usize).collect());
                let shown_path = if multi { output.clone() } else { output.join(meta.files.first().map(|f| f.path.as_str()).unwrap_or(&name)) };
                self.update(entry, |j| {
                    j.file_name = name.clone();
                    j.size = Some(match &only {
                        Some(sel) => meta.files.iter().filter(|f| sel.contains(&(f.index as usize))).map(|f| f.size).sum(),
                        None => meta.total,
                    });
                    j.path = Some(shown_path.to_string_lossy().into_owned());
                    j.resumable = true;
                    if let Some(t) = j.torrent.as_mut() {
                        t.info_hash = meta.info_hash.clone();
                        t.fetching_metadata = false;
                    }
                });
                if Self::stop_requested(entry) {
                    return Ok(TDone::Stopped);
                }
                let wanted: Vec<(usize, u64)> = meta.files.iter().filter(|f| only.as_ref().is_none_or(|o| o.contains(&(f.index as usize)))).map(|f| (f.index as usize, f.size)).collect();
                let cfgs = self.torrent_cfgs();
                let add = AddOpts { output, only_files: only, initial_peers: self.torrent_peers.clone(), paused: false };
                let h = if cfgs.len() > 1 && wanted.len() > 1 {
                    self.torrents.add_multi(&cfgs, &source, &self.http, add, &wanted).await?
                } else {
                    self.torrents.add(&cfg, &source, &self.http, add).await?
                };
                if h.parts.len() > 1 {
                    self.log(entry, &format!("Splitting this torrent's {} files across {} networks.", wanted.len(), h.parts.len()));
                }
                entry.ctl.lock().torrent = Some(h.clone());
                h
            }
        };

        self.update(entry, |j| {
            j.status = JobStatus::Downloading;
            j.resumable = true;
        });
        self.torrent_loop(entry, &handle, &token, &settings).await
    }

    fn stop_requested(entry: &JobEntry) -> bool {
        entry.ctl.lock().want != Want::None
    }

    /// Watch a running torrent until it stops, and carry it through seeding.
    async fn torrent_loop(self: &Arc<Self>, entry: &Arc<JobEntry>, h: &Handle, token: &tokio_util::sync::CancellationToken, settings: &Settings) -> Result<TDone> {
        let net_id = self.torrent_cfg().bind_device.unwrap_or_else(|| "default".to_string());
        let net_label = self.ifaces.read().iter().find(|i| i.id == net_id).map(|i| i.label.clone()).unwrap_or_else(|| "System default".to_string());
        let kind = self.ifaces.read().iter().find(|i| i.id == net_id).map(|i| i.kind).unwrap_or(IfKind::Unknown);
        let mut seeding_since: Option<Instant> = None;
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                _ = token.cancelled() => return self.torrent_stop(entry, h, seeding_since.is_some()).await,
                _ = tick.tick() => {}
            }
            let s = torrent::snapshot(h);
            if let Some(e) = &s.error {
                return Err(Error::msg(format!("The torrent engine stopped this download: {e}")));
            }
            let seeding = s.finished;
            let ratio = if s.downloaded > 0 { s.uploaded as f64 / s.downloaded as f64 } else { 0.0 };
            let state = if !s.have_metadata { "initializing" } else if seeding { "seeding" } else if s.state == "paused" { "paused" } else if s.state == "initializing" { "initializing" } else { "downloading" };
            self.update_quiet(entry, |j| {
                j.downloaded = s.downloaded;
                j.size = Some(s.total.max(j.size.unwrap_or(0).min(s.total.max(1)))).filter(|v| *v > 0).or(j.size);
                j.speed = s.down_bps;
                j.eta = s.eta;
                j.status = if seeding { JobStatus::Seeding } else if s.have_metadata && s.state != "initializing" { JobStatus::Downloading } else { JobStatus::Probing };
                j.networks = if s.parts.len() > 1 {
                    s.parts
                        .iter()
                        .map(|p| {
                            let (label, kind) = p.net.as_ref().and_then(|id| self.ifaces.read().iter().find(|i| &i.id == id).map(|i| (i.label.clone(), i.kind))).unwrap_or(("System default".into(), IfKind::Unknown));
                            NetStat { id: p.net.clone().unwrap_or_default(), label, kind, bytes: p.downloaded, speed: p.down_bps, streams: p.peers, active: p.peers, latency_ms: 0.0, health: 100, errors: 0, state: NetState::Active, error: None, enabled: true }
                        })
                        .collect()
                } else {
                    vec![NetStat { id: net_id.clone(), label: net_label.clone(), kind, bytes: s.downloaded, speed: s.down_bps, streams: s.peers, active: s.peers, latency_ms: 0.0, health: 100, errors: 0, state: NetState::Active, error: None, enabled: true }]
                };
                j.peak_streams = j.peak_streams.max(s.peers);
                if !seeding {
                    j.speed_history.push(s.down_bps as f32);
                    if j.speed_history.len() > 600 {
                        j.speed_history = j.speed_history.chunks(2).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
                    }
                }
                j.torrent = Some(TorrentStatus {
                    info_hash: s.info_hash.clone(),
                    peers: s.peers,
                    known_peers: s.known_peers,
                    uploaded: s.uploaded,
                    upload_speed: s.up_bps,
                    ratio,
                    state: state.into(),
                    files: s.files.iter().enumerate().map(|(i, f)| TorrentFile { index: i as u32, path: f.0.clone(), size: f.1, downloaded: f.2, selected: f.3 }).collect(),
                    fetching_metadata: !s.have_metadata,
                });
                if j.file_name.is_empty() || j.file_name.len() < 2 {
                    j.file_name = s.name.clone();
                }
            });

            if seeding {
                let since = *seeding_since.get_or_insert_with(|| {
                    // Download finished: say so once, then decide whether to keep sharing.
                    self.emit(Event::Notice { level: "complete".into(), message: entry.job.lock().file_name.clone() });
                    Instant::now()
                });
                let t = &settings.torrent;
                let done_by_ratio = t.seed_ratio > 0.0 && ratio >= t.seed_ratio;
                let done_by_time = t.seed_minutes > 0 && since.elapsed() >= Duration::from_secs(t.seed_minutes as u64 * 60);
                if !t.seed || done_by_ratio || done_by_time {
                    return self.torrent_stop(entry, h, true).await;
                }
            }
        }
    }

    /// The loop ended: because the user asked, or because there is nothing left to do.
    async fn torrent_stop(self: &Arc<Self>, entry: &Arc<JobEntry>, h: &Handle, finished: bool) -> Result<TDone> {
        let want = entry.ctl.lock().want;
        match want {
            Want::Cancel { delete } | Want::Remove { delete } if !finished => {
                // Nothing worth keeping: drop it and (usually) what it fetched.
                let _ = self.torrents.remove(h, delete).await;
                entry.ctl.lock().torrent = None;
                Ok(TDone::Stopped)
            }
            Want::Pause | Want::Requeue if !finished => {
                // Keep it in the session, paused, so resuming is instant.
                self.torrents.pause(h).await?;
                Ok(TDone::Stopped)
            }
            _ => {
                // Finished (or the user stopped sharing): files stay, the torrent leaves the session.
                let snap = torrent::snapshot(h);
                let _ = self.torrents.remove(h, false).await;
                entry.ctl.lock().torrent = None;
                self.update(entry, |j| {
                    j.status = JobStatus::Completed;
                    j.completed_at = Some(now_ts());
                    j.speed = 0.0;
                    j.eta = None;
                    j.downloaded = snap.downloaded.max(j.downloaded);
                    if let Some(t) = j.torrent.as_mut() {
                        t.state = "completed".into();
                        t.peers = 0;
                        t.upload_speed = 0.0;
                    }
                });
                Ok(TDone::Completed)
            }
        }
    }
}
