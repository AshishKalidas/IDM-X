//! One download from "queued" to "completed": probe, plan, run, verify, finish, post-process.

use crate::engine::{Inner, JobEntry, Want};
use crate::error::{Error, Result};
use crate::http::{base_headers, header_checksum, probe};
use crate::limiter::Limiter;
use crate::run::{NetPlan, Outcome, Refresh, Run, RunConfig};
use crate::types::*;
use crate::util::{now_ts, staging_path};
use crate::verify::{self, Algo, Expected};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Duration;

const METERED_DEFAULT_MIN: u64 = 256 << 20;

enum Done {
    Completed,
    Stopped,
}

impl Inner {
    pub(crate) fn spawn_job(self: &Arc<Self>, entry: Arc<JobEntry>) {
        {
            let mut c = entry.ctl.lock();
            if c.running {
                return;
            }
            c.running = true;
            c.want = Want::None;
            c.token = tokio_util::sync::CancellationToken::new();
        }
        let me = self.clone();
        tokio::spawn(async move { me.job_task(entry).await });
    }

    async fn job_task(self: Arc<Self>, entry: Arc<JobEntry>) {
        let result = self.pipeline(&entry).await;
        let (want, saved) = {
            let mut c = entry.ctl.lock();
            let saved = c.run.take().map(|r| r.saved());
            if saved.is_some() {
                c.saved = saved.clone();
            }
            c.running = false;
            (std::mem::take(&mut c.want), saved)
        };
        let _ = saved;
        match result {
            Ok(Done::Completed) => {}
            Ok(Done::Stopped) => self.apply_stopped(&entry, want),
            Err(e) => {
                // A pause that arrived while we were failing still wins.
                if matches!(want, Want::Cancel { .. } | Want::Remove { .. }) {
                    self.apply_stopped(&entry, want);
                } else {
                    let msg = e.to_string();
                    tracing::warn!("job failed: {msg}");
                    let name = entry.job.lock().file_name.clone();
                    self.update(&entry, |j| {
                        j.status = JobStatus::Failed;
                        j.error = Some(msg.clone());
                        j.speed = 0.0;
                        j.eta = None;
                        for n in &mut j.networks {
                            n.speed = 0.0;
                            n.active = 0;
                        }
                    });
                    if self.settings.read().notifications {
                        self.emit(Event::Notice { level: "error".into(), message: format!("{name} failed — {msg}") });
                    }
                }
            }
        }
        self.dirty.store(true, Relaxed);
        self.kick.notify_one();
    }

    fn wanted_stop(entry: &JobEntry) -> bool {
        entry.ctl.lock().want != Want::None
    }

