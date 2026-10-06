//! In-depth speed test: pull a 500 MB file over and over, on every chosen network, until told to stop,
//! while logging throughput, latency, jitter and lost probes every second to a log file on disk.
//!
//! Nothing is kept: the downloaded bytes are counted and thrown away, the log goes straight to disk
//! (so a run of hours doesn't grow the app), and the live view only holds the last couple of minutes.

use crate::engine::{Engine, Inner};
use crate::error::{Error, Result};
use crate::http::{build_client, ClientSpec};
use crate::net::local_addr;
use crate::types::*;
use futures_util::StreamExt;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One "file": a cycle ends once this many bytes have come down on a network.
pub const CYCLE_BYTES: u64 = 500_000_000;
const STREAMS: usize = 4;
const RECENT: usize = 150;
const STALL_BELOW_MBPS: f64 = 0.5;
const CHUNK_TIMEOUT: Duration = Duration::from_secs(15);
/// Cloudflare caps one request below 100 MB, so the default file is fetched in pieces until the 500 MB is done.
const DEFAULT_URL: &str = "https://speed.cloudflare.com/__down?bytes=25000000";

fn logs_dir(data: &std::path::Path) -> PathBuf {
    data.join("speedtest-logs")
}

fn safe_name(name: &str) -> bool {
    !name.is_empty() && name.len() < 80 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Engine {
    /// Begin an in-depth run on `net_ids` (every enabled network if empty).
    pub fn endurance_start(&self, url: Option<String>, net_ids: Vec<String>) -> Result<()> {
        if self.inner.speedtest_busy.swap(true, Relaxed) {
            return Err(Error::msg("a speed test is already running"));
        }
        self.inner.endurance_stop.store(false, Relaxed);
        let settings = self.inner.settings.read().clone();
        let nets: Vec<NetInterface> = self
            .inner
            .ifaces
            .read()
            .iter()
            .filter(|i| i.usable)
            .filter(|i| if net_ids.is_empty() { settings.interfaces.get(&i.id).and_then(|r| r.enabled).unwrap_or(i.kind.default_enabled()) } else { net_ids.contains(&i.id) })
            .cloned()
            .collect();
        if nets.is_empty() {
            self.inner.speedtest_busy.store(false, Relaxed);
            return Err(Error::NoNetwork);
        }
        let url = url.filter(|u| !u.trim().is_empty()).unwrap_or_else(|| DEFAULT_URL.into());
        tokio::spawn(self.inner.clone().endurance_task(url, nets));
        Ok(())
    }

    pub fn endurance_stop(&self) {
        self.inner.endurance_stop.store(true, Relaxed);
    }

    pub fn endurance_status(&self) -> Option<EnduranceStatus> {
        self.inner.endurance_status.lock().clone()
    }

    /// Finished runs, newest first.
    pub fn endurance_logs(&self) -> Vec<EnduranceSummary> {
        let mut out: Vec<EnduranceSummary> = std::fs::read_dir(logs_dir(&self.inner.data_dir))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
            .collect();
        out.sort_by_key(|s: &EnduranceSummary| std::cmp::Reverse(s.started_at));
        out
    }

    pub fn endurance_log_path(&self, name: &str) -> Result<PathBuf> {
        if !safe_name(name) {
            return Err(Error::NotFound);
        }
        let p = logs_dir(&self.inner.data_dir).join(format!("{name}.csv"));
        if p.exists() { Ok(p) } else { Err(Error::NotFound) }
    }

    pub fn endurance_delete_log(&self, name: &str) -> Result<()> {
        let csv = self.endurance_log_path(name)?;
        let _ = std::fs::remove_file(csv.with_extension("json"));
        std::fs::remove_file(csv)?;
        Ok(())
    }
}

/// Counters a network's download workers share with the sampler.
#[derive(Default)]
struct Counters {
    bytes: AtomicU64,
    cycle_bytes: AtomicU64,
    cycles: AtomicU64,
    errors: AtomicU64,
}

#[derive(Default)]
struct Track {
    mbps: Vec<f32>,
    rtts: Vec<f32>,
    sent: u64,
    lost: u64,
    run: u32,
    longest_run: u32,
    stalls: u64,
    jitter_sum: f64,
    jitter_n: u64,
    last_rtt: Option<f64>,
    last_bytes: u64,
}

impl Inner {
    async fn endurance_task(self: Arc<Self>, url: String, nets: Vec<NetInterface>) {
        let r = self.endurance(&url, &nets).await;
        if let Err(e) = r {
            let mut st = self.endurance_status.lock().clone().unwrap_or_default();
            st.running = false;
            st.phase = "failed".into();
            st.warnings.push(e.to_string());
            *self.endurance_status.lock() = Some(st.clone());
            self.emit(Event::Endurance(Box::new(st)));
        }
        self.speedtest_busy.store(false, Relaxed);
    }

    fn publish(&self, st: &EnduranceStatus) {
        *self.endurance_status.lock() = Some(st.clone());
        self.emit(Event::Endurance(Box::new(st.clone())));
    }

    async fn endurance(self: &Arc<Self>, url: &str, nets: &[NetInterface]) -> Result<()> {
        let dir = logs_dir(&self.data_dir);
        std::fs::create_dir_all(&dir)?;
        let started = chrono::Local::now();
        let name = format!("endurance-{}", started.format("%Y%m%d-%H%M%S"));
        let csv_path = dir.join(format!("{name}.csv"));
        let mut log = std::io::BufWriter::new(std::fs::File::create(&csv_path)?);
        writeln!(log, "timestamp,elapsed_s,network,mbps,rtt_ms,probe_lost,stalled,errors_total,cycle,cycle_pct,bytes_total")?;

        let counters: Vec<Arc<Counters>> = nets.iter().map(|_| Arc::new(Counters::default())).collect();
        let mut tracks: Vec<Track> = nets.iter().map(|_| Track::default()).collect();
        let bind = self.bind_device;
        let proxy = self.settings.read().proxy.clone();

        let mut workers = Vec::new();
        for (n, c) in nets.iter().zip(&counters) {
            let client = build_client(&ClientSpec { local: local_addr(n, false), device: bind.then(|| n.id.clone()), proxy: proxy.clone(), pin: None })?;
            let me = self.clone();
            let (c, u) = (c.clone(), url.to_string());
            workers.push(tokio::spawn(async move { me.endurance_worker(client, u, c).await }));
        }

        let mut st = EnduranceStatus {
            running: true,
            phase: "Running".into(),
            started_at: started.timestamp(),
            nets: nets.iter().map(|n| EnduranceNet { id: n.id.clone(), label: n.label.clone(), kind: Some(n.kind), ..Default::default() }).collect(),
            ..Default::default()
        };
        self.publish(&st);

        let t0 = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await;
        let mut ping_down = false;

        while !self.endurance_stop.load(Relaxed) {
            tick.tick().await;
            if self.endurance_stop.load(Relaxed) {
                break;
            }
            let elapsed = t0.elapsed().as_secs();
            // Probe every network at the same moment, while it is flat out downloading.
            let probes = futures_util::future::join_all(nets.iter().map(|n| {
                let n = n.clone();
                async move { crate::ping::ping(&n, bind).await }
            }))
            .await;

            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let (mut sum_mbps, mut rtt_acc, mut rtt_n, mut any_lost) = (0.0, 0.0, 0, false);
            for (i, n) in nets.iter().enumerate() {
                let (c, t) = (&counters[i], &mut tracks[i]);
                let b = c.bytes.load(Relaxed);
                let mbps = (b - t.last_bytes) as f64 * 8.0 / 1e6;
                t.last_bytes = b;
                let stalled = elapsed > 2 && mbps < STALL_BELOW_MBPS;
                if stalled {
                    t.stalls += 1;
                }
                let rtt = probes[i];
                t.sent += 1;
                match rtt {
                    Some(r) => {
                        t.run = 0;
                        t.rtts.push(r as f32);
                        if let Some(l) = t.last_rtt {
                            t.jitter_sum += (r - l).abs();
                            t.jitter_n += 1;
                        }
                        t.last_rtt = Some(r);
                        rtt_acc += r;
                        rtt_n += 1;
                    }
                    None => {
                        t.lost += 1;
                        t.run += 1;
                        t.longest_run = t.longest_run.max(t.run);
                        any_lost = true;
                    }
                }
                t.mbps.push(mbps as f32);
                sum_mbps += mbps;
                let cyc = c.cycle_bytes.load(Relaxed);
                let _ = writeln!(
                    log,
                    "{now},{elapsed},{},{mbps:.2},{},{},{},{},{},{:.1},{b}",
                    n.label.replace(',', " "),
                    rtt.map(|r| format!("{r:.1}")).unwrap_or_default(),
                    u8::from(rtt.is_none()),
                    u8::from(stalled),
                    c.errors.load(Relaxed),
                    c.cycles.load(Relaxed) + 1,
                    cyc as f64 / CYCLE_BYTES as f64 * 100.0,
                );
                st.nets[i] = net_stats(n, c, t, Some(mbps), rtt);
            }
            let _ = log.flush();
            ping_down = ping_down || tracks.iter().all(|t| t.sent >= 10 && t.lost == t.sent);

            let total: u64 = counters.iter().map(|c| c.bytes.load(Relaxed)).sum();
            st.elapsed_secs = elapsed;
            st.total_bytes = total;
            st.cycle_bytes = counters.iter().map(|c| c.cycle_bytes.load(Relaxed)).max().unwrap_or(0);
            st.log_bytes = std::fs::metadata(&csv_path).map(|m| m.len()).unwrap_or(0);
            st.recent.push(EndurancePoint { t: elapsed, mbps: sum_mbps, rtt_ms: (rtt_n > 0).then(|| rtt_acc / rtt_n as f64), lost: any_lost });
            if st.recent.len() > RECENT {
                st.recent.remove(0);
            }
            self.publish(&st);
        }

        for w in &workers {
            w.abort();
        }
        let _ = log.flush();
        drop(log);

        // Final numbers, notes, and the summary that sits beside the log.
        let ended = chrono::Local::now();
        let finals: Vec<EnduranceNet> = nets.iter().enumerate().map(|(i, n)| net_stats(n, &counters[i], &tracks[i], None, None)).collect();
        let mut notes = Vec::new();
        for f in &finals {
            if f.loss_pct >= 1.0 {
                notes.push(format!("{}: {:.1}% of probes were lost (longest gap {} s).", f.label, f.loss_pct, f.longest_loss_run));
            }
            if let (Some(min), Some(p95)) = (f.min_rtt_ms, f.p95_rtt_ms) {
                if p95 > min * 4.0 && p95 - min > 60.0 {
                    notes.push(format!("{}: latency rose from {min:.0} ms idle to {p95:.0} ms under load — the connection queues up when saturated.", f.label));
                }
            }
            if f.stall_secs > 0 {
                notes.push(format!("{}: {} s with almost no data flowing.", f.label, f.stall_secs));
            }
            if f.avg_mbps > 0.0 && f.low_mbps < f.avg_mbps * 0.4 {
                notes.push(format!("{}: speed was uneven — the slowest 5% of seconds ran at {:.0} Mbit/s against an average of {:.0}.", f.label, f.low_mbps, f.avg_mbps));
            }
        }
        if ping_down {
            notes.push("No latency probe ever got through, so loss figures aren't meaningful here.".into());
        }
        if notes.is_empty() {
            notes.push("No packet loss, stalls or large latency swings were seen.".into());
        }
        let summary = EnduranceSummary {
            name: name.clone(),
            started_at: started.timestamp(),
            ended_at: ended.timestamp(),
            duration_secs: t0.elapsed().as_secs(),
            url: url.to_string(),
            total_bytes: counters.iter().map(|c| c.bytes.load(Relaxed)).sum(),
            nets: finals.clone(),
            notes,
            csv_path: csv_path.to_string_lossy().into(),
            csv_bytes: std::fs::metadata(&csv_path).map(|m| m.len()).unwrap_or(0),
        };
        let _ = std::fs::write(csv_path.with_extension("json"), serde_json::to_vec_pretty(&summary).unwrap_or_default());

        st.running = false;
        st.phase = "Saved".into();
        st.nets = finals;
        st.recent.clear();
        st.summary = Some(summary);
        self.publish(&st);
        Ok(())
    }

    /// Keep pulling 500 MB "files" on one network: count the bytes, throw them away, start the next.
    async fn endurance_worker(self: Arc<Self>, client: reqwest::Client, url: String, c: Arc<Counters>) {
        loop {
            c.cycle_bytes.store(0, Relaxed);
            let mut streams = Vec::new();
            for _ in 0..STREAMS {
                let (client, url, c) = (client.clone(), url.clone(), c.clone());
                streams.push(tokio::spawn(async move {
                    while c.cycle_bytes.load(Relaxed) < CYCLE_BYTES {
                        let resp = match client.get(&url).send().await {
                            Ok(r) if r.status().is_success() => r,
                            _ => {
                                c.errors.fetch_add(1, Relaxed);
                                tokio::time::sleep(Duration::from_millis(800)).await;
                                continue;
                            }
                        };
                        let mut body = resp.bytes_stream();
                        loop {
                            match tokio::time::timeout(CHUNK_TIMEOUT, body.next()).await {
                                Ok(Some(Ok(chunk))) => {
                                    c.bytes.fetch_add(chunk.len() as u64, Relaxed);
                                    if c.cycle_bytes.fetch_add(chunk.len() as u64, Relaxed) + chunk.len() as u64 >= CYCLE_BYTES {
                                        return;
                                    }
                                }
                                Ok(None) => break,
                                _ => {
                                    c.errors.fetch_add(1, Relaxed);
                                    break;
                                }
                            }
                        }
                    }
                }));
            }
            for s in streams {
                let _ = s.await;
            }
            c.cycles.fetch_add(1, Relaxed);
        }
    }
}

fn net_stats(n: &NetInterface, c: &Counters, t: &Track, mbps: Option<f64>, rtt: Option<f64>) -> EnduranceNet {
    let m = &t.mbps;
    let avg = if m.is_empty() { 0.0 } else { m.iter().map(|v| *v as f64).sum::<f64>() / m.len() as f64 };
    let mut sorted = m.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let low = sorted.get(sorted.len() * 5 / 100).copied().unwrap_or(0.0) as f64;
    let mut rs = t.rtts.clone();
    rs.sort_by(|a, b| a.total_cmp(b));
    let rtt_avg = (!rs.is_empty()).then(|| rs.iter().map(|v| *v as f64).sum::<f64>() / rs.len() as f64);
    EnduranceNet {
        id: n.id.clone(),
        label: n.label.clone(),
        kind: Some(n.kind),
        mbps: mbps.unwrap_or(0.0),
        avg_mbps: avg,
        min_mbps: sorted.first().copied().unwrap_or(0.0) as f64,
        max_mbps: sorted.last().copied().unwrap_or(0.0) as f64,
        low_mbps: low,
        rtt_ms: rtt,
        min_rtt_ms: rs.first().map(|v| *v as f64),
        avg_rtt_ms: rtt_avg,
        p95_rtt_ms: rs.get((rs.len() * 95 / 100).min(rs.len().saturating_sub(1))).map(|v| *v as f64),
        max_rtt_ms: rs.last().map(|v| *v as f64),
        jitter_ms: (t.jitter_n > 0).then(|| t.jitter_sum / t.jitter_n as f64),
        sent: t.sent,
        lost: t.lost,
        loss_pct: if t.sent > 0 { t.lost as f64 / t.sent as f64 * 100.0 } else { 0.0 },
        longest_loss_run: t.longest_run,
        stall_secs: t.stalls,
        errors: c.errors.load(Relaxed),
        bytes: c.bytes.load(Relaxed),
        cycles: c.cycles.load(Relaxed) as u32,
        cycle_pct: c.cycle_bytes.load(Relaxed) as f64 / CYCLE_BYTES as f64 * 100.0,
    }
}
