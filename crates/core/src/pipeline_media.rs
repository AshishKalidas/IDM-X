//! A video or audio page: ask yt-dlp what it is, fetch the streams (across all your networks when
//! they're plain files), put them together, and file the result.

use crate::engine::{Inner, JobEntry, Want};
use crate::error::{Error, Result};
use crate::media::{self, Extracted, RunOpts, Tool};
use crate::types::*;
use crate::util::{now_ts, sanitize_title, unique_name};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Shown (and recognised by the UI, which offers an install button) when yt-dlp isn't available.
pub const YTDLP_MISSING_MSG: &str = "Saving videos needs yt-dlp, which isn't installed yet. Open Settings → Video downloads and press Install — it takes a few seconds.";

pub enum MDone {
    Completed,
    Stopped,
}

impl Inner {
    pub(crate) async fn tools(&self, refresh: bool) -> (Option<Tool>, Option<Tool>) {
        {
            let c = self.tool_cache.lock();
            if !refresh && c.at.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
                return (c.ytdlp.clone(), c.ffmpeg.clone());
            }
        }
        let (yp, fp, jp) = {
            let s = self.settings.read();
            (s.media.ytdlp_path.clone(), s.media.ffmpeg_path.clone(), s.media.js_runtime_path.clone())
        };
        let (y, f) = tokio::join!(media::detect("yt-dlp", &yp, &self.data_dir), media::detect("ffmpeg", &fp, &self.data_dir));
        let js = match media::find_js_runtime(&jp, &self.data_dir) {
            Some((name, path)) => media::tool_version(&path, "--version").await.map(|v| (name.clone(), Tool { version: format!("{name} {}", v.trim_start_matches(&format!("{name} "))), managed: path.starts_with(media::bin_dir(&self.data_dir)), path })),
            None => None,
        };
        let mut c = self.tool_cache.lock();
        *c = crate::engine::ToolCache { ytdlp: y.clone(), ffmpeg: f.clone(), js, at: Some(Instant::now()) };
        (y, f)
    }

    pub(crate) fn tools_status(&self) -> ToolsStatus {
        let c = self.tool_cache.lock();
        let info = |t: &Option<Tool>| t.as_ref().map(|t| ToolInfo { path: t.path.to_string_lossy().into_owned(), version: t.version.clone(), managed: t.managed });
        ToolsStatus { ytdlp: info(&c.ytdlp), ffmpeg: info(&c.ffmpeg), js: c.js.as_ref().map(|(_, t)| ToolInfo { path: t.path.to_string_lossy().into_owned(), version: t.version.clone(), managed: t.managed }), installing: self.installing.lock().clone() }
    }

    fn run_opts(&self, opts: &JobOptions, ffmpeg: &Option<Tool>, source_address: Option<String>) -> RunOpts {
        let s = self.settings.read();
        RunOpts {
            cookies_from_browser: Some(s.media.cookies_from_browser.clone()).filter(|b| !b.is_empty()),
            cookie_header: opts.cookies.clone(),
            headers: opts.headers.iter().filter(|(k, _)| !k.eq_ignore_ascii_case("cookie")).cloned().collect(),
            proxy: opts.proxy.clone().or(s.proxy.clone()),
            ffmpeg: ffmpeg.as_ref().map(|f| f.path.clone()),
            source_address,
            js_runtime: self.tool_cache.lock().js.as_ref().map(|(n, t)| (n.clone(), t.path.clone())),
            fragments: opts.streams_per_network.unwrap_or(s.default_streams).clamp(4, 32),
        }
    }

    /// What the page offers, for the Add sheet (nothing is downloaded).
    pub(crate) async fn media_probe(self: &Arc<Self>, url: &str, opts: &JobOptions) -> Result<MediaProbe> {
        let (y, f) = self.tools(false).await;
        let y = y.ok_or_else(|| Error::msg(YTDLP_MISSING_MSG))?;
        let quality = opts.media_format.clone().unwrap_or_else(|| self.settings.read().media.default_quality.clone());
        let selector = media::selector_for(&quality, f.is_some());
        let ro = self.run_opts(opts, &f, None);
        let e = media::extract(&y, url, &selector, &ro, &tokio_util::sync::CancellationToken::new()).await?;
        Ok(MediaProbe {
            title: e.title.clone(),
            uploader: e.uploader.clone(),
            thumbnail: e.thumbnail.clone(),
            duration: e.duration,
            extractor: e.extractor.clone(),
            heights: e.heights.clone(),
            format: e.label(),
            size: e.parts.iter().map(|p| p.size).sum::<Option<u64>>(),
            direct: e.all_direct(),
            playlist: e.is_playlist.then_some(e.playlist_entries.len() as u32),
        })
    }

    pub(crate) async fn media_pipeline(self: &Arc<Self>, entry: &Arc<JobEntry>) -> Result<MDone> {
        let token = entry.ctl.lock().token.clone();
        let settings = self.settings.read().clone();
        let (opts, url, id, dest_dir) = {
            let j = entry.job.lock();
            (j.options.clone(), j.url.clone(), j.id.clone(), j.dest_dir.clone())
        };
        self.update(entry, |j| {
            j.status = JobStatus::Probing;
            j.error = None;
            j.started_at.get_or_insert_with(now_ts);
        });
        if !self.ifaces.read().iter().any(|i| i.usable) && !self.fixed_ifaces {
            return Err(Error::NoNetwork);
        }
        let (y, f) = self.tools(true).await;
        let y = y.ok_or_else(|| Error::msg(YTDLP_MISSING_MSG))?;
        let dest = PathBuf::from(&dest_dir);
        std::fs::create_dir_all(&dest).map_err(|e| Error::msg(format!("Can't save to “{}”: {}", dest.display(), crate::error::io_message(&e))))?;

        // ── what is it, and which streams? ──
        let quality = opts.media_format.clone().unwrap_or_else(|| settings.media.default_quality.clone());
        let mut can_merge = f.is_some();
        let mut selector = media::selector_for(&quality, can_merge);
        let ro = self.run_opts(&opts, &f, None);
        let mut ex = tokio::select! {
            _ = token.cancelled() => return Ok(MDone::Stopped),
            r = media::extract(&y, &url, &selector, &ro, &token) => r?,
        };
        // A pair of streams with no ffmpeg to join them: fall back to a single file with sound.
        if ex.needs_merge() && !can_merge {
            can_merge = false;
            selector = media::selector_for("best", false);
            ex = media::extract(&y, &url, &selector, &ro, &token).await?;
            self.log(entry, "ffmpeg wasn't found, so this is the best single file (video and sound together) rather than the best quality. Install ffmpeg in Settings → Video downloads for more.");
        }
        let _ = can_merge;

        // A playlist or channel: one job per video.
        if ex.is_playlist {
            let urls: Vec<String> = ex.playlist_entries.iter().take(200).map(|(u, _)| u.clone()).collect();
            if urls.is_empty() {
                return Err(Error::msg("That playlist looks empty."));
            }
            let mut o = opts.clone();
            o.media = Some(true);
            let n = urls.len();
            let _ = self.add_internal(AddRequest { urls, dest_dir: Some(dest_dir.clone()), options: o, ..Default::default() });
            self.update(entry, |j| {
                j.status = JobStatus::Completed;
                j.completed_at = Some(now_ts());
                j.file_name = format!("{} ({n} videos)", sanitize_title(&ex.title));
                j.log.push(format!("Added {n} videos from this playlist as separate downloads."));
            });
            return Ok(MDone::Completed);
        }

        let name = sanitize_title(&ex.title);
        let direct = ex.all_direct();
        let bitrate = media::audio_bitrate(&quality);
        if bitrate.is_some() && f.is_none() {
            return Err(Error::msg("Saving audio as MP3 needs ffmpeg. Install it in Settings → Video, or pick plain “Audio”."));
        }
        let ext = if bitrate.is_some() { "mp3".to_string() } else if ex.needs_merge() { media::merged_ext(&ex.parts).to_string() } else { ex.parts.first().map(|p| p.ext.clone()).unwrap_or_else(|| "mp4".into()) };
        let total: Option<u64> = ex.parts.iter().map(|p| p.size).sum();
        self.update(entry, |j| {
            j.file_name = format!("{name}.{ext}");
            j.size = total;
            j.mime = Some(if ex.parts.iter().any(|p| p.has_video) { "video".into() } else { "audio".into() });
            j.resumable = true;
            j.media = Some(MediaStatus {
                title: ex.title.clone(),
                uploader: ex.uploader.clone(),
                thumbnail: ex.thumbnail.clone(),
                duration: ex.duration,
                extractor: ex.extractor.clone(),
                format: ex.label(),
                strategy: if direct { "direct" } else { "ytdlp" }.into(),
                parts: ex.parts.len() as u32,
            });
        });
        if Self::wanted(entry) {
            return Ok(MDone::Stopped);
        }

        let temp = dest.join(format!(".idmx-{id}"));
        let final_path = if direct {
            self.media_direct(entry, &ex, &opts, &temp, &dest, &name, &ext, &f, &token).await?
        } else {
            self.media_delegated(entry, &y, &url, &selector, &opts, &temp, &dest, &f, &token).await?
        };
        let Some(mut final_path) = final_path else { return Ok(MDone::Stopped) };
        if let (Some(kbps), Some(ff)) = (bitrate, &f) {
            if final_path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("mp3")) {
                self.update(entry, |j| {
                    j.status = JobStatus::Processing;
                    j.speed = 0.0;
                    j.eta = None;
                });
                let out = dest.join(unique_name(&dest, &format!("{name}.mp3"), |_| false));
                media::to_mp3(&ff.path, &final_path, &out, kbps).await?;
                let _ = std::fs::remove_file(&final_path);
                final_path = out;
            }
        }
        let _ = std::fs::remove_dir_all(&temp);
        self.finish_media(entry, final_path, &settings).await;
        Ok(MDone::Completed)
    }

    fn wanted(entry: &JobEntry) -> bool {
        entry.ctl.lock().want != Want::None
    }

    /// Fetch each stream as a hidden child download, so the whole multi-network machinery
    /// (blocks, retries, resume, limits) does the work, then combine them.
    #[allow(clippy::too_many_arguments)]
    async fn media_direct(self: &Arc<Self>, entry: &Arc<JobEntry>, ex: &Extracted, opts: &JobOptions, temp: &Path, dest: &Path, name: &str, ext: &str, ffmpeg: &Option<Tool>, token: &tokio_util::sync::CancellationToken) -> Result<Option<PathBuf>> {
        let parent = entry.job.lock().id.clone();
        std::fs::create_dir_all(temp)?;
        let priority = entry.job.lock().priority;
        // One child per stream; a resumed job picks up the children it already has.
        let mut children: Vec<(Arc<JobEntry>, String)> = Vec::new();
        for p in &ex.parts {
            let fname = format!("{}.{}.{}", parent, p.format_id, p.ext);
            let existing = self.entries().into_iter().find(|c| {
                let j = c.job.lock();
                j.parent.as_deref() == Some(parent.as_str()) && j.options.file_name.as_deref() == Some(fname.as_str())
            });
            let headers: Vec<(String, String)> = p.headers.iter().filter(|(k, _)| !matches!(k.to_ascii_lowercase().as_str(), "accept-encoding" | "range" | "host" | "content-length")).cloned().collect();
            match existing {
                Some(c) => {
                    // Signed URLs expire: give the child a fresh one.
                    self.update(&c, |j| {
                        j.url = p.url.clone();
                        j.options.headers = headers.clone();
                        if matches!(j.status, JobStatus::Paused | JobStatus::Failed | JobStatus::Cancelled) {
                            j.status = JobStatus::Queued;
                            j.error = None;
                        }
                    });
                    children.push((c, fname));
                }
                None => {
                    let mut o = opts.clone();
                    o.headers = headers;
                    o.file_name = Some(fname.clone());
                    o.force_http = true;
                    o.skip_post = true;
                    o.restart_if_changed = true;
                    o.checksum = None;
                    o.media = None;
                    let c = self.spawn_child(&parent, &p.url, o, temp, priority);
                    children.push((c, fname));
                }
            }
        }
        self.kick.notify_one();

        let mut tick = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    let want = entry.ctl.lock().want;
                    for (c, _) in &children {
                        match want {
                            Want::Cancel { .. } | Want::Remove { .. } => self.stop_entry(c, Want::Remove { delete: true }),
                            _ => self.stop_entry(c, Want::Pause),
                        }
                    }
                    if matches!(want, Want::Cancel { .. } | Want::Remove { .. }) {
                        let _ = std::fs::remove_dir_all(temp);
                    }
                    return Ok(None);
                }
                _ = tick.tick() => {}
            }
            // Aggregate the children into the parent's numbers.
            let snaps: Vec<Job> = children.iter().map(|(c, _)| c.job.lock().clone()).collect();
            if let Some(bad) = snaps.iter().find(|j| j.status == JobStatus::Failed) {
                for (c, _) in &children {
                    self.stop_entry(c, Want::Pause);
                }
                return Err(Error::msg(bad.error.clone().unwrap_or_else(|| "A part of the video failed to download.".into())));
            }
            let downloaded: u64 = snaps.iter().map(|j| j.downloaded).sum();
            let speed: f64 = snaps.iter().map(|j| j.speed).sum();
            let known_total: Option<u64> = snaps.iter().map(|j| j.size).sum::<Option<u64>>().or(ex.parts.iter().map(|p| p.size).sum());
            let eta = known_total.filter(|t| speed > 1.0 && *t > downloaded).map(|t| ((t - downloaded) as f64 / speed).ceil() as u64);
            let nets = aggregate_networks(&snaps);
            let all_done = snaps.iter().all(|j| j.status == JobStatus::Completed);
            self.update_quiet(entry, |j| {
                j.downloaded = downloaded;
                j.size = known_total.or(j.size);
                j.speed = speed;
                j.eta = eta;
                j.networks = nets;
                j.status = if snaps.iter().any(|s| matches!(s.status, JobStatus::Downloading)) { JobStatus::Downloading } else if all_done { JobStatus::Processing } else { JobStatus::Downloading };
                j.peak_streams = j.peak_streams.max(snaps.iter().map(|s| s.peak_streams).max().unwrap_or(0));
                j.speed_history.push(speed as f32);
                if j.speed_history.len() > 600 {
                    j.speed_history = j.speed_history.chunks(2).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
                }
            });
            if all_done {
                break;
            }
        }

        // ── put it together ──
        self.update(entry, |j| {
            j.status = JobStatus::Processing;
            j.speed = 0.0;
            j.eta = None;
        });
        let part_paths: Vec<PathBuf> = children.iter().map(|(c, _)| PathBuf::from(c.job.lock().path.clone().unwrap_or_default())).collect();
        let final_name = unique_name(dest, &format!("{name}.{ext}"), |_| false);
        let final_path = dest.join(&final_name);
        if part_paths.len() == 1 {
            move_file(&part_paths[0], &final_path)?;
        } else {
            let ff = ffmpeg.as_ref().ok_or_else(|| Error::msg("This video comes as separate picture and sound, and ffmpeg is needed to join them. Install it in Settings → Video downloads."))?;
            let (v, a) = {
                let vi = ex.parts.iter().position(|p| p.has_video).unwrap_or(0);
                let ai = ex.parts.iter().position(|p| !p.has_video && p.has_audio).unwrap_or(1.min(part_paths.len() - 1));
                (part_paths[vi].clone(), part_paths[ai].clone())
            };
            let v2 = v.clone();
            let _ = v2;
            media::merge(&ff.path, &v, &a, &final_path).await?;
        }
        // The pieces are no longer needed.
        for (c, _) in &children {
            self.stop_entry(c, Want::Remove { delete: false });
        }
        Ok(Some(final_path))
    }

    /// A stream yt-dlp has to download itself (HLS, DASH…), still on whichever network the user prefers.
    #[allow(clippy::too_many_arguments)]
    async fn media_delegated(self: &Arc<Self>, entry: &Arc<JobEntry>, y: &Tool, url: &str, selector: &str, opts: &JobOptions, temp: &Path, dest: &Path, ffmpeg: &Option<Tool>, token: &tokio_util::sync::CancellationToken) -> Result<Option<PathBuf>> {
        std::fs::create_dir_all(temp)?;
        // yt-dlp can only use one network; pick the first enabled one that's up.
        let net = {
            let settings = self.settings.read();
            self.ifaces.read().iter().find(|i| i.usable && opts.interfaces.as_ref().map_or(settings.interfaces.get(&i.id).and_then(|r| r.enabled).unwrap_or(i.kind.default_enabled()), |l| l.contains(&i.id))).cloned()
        };
        let ro = self.run_opts(opts, ffmpeg, net.as_ref().and_then(|n| n.ipv4.first().cloned()));
        let ro_fragments = ro.fragments;
        let limit = opts.speed_limit.or(self.settings.read().global_speed_limit);
        let (net_id, net_label, net_kind) = net.map(|n| (n.id, n.label, n.kind)).unwrap_or(("default".into(), "System default".into(), IfKind::Unknown));
        self.update(entry, |j| j.status = JobStatus::Downloading);
        let me = self.clone();
        let e2 = entry.clone();
        let result = media::download_with_ytdlp(y, url, selector, dest, temp, &ro, limit, token, move |p| {
            me.update_quiet(&e2, |j| {
                j.downloaded = p.downloaded;
                if p.total.is_some() {
                    j.size = p.total;
                }
                j.speed = p.speed;
                j.eta = p.eta;
                j.networks = vec![NetStat { id: net_id.clone(), label: net_label.clone(), kind: net_kind, bytes: p.downloaded, speed: p.speed, streams: ro_fragments, active: ro_fragments, latency_ms: 0.0, health: 100, errors: 0, state: NetState::Active, error: None, enabled: true }];
                if p.finished {
                    j.status = JobStatus::Processing;
                }
            });
        })
        .await;
        match result {
            Ok(p) => Ok(Some(p)),
            Err(_) if token.is_cancelled() => {
                if matches!(entry.ctl.lock().want, Want::Cancel { .. } | Want::Remove { .. }) {
                    let _ = std::fs::remove_dir_all(temp);
                }
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    async fn finish_media(self: &Arc<Self>, entry: &Arc<JobEntry>, mut path: PathBuf, settings: &Settings) {
        let opts = entry.job.lock().options.clone();
        if !opts.skip_post && !settings.post_rules.is_empty() {
            self.update(entry, |j| j.status = JobStatus::Processing);
            let job = entry.job.lock().clone();
            let me = self.clone();
            let emit = move |e: Event| me.emit(e);
            let out = crate::post::apply(&settings.post_rules, &job, path.clone(), None, &emit).await;
            path = out.path;
            if !out.log.is_empty() {
                self.update(entry, |j| j.log.extend(out.log.clone()));
            }
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let size = std::fs::metadata(&path).map(|m| m.len()).ok();
        self.update(entry, |j| {
            j.status = JobStatus::Completed;
            j.completed_at = Some(now_ts());
            j.path = Some(path.to_string_lossy().into_owned());
            j.file_name = name.clone();
            j.size = size.or(j.size);
            j.downloaded = size.unwrap_or(j.downloaded);
            j.speed = 0.0;
            j.eta = None;
            j.error = None;
        });
        if settings.notifications {
            self.emit(Event::Notice { level: "complete".into(), message: name });
        }
    }
}

fn move_file(from: &Path, to: &Path) -> Result<()> {
    if std::fs::rename(from, to).is_err() {
        std::fs::copy(from, to)?;
        std::fs::remove_file(from)?;
    }
    Ok(())
}

/// One row per network, summed over the parts of a video.
fn aggregate_networks(parts: &[Job]) -> Vec<NetStat> {
    let mut out: Vec<NetStat> = Vec::new();
    for j in parts {
        for n in &j.networks {
            match out.iter_mut().find(|o| o.id == n.id) {
                Some(o) => {
                    o.bytes += n.bytes;
                    o.speed += n.speed;
                    o.streams += n.streams;
                    o.active += n.active;
                    o.errors += n.errors;
                    o.health = o.health.min(n.health);
                    if n.error.is_some() && o.error.is_none() {
                        o.error = n.error.clone();
                        o.state = n.state;
                    }
                    o.latency_ms = o.latency_ms.max(n.latency_ms);
                }
                None => out.push(n.clone()),
            }
        }
    }
    out
}