    async fn pipeline(self: &Arc<Self>, entry: &Arc<JobEntry>) -> Result<Done> {
        if entry.job.lock().kind == JobKind::Media {
            return Ok(match self.media_pipeline(entry).await? {
                crate::pipeline_media::MDone::Completed => Done::Completed,
                crate::pipeline_media::MDone::Stopped => Done::Stopped,
            });
        }
        if entry.job.lock().kind == JobKind::Torrent {
            return Ok(match self.torrent_pipeline(entry).await? {
                crate::pipeline_torrent::TDone::Completed => Done::Completed,
                crate::pipeline_torrent::TDone::Stopped => Done::Stopped,
            });
        }
        let token = entry.ctl.lock().token.clone();
        let settings = self.settings.read().clone();
        let (opts, url, prior) = {
            let j = entry.job.lock();
            (j.options.clone(), j.url.clone(), (j.size, j.etag.clone(), j.last_modified.clone()))
        };
        self.update(entry, |j| {
            j.status = JobStatus::Probing;
            j.error = None;
            j.started_at.get_or_insert_with(now_ts);
        });

        // Nothing to download over: say so plainly (the job is requeued when a network returns).
        if !self.ifaces.read().iter().any(|i| i.usable) {
            return Err(Error::NoNetwork);
        }
        let headers = base_headers(&opts, settings.user_agent.as_deref());
        let proxy = opts.proxy.clone().or(settings.proxy.clone());
        let probe_client = self.probe_client(&proxy)?;

        // ── 1. probe, retrying through blips ──
        let (info, header_sum) = {
            let mut attempt = 0u32;
            loop {
                let r = tokio::select! {
                    _ = token.cancelled() => return Ok(Done::Stopped),
                    r = probe(&probe_client, &url, &headers) => r,
                };
                match r {
                    Ok((info, resp)) => {
                        let sum = header_checksum(&resp);
                        break (info, sum);
                    }
                    Err(e) if e.is_transient() && attempt < 3 => {
                        attempt += 1;
                        tokio::select! {
                            _ = token.cancelled() => return Ok(Done::Stopped),
                            _ = tokio::time::sleep(Duration::from_secs(1 << (attempt - 1))) => {}
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
        };
        if Self::wanted_stop(entry) {
            return Ok(Done::Stopped);
        }

        // A web page rather than a file: if it has video on it, that's what was meant.
        if !opts.force_http && settings.media.enabled && info.mime.as_deref().is_some_and(|m| m.starts_with("text/html")) {
            let (y, _) = self.tools(false).await;
            if y.is_some() {
                self.update(entry, |j| {
                    j.kind = JobKind::Media;
                    j.options.media = Some(true);
                });
                return Ok(match self.media_pipeline(entry).await? {
                    crate::pipeline_media::MDone::Completed => Done::Completed,
                    crate::pipeline_media::MDone::Stopped => Done::Stopped,
                });
            }
            return Err(Error::msg(format!("That link is a web page, not a file. If it has a video on it, {} Otherwise use “Download as a plain file”.", crate::pipeline_media::YTDLP_MISSING_MSG.trim_start_matches("Saving videos needs yt-dlp, which isn't installed yet. ").to_lowercase())));
        }

        // ── 2. is this still the file we were downloading? ──
        let first_time = prior.0.is_none();
        let mut saved = entry.ctl.lock().saved.clone();
        if !first_time && saved.is_some() {
            let size_changed = prior.0 != info.size;
            let tag_changed = matches!((&prior.1, &info.etag), (Some(a), Some(b)) if a != b);
            let lm_changed = matches!((&prior.2, &info.last_modified), (Some(a), Some(b)) if a != b);
            if size_changed || tag_changed || lm_changed {
                if settings.refuse_changed_resume && !opts.restart_if_changed {
                    // Splicing two versions of a file together corrupts it. Say so, and leave the
                    // choice (start over, or give up) to the user.
                    self.update(entry, |j| j.resumable = false);
                    return Err(Error::msg("The file changed on the server since this download began, so it can't be resumed. Use “Start over” to get the new version."));
                }
                self.discard_partial(entry);
                saved = None;
                self.log(entry, "The file changed on the server — starting over");
            }
        }

        // ── 3. name it, find it a place ──
        let dest = PathBuf::from(&entry.job.lock().dest_dir);
        std::fs::create_dir_all(&dest).map_err(|e| Error::msg(format!("Can't save to “{}”: {}", dest.display(), crate::error::io_message(&e))))?;
        if first_time || prior.0.is_none() {
            let wanted = opts.file_name.clone().map(|n| crate::util::sanitize_filename(&n)).unwrap_or(info.file_name.clone());
            let name = {
                let mut reserved = self.reserved.lock();
                let unique = crate::util::unique_name(&dest, &wanted, |n| reserved.contains(&dest.join(n)));
                reserved.insert(dest.join(&unique));
                unique
            };
            self.update(entry, |j| j.file_name = name);
        }
        let name = entry.job.lock().file_name.clone();
        let staging = staging_path(&dest, &name);

        self.update(entry, |j| {
            j.size = info.size;
            j.final_url = Some(info.final_url.clone());
            j.etag = info.etag.clone();
            j.last_modified = info.last_modified.clone();
            j.resumable = info.ranges && info.size.is_some();
            j.mime = info.mime.clone();
            j.downloaded = 0;
        });

        // ── 4. which networks, and which IP version each should use ──
        let size_hint = info.size.unwrap_or(0);
        let (nets, notes) = self.select_nets(&opts, size_hint, &proxy);
        let target = if proxy.as_deref().is_some_and(|p| !p.is_empty()) { None } else { resolve_target(&info.final_url).await };
        for n in notes {
            self.log(entry, &n);
        }

        // ── 5. go ──
        let validator = info
            .etag
            .clone()
            .filter(|t| !t.starts_with("W/"))
            .or_else(|| info.last_modified.clone());
        let refresh: Refresh = {
            let (client, url, headers, tag, lm) = (probe_client.clone(), url.clone(), headers.clone(), info.etag.clone(), info.last_modified.clone());
            Arc::new(move || {
                let (client, url, headers, tag, lm) = (client.clone(), url.clone(), headers.clone(), tag.clone(), lm.clone());
                Box::pin(async move {
                    let (i, _) = probe(&client, &url, &headers).await?;
                    // A fresh URL is only useful if it still points at the same file.
                    let same = (tag.is_none() || tag == i.etag) && (lm.is_none() || lm == i.last_modified);
                    if same { Ok(i.final_url) } else { Err(Error::RemoteChanged) }
                })
            })
        };
        let run = Run::new(RunConfig {
            url: info.final_url.clone(),
            size: info.size,
            ranges: info.ranges && info.size.is_some(),
            validator,
            headers,
            staging: staging.clone(),
            nets,
            max_streams: settings.max_streams,
            hedging: settings.hedging,
            saved,
            global_limiter: self.global_limiter.clone(),
            job_limiter: entry.limiter.clone(),
            refresh: Some(refresh),
            stall_timeout: Duration::from_secs(20),
            give_up_after: Duration::from_secs(600),
            check_disk: true,
            target,
        })?;
        entry.ctl.lock().run = Some(run.clone());
        self.update(entry, |j| {
            j.status = JobStatus::Downloading;
            j.downloaded = run.downloaded();
        });
        // A pause that landed while we were setting up.
        if Self::wanted_stop(entry) || token.is_cancelled() {
            run.stop();
        }

        let outcome = run.drive().await;
        let snap = run.snapshot();
        entry.ctl.lock().saved = Some(run.saved());
        self.update(entry, |j| {
            j.downloaded = snap.downloaded;
            j.networks = snap.nets.clone();
            j.blocks_done = snap.blocks_done;
            j.blocks_total = snap.blocks_total;
            j.speed = 0.0;
            j.eta = None;
        });

        match outcome? {
            Outcome::Stopped => Ok(Done::Stopped),
            Outcome::Completed { size } => {
                entry.ctl.lock().run = None;
                self.finalize(entry, &staging, size, header_sum, &info, &opts, &settings, &probe_client).await?;
                Ok(Done::Completed)
            }
        }
    }

    pub(crate) fn log(&self, entry: &Arc<JobEntry>, line: &str) {
        let line = line.to_string();
        self.update(entry, |j| {
            j.log.push(line);
            if j.log.len() > 200 {
                j.log.drain(..50);
            }
        });
    }

    /// One network's plan for a download, from the user's rules.
    fn plan_one(&self, settings: &Settings, i: &NetInterface, opts: &JobOptions, size: u64, proxy: &Option<String>, notes: &mut Vec<String>) -> NetPlan {
        let rule = settings.interfaces.get(&i.id).cloned().unwrap_or_default();
        let name = rule.label.clone().unwrap_or(i.label.clone());
        let mut enabled = match &opts.interfaces {
            Some(list) => list.contains(&i.id),
            None => rule.enabled.unwrap_or(i.kind.default_enabled()),
        };
        if enabled && opts.interfaces.is_none() {
            let min = rule.min_file_size.or(rule.metered.then_some(METERED_DEFAULT_MIN)).unwrap_or(0);
            if size < min {
                notes.push(format!("{name} sits this one out (files under {} only use free networks)", crate::util::human_bytes(min)));
                enabled = false;
            }
        }
        let usage = self.usage_counter(&i.id, rule.cap_period);
        if enabled && rule.data_cap.is_some_and(|c| usage.load(Relaxed) >= c) {
            notes.push(format!("{name} has reached its data cap"));
            enabled = false;
        }
        NetPlan {
            iface: i.clone(),
            streams: opts.streams_per_network.or(rule.streams).unwrap_or(settings.default_streams),
            limiter: self.net_limiter(&i.id),
            usage,
            cap: rule.data_cap,
            rule,
            bind_device: self.bind_device,
            proxy: proxy.clone(),
            enabled,
        }
    }

    /// Every usable network takes part in the run, so any can be switched on mid-download; the
    /// rules decide which start switched on.
    fn select_nets(&self, opts: &JobOptions, size: u64, proxy: &Option<String>) -> (Vec<NetPlan>, Vec<String>) {
        let settings = self.settings.read().clone();
        let ifaces = self.ifaces.read().clone();
        let mut notes = Vec::new();
        let mut plans: Vec<NetPlan> = ifaces.iter().filter(|i| i.usable).map(|i| self.plan_one(&settings, i, opts, size, proxy, &mut notes)).collect();
        if !plans.iter().any(|p| p.enabled) {
            notes.push("No enabled network is up — using the system's default route".into());
            plans.push(system_default(&settings, proxy, self.usage_counter("default", CapPeriod::Monthly), self.net_limiter("default")));
        }
        (plans, notes)
    }

    /// A network turned up while this download was running: give the run a (switched-off) slot
    /// for it, and tell the user it's there to be switched on.
    pub(crate) fn offer_network(&self, entry: &Arc<JobEntry>, run: &Arc<Run>, iface: &NetInterface) {
        let settings = self.settings.read().clone();
        let (opts, size, name) = {
            let j = entry.job.lock();
            (j.options.clone(), j.size.unwrap_or(0), j.file_name.clone())
        };
        let proxy = opts.proxy.clone().or(settings.proxy.clone());
        let plan = self.plan_one(&settings, iface, &opts, size, &proxy, &mut Vec::new());
        let label = plan.rule.label.clone().unwrap_or(iface.label.clone());
        if run.add_network(plan) {
            self.emit(Event::Notice {
                level: "info".into(),
                message: format!("{label} just connected. It's switched off for “{name}” — turn it on in the download's Networks list to add its speed."),
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn finalize(
        self: &Arc<Self>,
        entry: &Arc<JobEntry>,
        staging: &Path,
        size: u64,
        header_sum: Option<(String, String)>,
        info: &ProbeInfo,
        opts: &JobOptions,
        settings: &Settings,
        client: &reqwest::Client,
    ) -> Result<()> {
        // ── verify ──
        self.update(entry, |j| {
            j.status = JobStatus::Verifying;
            j.size = Some(size);
            j.downloaded = size;
        });

        let user = opts.checksum.as_deref().and_then(verify::parse_expected);
        let mut expected: Option<(Algo, String, &'static str)> = None;
        match &user {
            Some(Expected::Hash(a, h)) => expected = Some((*a, h.clone(), "user")),
            Some(Expected::Auto) | None => {
                if let Some((a, h)) = &header_sum {
                    if let Some(algo) = Algo::parse(a) {
                        expected = Some((algo, h.clone(), "header"));
                    }
                }
            }
        }
        if expected.is_none() && matches!(user, Some(Expected::Auto)) {
            let name = entry.job.lock().file_name.clone();
            expected = fetch_sidecar(client, &info.final_url, &name).await;
        }

        let hash_it = expected.is_some() || (settings.hash_on_complete && size <= settings.hash_limit);
        if hash_it {
            let algo = expected.as_ref().map(|e| e.0).unwrap_or(Algo::Sha256);
            let path = staging.to_path_buf();
            let actual = tokio::task::spawn_blocking(move || verify::hash_file(&path, algo, |_| {}))
                .await
                .map_err(|e| Error::msg(e.to_string()))??;
            let result = verify::compare(algo, &actual, expected.as_ref().map(|(_, h, src)| (src.to_string(), h.clone())));
            let bad = result.verified == Some(false);
            self.update(entry, |j| j.checksum = Some(result.clone()));
            if bad {
                let err = verify::mismatch_error(&result);
                // A corrupt file must never be handed over as complete.
                self.discard_partial(entry);
                self.update(entry, |j| {
                    j.size = None;
                    j.downloaded = 0;
                });
                return Err(err);
            }
        }

        // ── put it in place ──
        let (dir, name) = {
            let j = entry.job.lock();
            (PathBuf::from(&j.dest_dir), j.file_name.clone())
        };
        let mut final_path = dir.join(&name);
        if final_path.exists() {
            // Something appeared under our name while we were downloading.
            let free = crate::util::unique_name(&dir, &name, |_| false);
            final_path = dir.join(free);
        }
        std::fs::rename(staging, &final_path)?;
        self.reserved.lock().remove(&dir.join(&name));
        if let Some(t) = info.last_modified.as_deref().and_then(|v| httpdate::parse_http_date(v).ok()) {
            if let Ok(f) = std::fs::File::options().write(true).open(&final_path) {
                let _ = f.set_modified(t);
            }
        }
        entry.ctl.lock().saved = None;

        // ── post-processing ──
        let mut final_path = final_path;
        if !opts.skip_post && !settings.post_rules.is_empty() {
            self.update(entry, |j| j.status = JobStatus::Processing);
            let job = entry.job.lock().clone();
            let sha = job.checksum.as_ref().filter(|c| c.algo == "sha256").and_then(|c| c.actual.clone());
            let me = self.clone();
            let emit = move |e: Event| me.emit(e);
            let out = crate::post::apply(&settings.post_rules, &job, final_path.clone(), sha.as_deref(), &emit).await;
            final_path = out.path;
            if !out.log.is_empty() {
                self.update(entry, |j| j.log.extend(out.log.clone()));
            }
        }

        let name = final_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.update(entry, |j| {
            j.status = JobStatus::Completed;
            j.completed_at = Some(now_ts());
            j.path = Some(final_path.to_string_lossy().into_owned());
            j.file_name = name.clone();
            j.error = None;
        });
        if settings.notifications {
            self.emit(Event::Notice { level: "complete".into(), message: name });
        }
        Ok(())
    }
}

/// When nothing is enabled or usable we still download — through whatever route the OS picks.
fn system_default(settings: &Settings, proxy: &Option<String>, usage: Arc<std::sync::atomic::AtomicU64>, limiter: Arc<Limiter>) -> NetPlan {
    NetPlan {
        iface: NetInterface {
            id: "default".into(),
            label: "System default".into(),
            kind: IfKind::Unknown,
            ipv4: vec![],
            ipv6: vec![],
            gateway: None,
            subnets: vec![],
            mac: None,
            up: true,
            link_mbps: None,
            usable: true,
        },
        rule: InterfaceRule::default(),
        streams: settings.default_streams,
        limiter,
        usage,
        cap: None,
        bind_device: false,
        proxy: proxy.clone(),
        enabled: true,
    }
}

/// The server's host, port and every address it resolves to. `None` if it can't be worked out
/// (the system resolver is then left to sort it out per connection).
async fn resolve_target(url: &str) -> Option<(String, u16, Vec<std::net::IpAddr>)> {
    let u = url::Url::parse(url).ok()?;
    let port = u.port_or_known_default()?;
    let (host, ips) = match u.host()? {
        url::Host::Ipv4(a) => (a.to_string(), vec![std::net::IpAddr::V4(a)]),
        url::Host::Ipv6(a) => (a.to_string(), vec![std::net::IpAddr::V6(a)]),
        url::Host::Domain(d) => {
            let addrs = tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((d, port))).await.ok()?.ok()?;
            let mut ips: Vec<std::net::IpAddr> = addrs.map(|a| a.ip()).collect();
            ips.sort();
            ips.dedup();
            (d.to_string(), ips)
        }
    };
    (!ips.is_empty()).then_some((host, port, ips))
}

/// Look for `<url>.sha256`, `.sha256sum`, `.sha1`, `.md5` next to the file.
async fn fetch_sidecar(client: &reqwest::Client, final_url: &str, name: &str) -> Option<(Algo, String, &'static str)> {
    let base = url::Url::parse(final_url).ok()?;
    for (suffix, algo) in [(".sha256", Algo::Sha256), (".sha256sum", Algo::Sha256), (".sha1", Algo::Sha1), (".md5", Algo::Md5)] {
        let mut u = base.clone();
        u.set_query(None);
        u.set_path(&format!("{}{}", base.path(), suffix));
        let Ok(Ok(resp)) = tokio::time::timeout(Duration::from_secs(8), client.get(u).send()).await else { continue };
        if !resp.status().is_success() || resp.content_length().is_some_and(|l| l > 64 * 1024) {
            continue;
        }
        let Ok(text) = resp.text().await else { continue };
        if let Some(h) = verify::parse_sidecar(&text, name) {
            let expect_len = match algo {
                Algo::Sha256 => 64,
                Algo::Sha1 => 40,
                Algo::Md5 => 32,
                Algo::Blake3 => 64,
            };
            if h.len() == expect_len {
                return Some((algo, h, "sidecar"));
            }
        }
    }
    None
}
